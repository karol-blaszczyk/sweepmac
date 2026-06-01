//! sweepmac core — shared scan/clean logic used by both the CLI and the GUI.
//!
//! Safe by design: nothing here deletes until `clean_target` is called, and the
//! catalogue only ever points at regenerable caches.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A directory we know how to clean.
pub struct Target {
    /// Short identifier shown in reports.
    pub id: &'static str,
    /// Category used for filtering.
    pub category: &'static str,
    /// Human description.
    pub desc: &'static str,
    /// Path relative to the user's home directory.
    pub rel: &'static str,
    /// If true, delete the directory's *contents* but keep the directory itself
    /// (some tools expect the folder to exist). If false, remove the whole path.
    pub clear_contents: bool,
}

/// The catalogue of known-safe, regenerable caches.
///
/// `dev`, `macos`, and `xcode` are cleaned by default. `app` caches re-download
/// and are app-specific, so they are opt-in.
pub const TARGETS: &[Target] = &[
    // --- System (logs + trash; conventionally safe to clear) ---
    Target {
        id: "user-logs",
        category: "system",
        desc: "User logs + diagnostic reports",
        rel: "Library/Logs",
        clear_contents: true,
    },
    Target {
        id: "trash",
        category: "system",
        desc: "Trash (empty it)",
        rel: ".Trash",
        clear_contents: true,
    },
    // --- macOS temp staging ---
    Target {
        id: "macos-temp",
        category: "macos",
        desc: "macOS \"Cleanup At Startup\" temp staging",
        rel: "Library/Caches/Cleanup At Startup",
        clear_contents: true,
    },
    // --- Dev / package-manager caches ---
    Target {
        id: "poetry",
        category: "dev",
        desc: "Python Poetry package cache",
        rel: "Library/Caches/pypoetry",
        clear_contents: false,
    },
    Target {
        id: "pip",
        category: "dev",
        desc: "pip download cache",
        rel: "Library/Caches/pip",
        clear_contents: false,
    },
    Target {
        id: "yarn",
        category: "dev",
        desc: "Yarn package cache",
        rel: "Library/Caches/Yarn",
        clear_contents: false,
    },
    Target {
        id: "pnpm",
        category: "dev",
        desc: "pnpm store cache",
        rel: "Library/Caches/pnpm",
        clear_contents: false,
    },
    Target {
        id: "playwright",
        category: "dev",
        desc: "Playwright browser cache",
        rel: "Library/Caches/ms-playwright",
        clear_contents: false,
    },
    Target {
        id: "homebrew",
        category: "dev",
        desc: "Homebrew download cache",
        rel: "Library/Caches/Homebrew",
        clear_contents: false,
    },
    Target {
        id: "npm",
        category: "dev",
        desc: "npm cache",
        rel: ".npm/_cacache",
        clear_contents: false,
    },
    Target {
        id: "gradle",
        category: "dev",
        desc: "Gradle build cache",
        rel: ".gradle/caches",
        clear_contents: false,
    },
    Target {
        id: "maven",
        category: "dev",
        desc: "Maven local repository",
        rel: ".m2/repository",
        clear_contents: false,
    },
    Target {
        id: "cargo",
        category: "dev",
        desc: "Cargo registry download cache",
        rel: ".cargo/registry/cache",
        clear_contents: false,
    },
    Target {
        id: "go-build",
        category: "dev",
        desc: "Go build cache",
        rel: "Library/Caches/go-build",
        clear_contents: false,
    },
    Target {
        id: "jetbrains",
        category: "dev",
        desc: "JetBrains IDE caches",
        rel: "Library/Caches/JetBrains",
        clear_contents: false,
    },
    // --- Xcode ---
    Target {
        id: "deriveddata",
        category: "xcode",
        desc: "Xcode DerivedData (build cache)",
        rel: "Library/Developer/Xcode/DerivedData",
        clear_contents: true,
    },
    Target {
        id: "devicesupport",
        category: "xcode",
        desc: "Xcode iOS DeviceSupport",
        rel: "Library/Developer/Xcode/iOS DeviceSupport",
        clear_contents: false,
    },
    // --- App caches (opt-in: these re-download) ---
    Target {
        id: "brave",
        category: "app",
        desc: "Brave browser cache",
        rel: "Library/Caches/BraveSoftware",
        clear_contents: false,
    },
    Target {
        id: "spotify",
        category: "app",
        desc: "Spotify cache",
        rel: "Library/Caches/com.spotify.client",
        clear_contents: false,
    },
    Target {
        id: "ableton",
        category: "app",
        desc: "Ableton Live cache",
        rel: "Library/Caches/Ableton",
        clear_contents: false,
    },
];

