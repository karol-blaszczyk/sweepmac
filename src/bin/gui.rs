//! sweepmac GUI — a small, modern native window over the shared cleanup core.
//!
//! Each cache, plus Docker and old simulators, is cleared independently via its
//! own button and confirmation dialog — no bulk checkboxes. Scanning and
//! cleaning run on background threads; the worker talks back over a channel and
//! wakes the UI with `request_repaint`.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::Instant;

use eframe::egui;
use egui::{Color32, Margin, RichText, Rounding};
use sweepmac::{
    clean_target, dir_size, disk_free, docker_info, find_node_modules, home, human,
    persist_pty_limit, pty_status, raise_pty_limit, scan, target_by_id, DockerInfo, DockerVolume,
    NodeModules, PtyLevel, PtyStatus, ALL_CATEGORIES,
};

const PTY_TARGET: u64 = 999;

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([560.0, 720.0])
        .with_min_inner_size([460.0, 480.0])
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
            install_style(&cc.egui_ctx);
            Ok(Box::new(App::new(cc.egui_ctx.clone())))
        }),
    )
}

// --- palette -------------------------------------------------------------

const BG: Color32 = Color32::from_rgb(0x14, 0x16, 0x1b);
const CARD: Color32 = Color32::from_rgb(0x1e, 0x21, 0x2a);
const CARD_HOVER: Color32 = Color32::from_rgb(0x26, 0x2a, 0x35);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xee);
const MUTED: Color32 = Color32::from_rgb(0x8a, 0x90, 0x9e);
const ACCENT: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
const DANGER: Color32 = Color32::from_rgb(0xe5, 0x4b, 0x4b);
const NM: Color32 = Color32::from_rgb(0x3f, 0xc1, 0x80);
const AMBER: Color32 = Color32::from_rgb(0xf2, 0xae, 0x3c);

fn category_color(cat: &str) -> Color32 {
    match cat {
        "system" => Color32::from_rgb(0x5a, 0xd0, 0xd6),
        "macos" => Color32::from_rgb(0x9a, 0xa0, 0xae),
        "dev" => Color32::from_rgb(0x4c, 0x8d, 0xff),
        "xcode" => Color32::from_rgb(0xff, 0x9f, 0x43),
        "app" => Color32::from_rgb(0xa9, 0x6b, 0xff),
        _ => MUTED,
    }
}

fn install_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.window_stroke = egui::Stroke::new(1.0, Color32::from_rgb(0x33, 0x37, 0x42));
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.4);
    let r = Rounding::same(8.0);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
        &mut v.widgets.noninteractive,
    ] {
        w.rounding = r;
    }
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style(style);
}

// --- state ---------------------------------------------------------------

struct Row {
    id: &'static str,
    category: &'static str,
    desc: &'static str,
    path: PathBuf,
    size: u64,
}

/// One node_modules folder queued for bulk deletion.
#[derive(Clone)]
struct BulkItem {
    label: String,
    path: PathBuf,
    size: u64,
}

/// What a confirmation dialog is about to do.
#[derive(Clone)]
enum Pending {
    Cache {
        id: &'static str,
        desc: String,
        size: u64,
        path: PathBuf,
    },
    Simulators {
        size: u64,
    },
    /// Prune a single Docker category (kind: builder|image|container|network).
    DockerPrune {
        kind: &'static str,
        label: String,
        size: u64,
    },
    /// Delete specific Docker volumes (destroys their data).
    DockerVolumes {
        names: Vec<String>,
        total: u64,
    },
    /// Delete one or more selected node_modules folders.
    NodeModulesBulk {
        items: Vec<BulkItem>,
        total: u64,
    },
}

enum Msg {
    Scanned {
        rows: Vec<Row>,
        extras: Vec<(String, u64)>,
        disk: (u64, u64),
        pty: Option<PtyStatus>,
    },
    /// Docker breakdown finished (separate — `docker system df -v` is slow).
    DockerScanned(Option<DockerInfo>),
    /// node_modules discovery finished (runs separately — it's slower).
    NmScanned(Vec<NodeModules>),
    /// A live output line from a running operation.
    Log(String),
    Cleaned {
        freed: u64,
    },
    /// One node_modules dir was removed — drop it from the list as we go.
    NmRemoved {
        path: PathBuf,
    },
    /// A node_modules (bulk) delete finished — settle UI, no cache rescan.
    NmDone {
        freed: u64,
    },
    /// A pty-limit admin action finished.
    PtyDone {
        note: String,
    },
}

#[derive(PartialEq)]
enum Phase {
    Scanning,
    Idle,
    Cleaning,
}

struct App {
    ctx: egui::Context,
    phase: Phase,
    rows: Vec<Row>,
    extras: Vec<(String, u64)>,
    disk: (u64, u64),
    pty: Option<PtyStatus>,
    docker: Option<DockerInfo>,
    docker_scanning: bool,
    dvol_selected: std::collections::HashSet<String>,
    node_modules: Vec<NodeModules>,
    nm_selected: std::collections::HashSet<PathBuf>,
    nm_scanning: bool,
    pending: Option<Pending>,
    status: String,
    log: Vec<String>,
    started: Option<Instant>,
    rx: Option<Receiver<Msg>>,
    nm_rx: Option<Receiver<Msg>>,
    docker_rx: Option<Receiver<Msg>>,
}

