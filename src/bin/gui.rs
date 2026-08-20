//! sweepmac GUI — a focused native window over the shared cleanup core.
//!
//! The screen is ordered by decision risk: what is safe to reclaim comes first
//! and is preselected, conditional developer cleanup is one disclosure away,
//! and anything that destroys data lives in a separate collapsed danger zone
//! that is never bulk-selected.
//!
//! Selection is aggregated by the library (`sweepmac::summarize`), so the rules
//! about what may be selected and when the primary action is live are tested
//! without a window. Scanning and cleaning run on background threads; workers
//! talk back over channels and wake the UI with `request_repaint`.

mod ui;

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Instant;

use eframe::egui;
use egui::RichText;
use sweepmac::{
    action_enabled, bulk_selectable_keys, clean_target, dir_size, disk_free, docker_info,
    find_node_modules, home, human, persist_pty_limit, pty_status, raise_pty_limit, scan,
    summarize, target_by_id, DockerImage, DockerInfo, DockerVolume, NodeModules, PtyLevel,
    PtyStatus, Risk, Selectable, SelectionSummary, ALL_CATEGORIES, DEFAULT_CLEAN_CATEGORIES,
};

use ui::style::{self, Tokens};
use ui::widgets::{self as w, Activity, RowView};

const PTY_TARGET: u64 = 999;
/// How many recommended rows stay on the first screen before the rest are
/// folded into their category group.
const RECOMMENDED_VISIBLE: usize = 5;

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([560.0, 720.0])
        .with_min_inner_size([460.0, 520.0])
        .with_title("sweepmac");
    viewport = viewport.with_icon(egui::IconData {
        rgba: sweepmac::broom_rgba(64, false),
        width: 64,
        height: 64,
    });
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "sweepmac",
        options,
        Box::new(|cc| {
            style::install_system_font(&cc.egui_ctx);
            let tokens = resolve_tokens(&cc.egui_ctx);
            style::install(&cc.egui_ctx, &tokens);
            // `--review` (used by the menu bar) opens straight on the review
            // step for the recommended selection, once the first scan lands.
            let review_on_open = std::env::args().any(|a| a == "--review");
            Ok(Box::new(App::new(
                cc.egui_ctx.clone(),
                tokens,
                review_on_open,
            )))
        }),
    )
}

/// Follow the system appearance when macOS reports one; dark otherwise.
fn resolve_tokens(ctx: &egui::Context) -> Tokens {
    match ctx.system_theme() {
        Some(theme) => Tokens::for_theme(theme),
        None => Tokens::dark(),
    }
}

// --- state ---------------------------------------------------------------

struct Row {
    id: &'static str,
    category: &'static str,
    desc: &'static str,
    path: PathBuf,
    size: u64,
}

/// One unit of work in an approved batch.
#[derive(Clone)]
enum Job {
    Cache {
        id: &'static str,
        desc: String,
        size: u64,
        path: PathBuf,
    },
    /// `docker <kind> prune` (kind: builder|image|container|network).
    DockerPrune {
        kind: &'static str,
        label: String,
    },
    NodeModules {
        label: String,
        path: PathBuf,
        size: u64,
    },
    Simulators,
}

impl Job {
    fn label(&self) -> String {
        match self {
            Job::Cache { desc, .. } => desc.clone(),
            Job::DockerPrune { label, .. } => label.clone(),
            Job::NodeModules { label, .. } => label.clone(),
            Job::Simulators => "Unavailable iOS simulators".to_string(),
        }
    }

    fn size(&self) -> u64 {
        match self {
            Job::Cache { size, .. } | Job::NodeModules { size, .. } => *size,
            // Docker/simctl report their own reclaim; we don't pre-credit it.
            Job::DockerPrune { .. } | Job::Simulators => 0,
        }
    }
}

/// The safe, reviewable batch built from the current selection.
#[derive(Clone)]
struct Review {
    jobs: Vec<Job>,
    /// Estimated reclaim, from the sizes we actually measured.
    estimate: u64,
    /// "Developer caches — 3 items · 2.1 GB" lines for the dialog.
    groups: Vec<(String, usize, u64)>,
    /// Selection keys this batch consumes — deselected when it is confirmed.
    keys: Vec<String>,
}

/// An irreversible action, confirmed on its own danger path.
#[derive(Clone)]
enum Danger {
    /// Delete Docker volumes — destroys their contents.
    DockerVolumes { names: Vec<String>, total: u64 },
}

/// A non-destructive Docker action kept next to its own list.
#[derive(Clone)]
enum DockerAction {
    RemoveImages {
        ids: Vec<String>,
        names: Vec<String>,
        total: u64,
    },
}

enum Msg {
    Scanned {
        rows: Vec<Row>,
        sims: u64,
        disk: (u64, u64),
        pty: Option<PtyStatus>,
    },
    DockerScanned(Option<DockerInfo>),
    NmScanned(Vec<NodeModules>),
    Log(String),
    /// A batch finished: what it freed and whether every job succeeded.
    Done {
        freed: u64,
        ok: bool,
        summary: String,
        rescan: bool,
    },
    NmRemoved {
        path: PathBuf,
    },
}

#[derive(PartialEq)]
enum Phase {
    Scanning,
    Idle,
    Working,
}

struct App {
    ctx: egui::Context,
    tokens: Tokens,
    mark: Option<egui::TextureHandle>,
    phase: Phase,

    rows: Vec<Row>,
    sims: u64,
    disk: (u64, u64),
    pty: Option<PtyStatus>,
    docker: Option<DockerInfo>,
    docker_scanning: bool,
    node_modules: Vec<NodeModules>,
    nm_scanning: bool,

    /// The shared selection for safe/conditional items, keyed by `Selectable`.
    selected: HashSet<String>,
    /// Volumes are deliberately kept out of the shared selection.
    vol_selected: HashSet<String>,
    img_selected: HashSet<String>,

    review: Option<Review>,
    /// Set by `--review`: open the review step as soon as a scan has run.
    review_on_open: bool,
    danger: Option<Danger>,
    /// Gate on the danger dialog — the user must acknowledge explicitly.
    danger_ack: bool,
    docker_action: Option<DockerAction>,

    activity: Vec<Activity>,
    activity_open: bool,
    log: Vec<String>,
    status: String,
    started: Option<Instant>,
    last_scan: Option<Instant>,

    /// Scan-result channel (Msg::Scanned only).
    rx: Option<Receiver<Msg>>,
    /// Worker channel for jobs/danger/docker/pty (Log, Done, NmRemoved) — kept
    /// separate from `rx` so starting work never discards an in-flight scan.
    work_rx: Option<Receiver<Msg>>,
    nm_rx: Option<Receiver<Msg>>,
    docker_rx: Option<Receiver<Msg>>,
}