/// Categories cleaned when no explicit selection is given.
pub const DEFAULT_CLEAN_CATEGORIES: &[&str] = &["system", "macos", "dev", "xcode"];

/// Every category, in display order.
pub const ALL_CATEGORIES: &[&str] = &["system", "macos", "dev", "xcode", "app"];

/// A measured target ready to show or clean.
pub struct ScanRow {
    pub id: &'static str,
    pub category: &'static str,
    pub desc: &'static str,
    pub path: PathBuf,
    pub size: u64,
}

/// Resolve the user's home directory.
pub fn home() -> io::Result<PathBuf> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "$HOME is not set"))
}

/// Measure every target whose category is in `categories`.
pub fn scan(categories: &[&str]) -> io::Result<Vec<ScanRow>> {
    let home = home()?;
    let mut rows: Vec<ScanRow> = TARGETS
        .iter()
        .filter(|t| categories.contains(&t.category))
        .map(|t| {
            let path = home.join(t.rel);
            let size = if path.exists() { dir_size(&path) } else { 0 };
            ScanRow {
                id: t.id,
                category: t.category,
                desc: t.desc,
                path,
                size,
            }
        })
        .collect();
    rows.sort_by(|a, b| b.size.cmp(&a.size));
    Ok(rows)
}

/// Look up a target by id.
pub fn target_by_id(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.id == id)
}

/// A discovered `node_modules` directory.
pub struct NodeModules {
    pub path: PathBuf,
    /// Display path, shortened to `~/…` when under the home directory.
    pub label: String,
    pub size: u64,
}

/// Directory names we never descend into while hunting for `node_modules`
/// (huge, non-project, or irrelevant — keeps the walk fast).
const NM_SKIP: &[&str] = &[
    "Library",
    ".Trash",
    "Pictures",
    "Movies",
    "Music",
    "Applications",
];

/// Recursively find `node_modules` directories under `root`, with sizes.
///
/// Does not descend into a `node_modules` once found (so nested deps are
/// counted once, in their parent's total), skips symlinks, and skips dotdirs
/// other than `.next` (Next.js standalone output keeps a real `node_modules`).
pub fn find_node_modules(root: &Path) -> Vec<NodeModules> {
    let home = home().ok();
    let mut out = Vec::new();
    walk_node_modules(root, &home, &mut out);
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

fn walk_node_modules(dir: &Path, home: &Option<PathBuf>, out: &mut Vec<NodeModules>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name == "node_modules" {
            let path = entry.path();
            let size = dir_size(&path);
            let label = home
                .as_ref()
                .and_then(|h| path.strip_prefix(h).ok())
                .map(|p| format!("~/{}", p.display()))
                .unwrap_or_else(|| path.display().to_string());
            out.push(NodeModules { path, label, size });
            continue; // prune: don't descend into deps
        }

        if NM_SKIP.iter().any(|s| *s == name) {
            continue;
        }
        // Skip dotdirs (.git, .cache, .npm, …) except Next.js output.
        if name.starts_with('.') && name != ".next" {
            continue;
        }
        walk_node_modules(&entry.path(), home, out);
    }
}

/// Recursively sum the size of a directory, not following symlinks.
pub fn dir_size(path: &Path) -> u64 {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if meta.is_file() {
        return meta.len();
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            total += dir_size(&entry.path());
        }
    }
    total
}

/// Delete a target according to its `clear_contents` policy.
pub fn clean_target(t: &Target, path: &Path) -> io::Result<()> {
    if t.clear_contents {
        if !path.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(&p)?;
            } else {
                fs::remove_file(&p)?;
            }
        }
        Ok(())
    } else if path.is_dir() {
        fs::remove_dir_all(path)
    } else if path.exists() {
        fs::remove_file(path)
    } else {
        Ok(())
    }
}