impl App {
    fn new(ctx: egui::Context) -> Self {
        let mut app = App {
            ctx,
            phase: Phase::Idle,
            rows: Vec::new(),
            extras: Vec::new(),
            disk: (0, 0),
            pty: None,
            docker: None,
            docker_scanning: false,
            dvol_selected: std::collections::HashSet::new(),
            node_modules: Vec::new(),
            nm_selected: std::collections::HashSet::new(),
            nm_scanning: false,
            pending: None,
            status: String::new(),
            log: Vec::new(),
            started: None,
            rx: None,
            nm_rx: None,
            docker_rx: None,
        };
        app.start_scan();
        app.start_nm_scan();
        app.start_docker_scan();
        app
    }

    /// Query Docker on its own thread — `docker system df -v` can take ~15s, so
    /// it must not block the cache view or button interactivity.
    fn start_docker_scan(&mut self) {
        self.docker_scanning = true;
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.docker_rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let info = docker_info();
            let _ = tx.send(Msg::DockerScanned(info));
            ctx.request_repaint();
        });
    }

    /// Discover node_modules dirs on a background thread (slower than caches,
    /// so it runs on its own channel and doesn't block the cache view).
    fn start_nm_scan(&mut self) {
        self.nm_scanning = true;
        self.nm_selected.clear();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.nm_rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let found = home().map(|h| find_node_modules(&h)).unwrap_or_default();
            let _ = tx.send(Msg::NmScanned(found));
            ctx.request_repaint();
        });
    }

    fn nm_total(&self) -> u64 {
        self.node_modules.iter().map(|n| n.size).sum()
    }

    fn nm_selected_total(&self) -> u64 {
        self.node_modules
            .iter()
            .filter(|n| self.nm_selected.contains(&n.path))
            .map(|n| n.size)
            .sum()
    }

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
            // Only the iOS simulator size here (fast-ish). Docker is scanned
            // separately because `docker system df -v` is slow.
            let extras = home()
                .ok()
                .map(|h| h.join("Library/Developer/CoreSimulator"))
                .filter(|p| p.exists())
                .map(|p| dir_size(&p))
                .filter(|s| *s > 0)
                .map(|s| vec![("iOS Simulators (CoreSimulator)".to_string(), s)])
                .unwrap_or_default();
            let disk = disk_free().unwrap_or((0, 0));
            let pty = pty_status();
            let _ = tx.send(Msg::Scanned {
                rows,
                extras,
                disk,
                pty,
            });
            ctx.request_repaint();
        });
    }

    /// Run a pty-limit admin action (raise or persist) on a background thread.
    fn run_pty(&mut self, persist: bool) {
        self.phase = Phase::Cleaning;
        self.log.clear();
        self.started = Some(Instant::now());
        self.status = "Awaiting admin password…".into();
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let ctx = self.ctx.clone();
        thread::spawn(move || {
            let action = if persist {
                persist_pty_limit(PTY_TARGET)
            } else {
                raise_pty_limit(PTY_TARGET)
            };
            let note = match action {
                Ok(_) => {
                    let _ = tx.send(Msg::Log(format!(
                        "✓ kern.tty.ptmx_max set to {PTY_TARGET}{}",
                        if persist {
                            " (persisted to /etc/sysctl.conf)"
                        } else {
                            ""
                        }
                    )));
                    format!("pty limit raised to {PTY_TARGET}")
                }
                Err(e) if e == "cancelled" => {
                    let _ = tx.send(Msg::Log("✗ cancelled at password prompt".into()));
                    "cancelled".into()
                }
                Err(e) => {
                    let _ = tx.send(Msg::Log(format!("✗ {e}")));
                    "failed".into()
                }
            };
            let _ = tx.send(Msg::PtyDone { note });
            ctx.request_repaint();
        });
    }

    /// Execute whatever the confirmation approved.
    fn run_pending(&mut self, p: Pending) {
        self.phase = Phase::Cleaning;
        self.log.clear();
        self.started = Some(Instant::now());
        let (tx, rx): (Sender<Msg>, Receiver<Msg>) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        let ctx = self.ctx.clone();
        match p {
            Pending::Cache {
                id,
                desc,
                size,
                path,
            } => {
                self.status = format!("Clearing {desc}…");
                thread::spawn(move || {
                    let _ = tx.send(Msg::Log(format!("Removing {}", path.display())));
                    ctx.request_repaint();
                    let freed = match target_by_id(id) {
                        Some(t) if clean_target(t, &path).is_ok() => {
                            let _ =
                                tx.send(Msg::Log(format!("✓ cleared {desc} ({})", human(size))));
                            size
                        }
                        _ => {
                            let _ = tx.send(Msg::Log(format!("✗ failed to clear {desc}")));
                            0
                        }
                    };
                    let _ = tx.send(Msg::Cleaned { freed });
                    ctx.request_repaint();
                });
            }
            Pending::DockerPrune { kind, label, .. } => {
                self.status = format!("Pruning {label}…");
                thread::spawn(move || {
                    let args: &[&str] = match kind {
                        "builder" => &["builder", "prune", "-af"],
                        "image" => &["image", "prune", "-af"],
                        "container" => &["container", "prune", "-f"],
                        "network" => &["network", "prune", "-f"],
                        _ => &[],
                    };
                    let _ = tx.send(Msg::Log(format!("$ docker {}", args.join(" "))));
                    ctx.request_repaint();
                    stream(&tx, &ctx, "docker", args);
                    let _ = tx.send(Msg::Cleaned { freed: 0 });
                    ctx.request_repaint();
                });
            }
            Pending::DockerVolumes { names, total } => {
                self.status = format!("Deleting {} volume(s) ({})…", names.len(), human(total));
                thread::spawn(move || {
                    for name in &names {
                        let _ = tx.send(Msg::Log(format!("$ docker volume rm {name}")));
                        ctx.request_repaint();
                        stream(&tx, &ctx, "docker", &["volume", "rm", name]);
                    }
                    let _ = tx.send(Msg::Cleaned { freed: 0 });
                    ctx.request_repaint();
                });
            }
            Pending::Simulators { .. } => {
                self.status = "Deleting unavailable simulators…".into();
                thread::spawn(move || {
                    let _ = tx.send(Msg::Log("$ xcrun simctl delete unavailable".into()));
                    ctx.request_repaint();
                    stream(&tx, &ctx, "xcrun", &["simctl", "delete", "unavailable"]);
                    let _ = tx.send(Msg::Log(
                        "done — this can take a minute on a big device set".into(),
                    ));
                    let _ = tx.send(Msg::Cleaned { freed: 0 });
                    ctx.request_repaint();
                });
            }
            Pending::NodeModulesBulk { items, total } => {
                self.status = format!("Removing {} folders ({})…", items.len(), human(total));
                thread::spawn(move || {
                    let mut freed = 0u64;
                    for item in items {
                        let _ = tx.send(Msg::Log(format!("rm -rf {}", item.path.display())));
                        ctx.request_repaint();
                        match std::fs::remove_dir_all(&item.path) {
                            Ok(()) => {
                                freed += item.size;
                                let _ = tx.send(Msg::Log(format!(
                                    "✓ {} ({})",
                                    item.label,
                                    human(item.size)
                                )));
                                let _ = tx.send(Msg::NmRemoved { path: item.path });
                            }
                            Err(e) => {
                                let _ = tx.send(Msg::Log(format!("✗ {}: {e}", item.label)));
                            }
                        }
                        ctx.request_repaint();
                    }
                    let _ = tx.send(Msg::NmDone { freed });
                    ctx.request_repaint();
                });
            }
        }
    }

    fn cache_total(&self) -> u64 {
        self.rows.iter().map(|r| r.size).sum()
    }

    fn elapsed_secs(&self) -> u64 {
        self.started.map(|t| t.elapsed().as_secs()).unwrap_or(0)
    }

    fn poll(&mut self) {
        // Drain everything available this frame (streaming sends many lines).
        let mut batch: Vec<Msg> = Vec::new();
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                batch.push(msg);
            }
        }
        for msg in batch {
            match msg {
                Msg::Log(line) => {
                    self.log.push(line);
                    let overflow = self.log.len().saturating_sub(200);
                    if overflow > 0 {
                        self.log.drain(0..overflow);
                    }
                }
                Msg::Scanned {
                    rows,
                    extras,
                    disk,
                    pty,
                } => {
                    self.rows = rows;
                    self.extras = extras;
                    self.disk = disk;
                    self.pty = pty;
                    self.phase = Phase::Idle;
                    self.started = None;
                    self.status = format!("{} reclaimable", human(self.cache_total()));
                    self.rx = None;
                }
                Msg::DockerScanned(_) => {} // arrives on docker_rx, handled below
                Msg::PtyDone { note } => {
                    self.phase = Phase::Idle;
                    self.started = None;
                    self.pty = pty_status(); // cheap re-check on the main thread
                    self.status = note;
                    self.rx = None;
                }
                Msg::Cleaned { freed } => {
                    self.phase = Phase::Idle;
                    self.started = None;
                    self.dvol_selected.clear();
                    self.status = if freed > 0 {
                        format!("Reclaimed {}", human(freed))
                    } else {
                        "Done".into()
                    };
                    self.start_scan(); // replaces self.rx with the scan channel
                    self.start_docker_scan(); // refresh Docker too (e.g. after prune)
                }
                Msg::NmRemoved { path } => {
                    self.node_modules.retain(|n| n.path != path);
                    self.nm_selected.remove(&path);
                }
                Msg::NmDone { freed } => {
                    self.phase = Phase::Idle;
                    self.started = None;
                    self.status = format!("Reclaimed {}", human(freed));
                    self.rx = None;
                }
                Msg::NmScanned(_) => {} // arrives on nm_rx, handled below
            }
        }

        // node_modules discovery runs on its own channel.
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

        // Docker scan runs on its own (slow) channel.
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

