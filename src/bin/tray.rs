//! sweepmac-tray — a macOS menu-bar (status bar) front-end.
//!
//! The status item is a monochrome template glyph, so macOS tints it for light
//! and dark menu bars automatically. Its menu is deliberately small: it reports
//! what is safely reclaimable and hands every cleanup decision to the window's
//! review step. Nothing here deletes anything — a menu click can start a scan
//! (which never deletes) or open the GUI focused on review, and that is all.
//!
//! Scanning runs on a background thread; results are pushed back to the main
//! thread (where the menu lives, since menu items are not `Send`) via the tao
//! event loop.

use std::process::Command;
use std::thread;

use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use sweepmac::{disk_free, find_git_repos, home, human, scan, DEFAULT_CLEAN_CATEGORIES};

/// Nominal template size in points; macOS wants the @2x pixel buffer.
const TEMPLATE_PT: u32 = 18;
const TEMPLATE_SCALE: u32 = 2;

/// What a scan found — everything the menu needs to render.
struct ScanReport {
    /// Regenerable caches in the default (safe) categories.
    safe: u64,
    disk_free: u64,
    disk_total: u64,
}

/// What the opt-in repository walk found. Kept out of `ScanReport` — and out
/// of the headline total — because scanning repos is a separate, explicit ask.
struct GitReport {
    /// Regenerable build dirs across every worktree.
    build: u64,
    /// Worktrees sweepmac verified as safe to remove, and what they hold.
    safe_worktrees: usize,
    safe_bytes: u64,
}

enum UserEvent {
    Menu(MenuEvent),
    Scanned(ScanReport),
    GitScanned(GitReport),
}

/// Long-lived menu handles updated after each scan.
struct Tray {
    _tray: TrayIcon, // kept alive; dropping removes the icon
    status: MenuItem,
    disk: MenuItem,
    review: MenuItem,
    scan: MenuItem,
    git: MenuItem,
    scanning: bool,
    git_scanning: bool,
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
                if let Some(t) = tray.as_mut() {
                    t.mark_scanning();
                }
                spawn_scan(&proxy);
            }

            Event::UserEvent(UserEvent::Scanned(r)) => {
                if let Some(t) = tray.as_mut() {
                    t.apply_report(&r);
                }
            }

            Event::UserEvent(UserEvent::GitScanned(r)) => {
                if let Some(t) = tray.as_mut() {
                    t.apply_git(&r);
                }
            }

            Event::UserEvent(UserEvent::Menu(e)) => {
                let scanning = tray.as_ref().map(|t| t.scanning).unwrap_or(false);
                match e.id.0.as_str() {
                    "scan" if !scanning => {
                        if let Some(t) = tray.as_mut() {
                            t.mark_scanning();
                        }
                        spawn_scan(&proxy);
                    }
                    // Repositories are only walked when asked: the menu never
                    // does it as part of the ordinary scan.
                    "git" if !scanning => {
                        if let Some(t) = tray.as_mut() {
                            t.mark_git_scanning();
                        }
                        spawn_git_scan(&proxy);
                    }
                    // Both routes open the window; cleaning is always confirmed
                    // there, never silently from the menu bar.
                    "review" => open_window(true),
                    "open" => open_window(false),
                    "quit" => *control_flow = ControlFlow::Exit,
                    _ => {}
                }
            }

            _ => {}
        }
    });
}

impl Tray {
    fn mark_scanning(&mut self) {
        self.scanning = true;
        self.status.set_text("Scanning…");
        self.scan.set_enabled(false);
    }

    fn mark_git_scanning(&mut self) {
        self.git_scanning = true;
        self.git.set_text("Scanning repositories…");
        self.git.set_enabled(false);
    }

    fn apply_git(&mut self, r: &GitReport) {
        self.git_scanning = false;
        self.git.set_enabled(true);
        if r.build == 0 && r.safe_worktrees == 0 {
            self.git.set_text("Git worktrees: nothing to reclaim");
            return;
        }
        // Two numbers, because they carry different weight: build dirs are
        // ordinary regenerable cleanup, a worktree removal is a decision.
        let mut text = format!("Git build dirs: {}", human(r.build));
        if r.safe_worktrees > 0 {
            text.push_str(&format!(
                " · {} worktree{} safe to remove ({})",
                r.safe_worktrees,
                if r.safe_worktrees == 1 { "" } else { "s" },
                human(r.safe_bytes)
            ));
        }
        self.git.set_text(text);
    }