/// Informational extras (Docker/Colima VM, iOS simulators) — reported, not part
/// of the cache catalogue because they need their own tooling to clean.
pub fn extras() -> Vec<(&'static str, PathBuf, u64)> {
    let home = match home() {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let candidates = [
        ("Docker/Colima VM (~/.colima)", ".colima"),
        (
            "iOS Simulators (CoreSimulator)",
            "Library/Developer/CoreSimulator",
        ),
    ];
    candidates
        .iter()
        .filter_map(|(label, rel)| {
            let p = home.join(rel);
            if p.exists() {
                let s = dir_size(&p);
                if s > 0 {
                    return Some((*label, p, s));
                }
            }
            None
        })
        .collect()
}

/// Run `docker builder prune -af` then `docker image prune -af`.
/// Returns a human-readable result line.
pub fn run_docker_prune() -> String {
    let mut out = String::new();
    for args in [["builder", "prune", "-af"], ["image", "prune", "-af"]] {
        match Command::new("docker").args(args).output() {
            Ok(o) if o.status.success() => {
                out.push_str(&format!("docker {} ok\n", args.join(" ")));
            }
            Ok(o) => out.push_str(&format!(
                "docker {} failed: {}\n",
                args.join(" "),
                String::from_utf8_lossy(&o.stderr).trim()
            )),
            Err(e) => return format!("could not run docker: {e}"),
        }
    }
    out.trim_end().to_string()
}

/// Free and total bytes on the data volume, via `df -k`. Returns
/// `(free, total)` or `None` if it can't be determined.
pub fn disk_free() -> Option<(u64, u64)> {
    let out = Command::new("df")
        .args(["-k", "/System/Volumes/Data"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Second line: Filesystem 1024-blocks Used Available Capacity ... Mounted
    let line = text.lines().nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    let total_kb: u64 = cols.get(1)?.parse().ok()?;
    let avail_kb: u64 = cols.get(3)?.parse().ok()?;
    Some((avail_kb * 1024, total_kb * 1024))
}

/// Severity of the pseudo-terminal (pty) node situation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PtyLevel {
    Ok,
    Warn,
    Critical,
}

/// Pseudo-terminal device-node usage vs. the kernel cap.
///
/// macOS never deletes `/dev/ttysNNN` nodes once created, and allocation is
/// index-based against `kern.tty.ptmx_max` (default 511). Over a long uptime
/// the node count climbs until new shells fail with `forkpty: Device not
/// configured` (ENXIO) — even though almost all are idle. See `raise_pty_limit`.
#[derive(Clone, Copy)]
pub struct PtyStatus {
    pub nodes: usize,
    pub cap: u64,
    pub level: PtyLevel,
}

/// Read the current pty node count and cap. `None` if it can't be determined.
pub fn pty_status() -> Option<PtyStatus> {
    let out = Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.tty.ptmx_max"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let cap: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;

    let mut nodes = 0usize;
    if let Ok(rd) = fs::read_dir("/dev") {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("ttys") {
                nodes += 1;
            }
        }
    }

    let level = if nodes as u64 >= cap {
        PtyLevel::Critical
    } else if nodes as f64 >= cap as f64 * 0.9 {
        PtyLevel::Warn
    } else {
        PtyLevel::Ok
    };
    Some(PtyStatus { nodes, cap, level })
}

/// Raise `kern.tty.ptmx_max` immediately (max 999) via a native admin prompt.
/// Takes effect at once; resets to 511 on reboot.
pub fn raise_pty_limit(value: u64) -> Result<String, String> {
    run_admin(&format!("/usr/sbin/sysctl -w kern.tty.ptmx_max={value}"))
}

/// Raise the limit now AND persist it to `/etc/sysctl.conf` so it survives
/// reboots. Prompts for an admin password once.
pub fn persist_pty_limit(value: u64) -> Result<String, String> {
    let script = format!(
        "/usr/sbin/sysctl -w kern.tty.ptmx_max={v}; \
         if grep -q '^kern.tty.ptmx_max' /etc/sysctl.conf 2>/dev/null; then \
         /usr/bin/sed -i '' 's/^kern.tty.ptmx_max=.*/kern.tty.ptmx_max={v}/' /etc/sysctl.conf; \
         else echo 'kern.tty.ptmx_max={v}' >> /etc/sysctl.conf; fi",
        v = value
    );
    run_admin(&script)
}

/// Run a shell snippet as root via macOS's native GUI password dialog
/// (`osascript … with administrator privileges`) — no working terminal needed.
fn run_admin(shell_script: &str) -> Result<String, String> {
    let escaped = shell_script.replace('\\', "\\\\").replace('"', "\\\"");
    let apple = format!("do shell script \"{escaped}\" with administrator privileges");
    let out = Command::new("osascript")
        .arg("-e")
        .arg(apple)
        .output()
        .map_err(|e| format!("could not run osascript: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("-128") {
            Err("cancelled".to_string())
        } else {
            Err(err)
        }
    }
}

/// Run `xcrun simctl delete unavailable`.
pub fn run_simctl_prune() -> String {
    match Command::new("xcrun")
        .args(["simctl", "delete", "unavailable"])
        .output()
    {
        Ok(o) if o.status.success() => "deleted unavailable simulators".to_string(),
        Ok(o) => format!(
            "simctl failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => format!("could not run xcrun simctl: {e}"),
    }
}

/// Human-readable byte size (binary units).
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

// --- Icon generation -----------------------------------------------------

/// Render a broom icon as straight-alpha RGBA, `size`×`size`.
///
/// `template = true` → white glyph on transparent (a macOS menu-bar template
/// image; the system tints it white/black to match the bar). `template = false`
/// → a white broom on a rounded accent-blue tile, for the app / window icon.
pub fn broom_rgba(size: u32, template: bool) -> Vec<u8> {
    let n = size as usize;
    let mut buf = vec![0u8; n * n * 4];
    const SS: usize = 3; // supersampling for smooth edges
    let accent = (0x4cu32, 0x8du32, 0xffu32);

    for y in 0..n {
        for x in 0..n {
            let (mut cr, mut cg, mut cb, mut cov) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = (x as f32 + (sx as f32 + 0.5) / SS as f32) / n as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / SS as f32) / n as f32;
                    if in_broom(fx, fy) {
                        cr += 255;
                        cg += 255;
                        cb += 255;
                        cov += 1;
                    } else if !template && in_tile(fx, fy) {
                        cr += accent.0;
                        cg += accent.1;
                        cb += accent.2;
                        cov += 1;
                    }
                }
            }
            let total = (SS * SS) as u32;
            let idx = (y * n + x) * 4;
            if cov > 0 {
                buf[idx] = (cr / cov) as u8;
                buf[idx + 1] = (cg / cov) as u8;
                buf[idx + 2] = (cb / cov) as u8;
                buf[idx + 3] = (cov * 255 / total) as u8;
            }
        }
    }
    buf
}