// --- rendering -----------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.phase != Phase::Idle {
            ctx.request_repaint();
        }

        self.top_panel(ctx);
        self.bottom_panel(ctx);
        self.central(ctx);
        self.confirm_dialog(ctx);
    }
}

impl App {
    fn top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(16.0, 14.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🧹").size(24.0));
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("sweepmac").size(20.0).strong().color(TEXT));
                        ui.label(
                            RichText::new("Regenerable caches — safe to clear")
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let busy = self.phase != Phase::Idle;
                        if ui.add_enabled(!busy, ghost_button("↻  Rescan")).clicked() {
                            self.start_scan();
                        }
                    });
                });

                // Disk usage bar.
                let (free, total) = self.disk;
                if total > 0 {
                    ui.add_space(12.0);
                    let used = total.saturating_sub(free);
                    let frac = used as f32 / total as f32;
                    let bar = egui::ProgressBar::new(frac)
                        .desired_height(10.0)
                        .rounding(Rounding::same(5.0))
                        .fill(if frac > 0.9 { DANGER } else { ACCENT });
                    ui.add(bar);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} free", human(free)))
                                .size(11.0)
                                .color(MUTED),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Compact pty indicator on the right of the disk line.
                            if let Some(pty) = self.pty {
                                let c = match pty.level {
                                    PtyLevel::Ok => MUTED,
                                    PtyLevel::Warn => AMBER,
                                    PtyLevel::Critical => DANGER,
                                };
                                ui.label(
                                    RichText::new(format!("· pty {}/{}", pty.nodes, pty.cap))
                                        .size(11.0)
                                        .color(c),
                                );
                            }
                            ui.label(
                                RichText::new(format!("of {}", human(total)))
                                    .size(11.0)
                                    .color(MUTED),
                            );
                        });
                    });
                }
            });
    }

    fn bottom_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bottom")
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(16.0, 10.0)),
            )
            .show(ctx, |ui| {
                // Live activity log — visible while running or right after.
                let running = self.phase == Phase::Cleaning;
                if running || !self.log.is_empty() {
                    egui::Frame::default()
                        .fill(Color32::from_rgb(0x10, 0x12, 0x16))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Activity").size(11.0).strong().color(MUTED),
                                );
                                if running {
                                    ui.label(
                                        RichText::new(format!("• {}s", self.elapsed_secs()))
                                            .size(11.0)
                                            .color(ACCENT),
                                    );
                                }
                            });
                            egui::ScrollArea::vertical()
                                .max_height(96.0)
                                .stick_to_bottom(true)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    if self.log.is_empty() {
                                        ui.label(
                                            RichText::new("starting…")
                                                .size(11.5)
                                                .monospace()
                                                .color(MUTED),
                                        );
                                    }
                                    for line in &self.log {
                                        ui.label(
                                            RichText::new(line).size(11.5).monospace().color(MUTED),
                                        );
                                    }
                                });
                        });
                    ui.add_space(8.0);
                }

                ui.horizontal(|ui| {
                    if running {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("{} ({}s)", self.status, self.elapsed_secs()))
                                .size(12.0)
                                .color(MUTED),
                        );
                    } else {
                        ui.label(RichText::new(&self.status).size(12.0).color(MUTED));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} in caches", human(self.cache_total())))
                                .size(12.0)
                                .strong()
                                .color(TEXT),
                        );
                    });
                });
            });
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(BG).inner_margin(Margin::symmetric(14.0, 8.0)))
            .show(ctx, |ui| {
                if self.phase == Phase::Scanning && self.rows.is_empty() {
                    ui.centered_and_justified(|ui| ui.add(egui::Spinner::new().size(28.0)));
                    return;
                }

                // Only a running *cleanup* blocks actions — a background (re)scan
                // must not grey out the whole UI.
                let can_act = self.phase != Phase::Cleaning;
                // Collect actions to run after the borrow ends (avoids borrow conflict).
                let mut request: Option<Pending> = None;
                let mut rescan_nm = false;
                let mut pty_action: Option<bool> = None; // Some(persist?)
                // node_modules selection: snapshot for borrow-safe checkbox rendering.
                let nm_sel = self.nm_selected.clone();
                let mut nm_toggles: Vec<PathBuf> = Vec::new();
                let mut nm_set_all: Option<bool> = None;
                let mut nm_clear_selected = false;
                // Docker volume selection snapshot.
                let dvol_sel = self.dvol_selected.clone();
                let mut dvol_toggles: Vec<String> = Vec::new();
                let mut dvol_delete = false;

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    // Pseudo-terminal warning banner (only when near/at the cap).
                    if let Some(pty) = self.pty {
                        if pty.level != PtyLevel::Ok {
                            if let Some(act) = pty_banner(ui, &pty, can_act) {
                                pty_action = Some(act);
                            }
                        }
                    }

                    for cat in ALL_CATEGORIES {
                        // Only show caches with something to clear; skip empties.
                        let idxs: Vec<usize> = self
                            .rows
                            .iter()
                            .enumerate()
                            .filter(|(_, r)| &r.category == cat && r.size > 0)
                            .map(|(i, _)| i)
                            .collect();
                        if idxs.is_empty() {
                            continue;
                        }
                        section_header(ui, cat);
                        card(ui, |ui| {
                            for (n, i) in idxs.iter().enumerate() {
                                if n > 0 {
                                    ui.add_space(2.0);
                                }
                                let r = &self.rows[*i];
                                if let Some(p) = cache_row(ui, r, can_act) {
                                    request = Some(p);
                                }
                            }
                        });
                    }

                    // Note how many empty caches are tracked but hidden.
                    let hidden = self.rows.iter().filter(|r| r.size == 0).count();
                    if hidden > 0 {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("{hidden} empty caches tracked (hidden)"))
                                .size(10.5)
                                .color(MUTED),
                        );
                    }

                    // Docker — granular prune actions + per-volume management.
                    if self.docker.is_none() && self.docker_scanning {
                        section_header(ui, "docker");
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(12.0));
                            ui.label(
                                RichText::new("querying Docker (docker system df)…")
                                    .size(11.5)
                                    .color(MUTED),
                            );
                        });
                    }
                    if let Some(d) = &self.docker {
                        section_header(ui, "docker");
                        card(ui, |ui| {
                            let cats = [
                                ("builder", "Build cache", d.build_cache),
                                ("image", "Unused images", d.images),
                                ("container", "Stopped containers", d.containers),
                            ];
                            for (n, (kind, label, size)) in cats.into_iter().enumerate() {
                                if n > 0 {
                                    ui.add_space(2.0);
                                }
                                if docker_prune_row(ui, label, size, can_act, false) {
                                    request = Some(Pending::DockerPrune {
                                        kind,
                                        label: label.to_string(),
                                        size,
                                    });
                                }
                            }
                            // Networks: no size, count-less; offer a plain prune.
                            ui.add_space(2.0);
                            if docker_prune_row(ui, "Unused networks", 0, can_act, true) {
                                request = Some(Pending::DockerPrune {
                                    kind: "network",
                                    label: "Unused networks".into(),
                                    size: 0,
                                });
                            }
                        });

                        // Volumes (data!) — pick exactly which to delete.
                        if !d.volumes.is_empty() {
                            let dsel_count =
                                d.volumes.iter().filter(|v| dvol_sel.contains(&v.name)).count();
                            let dsel_total: u64 = d
                                .volumes
                                .iter()
                                .filter(|v| dvol_sel.contains(&v.name))
                                .map(|v| v.size)
                                .sum();
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Volumes — contain data; deleting is permanent")
                                    .size(11.0)
                                    .strong()
                                    .color(DANGER),
                            );
                            card(ui, |ui| {
                                for (n, v) in d.volumes.iter().enumerate() {
                                    if n > 0 {
                                        ui.add_space(2.0);
                                    }
                                    let checked = dvol_sel.contains(&v.name);
                                    if docker_volume_row(ui, v, checked, can_act) {
                                        dvol_toggles.push(v.name.clone());
                                    }
                                }
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(if dsel_count > 0 {
                                        format!("{dsel_count} selected · {}", human(dsel_total))
                                    } else {
                                        "Tick unused volumes to delete".into()
                                    })
                                    .size(11.5)
                                    .color(MUTED),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let enabled = can_act && dsel_count > 0;
                                    let btn = danger_button(&format!("Delete {dsel_count} volume(s)"));
                                    if ui.add_enabled(enabled, btn).clicked() {
                                        dvol_delete = true;
                                    }
                                });
                            });
                        }
                        ui.label(
                            RichText::new("Prune frees space inside the VM (keeps running containers). It does not shrink the ~/.colima disk image on your Mac.")
                                .size(10.5)
                                .color(MUTED),
                        );
                    }

                    // iOS simulators (from extras; Docker is handled above).
                    let sim_extras: Vec<&(String, u64)> =
                        self.extras.iter().filter(|(l, _)| !l.contains("Docker")).collect();
                    if !sim_extras.is_empty() {
                        section_header(ui, "extras");
                        card(ui, |ui| {
                            for (label, size) in sim_extras {
                                if extra_row(ui, label, *size, "Delete", can_act) {
                                    request = Some(Pending::Simulators { size: *size });
                                }
                            }
                        });
                    }

                    // node_modules — discovered project dependency folders.
                    let sel_count = self.node_modules.iter().filter(|n| nm_sel.contains(&n.path)).count();
                    let sel_total = self.nm_selected_total();
                    let all_selected = !self.node_modules.is_empty() && sel_count == self.node_modules.len();

                    ui.add_space(12.0);

                    if self.node_modules.is_empty() {
                        ui.horizontal(|ui| {
                            ui.colored_label(NM, RichText::new("●").size(9.0));
                            ui.label(RichText::new("NODE_MODULES").size(11.5).strong().color(MUTED));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if self.nm_scanning {
                                    ui.add(egui::Spinner::new().size(12.0));
                                } else if ui.add(ghost_button("↻ Rescan")).clicked() {
                                    rescan_nm = true;
                                }
                            });
                        });
                        let msg = if self.nm_scanning {
                            "scanning your home folder for node_modules…"
                        } else {
                            "none found"
                        };
                        ui.label(RichText::new(msg).size(11.5).color(MUTED));
                    } else {
                        // Collapsible: the folder list lives in the body; the header
                        // and the bulk-action bar stay visible even when collapsed.
                        let id = ui.make_persistent_id("nm_section");
                        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                            .show_header(ui, |ui| {
                                ui.colored_label(NM, RichText::new("●").size(9.0));
                                ui.label(RichText::new("NODE_MODULES").size(11.5).strong().color(MUTED));
                                ui.label(
                                    RichText::new(format!("· {} · {}", self.node_modules.len(), human(self.nm_total())))
                                        .size(11.0)
                                        .color(MUTED),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if self.nm_scanning {
                                        ui.add(egui::Spinner::new().size(12.0));
                                    } else {
                                        if ui.add(ghost_button("↻ Rescan")).clicked() {
                                            rescan_nm = true;
                                        }
                                        let label = if all_selected { "Select none" } else { "Select all" };
                                        if ui.add(ghost_button(label)).clicked() {
                                            nm_set_all = Some(!all_selected);
                                        }
                                    }
                                });
                            })
                            .body(|ui| {
                                card(ui, |ui| {
                                    for (n, nm) in self.node_modules.iter().enumerate() {
                                        if n > 0 {
                                            ui.add_space(2.0);
                                        }
                                        let checked = nm_sel.contains(&nm.path);
                                        if nm_row(ui, nm, checked, can_act) {
                                            nm_toggles.push(nm.path.clone());
                                        }
                                    }
                                });
                            });

                        ui.add_space(6.0);
                        // Bulk action bar — always visible, even when collapsed.
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(if sel_count > 0 {
                                    format!("{sel_count} selected · {}", human(sel_total))
                                } else {
                                    "Expand and tick folders to delete in bulk".into()
                                })
                                .size(11.5)
                                .color(MUTED),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let enabled = can_act && sel_count > 0;
                                let btn = danger_button(&format!("Delete {sel_count} selected"));
                                if ui.add_enabled(enabled, btn).clicked() {
                                    nm_clear_selected = true;
                                }
                            });
                        });
                        ui.label(
                            RichText::new("Safe to delete — rebuild any project later with npm / yarn / pnpm install.")
                                .size(10.5)
                                .color(MUTED),
                        );
                    }

                    ui.add_space(8.0);
                });

                // Apply node_modules selection changes after the render borrow ends.
                for p in nm_toggles {
                    if !self.nm_selected.remove(&p) {
                        self.nm_selected.insert(p);
                    }
                }
                if let Some(all) = nm_set_all {
                    if all {
                        self.nm_selected = self.node_modules.iter().map(|n| n.path.clone()).collect();
                    } else {
                        self.nm_selected.clear();
                    }
                }
                if nm_clear_selected {
                    let items: Vec<BulkItem> = self
                        .node_modules
                        .iter()
                        .filter(|n| self.nm_selected.contains(&n.path))
                        .map(|n| BulkItem { label: n.label.clone(), path: n.path.clone(), size: n.size })
                        .collect();
                    let total = items.iter().map(|i| i.size).sum();
                    if !items.is_empty() {
                        request = Some(Pending::NodeModulesBulk { items, total });
                    }
                }

                // Apply Docker volume selection changes.
                for name in dvol_toggles {
                    if !self.dvol_selected.remove(&name) {
                        self.dvol_selected.insert(name);
                    }
                }
                if dvol_delete {
                    if let Some(d) = &self.docker {
                        let chosen: Vec<&DockerVolume> = d
                            .volumes
                            .iter()
                            .filter(|v| self.dvol_selected.contains(&v.name))
                            .collect();
                        let total = chosen.iter().map(|v| v.size).sum();
                        let names: Vec<String> = chosen.iter().map(|v| v.name.clone()).collect();
                        if !names.is_empty() {
                            request = Some(Pending::DockerVolumes { names, total });
                        }
                    }
                }

                if let Some(p) = request {
                    self.pending = Some(p);
                }
                if rescan_nm {
                    self.start_nm_scan();
                }
                if let Some(persist) = pty_action {
                    self.run_pty(persist);
                }
            });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        let (title, body, freed) = match &pending {
            Pending::Cache { desc, size, .. } => (
                "Clear cache",
                format!("Clear “{desc}” and reclaim {}?", human(*size)),
                *size,
            ),
            Pending::DockerPrune { label, size, .. } => (
                "Prune Docker",
                if *size > 0 {
                    format!("Prune {} (~{})? Running containers + volumes are kept.", label.to_lowercase(), human(*size))
                } else {
                    format!("Prune {}? Running containers + volumes are kept.", label.to_lowercase())
                },
                *size,
            ),
            Pending::DockerVolumes { names, total } => (
                "Delete Docker volumes",
                format!(
                    "⚠ Permanently delete {} volume{} and ALL their data ({})?\n\nThis cannot be undone.",
                    names.len(),
                    if names.len() == 1 { "" } else { "s" },
                    human(*total)
                ),
                *total,
            ),
            Pending::Simulators { size } => (
                "Delete simulators",
                format!("Delete unavailable iOS simulators (~{})?", human(*size)),
                *size,
            ),
            Pending::NodeModulesBulk { items, total } => (
                "Delete node_modules",
                format!(
                    "Delete {} node_modules folder{} and reclaim {}?\n\nRebuild later with npm / yarn / pnpm install.",
                    items.len(),
                    if items.len() == 1 { "" } else { "s" },
                    human(*total)
                ),
                *total,
            ),
        };

        // Dim the backdrop for a modal feel.
        egui::Area::new(egui::Id::new("backdrop"))
            .order(egui::Order::Background)
            .show(ctx, |ui| {
                let r = ui.max_rect();
                ui.painter()
                    .rect_filled(r, 0.0, Color32::from_black_alpha(120));
            });

        let mut close = false;
        let mut confirm = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::default()
                    .fill(CARD)
                    .rounding(Rounding::same(12.0))
                    .inner_margin(Margin::same(18.0))
                    .stroke(egui::Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3f, 0x4c))),
            )
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                ui.label(RichText::new(body).size(13.5).color(TEXT));
                let _ = freed;
                // For bulk node_modules, list exactly what will be deleted.
                if let Pending::NodeModulesBulk { items, .. } = &pending {
                    ui.add_space(8.0);
                    egui::Frame::default()
                        .fill(Color32::from_rgb(0x10, 0x12, 0x16))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .max_height(160.0)
                                .show(ui, |ui| {
                                    for it in items {
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&it.label)
                                                        .size(11.0)
                                                        .monospace()
                                                        .color(MUTED),
                                                )
                                                .truncate(),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(human(it.size))
                                                            .size(11.0)
                                                            .monospace()
                                                            .color(MUTED),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                });
                        });
                }
                // For volume deletion, list the volume names being destroyed.
                if let Pending::DockerVolumes { names, .. } = &pending {
                    ui.add_space(8.0);
                    egui::Frame::default()
                        .fill(Color32::from_rgb(0x10, 0x12, 0x16))
                        .rounding(Rounding::same(8.0))
                        .inner_margin(Margin::symmetric(10.0, 8.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            for name in names {
                                ui.label(RichText::new(name).size(11.0).monospace().color(MUTED));
                            }
                        });
                }
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    if ui.add(ghost_button("Cancel")).clicked() {
                        close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(danger_button("Clear")).clicked() {
                            confirm = true;
                        }
                    });
                });
            });

        if confirm {
            self.pending = None;
            self.run_pending(pending);
        } else if close {
            self.pending = None;
        }
    }
}

