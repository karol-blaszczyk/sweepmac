//! sweepmac-tray — a macOS menu-bar (status bar) front-end.
//!
//! Puts a 🧹 icon in the top-right menu bar. Its dropdown shows free disk space,
//! how much each action will reclaim, and a per-cache breakdown. Scanning and
//! cleaning run on background threads; results are pushed back to the main
//! thread (where the menu lives, since menu items are not `Send`) via the tao
//! event loop.

use std::process::Command;
use std::thread;

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use sweepmac::{
    clean_target, disk_free, human, persist_pty_limit, pty_status, raise_pty_limit,
    run_docker_prune, run_simctl_prune, scan, target_by_id, PtyLevel, PtyStatus, ALL_CATEGORIES,
    DEFAULT_CLEAN_CATEGORIES, TARGETS,
};

const PTY_TARGET: u64 = 999;

/// A full snapshot of what a scan found — everything the menu needs to render.
struct ScanReport {
    safe: u64,
    all: u64,
    per_target: Vec<(&'static str, u64)>,
    docker: u64,
    sims: u64,
    disk_free: u64,
    disk_total: u64,
    pty: Option<PtyStatus>,
}

/// Events delivered to the main event loop.
enum UserEvent {
    Menu(MenuEvent),
    Scanned(ScanReport),
    Cleaned(u64),
}

/// Long-lived menu handles we mutate after each scan.
struct Tray {
    _tray: TrayIcon, // kept alive; dropping removes the icon
    disk: MenuItem,
    header: MenuItem,
    pty: MenuItem,
    clean_safe: MenuItem,
    clean_all: MenuItem,
    docker: MenuItem,
    sims: MenuItem,
    details: Vec<(&'static str, MenuItem)>,
    busy: bool,
}

fn main() {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = proxy.send_event(UserEvent::Menu(e));
    }));

    let proxy = event_loop.create_proxy();
    let mut tray: Option<Tray> = None;

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::NewEvents(StartCause::Init) => {
                tray = Some(build_tray());
                spawn_scan(&proxy);
            }

            Event::UserEvent(UserEvent::Scanned(r)) => {
                if let Some(t) = tray.as_mut() {
                    t.apply_report(&r);
                }
            }

            Event::UserEvent(UserEvent::Cleaned(freed)) => {
                if let Some(t) = tray.as_mut() {
                    t.header
                        .set_text(format!("✓ Reclaimed ~{} — rescanning…", human(freed)));
                }
                spawn_scan(&proxy);
            }

            Event::UserEvent(UserEvent::Menu(e)) => {
                let busy = tray.as_ref().map(|t| t.busy).unwrap_or(false);
                match e.id.0.as_str() {
                    "clean_safe" if !busy => {
                        mark_busy(&mut tray, "Cleaning safe caches…");
                        spawn_clean(&proxy, DEFAULT_CLEAN_CATEGORIES, false, false);
                    }
                    "clean_all" if !busy => {
                        mark_busy(&mut tray, "Cleaning all caches…");
                        spawn_clean(&proxy, ALL_CATEGORIES, false, false);
                    }
                    "docker" if !busy => {
                        mark_busy(&mut tray, "Pruning Docker…");
                        spawn_clean(&proxy, &[], true, false);
                    }
                    "sims" if !busy => {
                        mark_busy(&mut tray, "Deleting old simulators…");
                        spawn_clean(&proxy, &[], false, true);
                    }
                    "rescan" if !busy => {
                        mark_busy(&mut tray, "Scanning…");
                        spawn_scan(&proxy);
                    }
                    "fix_pty" if !busy => {
                        mark_busy(&mut tray, "Raising pty limit (enter password)…");
                        spawn_pty(&proxy, false);
                    }
                    "open" => open_window(),
                    "quit" => *control_flow = ControlFlow::Exit,
                    _ => {}
                }
            }

            _ => {}
        }
    });
}