impl App {
    fn new(ctx: egui::Context, tokens: Tokens, review_on_open: bool) -> Self {
        let mark = load_mark(&ctx);
        let mut app = App {
            ctx,
            tokens,
            mark,
            review_on_open,
            phase: Phase::Idle,
            rows: Vec::new(),
            sims: 0,
            disk: (0, 0),
            pty: None,
            docker: None,
            docker_scanning: false,
            node_modules: Vec::new(),
            nm_scanning: false,
            selected: HashSet::new(),
            vol_selected: HashSet::new(),
            img_selected: HashSet::new(),
            review: None,
            danger: None,
            danger_ack: false,
            docker_action: None,
            activity: Vec::new(),
            activity_open: false,
            log: Vec::new(),
            status: String::new(),
            started: None,
            last_scan: None,
            rx: None,
            work_rx: None,
            nm_rx: None,
            docker_rx: None,
        };
        app.start_scan();
        app.start_nm_scan();
        app.start_docker_scan();
        app
    }

    // --- scanning (never deletes) ---------------------------------------

    fn start_scan(&mut self) {
        self.phase = Phase::Scanning;
        self.status = "Scanning…".into();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let rows = scan(ALL_CATEGORIES)
                .unwrap_or_default()
                .into_iter()
                .map(|r| Row {
                    id: r.id,
                    category: r.category,
                    desc: r.desc,
                    path: r.path,
                    size: r.size,
                })
                .collect();
            // Simulator size only; Docker is scanned separately (it is slow).
            let sims = home()
                .ok()
                .map(|h| h.join("Library/Developer/CoreSimulator"))
                .filter(|p| p.exists())
                .map(|p| dir_size(&p))
                .unwrap_or(0);
            let _ = tx.send(Msg::Scanned {
                rows,
                sims,
                disk: disk_free().unwrap_or((0, 0)),
                pty: pty_status(),
            });
            ctx.request_repaint();
        });
    }

    fn start_docker_scan(&mut self) {
        self.docker_scanning = true;
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.docker_rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(Msg::DockerScanned(docker_info()));
            ctx.request_repaint();
        });
    }

    fn start_nm_scan(&mut self) {
        self.nm_scanning = true;
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.nm_rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let found = home().map(|h| find_node_modules(&h)).unwrap_or_default();
            let _ = tx.send(Msg::NmScanned(found));
            ctx.request_repaint();
        });
    }

    // --- selection ------------------------------------------------------

    /// Every item that participates in the shared selection, in one list, so
    /// the library can do the aggregation and bulk-selection filtering.
    fn selectables(&self) -> Vec<Selectable> {
        let mut items = Vec::new();
        for r in self.rows.iter().filter(|r| r.size > 0) {
            items.push(Selectable::new(cache_key(r.id), r.size, Risk::Regenerable));
        }
        if let Some(d) = &self.docker {
            for (kind, _, size) in docker_prune_kinds(d) {
                items.push(Selectable::new(docker_key(kind), size, Risk::Conditional));
            }
        }
        for nm in &self.node_modules {
            items.push(Selectable::new(
                nm_key(&nm.path),
                nm.size,
                Risk::Conditional,
            ));
        }
        if self.sims > 0 {
            // Counted as an item but credited 0 bytes: the tree size is the
            // whole CoreSimulator folder, not what `delete unavailable` frees.
            items.push(Selectable::new("sims", 0, Risk::Conditional));
        }
        // Volumes are listed so bulk-selection filtering can *exclude* them.
        if let Some(d) = &self.docker {
            for v in &d.volumes {
                items.push(
                    Selectable::new(vol_key(&v.name), v.size, Risk::Irreversible).locked(v.in_use),
                );
            }
        }
        items
    }

    fn summary(&self) -> SelectionSummary {
        summarize(&self.selectables(), |k| self.selected.contains(k))
    }

    /// Total of the safe, regenerable caches — the headline number.
    fn safe_reclaimable(&self) -> u64 {
        self.rows
            .iter()
            .filter(|r| DEFAULT_CLEAN_CATEGORIES.contains(&r.category))
            .map(|r| r.size)
            .sum()
    }

    /// Preselect the regenerable caches a scan found. Re-derives only the
    /// cache portion of the selection — Advanced ticks (docker/nm/sims) are the
    /// user's own choices and survive a rescan.
    fn apply_recommended_selection(&mut self) {
        self.selected.retain(|k| !k.starts_with("cache:"));
        for r in self
            .rows
            .iter()
            .filter(|r| r.size > 0 && DEFAULT_CLEAN_CATEGORIES.contains(&r.category))
        {
            self.selected.insert(cache_key(r.id));
        }
    }

    /// The recommended rows, largest first. Only default-category caches
    /// qualify — a manually ticked app cache stays in its own opt-in group
    /// with its re-download warning, and is never presented as recommended.
    fn recommended(&self) -> Vec<&Row> {
        let mut v: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| {
                r.size > 0
                    && DEFAULT_CLEAN_CATEGORIES.contains(&r.category)
                    && self.selected.contains(&cache_key(r.id))
            })
            .collect();
        v.sort_by(|a, b| b.size.cmp(&a.size));
        v
    }

    fn toggle(&mut self, key: String) {
        if !self.selected.remove(&key) {
            self.selected.insert(key);
        }
    }

    // --- building and running work --------------------------------------

    /// Turn the current selection into a reviewable batch.
    fn build_review(&self) -> Option<Review> {
        let mut jobs: Vec<Job> = Vec::new();
        let mut groups: Vec<(String, usize, u64)> = Vec::new();
        let mut keys: Vec<String> = Vec::new();

        // Caches, grouped for the dialog by their display group.
        for (label, cats) in GROUPS {
            let picked: Vec<&Row> = self
                .rows
                .iter()
                .filter(|r| {
                    r.size > 0
                        && cats.contains(&r.category)
                        && self.selected.contains(&cache_key(r.id))
                })
                .collect();
            if picked.is_empty() {
                continue;
            }
            let bytes: u64 = picked.iter().map(|r| r.size).sum();
            groups.push((label.to_string(), picked.len(), bytes));
            for r in picked {
                keys.push(cache_key(r.id));
                jobs.push(Job::Cache {
                    id: r.id,
                    desc: r.desc.to_string(),
                    size: r.size,
                    path: r.path.clone(),
                });
            }
        }

        // Docker prune categories.
        if let Some(d) = &self.docker {
            let picked: Vec<(&'static str, &'static str, u64)> = docker_prune_kinds(d)
                .into_iter()
                .filter(|(kind, _, _)| self.selected.contains(&docker_key(kind)))
                .collect();
            if !picked.is_empty() {
                let bytes: u64 = picked.iter().map(|(_, _, s)| *s).sum();
                groups.push(("Docker".to_string(), picked.len(), bytes));
                for (kind, label, _) in picked {
                    keys.push(docker_key(kind));
                    jobs.push(Job::DockerPrune {
                        kind,
                        label: label.to_string(),
                    });
                }
            }
        }

        // node_modules.
        let nm: Vec<&NodeModules> = self
            .node_modules
            .iter()
            .filter(|n| self.selected.contains(&nm_key(&n.path)))
            .collect();
        if !nm.is_empty() {
            let bytes: u64 = nm.iter().map(|n| n.size).sum();
            groups.push(("node_modules".to_string(), nm.len(), bytes));
            for n in nm {
                keys.push(nm_key(&n.path));
                jobs.push(Job::NodeModules {
                    label: n.label.clone(),
                    path: n.path.clone(),
                    size: n.size,
                });
            }
        }

        if self.selected.contains("sims") && self.sims > 0 {
            // 0 bytes: only simctl knows how much of the tree is unavailable.
            groups.push(("iOS simulators".to_string(), 1, 0));
            keys.push("sims".to_string());
            jobs.push(Job::Simulators);
        }

        if jobs.is_empty() {
            return None;
        }
        let estimate = jobs.iter().map(|j| j.size()).sum();
        Some(Review {
            jobs,
            estimate,
            groups,
            keys,
        })
    }

    /// Run an approved batch of safe/conditional jobs on a worker thread.
    fn run_jobs(&mut self, jobs: Vec<Job>, estimate: u64) {
        self.phase = Phase::Working;
        self.log.clear();
        self.started = Some(Instant::now());
        self.status = format!("Cleaning {} item{}…", jobs.len(), w::plural(jobs.len()));
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.work_rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let mut freed = 0u64;
            let mut failures = 0usize;
            let count = jobs.len();
            for job in jobs {
                match job {
                    Job::Cache {
                        id,
                        desc,
                        size,
                        path,
                    } => {
                        let _ = tx.send(Msg::Log(format!("clean {}", path.display())));
                        ctx.request_repaint();
                        match target_by_id(id) {
                            Some(t) if clean_target(t, &path).is_ok() => {
                                freed += size;
                                let _ = tx
                                    .send(Msg::Log(format!("  cleaned {desc} ({})", human(size))));
                            }
                            _ => {
                                failures += 1;
                                let _ = tx.send(Msg::Log(format!("  FAILED to clean {desc}")));
                            }
                        }
                    }
                    Job::DockerPrune { kind, label } => {
                        let args: &[&str] = match kind {
                            "builder" => &["builder", "prune", "-af"],
                            "image" => &["image", "prune", "-af"],
                            "container" => &["container", "prune", "-f"],
                            "network" => &["network", "prune", "-f"],
                            _ => &[],
                        };
                        let _ = tx.send(Msg::Log(format!("$ docker {} — {label}", args.join(" "))));
                        ctx.request_repaint();
                        if !stream(&tx, &ctx, sweepmac::docker_bin(), args) {
                            failures += 1;
                        }
                    }
                    Job::NodeModules { label, path, size } => {
                        let _ = tx.send(Msg::Log(format!("rm -rf {}", path.display())));
                        ctx.request_repaint();
                        match std::fs::remove_dir_all(&path) {
                            Ok(()) => {
                                freed += size;
                                let _ = tx
                                    .send(Msg::Log(format!("  removed {label} ({})", human(size))));
                                let _ = tx.send(Msg::NmRemoved { path });
                            }
                            Err(e) => {
                                failures += 1;
                                let _ = tx.send(Msg::Log(format!("  FAILED {label}: {e}")));
                            }
                        }
                    }
                    Job::Simulators => {
                        let _ = tx.send(Msg::Log("$ xcrun simctl delete unavailable".into()));
                        ctx.request_repaint();
                        if !stream(&tx, &ctx, "xcrun", &["simctl", "delete", "unavailable"]) {
                            failures += 1;
                        }
                    }
                }
                ctx.request_repaint();
            }
            let ok = failures == 0;
            let summary = if ok {
                if freed > 0 {
                    format!(
                        "Cleaned {count} item{} · {}",
                        w::plural(count),
                        human(freed)
                    )
                } else {
                    format!("Cleaned {count} item{}", w::plural(count))
                }
            } else {
                format!(
                    "{failures} of {count} item{} failed · {} reclaimed",
                    w::plural(count),
                    human(freed)
                )
            };
            let _ = tx.send(Msg::Done {
                freed,
                ok,
                summary,
                rescan: true,
            });
            let _ = estimate;
            ctx.request_repaint();
        });
    }

    /// Run an irreversible action after its own explicit confirmation.
    fn run_danger(&mut self, d: Danger) {
        self.phase = Phase::Working;
        self.log.clear();
        self.started = Some(Instant::now());
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.work_rx = Some(rx);
        let ctx = self.ctx.clone();
        match d {
            Danger::DockerVolumes { names, total } => {
                self.status = format!(
                    "Deleting {} volume{} permanently…",
                    names.len(),
                    w::plural(names.len())
                );
                thread::spawn(move || {
                    let count = names.len();
                    for name in &names {
                        let _ = tx.send(Msg::Log(format!("$ docker volume rm {name}")));
                        ctx.request_repaint();
                        let _ = stream(&tx, &ctx, sweepmac::docker_bin(), &["volume", "rm", name]);
                    }
                    let _ = tx.send(Msg::Done {
                        freed: 0,
                        ok: true,
                        summary: format!(
                            "Deleted {count} volume{} permanently ({})",
                            w::plural(count),
                            human(total)
                        ),
                        rescan: true,
                    });
                    ctx.request_repaint();
                });
            }
        }
    }

    fn run_docker_action(&mut self, a: DockerAction) {
        self.phase = Phase::Working;
        self.log.clear();
        self.started = Some(Instant::now());
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.work_rx = Some(rx);
        let ctx = self.ctx.clone();
        match a {
            DockerAction::RemoveImages { ids, names, total } => {
                self.status = format!("Removing {} image{}…", names.len(), w::plural(names.len()));
                thread::spawn(move || {
                    let count = names.len();
                    let _ = tx.send(Msg::Log(format!("$ docker rmi {}", ids.join(" "))));
                    ctx.request_repaint();
                    let mut args = vec!["rmi"];
                    args.extend(ids.iter().map(|s| s.as_str()));
                    let ok = stream(&tx, &ctx, sweepmac::docker_bin(), &args);
                    let summary = if ok {
                        format!(
                            "Removed {count} image{} ({})",
                            w::plural(count),
                            human(total)
                        )
                    } else {
                        format!(
                            "Removing {count} image{} failed — view details",
                            w::plural(count)
                        )
                    };
                    let _ = tx.send(Msg::Done {
                        freed: 0,
                        ok,
                        summary,
                        rescan: true,
                    });
                    ctx.request_repaint();
                });
            }
        }
    }

    fn run_pty(&mut self, persist: bool) {
        self.phase = Phase::Working;
        self.log.clear();
        self.started = Some(Instant::now());
        self.status = "Waiting for your admin password…".into();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.work_rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let action = if persist {
                persist_pty_limit(PTY_TARGET)
            } else {
                raise_pty_limit(PTY_TARGET)
            };
            let (ok, summary) = match action {
                Ok(_) => {
                    let extra = if persist {
                        " (persisted to /etc/sysctl.conf)"
                    } else {
                        ""
                    };
                    let _ = tx.send(Msg::Log(format!(
                        "kern.tty.ptmx_max set to {PTY_TARGET}{extra}"
                    )));
                    (true, format!("Terminal limit raised to {PTY_TARGET}"))
                }
                Err(e) if e == "cancelled" => {
                    let _ = tx.send(Msg::Log("cancelled at the password prompt".into()));
                    (false, "Cancelled at the password prompt".to_string())
                }
                Err(e) => {
                    let _ = tx.send(Msg::Log(e.clone()));
                    (false, format!("Could not raise the terminal limit: {e}"))
                }
            };
            let _ = tx.send(Msg::Done {
                freed: 0,
                ok,
                summary,
                rescan: false,
            });
            ctx.request_repaint();
        });
    }

    fn elapsed_secs(&self) -> u64 {
        self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    /// Human age of the last completed scan, e.g. "just now", "4 min ago".
    fn last_scan_age(&self) -> Option<String> {
        let secs = self.last_scan?.elapsed().as_secs();
        Some(match secs {
            0..=20 => "just now".to_string(),
            21..=90 => "a minute ago".to_string(),
            _ if secs < 3600 => format!("{} min ago", secs / 60),
            _ => format!("{}h ago", secs / 3600),
        })
    }

    fn push_activity(&mut self, ok: bool, summary: String) {
        let detail = std::mem::take(&mut self.log);
        self.activity.push(Activity {
            ok,
            summary,
            detail,
        });
        let overflow = self.activity.len().saturating_sub(20);
        if overflow > 0 {
            self.activity.drain(0..overflow);
        }
        // Surface the result: the section opens itself once there is news.
        self.activity_open = true;
    }

    fn poll(&mut self) {
        let mut batch: Vec<Msg> = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                batch.push(msg);
            }
        }
        if let Some(rx) = &self.work_rx {
            while let Ok(msg) = rx.try_recv() {
                batch.push(msg);
            }
        }
        for msg in batch {
            match msg {
                Msg::Log(line) => {
                    self.log.push(line);
                    let overflow = self.log.len().saturating_sub(400);
                    if overflow > 0 {
                        self.log.drain(0..overflow);
                    }
                }
                Msg::Scanned {
                    rows,
                    sims,
                    disk,
                    pty,
                } => {
                    self.rows = rows;
                    self.sims = sims;
                    self.disk = disk;
                    self.pty = pty;
                    // A worker may have started while this scan was in flight;
                    // its Done, not the scan, ends the Working phase then.
                    if self.phase == Phase::Scanning {
                        self.phase = Phase::Idle;
                        self.started = None;
                        self.status =
                            format!("{} safely reclaimable", human(self.safe_reclaimable()));
                    }
                    self.last_scan = Some(Instant::now());
                    self.apply_recommended_selection();
                    self.rx = None;
                    if std::mem::take(&mut self.review_on_open) {
                        self.review = self.build_review();
                    }
                }
                Msg::Done {
                    freed,
                    ok,
                    summary,
                    rescan,
                } => {
                    self.phase = Phase::Idle;
                    self.started = None;
                    self.vol_selected.clear();
                    self.img_selected.clear();
                    self.danger_ack = false;
                    self.push_activity(ok, summary);
                    self.status = if freed > 0 {
                        format!("Reclaimed {}", human(freed))
                    } else {
                        "Done".into()
                    };
                    self.work_rx = None;
                    if rescan {
                        // Refresh totals and the selection from reality.
                        self.start_scan();
                        self.start_docker_scan();
                    } else {
                        self.pty = pty_status();
                    }
                }
                Msg::NmRemoved { path } => {
                    self.node_modules.retain(|n| n.path != path);
                    self.selected.remove(&nm_key(&path));
                }
                Msg::DockerScanned(_) | Msg::NmScanned(_) => {}
            }
        }

        let mut nm_batch: Vec<Msg> = Vec::new();
        if let Some(rx) = &self.nm_rx {
            while let Ok(msg) = rx.try_recv() {
                nm_batch.push(msg);
            }
        }
        for msg in nm_batch {
            if let Msg::NmScanned(found) = msg {
                self.node_modules = found;
                self.nm_scanning = false;
                self.nm_rx = None;
            }
        }

        let mut docker_batch: Vec<Msg> = Vec::new();
        if let Some(rx) = &self.docker_rx {
            while let Ok(msg) = rx.try_recv() {
                docker_batch.push(msg);
            }
        }
        for msg in docker_batch {
            if let Msg::DockerScanned(info) = msg {
                self.docker = info;
                self.docker_scanning = false;
                self.docker_rx = None;
            }
        }
    }
}