// --- widgets -------------------------------------------------------------

fn section_header(ui: &mut egui::Ui, cat: &str) {
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.colored_label(category_color(cat), RichText::new("●").size(9.0));
        let label = if cat == "app" {
            "app · re-downloads".to_string()
        } else {
            cat.to_uppercase()
        };
        ui.label(RichText::new(label).size(11.5).strong().color(MUTED));
    });
    ui.add_space(2.0);
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::default()
        .fill(CARD)
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(12.0, 10.0))
        .show(ui, add)
        .inner
}

/// One cache row. Returns `Some(Pending)` when its Clear button is clicked.
fn cache_row(ui: &mut egui::Ui, r: &Row, can_act: bool) -> Option<Pending> {
    let mut out = None;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(r.desc)
                .size(13.0)
                .color(if r.size > 0 { TEXT } else { MUTED }),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if r.size > 0 {
                if ui.add_enabled(can_act, danger_button("Clear")).clicked() {
                    out = Some(Pending::Cache {
                        id: r.id,
                        desc: r.desc.to_string(),
                        size: r.size,
                        path: r.path.clone(),
                    });
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(human(r.size))
                        .size(12.5)
                        .monospace()
                        .color(TEXT),
                );
            } else {
                ui.label(RichText::new("empty").size(11.5).color(MUTED));
            }
        });
    });
    out
}