/// Rounded-square app tile.
fn in_tile(x: f32, y: f32) -> bool {
    let inset = 0.05;
    let r = 0.22;
    let lo = inset + r;
    let hi = 1.0 - inset - r;
    let qx = x - x.clamp(lo, hi);
    let qy = y - y.clamp(lo, hi);
    (qx * qx + qy * qy).sqrt() <= r
}

/// Broom glyph: a slanted handle plus a trapezoidal brush head with bristle
/// notches along the bottom.
fn in_broom(x: f32, y: f32) -> bool {
    let handle = seg_dist(x, y, 0.72, 0.14, 0.45, 0.52) <= 0.045;
    let tl = (0.30, 0.50);
    let tr = (0.52, 0.50);
    let br = (0.66, 0.88);
    let bl = (0.16, 0.88);
    let head = in_tri(x, y, tl.0, tl.1, tr.0, tr.1, br.0, br.1)
        || in_tri(x, y, tl.0, tl.1, br.0, br.1, bl.0, bl.1);

    if head && y > 0.70 {
        // Carve thin vertical gaps to suggest bristles.
        let stripe = (x * 34.0).floor() as i32;
        if stripe.rem_euclid(6) == 0 {
            return handle;
        }
    }
    head || handle
}

fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

#[allow(clippy::too_many_arguments)]
fn in_tri(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> bool {
    let sign = |x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32| {
        (x1 - x3) * (y2 - y3) - (x2 - x3) * (y1 - y3)
    };
    let d1 = sign(px, py, ax, ay, bx, by);
    let d2 = sign(px, py, bx, by, cx, cy);
    let d3 = sign(px, py, cx, cy, ax, ay);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}