    fn apply_report(&mut self, r: &ScanReport) {
        self.scanning = false;
        self.scan.set_enabled(!self.git_scanning);
        self.status
            .set_text(format!("{} safely reclaimable", human(r.safe)));
        if r.disk_total > 0 {
            self.disk.set_text(format!(
                "{} free of {}",
                human(r.disk_free),
                human(r.disk_total)
            ));
        } else {
            self.disk.set_text("Disk usage unavailable");
        }
        // Nothing to reclaim: leave the review route visible but inert.
        self.review.set_enabled(r.safe > 0);
    }
}

/// Construct the status item and its dropdown.
fn build_tray() -> Tray {
    // Disabled status lines: information, not actions.
    let status = MenuItem::with_id("status", "Scanning…", false, None);
    let disk = MenuItem::with_id("disk", "Reading disk…", false, None);
    let scan = MenuItem::with_id("scan", "Scan now", true, None);
    let review = MenuItem::with_id("review", "Review recommended cleanup…", false, None);
    // Opt-in: repositories are never walked by the automatic scan.
    let git = MenuItem::with_id("git", "Scan git worktrees…", true, None);
    let open = MenuItem::with_id("open", "Open sweepmac", true, None);
    let quit = MenuItem::with_id("quit", "Quit sweepmac", true, None);

    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &disk,
        &PredefinedMenuItem::separator(),
        &scan,
        &review,
        &git,
        &open,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .expect("build menu");

    let mut builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("sweepmac — safe cleanup");
    // The designed monochrome alpha mask at @2x for the nominal 18pt slot.
    // `with_icon_as_template` hands tinting to macOS, so the mark tracks light
    // and dark menu bars (and the highlighted state) on its own.
    let (mask, px) = sweepmac::tray_template_rgba(TEMPLATE_PT * TEMPLATE_SCALE);
    match Icon::from_rgba(mask, px, px) {
        Ok(icon) => {
            builder = builder.with_icon(icon).with_icon_as_template(true);
        }
        Err(_) => {
            // Text fallback only if the glyph can't be built — never an emoji.
            builder = builder.with_title("sweepmac");
        }
    }
    let tray = builder.build().expect("build tray icon");

    Tray {
        _tray: tray,
        status,
        disk,
        review,
        scan,
        git,
        scanning: false,
        git_scanning: false,
    }
}

/// Walk repositories in the background. Read-only, like every scan here.
fn spawn_git_scan(proxy: &EventLoopProxy<UserEvent>) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let repos = home().map(|h| find_git_repos(&h)).unwrap_or_default();
        let build: u64 = repos.iter().map(|r| r.build_bytes()).sum();
        let safe: Vec<u64> = repos
            .iter()
            .flat_map(|r| r.worktrees.iter())
            .filter(|w| w.removable())
            .map(|w| w.total)
            .collect();
        let _ = proxy.send_event(UserEvent::GitScanned(GitReport {
            build,
            safe_worktrees: safe.len(),
            safe_bytes: safe.iter().sum(),
        }));
    });
}

/// Scan in the background and report a snapshot back to the menu. Scanning only
/// measures — it never deletes.
fn spawn_scan(proxy: &EventLoopProxy<UserEvent>) {
    let proxy = proxy.clone();
    thread::spawn(move || {
        let safe: u64 = scan(DEFAULT_CLEAN_CATEGORIES)
            .unwrap_or_default()
            .iter()
            .map(|r| r.size)
            .sum();
        let (disk_free, disk_total) = disk_free().unwrap_or((0, 0));
        let _ = proxy.send_event(UserEvent::Scanned(ScanReport {
            safe,
            disk_free,
            disk_total,
        }));
    });
}

/// Launch the full window, preferring the binary next to us. With `review`, ask
/// it to open on the review step for the recommended selection.
fn open_window(review: bool) {
    let run = |program: &std::ffi::OsStr| -> bool {
        let mut cmd = Command::new(program);
        if review {
            cmd.arg("--review");
        }
        cmd.spawn().is_ok()
    };
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("sweepmac-gui")))
        .filter(|p| p.exists());
    if let Some(path) = sibling {
        if run(path.as_os_str()) {
            return;
        }
    }
    run(std::ffi::OsStr::new("sweepmac-gui"));
}