/// One extras row (Docker / simulators). Returns true when its button clicks.
fn extra_row(ui: &mut egui::Ui, label: &str, size: u64, verb: &str, can_act: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(13.0)
                .color(if size > 0 { TEXT } else { MUTED }),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if size > 0 {
                if ui.add_enabled(can_act, accent_button(verb)).clicked() {
                    clicked = true;
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(human(size))
                        .size(12.5)
                        .monospace()
                        .color(TEXT),
                );
            } else {
                ui.label(RichText::new("—").size(12.0).color(MUTED));
            }
        });
    });
    clicked
}

/// One Docker prune category row. Returns true when "Prune" is clicked.
/// `allow_empty` lets count-less actions (networks) stay clickable at size 0.
fn docker_prune_row(
    ui: &mut egui::Ui,
    label: &str,
    size: u64,
    can_act: bool,
    allow_empty: bool,
) -> bool {
    let mut clicked = false;
    let actionable = size > 0 || allow_empty;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(13.0)
                .color(if actionable { TEXT } else { MUTED }),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(can_act && actionable, accent_button("Prune"))
                .clicked()
            {
                clicked = true;
            }
            ui.add_space(8.0);
            let txt = if size > 0 {
                human(size)
            } else {
                "—".to_string()
            };
            ui.label(
                RichText::new(txt)
                    .size(12.5)
                    .monospace()
                    .color(if size > 0 { TEXT } else { MUTED }),
            );
        });
    });
    clicked
}