impl Tray {
    /// Push a fresh scan report into all the menu labels.
    fn apply_report(&mut self, r: &ScanReport) {
        self.busy = false;

        let used = r.disk_total.saturating_sub(r.disk_free);
        let pct = if r.disk_total > 0 {
            (used as f64 / r.disk_total as f64 * 100.0).round() as u64
        } else {
            0
        };
        self.disk.set_text(format!(
            "💾  {} free of {}  ({}% used)",
            human(r.disk_free),
            human(r.disk_total),
            pct
        ));
        self.header
            .set_text(format!("🧹  {} reclaimable in caches", human(r.all)));

        set_action(
            &self.clean_safe,
            "Clean safe caches (dev · xcode · macOS)",
            r.safe,
        );
        set_action(&self.clean_all, "Clean all, incl. app caches", r.all);
        set_action(
            &self.docker,
            "Prune Docker (build cache + images)",
            r.docker,
        );
        set_action(&self.sims, "Delete unavailable iOS simulators", r.sims);

        // pty: only actionable (and only visible-as-warning) when near/at cap.
        match r.pty {
            Some(p) if p.level != PtyLevel::Ok => {
                let icon = if p.level == PtyLevel::Critical {
                    "⚠ pty LIMIT"
                } else {
                    "⚠ pty low"
                };
                self.pty.set_text(format!(
                    "{icon}: {}/{} — Raise to {PTY_TARGET}",
                    p.nodes, p.cap
                ));
                self.pty.set_enabled(true);
            }
            Some(p) => {
                self.pty
                    .set_text(format!("pty terminals: {}/{} ok", p.nodes, p.cap));
                self.pty.set_enabled(false);
            }
            None => {
                self.pty.set_text("pty terminals: n/a");
                self.pty.set_enabled(false);
            }
        }

        for (id, item) in &self.details {
            let size = r
                .per_target
                .iter()
                .find(|(tid, _)| tid == id)
                .map(|(_, s)| *s)
                .unwrap_or(0);
            let desc = target_by_id(id).map(|t| t.desc).unwrap_or(id);
            item.set_text(detail_label(desc, size));
            item.set_enabled(false); // informational
        }
    }
}

/// Format an actionable item as "Label — 1.2 GB", disabling it when empty.
fn set_action(item: &MenuItem, label: &str, size: u64) {
    if size > 0 {
        item.set_text(format!("{label}  —  {}", human(size)));
        item.set_enabled(true);
    } else {
        item.set_text(format!("{label}  —  empty"));
        item.set_enabled(false);
    }
}

fn detail_label(desc: &str, size: u64) -> String {
    let dot = if size > 0 { "•" } else { "·" };
    format!("{dot}  {desc}  —  {}", human(size))
}

fn mark_busy(tray: &mut Option<Tray>, msg: &str) {
    if let Some(t) = tray.as_mut() {
        t.busy = true;
        t.header.set_text(format!("⏳  {msg}"));
        t.clean_safe.set_enabled(false);
        t.clean_all.set_enabled(false);
        t.docker.set_enabled(false);
        t.sims.set_enabled(false);
    }
}