/// Everything a render pass wants to change about app state, collected while
/// the UI holds `&self` and applied once the borrow ends.
#[derive(Default)]
struct Effects {
    /// Shared-selection keys to toggle.
    toggles: Vec<String>,
    vol_toggles: Vec<String>,
    img_toggles: Vec<String>,
    img_set_all: Option<bool>,
    request_images: bool,
    request_volumes: bool,
    /// `Some(persist)` when a pty action was clicked.
    pty_action: Option<bool>,
    rescan_nm: bool,
}

/// Display groups for cache categories, in first-screen order.
const GROUPS: &[(&str, &[&str])] = &[
    ("Developer caches", &["dev", "xcode"]),
    ("System caches", &["system", "macos"]),
    ("App caches", &["app"]),
];

fn cache_key(id: &str) -> String {
    format!("cache:{id}")
}
fn docker_key(kind: &str) -> String {
    format!("docker:{kind}")
}
fn nm_key(p: &std::path::Path) -> String {
    format!("nm:{}", p.display())
}
fn vol_key(name: &str) -> String {
    format!("vol:{name}")
}

/// The Docker prune categories shown in Advanced, with their reclaimable size.
fn docker_prune_kinds(d: &DockerInfo) -> Vec<(&'static str, &'static str, u64)> {
    vec![
        ("builder", "Build cache", d.build_cache),
        ("image", "Unused images", d.images),
        ("container", "Stopped containers", d.containers),
        ("network", "Unused networks", 0),
    ]
}