/// One Docker volume row with a checkbox. In-use volumes are locked (can't be
/// removed while a container mounts them). Returns true when the box toggles.
fn docker_volume_row(ui: &mut egui::Ui, v: &DockerVolume, checked: bool, can_act: bool) -> bool {
    let mut toggled = false;
    let selectable = can_act && !v.in_use;
    ui.horizontal(|ui| {
        let mut c = checked;
        if ui
            .add_enabled(selectable, egui::Checkbox::new(&mut c, ""))
            .changed()
        {
            toggled = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(human(v.size))
                    .size(12.5)
                    .monospace()
                    .color(TEXT),
            );
            ui.add_space(8.0);
            if v.in_use {
                ui.label(RichText::new("in use").size(10.5).color(AMBER));
                ui.add_space(6.0);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let color = if v.in_use { MUTED } else { TEXT };
                ui.add(egui::Label::new(RichText::new(&v.name).size(12.0).color(color)).truncate());
            });
        });
    });
    toggled
}

/// One node_modules row with a checkbox. Returns true when the checkbox toggles.
fn nm_row(ui: &mut egui::Ui, nm: &NodeModules, checked: bool, can_act: bool) -> bool {
    let mut toggled = false;
    ui.horizontal(|ui| {
        let mut c = checked;
        if ui
            .add_enabled(can_act, egui::Checkbox::new(&mut c, ""))
            .changed()
        {
            toggled = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(human(nm.size))
                    .size(12.5)
                    .monospace()
                    .color(TEXT),
            );
            ui.add_space(8.0);
            // Path fills the remaining space, truncated with an ellipsis.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let color = if checked { TEXT } else { MUTED };
                ui.add(
                    egui::Label::new(RichText::new(&nm.label).size(12.0).color(color)).truncate(),
                );
            });
        });
    });
    toggled
}