/// Construct the menu-bar icon and its dropdown.
fn build_tray() -> Tray {
    let disk = MenuItem::with_id("disk", "💾  Reading disk…", false, None);
    let header = MenuItem::with_id("header", "🧹  Scanning caches…", false, None);
    let pty = MenuItem::with_id("fix_pty", "pty terminals: …", false, None);
    let clean_safe = MenuItem::with_id("clean_safe", "Clean safe caches", false, None);
    let clean_all = MenuItem::with_id("clean_all", "Clean all, incl. app caches", false, None);
    let docker = MenuItem::with_id("docker", "Prune Docker", false, None);
    let sims = MenuItem::with_id("sims", "Delete unavailable iOS simulators", false, None);
    let rescan = MenuItem::with_id("rescan", "↻  Rescan", true, None);
    let open = MenuItem::with_id("open", "⤢  Open full window…", true, None);
    let quit = MenuItem::with_id("quit", "Quit sweepmac", true, None);

    // Per-cache breakdown lives in a submenu so the top level stays tidy.
    let details_menu = Submenu::new("Cache breakdown", true);
    let mut details: Vec<(&'static str, MenuItem)> = Vec::new();
    for t in TARGETS {
        let item = MenuItem::with_id(t.id, detail_label(t.desc, 0), false, None);
        details_menu.append(&item).ok();
        details.push((t.id, item));
    }

    let menu = Menu::new();
    menu.append_items(&[
        &disk,
        &header,
        &pty,
        &PredefinedMenuItem::separator(),
        &clean_safe,
        &clean_all,
        &docker,
        &sims,
        &PredefinedMenuItem::separator(),
        &details_menu,
        &PredefinedMenuItem::separator(),
        &rescan,
        &open,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .expect("build menu");

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("sweepmac — disk cleanup");
    // A monochrome template icon: macOS tints it white/black to match the bar.
    if let Ok(icon) = Icon::from_rgba(sweepmac::broom_rgba(36, true), 36, 36) {
        builder = builder.with_icon(icon).with_icon_as_template(true);
    } else {
        builder = builder.with_title("🧹"); // fallback
    }
    let tray = builder.build().expect("build tray icon");

    Tray {
        _tray: tray,
        disk,
        header,
        pty,
        clean_safe,
        clean_all,
        docker,
        sims,
        details,
        busy: false,
    }
}

/// Scan in the background and report a full snapshot back to the UI.
fn spawn_scan(proxy: &EventLoopProxy<UserEvent>) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let rows = scan(ALL_CATEGORIES).unwrap_or_default();
        let all: u64 = rows.iter().map(|r| r.size).sum();
        let safe: u64 = rows
            .iter()
            .filter(|r| DEFAULT_CLEAN_CATEGORIES.contains(&r.category))
            .map(|r| r.size)
            .sum();
        let per_target: Vec<(&'static str, u64)> = rows.iter().map(|r| (r.id, r.size)).collect();

        let extras = sweepmac::extras();
        let docker = extras
            .iter()
            .find(|(l, _, _)| l.contains("Colima"))
            .map(|(_, _, s)| *s)
            .unwrap_or(0);
        let sims = extras
            .iter()
            .find(|(l, _, _)| l.contains("Simulators"))
            .map(|(_, _, s)| *s)
            .unwrap_or(0);

        let (disk_free, disk_total) = disk_free().unwrap_or((0, 0));
        let pty = pty_status();

        let _ = proxy.send_event(UserEvent::Scanned(ScanReport {
            safe,
            all,
            per_target,
            docker,
            sims,
            disk_free,
            disk_total,
            pty,
        }));
    });
}

/// Raise the pty limit via a native admin prompt, then rescan to refresh.
fn spawn_pty(proxy: &EventLoopProxy<UserEvent>, persist: bool) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let _ = if persist {
            persist_pty_limit(PTY_TARGET)
        } else {
            raise_pty_limit(PTY_TARGET)
        };
        // Cleaned(0) settles the busy state; the follow-on rescan refreshes pty.
        let _ = proxy.send_event(UserEvent::Cleaned(0));
    });
}

/// Clean categories (and optionally docker / simulators) in the background.
fn spawn_clean(
    proxy: &EventLoopProxy<UserEvent>,
    categories: &'static [&'static str],
    docker: bool,
    sims: bool,
) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let mut freed = 0u64;
        if !categories.is_empty() {
            for row in scan(categories).unwrap_or_default() {
                if row.size == 0 {
                    continue;
                }
                if let Some(t) = target_by_id(row.id) {
                    if clean_target(t, &row.path).is_ok() {
                        freed += row.size;
                    }
                }
            }
        }
        if docker {
            run_docker_prune();
        }
        if sims {
            run_simctl_prune();
        }
        let _ = proxy.send_event(UserEvent::Cleaned(freed));
    });
}

/// Launch the full egui window (sibling binary), preferring the one next to us.
fn open_window() {
    let candidate = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("sweepmac-gui")));
    let launched = candidate
        .as_ref()
        .filter(|p| p.exists())
        .map(|p| Command::new(p).spawn().is_ok())
        .unwrap_or(false);
    if !launched {
        let _ = Command::new("sweepmac-gui").spawn();
    }
}