/// The window's sweepmac mark: the same alpha-mask glyph the menu bar uses, so
/// the two front-ends read as one product. It is tinted by the text colour at
/// draw time, which is why the mask is loaded rather than a coloured icon.
fn load_mark(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let (rgba, px) = sweepmac::tray_template_rgba(54);
    let size = px as usize;
    // Tinting multiplies, so a black mask would stay black: keep the alpha and
    // make the covered pixels white.
    let white: Vec<u8> = rgba.chunks(4).flat_map(|p| [255, 255, 255, p[3]]).collect();
    let image = egui::ColorImage::from_rgba_unmultiplied([size, size], &white);
    Some(ctx.load_texture("sweepmac-mark", image, egui::TextureOptions::LINEAR))
}

// --- rendering -----------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.phase != Phase::Idle {
            ctx.request_repaint();
        }
        // Follow a system appearance change without a restart.
        let wanted = resolve_tokens(ctx);
        if wanted != self.tokens {
            self.tokens = wanted;
            style::install(ctx, &self.tokens);
        }

        self.top_panel(ctx);
        self.bottom_panel(ctx);
        self.central(ctx);
        self.review_dialog(ctx);
        self.danger_dialog(ctx);
        self.docker_action_dialog(ctx);
    }
}

impl App {
    fn top_panel(&mut self, ctx: &egui::Context) {
        let t = self.tokens;
        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::default()
                    .fill(t.canvas)
                    .inner_margin(egui::Margin::symmetric(style::GUTTER, style::PANEL_PAD_Y)),
            )
            .show(ctx, |ui| {
                let age = self.last_scan_age();
                let res = w::header(
                    ui,
                    &t,
                    self.mark.as_ref(),
                    age.as_deref(),
                    self.phase != Phase::Idle,
                );
                if res.rescan_clicked {
                    self.start_scan();
                    self.start_docker_scan();
                    self.start_nm_scan();
                }
                ui.add_space(14.0);
                w::storage_summary(
                    ui,
                    &t,
                    self.safe_reclaimable(),
                    self.summary().bytes,
                    self.disk,
                    self.phase == Phase::Scanning,
                );
            });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) {
        let t = self.tokens;
        egui::TopBottomPanel::bottom("actions")
            .frame(
                egui::Frame::default()
                    .fill(t.canvas)
                    .inner_margin(egui::Margin::symmetric(style::GUTTER, 12.0))
                    .stroke(egui::Stroke::new(1.0, t.divider)),
            )
            .show(ctx, |ui| {
                let summary = self.summary();
                let busy = self.phase == Phase::Working;
                let busy_label = busy.then(|| self.status.clone());
                let enabled = action_enabled(summary, busy);
                if w::action_bar(ui, &t, summary, enabled, busy_label.as_deref()) {
                    self.review = self.build_review();
                }
            });
    }

    fn central(&mut self, ctx: &egui::Context) {
        let t = self.tokens;
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(t.canvas)
                    .inner_margin(egui::Margin::symmetric(style::GUTTER, 4.0)),
            )
            .show(ctx, |ui| {
                if self.phase == Phase::Scanning && self.rows.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.add(egui::Spinner::new().size(26.0).color(t.text_muted));
                    });
                    return;
                }

                // Deferred effects: collected while rendering, applied after.
                let mut fx = Effects::default();

                // Only a running operation blocks interaction; a background
                // rescan must not grey out the window.
                let can_act = self.phase != Phase::Working;

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(pty) = self.pty {
                            if pty.level != PtyLevel::Ok {
                                fx.pty_action = self.pty_notice(ui, &pty, can_act);
                                ui.add_space(style::SECTION_GAP);
                            }
                        }

                        self.recommended_section(ui, can_act, &mut fx);
                        ui.add_space(style::SECTION_GAP);
                        self.cache_groups(ui, can_act, &mut fx);
                        ui.add_space(style::SECTION_GAP);
                        self.advanced_section(ui, can_act, &mut fx);
                        ui.add_space(style::SECTION_GAP);
                        self.danger_section(ui, can_act, &mut fx);
                        ui.add_space(style::SECTION_GAP);
                        w::activity_section(
                            ui,
                            &t,
                            &self.activity,
                            std::mem::take(&mut self.activity_open),
                            (self.phase == Phase::Working)
                                .then(|| (self.status.as_str(), self.elapsed_secs())),
                        );
                        ui.add_space(8.0);
                    });

                // --- apply deferred effects ---
                for key in std::mem::take(&mut fx.toggles) {
                    self.toggle(key);
                }
                for name in std::mem::take(&mut fx.vol_toggles) {
                    if !self.vol_selected.remove(&name) {
                        self.vol_selected.insert(name);
                    }
                }
                for id in std::mem::take(&mut fx.img_toggles) {
                    if !self.img_selected.remove(&id) {
                        self.img_selected.insert(id);
                    }
                }
                if let Some(all) = fx.img_set_all {
                    self.img_selected.clear();
                    if all {
                        if let Some(d) = &self.docker {
                            // Bulk selection uses the library rule, so locked
                            // images can never be swept in.
                            let items: Vec<Selectable> = d
                                .image_list
                                .iter()
                                .map(|i| {
                                    Selectable::new(i.id.clone(), i.size, Risk::Conditional)
                                        .locked(i.in_use)
                                })
                                .collect();
                            self.img_selected = bulk_selectable_keys(&items).into_iter().collect();
                        }
                    }
                }
                if fx.request_images {
                    if let Some(d) = &self.docker {
                        let chosen: Vec<&DockerImage> = d
                            .image_list
                            .iter()
                            .filter(|i| !i.in_use && self.img_selected.contains(&i.id))
                            .collect();
                        if !chosen.is_empty() {
                            self.docker_action = Some(DockerAction::RemoveImages {
                                // Refs, not raw ids: a multi-tagged image must
                                // be untagged name by name (see rmi_refs).
                                ids: chosen.iter().flat_map(|i| i.rmi_refs()).collect(),
                                names: chosen.iter().map(|i| i.name.clone()).collect(),
                                total: chosen.iter().map(|i| i.size).sum(),
                            });
                        }
                    }
                }
                if fx.request_volumes {
                    if let Some(d) = &self.docker {
                        let chosen: Vec<&DockerVolume> = d
                            .volumes
                            .iter()
                            .filter(|v| !v.in_use && self.vol_selected.contains(&v.name))
                            .collect();
                        if !chosen.is_empty() {
                            self.danger_ack = false;
                            self.danger = Some(Danger::DockerVolumes {
                                names: chosen.iter().map(|v| v.name.clone()).collect(),
                                total: chosen.iter().map(|v| v.size).sum(),
                            });
                        }
                    }
                }
                if fx.rescan_nm {
                    self.start_nm_scan();
                }
                if let Some(persist) = fx.pty_action {
                    self.run_pty(persist);
                }
            });
    }

    /// 3. Recommended cleanup — the automatically selected regenerable caches.
    fn recommended_section(&self, ui: &mut egui::Ui, can_act: bool, fx: &mut Effects) {
        let t = self.tokens;
        let rec = self.recommended();
        ui.horizontal(|ui| {
            style::section_label(ui, &t, "RECOMMENDED CLEANUP");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let bytes: u64 = rec.iter().map(|r| r.size).sum();
                if bytes > 0 {
                    ui.label(
                        RichText::new(human(bytes))
                            .size(style::T_LABEL)
                            .strong()
                            .color(t.safe),
                    );
                }
            });
        });
        ui.add_space(6.0);

        if rec.is_empty() {
            style::group(ui, &t, |ui| {
                ui.label(style::body(
                    &t,
                    "Nothing to reclaim — your caches are clear.",
                ));
                ui.label(style::meta(
                    &t,
                    "Caches refill as you build, install and browse. Rescan later.",
                ));
            });
            return;
        }

        style::group(ui, &t, |ui| {
            for (n, r) in rec.iter().take(RECOMMENDED_VISIBLE).enumerate() {
                if n > 0 {
                    ui.add_space(style::ROW_GAP);
                }
                let toggled = w::select_row(
                    ui,
                    &t,
                    RowView {
                        name: r.desc,
                        note: None,
                        size: r.size,
                        checked: true,
                        enabled: can_act,
                        locked: false,
                        lock_note: None,
                        details: Some(&r.path.to_string_lossy()),
                        tint: t.accent,
                    },
                );
                if toggled {
                    fx.toggles.push(cache_key(r.id));
                }
            }
            let hidden = rec.len().saturating_sub(RECOMMENDED_VISIBLE);
            if hidden > 0 {
                let bytes: u64 = rec.iter().skip(RECOMMENDED_VISIBLE).map(|r| r.size).sum();
                ui.add_space(style::ROW_GAP);
                ui.label(style::meta(
                    &t,
                    format!(
                        "+{hidden} more selected ({}) — listed in the groups below",
                        human(bytes)
                    ),
                ));
            }
        });
        ui.add_space(6.0);
        ui.label(style::meta(
            &t,
            "Regenerable caches only. Apps rebuild these the next time they need them.",
        ));
    }

    /// 4. Cache groups — collapsed unless they hold something selected.
    fn cache_groups(&self, ui: &mut egui::Ui, can_act: bool, fx: &mut Effects) {
        let t = self.tokens;
        let shown: HashSet<&str> = self
            .recommended()
            .into_iter()
            .take(RECOMMENDED_VISIBLE)
            .map(|r| r.id)
            .collect();

        for (label, cats) in GROUPS {
            let rows: Vec<&Row> = self
                .rows
                .iter()
                .filter(|r| cats.contains(&r.category) && r.size > 0 && !shown.contains(r.id))
                .collect();
            if rows.is_empty() {
                continue;
            }
            let bytes: u64 = rows.iter().map(|r| r.size).sum();
            // Open only a group that actually contains a selected item.
            let has_selected = rows
                .iter()
                .any(|r| self.selected.contains(&cache_key(r.id)));
            w::collapsing_group(
                ui,
                &t,
                w::GroupSpec {
                    id: label,
                    title: label,
                    count: rows.len(),
                    bytes,
                    default_open: has_selected,
                    tint: None,
                },
                |ui| {
                    if *label == "App caches" {
                        ui.label(style::meta(
                            &t,
                            "Left unselected: these re-download the next time you open the app.",
                        ));
                        ui.add_space(4.0);
                    }
                    for (n, r) in rows.iter().enumerate() {
                        if n > 0 {
                            ui.add_space(style::ROW_GAP);
                        }
                        let toggled = w::select_row(
                            ui,
                            &t,
                            RowView {
                                name: r.desc,
                                note: None,
                                size: r.size,
                                checked: self.selected.contains(&cache_key(r.id)),
                                enabled: can_act,
                                locked: false,
                                lock_note: None,
                                details: Some(&r.path.to_string_lossy()),
                                tint: t.accent,
                            },
                        );
                        if toggled {
                            fx.toggles.push(cache_key(r.id));
                        }
                    }
                },
            );
            ui.add_space(8.0);
        }

        let empty = self.rows.iter().filter(|r| r.size == 0).count();
        if empty > 0 {
            ui.label(style::meta(
                &t,
                format!("{empty} more caches tracked and already empty"),
            ));
        }
    }

    /// 5. Advanced developer cleanup — collapsed, conditional, caution-tinted.
    fn advanced_section(&self, ui: &mut egui::Ui, can_act: bool, fx: &mut Effects) {
        let t = self.tokens;
        let docker_bytes = self
            .docker
            .as_ref()
            .map(|d| d.build_cache + d.images + d.containers)
            .unwrap_or(0);
        let nm_bytes: u64 = self.node_modules.iter().map(|n| n.size).sum();
        let count = self
            .docker
            .as_ref()
            .map(|d| docker_prune_kinds(d).len())
            .unwrap_or(0)
            + self.node_modules.len()
            + usize::from(self.sims > 0);

        w::collapsing_group(
            ui,
            &t,
            w::GroupSpec {
                id: "advanced",
                title: "Advanced developer cleanup",
                count,
                bytes: docker_bytes + nm_bytes + self.sims,
                default_open: false,
                tint: Some(t.caution),
            },
            |ui| {
                ui.label(style::meta(
                    &t,
                    "Safe in most setups, but worth a deliberate choice — these are rebuilt by \
                     your tools, not by macOS.",
                ));
                ui.add_space(8.0);

                // Docker prune categories.
                if self.docker.is_none() {
                    ui.horizontal(|ui| {
                        if self.docker_scanning {
                            ui.add(egui::Spinner::new().size(12.0).color(t.text_muted));
                            ui.label(style::meta(&t, "querying Docker (docker system df)…"));
                        } else {
                            ui.label(style::meta(&t, "Docker is not reachable — skipped."));
                        }
                    });
                } else if let Some(d) = &self.docker {
                    style::section_label(ui, &t, "DOCKER");
                    ui.add_space(4.0);
                    for (n, (kind, label, size)) in docker_prune_kinds(d).into_iter().enumerate() {
                        if n > 0 {
                            ui.add_space(style::ROW_GAP);
                        }
                        let key = docker_key(kind);
                        let note = match kind {
                            "builder" => Some("Rebuilt on your next docker build"),
                            "image" => Some("Re-pulled or rebuilt when next needed"),
                            "network" => Some("Recreated by compose when needed"),
                            _ => None,
                        };
                        let toggled = w::select_row(
                            ui,
                            &t,
                            RowView {
                                name: label,
                                note,
                                size,
                                checked: self.selected.contains(&key),
                                enabled: can_act,
                                locked: false,
                                lock_note: None,
                                details: None,
                                tint: t.caution,
                            },
                        );
                        if toggled {
                            fx.toggles.push(key);
                        }
                    }
                    ui.add_space(6.0);
                    ui.label(style::meta(
                        &t,
                        "Pruning frees space inside the Docker VM and keeps running containers. \
                         It does not shrink the ~/.colima disk image on your Mac.",
                    ));

                    // Per-image removal keeps its own list and action: it needs
                    // image ids, and "Remove" is the honest verb for it.
                    if !d.image_list.is_empty() {
                        ui.add_space(10.0);
                        let selectable = d.image_list.iter().filter(|i| !i.in_use).count();
                        let sel: Vec<&DockerImage> = d
                            .image_list
                            .iter()
                            .filter(|i| !i.in_use && self.img_selected.contains(&i.id))
                            .collect();
                        let all = selectable > 0 && sel.len() == selectable;
                        ui.horizontal(|ui| {
                            style::section_label(
                                ui,
                                &t,
                                &format!("IMAGES ({})", d.image_list.len()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if selectable > 0 {
                                        let lbl = if all { "Select none" } else { "Select all" };
                                        if ui.add(style::quiet_button(&t, lbl)).clicked() {
                                            fx.img_set_all = Some(!all);
                                        }
                                    }
                                },
                            );
                        });
                        ui.add_space(4.0);
                        for (n, img) in d.image_list.iter().enumerate() {
                            if n > 0 {
                                ui.add_space(style::ROW_GAP);
                            }
                            let toggled = w::select_row(
                                ui,
                                &t,
                                RowView {
                                    name: &img.name,
                                    note: None,
                                    size: img.size,
                                    checked: self.img_selected.contains(&img.id),
                                    enabled: can_act && !img.in_use,
                                    locked: img.in_use,
                                    lock_note: Some("in use"),
                                    details: Some(&img.id),
                                    tint: t.caution,
                                },
                            );
                            if toggled {
                                fx.img_toggles.push(img.id.clone());
                            }
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            let total: u64 = sel.iter().map(|i| i.size).sum();
                            ui.label(style::meta(
                                &t,
                                if sel.is_empty() {
                                    "Select unused images to remove".to_string()
                                } else {
                                    format!(
                                        "{} image{} · {}",
                                        sel.len(),
                                        w::plural(sel.len()),
                                        human(total)
                                    )
                                },
                            ));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let enabled = can_act && !sel.is_empty();
                                    let btn = style::primary_button(
                                        &t,
                                        &format!(
                                            "Remove {} image{}",
                                            sel.len(),
                                            w::plural(sel.len())
                                        ),
                                    );
                                    let resp = ui.add_enabled(enabled, btn);
                                    if resp.clicked() {
                                        fx.request_images = true;
                                    }
                                    if !enabled {
                                        resp.on_disabled_hover_text(
                                            "Tick at least one unused image",
                                        );
                                    }
                                },
                            );
                        });
                    }
                }

                // node_modules.
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    style::section_label(ui, &t, "NODE_MODULES");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.nm_scanning {
                            ui.add(egui::Spinner::new().size(12.0).color(t.text_muted));
                        } else if ui.add(style::quiet_button(&t, "Rescan")).clicked() {
                            fx.rescan_nm = true;
                        }
                    });
                });
                ui.add_space(4.0);
                if self.node_modules.is_empty() {
                    ui.label(style::meta(
                        &t,
                        if self.nm_scanning {
                            "looking for node_modules folders…"
                        } else {
                            "none found"
                        },
                    ));
                } else {
                    for (n, nm) in self.node_modules.iter().take(12).enumerate() {
                        if n > 0 {
                            ui.add_space(style::ROW_GAP);
                        }
                        let key = nm_key(&nm.path);
                        let toggled = w::select_row(
                            ui,
                            &t,
                            RowView {
                                name: &nm.label,
                                note: None,
                                size: nm.size,
                                checked: self.selected.contains(&key),
                                enabled: can_act,
                                locked: false,
                                lock_note: None,
                                details: Some(&nm.path.to_string_lossy()),
                                tint: t.caution,
                            },
                        );
                        if toggled {
                            fx.toggles.push(key);
                        }
                    }
                    if self.node_modules.len() > 12 {
                        ui.add_space(style::ROW_GAP);
                        ui.label(style::meta(
                            &t,
                            format!(
                                "{} more folders found — the largest are listed first",
                                self.node_modules.len() - 12
                            ),
                        ));
                    }
                    ui.add_space(6.0);
                    ui.label(style::meta(
                        &t,
                        "Rebuild any project later with npm / yarn / pnpm install.",
                    ));
                }

                // iOS simulators.
                if self.sims > 0 {
                    ui.add_space(12.0);
                    style::section_label(ui, &t, "IOS SIMULATORS");
                    ui.add_space(4.0);
                    let toggled = w::select_row(
                        ui,
                        &t,
                        RowView {
                            name: "iOS simulators (CoreSimulator)",
                            note: Some(
                                "Size is the whole simulator folder; cleaning deletes only \
                                 unavailable runtimes (xcrun simctl delete unavailable) — \
                                 usually a fraction of it",
                            ),
                            size: self.sims,
                            checked: self.selected.contains("sims"),
                            enabled: can_act,
                            locked: false,
                            lock_note: None,
                            details: None,
                            tint: t.caution,
                        },
                    );
                    if toggled {
                        fx.toggles.push("sims".to_string());
                    }
                }
            },
        );
    }

    /// 6. Irreversible cleanup — data-bearing, never bulk-selected.
    fn danger_section(&self, ui: &mut egui::Ui, can_act: bool, fx: &mut Effects) {
        let t = self.tokens;
        let Some(d) = &self.docker else { return };
        if d.volumes.is_empty() {
            return;
        }
        let total: u64 = d.volumes.iter().map(|v| v.size).sum();
        w::collapsing_group(
            ui,
            &t,
            w::GroupSpec {
                id: "danger",
                title: "Irreversible cleanup",
                count: d.volumes.len(),
                bytes: total,
                default_open: false,
                tint: Some(t.danger),
            },
            |ui| {
                ui.label(
                    RichText::new(
                        "Docker volumes hold real data — databases, uploads, anything a container \
                         wrote. Deleting one cannot be undone, and nothing here is ever included \
                         in Review & Clean.",
                    )
                    .size(style::T_META)
                    .color(t.text_secondary),
                );
                ui.add_space(8.0);
                let sel: Vec<&DockerVolume> = d
                    .volumes
                    .iter()
                    .filter(|v| !v.in_use && self.vol_selected.contains(&v.name))
                    .collect();
                for (n, v) in d.volumes.iter().enumerate() {
                    if n > 0 {
                        ui.add_space(style::ROW_GAP);
                    }
                    let toggled = w::select_row(
                        ui,
                        &t,
                        RowView {
                            name: &v.name,
                            note: None,
                            size: v.size,
                            checked: self.vol_selected.contains(&v.name),
                            enabled: can_act && !v.in_use,
                            locked: v.in_use,
                            lock_note: Some("mounted"),
                            details: Some(&v.name),
                            tint: t.danger,
                        },
                    );
                    if toggled {
                        fx.vol_toggles.push(v.name.clone());
                    }
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let bytes: u64 = sel.iter().map(|v| v.size).sum();
                    ui.label(style::meta(
                        &t,
                        if sel.is_empty() {
                            "Select a volume to delete it permanently".to_string()
                        } else {
                            format!(
                                "{} volume{} · {}",
                                sel.len(),
                                w::plural(sel.len()),
                                human(bytes)
                            )
                        },
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let enabled = can_act && !sel.is_empty();
                        let resp =
                            ui.add_enabled(enabled, style::danger_button(&t, "Delete permanently"));
                        if resp.clicked() {
                            fx.request_volumes = true;
                        }
                        if !enabled {
                            resp.on_disabled_hover_text(
                                "Tick an unused volume — mounted volumes cannot be deleted",
                            );
                        }
                    });
                });
            },
        );
    }

    /// The macOS pty-exhaustion notice. Not cleanup: a system limit with a fix.
    fn pty_notice(&self, ui: &mut egui::Ui, pty: &PtyStatus, can_act: bool) -> Option<bool> {
        let t = self.tokens;
        let mut action = None;
        let critical = pty.level == PtyLevel::Critical;
        let tint = if critical { t.danger } else { t.caution };
        style::tinted_group(ui, &t, tint, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(if critical {
                        "Terminal limit reached"
                    } else {
                        "Terminal slots running low"
                    })
                    .size(style::T_BODY)
                    .strong()
                    .color(tint),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} / {}", pty.nodes, pty.cap))
                            .size(style::T_META)
                            .monospace()
                            .color(tint),
                    );
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Old terminal devices pile up over long uptimes and aren't freed by closing \
                     apps. At the cap, new shells fail with \"Device not configured\". Raising the \
                     limit fixes it immediately — no restart.",
                )
                .size(style::T_META)
                .color(t.text_secondary),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_act,
                        style::primary_button(&t, &format!("Raise limit to {PTY_TARGET}")),
                    )
                    .clicked()
                {
                    action = Some(false);
                }
                if ui
                    .add_enabled(can_act, style::quiet_button(&t, "Make permanent"))
                    .clicked()
                {
                    action = Some(true);
                }
                ui.label(style::meta(&t, "asks for your Mac password"));
            });
        });
        action
    }

    /// 3 (GUI spec) — the review step for the safe batch.
    fn review_dialog(&mut self, ctx: &egui::Context) {
        let Some(review) = self.review.clone() else {
            return;
        };
        let t = self.tokens;
        w::backdrop(ctx);
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("Review cleanup")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::default()
                    .fill(t.surface)
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::same(18.0))
                    .stroke(egui::Stroke::new(1.0, t.divider)),
            )
            .show(ctx, |ui| {
                ui.set_max_width(390.0);
                // Headline what the dialog's own group rows add up to. Docker
                // reports its exact reclaim only when pruning, so a batch that
                // includes it is an upper bound, not a measurement.
                let promised: u64 = review.groups.iter().map(|(_, _, b)| *b).sum();
                let headline = if promised == 0 {
                    "Size reported after cleanup".to_string()
                } else if promised > review.estimate {
                    format!("Up to {} to reclaim", human(promised))
                } else {
                    format!("{} to reclaim", human(promised))
                };
                ui.label(RichText::new(headline).size(22.0).strong().color(t.safe));
                ui.add_space(10.0);
                for (label, count, bytes) in &review.groups {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(style::T_BODY).color(t.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(style::meta(
                                &t,
                                format!("{count} item{} · {}", w::plural(*count), human(*bytes)),
                            ));
                        });
                    });
                }
                ui.add_space(10.0);
                style::divider(ui, &t);
                ui.add_space(10.0);
                let lines: Vec<(String, Option<u64>)> = review
                    .jobs
                    .iter()
                    .map(|j| {
                        let size = j.size();
                        (j.label(), (size > 0).then_some(size))
                    })
                    .collect();
                w::item_manifest(ui, &t, &lines);
                ui.add_space(10.0);
                // node_modules folders live inside the user's projects, so the
                // blanket "no projects are touched" claim must not cover them.
                let has_nm = review
                    .jobs
                    .iter()
                    .any(|j| matches!(j, Job::NodeModules { .. }));
                let note = if has_nm {
                    "Caches are regenerable: your tools rebuild them when next needed. The \
                     selected node_modules folders are deleted permanently from inside your \
                     project directories — rebuild each project with npm / yarn / pnpm install."
                } else {
                    "These are regenerable caches: your tools rebuild them the next time they \
                     need them, so they may reappear later. No documents, projects or Git \
                     repositories are touched."
                };
                ui.label(
                    RichText::new(note)
                        .size(style::T_META)
                        .color(t.text_secondary),
                );
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.add(style::quiet_button(&t, "Cancel")).clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if promised > review.estimate {
                            format!("Clean up to {}", human(promised))
                        } else if promised > 0 {
                            format!("Clean {}", human(promised))
                        } else {
                            "Clean selected".to_string()
                        };
                        if ui.add(style::primary_button(&t, &label)).clicked() {
                            confirm = true;
                        }
                    });
                });
            });

        if confirm {
            self.review = None;
            // The batch consumes its selection: once these run, they must not
            // stay ticked into the next scan.
            for key in &review.keys {
                self.selected.remove(key);
            }
            self.run_jobs(review.jobs, review.estimate);
        } else if cancel {
            self.review = None;
        }
    }

    /// The separate, explicit path for irreversible deletion.
    fn danger_dialog(&mut self, ctx: &egui::Context) {
        let Some(danger) = self.danger.clone() else {
            return;
        };
        let t = self.tokens;
        w::backdrop(ctx);
        let mut confirm = false;
        let mut cancel = false;
        let Danger::DockerVolumes { names, total } = &danger;
        egui::Window::new("Delete volumes permanently")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::default()
                    .fill(t.surface)
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::same(18.0))
                    .stroke(egui::Stroke::new(1.5, t.danger.gamma_multiply(0.7))),
            )
            .show(ctx, |ui| {
                ui.set_max_width(390.0);
                ui.label(
                    RichText::new(format!(
                        "Permanently delete {} volume{}",
                        names.len(),
                        w::plural(names.len())
                    ))
                    .size(17.0)
                    .strong()
                    .color(t.danger),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "This destroys {} of container data — databases, uploads and anything else \
                         written to these volumes. It cannot be undone, and nothing rebuilds it.",
                        human(*total)
                    ))
                    .size(style::T_BODY)
                    .color(t.text),
                );
                ui.add_space(10.0);
                let lines: Vec<(String, Option<u64>)> =
                    names.iter().map(|n| (n.clone(), None)).collect();
                w::item_manifest(ui, &t, &lines);
                ui.add_space(12.0);
                ui.checkbox(
                    &mut self.danger_ack,
                    RichText::new("I understand this data cannot be recovered")
                        .size(style::T_META)
                        .color(t.text_secondary),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.add(style::quiet_button(&t, "Cancel")).clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let resp = ui.add_enabled(
                            self.danger_ack,
                            style::danger_button(&t, "Delete permanently"),
                        );
                        if resp.clicked() {
                            confirm = true;
                        }
                        if !self.danger_ack {
                            resp.on_disabled_hover_text(
                                "Tick the box above to confirm you understand",
                            );
                        }
                    });
                });
            });

        if confirm {
            self.danger = None;
            self.danger_ack = false;
            self.run_danger(danger);
        } else if cancel {
            self.danger = None;
            self.danger_ack = false;
        }
    }

    /// Confirmation for removing selected Docker images (regenerable, so this
    /// is the ordinary accent path — not the danger one).
    fn docker_action_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.docker_action.clone() else {
            return;
        };
        let t = self.tokens;
        w::backdrop(ctx);
        let mut confirm = false;
        let mut cancel = false;
        let DockerAction::RemoveImages { names, total, .. } = &action;
        egui::Window::new("Remove images")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::default()
                    .fill(t.surface)
                    .rounding(egui::Rounding::same(12.0))
                    .inner_margin(egui::Margin::same(18.0))
                    .stroke(egui::Stroke::new(1.0, t.divider)),
            )
            .show(ctx, |ui| {
                ui.set_max_width(390.0);
                ui.label(
                    RichText::new(format!("Remove {}", human(*total)))
                        .size(20.0)
                        .strong()
                        .color(t.safe),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "These images are not used by any container. Docker re-pulls or rebuilds \
                         them the next time something needs them.",
                    )
                    .size(style::T_META)
                    .color(t.text_secondary),
                );
                ui.add_space(10.0);
                let lines: Vec<(String, Option<u64>)> =
                    names.iter().map(|n| (n.clone(), None)).collect();
                w::item_manifest(ui, &t, &lines);
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.add(style::quiet_button(&t, "Cancel")).clicked() {
                        cancel = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label =
                            format!("Remove {} image{}", names.len(), w::plural(names.len()));
                        if ui.add(style::primary_button(&t, &label)).clicked() {
                            confirm = true;
                        }
                    });
                });
            });

        if confirm {
            self.docker_action = None;
            self.run_docker_action(action);
        } else if cancel {
            self.docker_action = None;
        }
    }
}

/// Run an external command, forwarding each stdout/stderr line to the UI so
/// progress is visible live. Blocks until the command exits; returns whether
/// it ran and exited successfully.
fn stream(tx: &Sender<Msg>, ctx: &egui::Context, prog: &str, args: &[&str]) -> bool {
    let mut child = match Command::new(prog)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Msg::Log(format!("could not run {prog}: {e}")));
            ctx.request_repaint();
            return false;
        }
    };
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let _ = tx.send(Msg::Log(line));
            ctx.request_repaint();
        }
    }
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.send(Msg::Log(line));
            ctx.request_repaint();
        }
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}