/// Pseudo-terminal warning banner. Returns `Some(persist?)` when an action is
/// clicked: `Some(false)` = raise now, `Some(true)` = raise + persist.
fn pty_banner(ui: &mut egui::Ui, pty: &PtyStatus, can_act: bool) -> Option<bool> {
    let mut action = None;
    let critical = pty.level == PtyLevel::Critical;
    let accent = if critical { DANGER } else { AMBER };
    let title = if critical {
        "⚠ macOS terminal limit reached"
    } else {
        "⚠ macOS terminal slots running low"
    };

    egui::Frame::default()
        .fill(accent.gamma_multiply(0.14))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(12.0, 10.0))
        .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.6)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(13.0).strong().color(accent));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!("{} / {} nodes", pty.nodes, pty.cap))
                            .size(12.0)
                            .monospace()
                            .color(accent),
                    );
                });
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Old terminal device nodes pile up over long uptimes and aren't freed by \
                     closing apps. Once they hit the cap, new shells fail with \"Device not \
                     configured.\" Raising the limit fixes it instantly — no restart.",
                )
                .size(11.5)
                .color(TEXT),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        can_act,
                        accent_button(&format!("Raise limit ({PTY_TARGET})")),
                    )
                    .clicked()
                {
                    action = Some(false);
                }
                if ui
                    .add_enabled(can_act, ghost_button("Make permanent"))
                    .clicked()
                {
                    action = Some(true);
                }
                ui.label(
                    RichText::new("requires your Mac password")
                        .size(10.5)
                        .color(MUTED),
                );
            });
        });
    ui.add_space(4.0);
    action
}

fn danger_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(12.0).color(Color32::WHITE)).fill(DANGER)
}

fn accent_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(12.0).color(Color32::WHITE)).fill(ACCENT)
}

fn ghost_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(12.0).color(TEXT))
        .fill(CARD_HOVER)
        .stroke(egui::Stroke::NONE)
}

/// Run an external command, forwarding each stdout/stderr line to the UI as a
/// `Msg::Log` so progress is visible live. Blocks until the command exits.
fn stream(tx: &Sender<Msg>, ctx: &egui::Context, prog: &str, args: &[&str]) {
    let mut child = match Command::new(prog)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Msg::Log(format!("✗ could not run {prog}: {e}")));
            ctx.request_repaint();
            return;
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
    let _ = child.wait();
}
