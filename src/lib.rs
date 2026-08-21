//! sweepmac core — shared scan/clean logic used by both the CLI and the GUI.
//!
//! Safe by design: nothing here deletes until `clean_target` is called, and the
//! catalogue only ever points at regenerable caches.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Directories where `docker` commonly lives but which GUI apps launched from
/// Finder/Dock/Spotlight don't inherit (they get a minimal system PATH, not
/// your shell's). We search these explicitly so the Docker panel isn't
/// silently empty just because the app wasn't launched from a terminal.
const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/opt/homebrew/sbin",
    "/usr/local/sbin",
];

/// Resolve the `docker` binary once: prefer PATH, then fall back to common
/// Homebrew/Intel install locations.
pub fn docker_bin() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        // If it's already runnable via the inherited PATH, use the bare name.
        if Command::new("docker")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return "docker".to_string();
        }
        for dir in EXTRA_BIN_DIRS {
            let candidate = format!("{dir}/docker");
            if Path::new(&candidate).is_file() {
                return candidate;
            }
        }
        "docker".to_string() // give up gracefully; callers already handle failure
    })
    .as_str()
}

/// Run a `docker` subcommand, resolving its path even when launched without a
/// shell PATH (see `docker_bin`).
fn docker_cmd() -> Command {
    Command::new(docker_bin())
}

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
/// `DEFAULT_CLEAN_CATEGORIES` are cleaned by default. `app` caches re-download
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
    rows.sort_by_key(|r| std::cmp::Reverse(r.size));
    Ok(rows)
}

/// Look up a target by id.
pub fn target_by_id(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|t| t.id == id)
}

// --- Selection model -----------------------------------------------------
//
// Pure, dependency-free selection arithmetic shared by the front-ends. The GUI
// owns the widgets; the rules about *what may be selected in bulk* and *when an
// action may run* live here so they can be tested without a window.

/// How dangerous cleaning an item is. Drives selection defaults and the colour
/// / confirmation path a front-end must use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Risk {
    /// A regenerable cache: rebuilding it costs time, never data. Safe to
    /// preselect and to clean behind the ordinary review step.
    Regenerable,
    /// Developer state that is safe in most setups but worth a deliberate
    /// choice (Docker build cache, unused images, node_modules).
    Conditional,
    /// Removes data that cannot be regenerated (Docker volumes). Never bulk
    /// selected, always confirmed on its own danger path.
    Irreversible,
}

impl Risk {
    /// True when an item of this risk may be included by a "select all".
    pub fn bulk_selectable(self) -> bool {
        !matches!(self, Risk::Irreversible)
    }
}

/// One selectable line item, reduced to what the selection rules need.
#[derive(Clone, Debug)]
pub struct Selectable {
    /// Stable identity (target id, image id, volume name, path…).
    pub key: String,
    pub bytes: u64,
    pub risk: Risk,
    /// True when the item exists but cannot be acted on (e.g. an in-use Docker
    /// image). Locked items are never selectable.
    pub locked: bool,
}

impl Selectable {
    pub fn new(key: impl Into<String>, bytes: u64, risk: Risk) -> Self {
        Selectable {
            key: key.into(),
            bytes,
            risk,
            locked: false,
        }
    }

    pub fn locked(mut self, locked: bool) -> Self {
        self.locked = locked;
        self
    }
}

/// Aggregate of the current selection, as shown in the action bar.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct SelectionSummary {
    pub count: usize,
    pub bytes: u64,
}

impl SelectionSummary {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Total the items `is_selected` says are selected. Locked items never count,
/// even if a stale key lingers in the selection set.
pub fn summarize(items: &[Selectable], is_selected: impl Fn(&str) -> bool) -> SelectionSummary {
    items
        .iter()
        .filter(|i| !i.locked && is_selected(&i.key))
        .fold(SelectionSummary::default(), |mut acc, i| {
            acc.count += 1;
            acc.bytes += i.bytes;
            acc
        })
}

/// The keys a "select all" may legitimately tick: everything unlocked whose
/// risk allows bulk selection. Irreversible items are deliberately excluded —
/// destroying data must always be an individual, explicit choice.
pub fn bulk_selectable_keys(items: &[Selectable]) -> Vec<String> {
    items
        .iter()
        .filter(|i| !i.locked && i.risk.bulk_selectable())
        .map(|i| i.key.clone())
        .collect()
}

/// Whether the primary action may run: something is selected and no other
/// operation is in flight.
pub fn action_enabled(summary: SelectionSummary, busy: bool) -> bool {
    !summary.is_empty() && !busy
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
    out.sort_by_key(|n| std::cmp::Reverse(n.size));
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
    // Count *actual* allocated blocks (512-byte units), matching `du` — so a
    // sparse file like the Colima VM image reports real on-disk usage, not its
    // larger logical/apparent size.
    if meta.is_file() {
        return meta.blocks() * 512;
    }
    let mut total = meta.blocks() * 512; // the directory entry itself
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

/// Informational extras (Docker, iOS simulators) — reported, not part of the
/// cache catalogue because they need their own tooling to clean.
///
/// Each tuple is `(label, path, size)`. For Docker, `size` is what a prune would
/// reclaim *inside the VM* (from `docker system df`), NOT the host VM-image size
/// — pruning frees space inside the VM but does not shrink `~/.colima` on disk.
pub fn extras() -> Vec<(&'static str, PathBuf, u64)> {
    let home = match home() {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<(&'static str, PathBuf, u64)> = Vec::new();

    // Docker: report in-VM reclaimable (images + build cache).
    if let Some(reclaimable) = docker_reclaimable() {
        if reclaimable > 0 {
            out.push((
                "Docker images + build cache (in-VM)",
                home.join(".colima"),
                reclaimable,
            ));
        }
    }

    // iOS simulators: real on-disk size of unavailable-device clutter.
    let sims = home.join("Library/Developer/CoreSimulator");
    if sims.exists() {
        let s = dir_size(&sims);
        if s > 0 {
            out.push(("iOS Simulators (CoreSimulator)", sims, s));
        }
    }
    out
}

/// How much `docker builder/image prune -af` would reclaim inside the VM, in
/// bytes (images + build cache). `None` if Docker isn't reachable.
pub fn docker_reclaimable() -> Option<u64> {
    let out = docker_cmd()
        .args(["system", "df", "--format", "{{.Type}}\t{{.Reclaimable}}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut total = 0u64;
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let ty = parts.next().unwrap_or("").trim();
        let recl = parts.next().unwrap_or("").trim();
        if ty == "Images" || ty == "Build Cache" {
            // e.g. "42.39GB (93%)" -> take the size token.
            let val = recl.split_whitespace().next().unwrap_or("0B");
            total += parse_docker_size(val).unwrap_or(0);
        }
    }
    Some(total)
}

/// A Docker named volume.
pub struct DockerVolume {
    pub name: String,
    pub size: u64,
    /// True if a container currently mounts it (so it can't be removed).
    pub in_use: bool,
}

/// A Docker image. One entry per image *id* — `docker images` prints one line
/// per tag, and a multi-tagged image must not be listed (or sized) twice.
pub struct DockerImage {
    pub id: String,
    /// Display name: the tags joined with ", " (or "<none>:<none>" dangling).
    pub name: String,
    /// Every "repo:tag" that references this id.
    pub tags: Vec<String>,
    pub size: u64,
    /// True if a container (any state) references it — can't be removed.
    pub in_use: bool,
}

impl DockerImage {
    /// Arguments for `docker rmi` that actually delete this image: a
    /// multi-tagged image must be untagged name by name (removal by id is
    /// refused without -f), a single-tag or dangling one goes by id.
    pub fn rmi_refs(&self) -> Vec<String> {
        let real_tags: Vec<&String> = self
            .tags
            .iter()
            .filter(|t| !t.starts_with("<none>"))
            .collect();
        if real_tags.len() > 1 {
            real_tags.into_iter().cloned().collect()
        } else {
            vec![self.id.clone()]
        }
    }
}

/// Granular Docker reclaimable breakdown for the detailed panel.
#[derive(Default)]
pub struct DockerInfo {
    pub build_cache: u64,
    pub images: u64,     // reclaimable (unused) images
    pub containers: u64, // reclaimable (stopped) containers
    pub volumes: Vec<DockerVolume>,
    pub image_list: Vec<DockerImage>,
}

impl DockerInfo {
    /// Total bytes held by unused volumes (the safely-prunable ones).
    pub fn unused_volume_bytes(&self) -> u64 {
        self.volumes
            .iter()
            .filter(|v| !v.in_use)
            .map(|v| v.size)
            .sum()
    }
}

/// Gather a granular Docker breakdown (per-type reclaimable + per-volume).
/// `None` if Docker isn't reachable.
pub fn docker_info() -> Option<DockerInfo> {
    let df = docker_cmd()
        .args(["system", "df", "--format", "{{.Type}}\t{{.Reclaimable}}"])
        .output()
        .ok()?;
    if !df.status.success() {
        return None;
    }
    let mut info = DockerInfo::default();
    let text = String::from_utf8_lossy(&df.stdout);
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let ty = parts.next().unwrap_or("").trim();
        let recl = parts.next().unwrap_or("").trim();
        let val = recl.split_whitespace().next().unwrap_or("0B");
        let bytes = parse_docker_size(val).unwrap_or(0);
        match ty {
            "Images" => info.images = bytes,
            "Containers" => info.containers = bytes,
            "Build Cache" => info.build_cache = bytes,
            _ => {}
        }
    }
    info.volumes = docker_volumes();
    info.image_list = docker_images();
    Some(info)
}

/// List Docker images with sizes and best-effort in-use status.
fn docker_images() -> Vec<DockerImage> {
    let out = match docker_cmd()
        .args([
            "images",
            "--format",
            "{{.ID}}|{{.Repository}}:{{.Tag}}|{{.Size}}",
        ])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    // Image references used by any container (running or stopped).
    let used: Vec<String> = docker_cmd()
        .args(["ps", "-a", "--format", "{{.Image}}"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .collect()
        })
        .unwrap_or_default();

    let text = String::from_utf8_lossy(&out.stdout);
    let mut images: Vec<DockerImage> = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('|').collect();
        if cols.len() < 3 {
            continue;
        }
        let id = cols[0].trim().to_string();
        let tag = cols[1].trim().to_string();
        let size = parse_docker_size(cols[2].trim()).unwrap_or(0);
        // A container may reference an image by name or by (short) id.
        let in_use = used
            .iter()
            .any(|u| u == &tag || u == &id || u.starts_with(&id));
        // `docker images` emits one line per tag with the same id and the same
        // size — fold tags of one image into a single entry.
        if let Some(existing) = images.iter_mut().find(|i| i.id == id) {
            existing.tags.push(tag);
            existing.name = existing.tags.join(", ");
            existing.in_use |= in_use;
        } else {
            images.push(DockerImage {
                id,
                name: tag.clone(),
                tags: vec![tag],
                size,
                in_use,
            });
        }
    }
    images.sort_by_key(|i| std::cmp::Reverse(i.size));
    images
}

/// Remove specific Docker images by id (no `-f`, so in-use images are refused).
pub fn docker_image_rm(ids: &[String]) -> Result<String, String> {
    if ids.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec!["rmi".to_string()];
    args.extend(ids.iter().cloned());
    let out = docker_cmd()
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    // rmi can partially succeed; return stdout+stderr either way.
    let mut msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !err.is_empty() {
        if !msg.is_empty() {
            msg.push('\n');
        }
        msg.push_str(&err);
    }
    if out.status.success() {
        Ok(msg)
    } else {
        Err(msg)
    }
}

/// Per-volume name, size, and in-use status, parsed from `docker system df -v`.
fn docker_volumes() -> Vec<DockerVolume> {
    let out = match docker_cmd().args(["system", "df", "-v"]).output() {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut vols = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        if line.contains("VOLUME NAME") {
            in_section = true;
            continue;
        }
        if in_section {
            if line.trim().is_empty() {
                break;
            }
            // Columns: NAME  LINKS  SIZE (volume names have no spaces).
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() >= 3 {
                let links: u32 = cols[1].parse().unwrap_or(0);
                let size = parse_docker_size(cols[cols.len() - 1]).unwrap_or(0);
                vols.push(DockerVolume {
                    name: cols[0].to_string(),
                    size,
                    in_use: links > 0,
                });
            }
        }
    }
    vols.sort_by_key(|v| std::cmp::Reverse(v.size));
    vols
}

/// Run a single `docker <kind> prune` (kind: builder|image|container|network).
/// Returns combined output, or an error string.
pub fn docker_prune_kind(kind: &str) -> Result<String, String> {
    let args: &[&str] = match kind {
        "builder" => &["builder", "prune", "-af"],
        "image" => &["image", "prune", "-af"],
        "container" => &["container", "prune", "-f"],
        "network" => &["network", "prune", "-f"],
        _ => return Err(format!("unknown prune kind: {kind}")),
    };
    let out = docker_cmd()
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Remove specific Docker volumes by name. Returns combined output.
pub fn docker_volume_rm(names: &[String]) -> Result<String, String> {
    if names.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec!["volume".to_string(), "rm".to_string()];
    args.extend(names.iter().cloned());
    let out = docker_cmd()
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Parse a Docker size string like "42.39GB", "512MB", "0B" (decimal units).
fn parse_docker_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let pos = s.find(|c: char| c.is_alphabetic())?;
    let (num, unit) = s.split_at(pos);
    let n: f64 = num.trim().parse().ok()?;
    let mult = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => 1.0,
    };
    Some((n * mult) as u64)
}

/// Run `docker builder prune -af` then `docker image prune -af`.
/// Returns a human-readable result line.
pub fn run_docker_prune() -> String {
    let mut out = String::new();
    for args in [["builder", "prune", "-af"], ["image", "prune", "-af"]] {
        match docker_cmd().args(args).output() {
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

// --- Icon assets ---------------------------------------------------------

/// The designed menu-bar template glyph — a geometric broom with one sparkle —
/// as a black-on-transparent alpha mask, at 1x / 2x / 3x of the nominal 18 pt
/// status-item slot. macOS tints the mask itself, so only the alpha matters.
///
/// `assets/tray/tray-template.svg` is the master these were exported from.
#[cfg(feature = "icons")]
const TRAY_TEMPLATE_PNGS: &[(u32, &[u8])] = &[
    (18, include_bytes!("../assets/tray/tray-template-18.png")),
    (36, include_bytes!("../assets/tray/tray-template-18@2x.png")),
    (54, include_bytes!("../assets/tray/tray-template-18@3x.png")),
];

/// Decode a PNG to straight-alpha RGBA. `None` if it can't be read.
#[cfg(feature = "icons")]
pub fn decode_png_rgba(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);
    // Normalise to 8-bit RGBA; the bundled assets are already RGBA8.
    let rgba = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf,
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => buf
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => {
            buf.iter().flat_map(|v| [*v, *v, *v, 255]).collect()
        }
        _ => return None,
    };
    Some((rgba, w, h))
}

/// The menu-bar template mask at the requested pixel size, as straight-alpha
/// RGBA. Falls back to the procedural [`broom_rgba`] glyph if the bundled asset
/// can't be decoded, so the status item is never left without a mark.
#[cfg(feature = "icons")]
pub fn tray_template_rgba(px: u32) -> (Vec<u8>, u32) {
    if let Some((_, bytes)) = TRAY_TEMPLATE_PNGS.iter().find(|(size, _)| *size == px) {
        if let Some((rgba, w, h)) = decode_png_rgba(bytes) {
            if w == h {
                return (rgba, w);
            }
        }
    }
    (broom_rgba(px, true), px)
}

// --- Icon generation -----------------------------------------------------

/// Render a broom icon as straight-alpha RGBA, `size`×`size`.
///
/// `template = true` → a flat white glyph on transparent (the fallback macOS
/// menu-bar template image; the system tints it to match the bar).
/// `template = false` → the colour app/window icon: a graphite-navy rounded
/// tile, pale birch handle, teal brush head and one white sparkle. No
/// typography, no gradients.
pub fn broom_rgba(size: u32, template: bool) -> Vec<u8> {
    let n = size as usize;
    let mut buf = vec![0u8; n * n * 4];
    const SS: usize = 3; // supersampling for smooth edges

    // App-icon palette.
    const TILE: (u32, u32, u32) = (0x1E, 0x25, 0x33); // graphite navy
    const HANDLE: (u32, u32, u32) = (0xE8, 0xD9, 0xBE); // pale birch
    const HEAD: (u32, u32, u32) = (0x3F, 0xC1, 0xA9); // teal
    const SPARKLE: (u32, u32, u32) = (0xFF, 0xFF, 0xFF);

    for y in 0..n {
        for x in 0..n {
            let (mut cr, mut cg, mut cb, mut cov) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = (x as f32 + (sx as f32 + 0.5) / SS as f32) / n as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / SS as f32) / n as f32;
                    let part = broom_part(fx, fy);
                    let color = match (template, part) {
                        // Template: one flat mask, whatever the part.
                        (true, Some(_)) => Some(SPARKLE),
                        (true, None) => None,
                        (false, Some(Part::Handle)) => Some(HANDLE),
                        (false, Some(Part::Head)) => Some(HEAD),
                        (false, Some(Part::Sparkle)) => Some(SPARKLE),
                        (false, None) if in_tile(fx, fy) => Some(TILE),
                        (false, None) => None,
                    };
                    if let Some((r, g, b)) = color {
                        cr += r;
                        cg += g;
                        cb += b;
                        cov += 1;
                    }
                }
            }
            let total = (SS * SS) as u32;
            let idx = (y * n + x) * 4;
            if let (Some(r), Some(g), Some(b)) = (
                cr.checked_div(cov),
                cg.checked_div(cov),
                cb.checked_div(cov),
            ) {
                buf[idx] = r as u8;
                buf[idx + 1] = g as u8;
                buf[idx + 2] = b as u8;
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

/// Which part of the mark a point falls in — lets the colour icon paint the
/// handle, head and sparkle separately while the template stays a flat mask.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Handle,
    Head,
    Sparkle,
}

/// Broom glyph: a diagonal sweep — thick handle running lower-left to
/// upper-right, a wide brush head, and a four-point sparkle in the upper-right.
///
/// Tuned to stay legible as one connected mark at 16–18 px: the handle is
/// deliberately heavy (no hairlines), the head is a solid wedge, and bristle
/// notches only appear in the lower third where there is room for them. The
/// sparkle is detached by design — at 18 px it reads as a deliberate accent
/// rather than a broken-off fragment.
fn broom_part(x: f32, y: f32) -> Option<Part> {
    // Sparkle: a compact four-point star, upper-right.
    if in_sparkle(x, y, 0.80, 0.20, 0.13) {
        return Some(Part::Sparkle);
    }

    // Brush head: a solid wedge at the lower-left end of the handle.
    let tl = (0.10, 0.60);
    let tr = (0.42, 0.50);
    let br = (0.50, 0.86);
    let bl = (0.16, 0.92);
    let head = in_tri(x, y, tl.0, tl.1, tr.0, tr.1, br.0, br.1)
        || in_tri(x, y, tl.0, tl.1, br.0, br.1, bl.0, bl.1);
    // Two wide notches hint at bristles without adding fragile detail. A notch
    // cuts all the way out (the handle must not show through the gap).
    if head {
        let notched = y > 0.76 && (x * 12.0).floor() as i32 % 3 == 0;
        return (!notched).then_some(Part::Head);
    }

    // Handle: lower-left → upper-right diagonal, thick enough to survive
    // downsampling to 16 px.
    if seg_dist(x, y, 0.30, 0.74, 0.66, 0.24) <= 0.072 {
        return Some(Part::Handle);
    }
    None
}

/// A four-point sparkle centred on `(cx, cy)` with arm length `r`: two crossed
/// tapered spikes, which stay readable at menu-bar size.
fn in_sparkle(x: f32, y: f32, cx: f32, cy: f32, r: f32) -> bool {
    let dx = (x - cx) / r;
    let dy = (y - cy) / r;
    // |dx|^p + |dy|^p <= 1 with p < 1 gives a concave four-point star.
    let p = 0.62_f32;
    dx.abs().powf(p) + dy.abs().powf(p) <= 1.0
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn targets_are_sane() {
        let mut seen = std::collections::HashSet::new();
        for t in TARGETS {
            assert!(seen.insert(t.id), "duplicate target id: {}", t.id);
            assert!(
                ALL_CATEGORIES.contains(&t.category),
                "target {} has unknown category {}",
                t.id,
                t.category
            );
            // Paths must stay inside $HOME: relative, and no parent-dir escapes.
            assert!(!t.rel.is_empty(), "target {} has empty path", t.id);
            assert!(
                !t.rel.starts_with('/') && !t.rel.starts_with('~'),
                "target {} path must be relative to $HOME: {}",
                t.id,
                t.rel
            );
            assert!(
                !t.rel.split('/').any(|c| c == ".."),
                "target {} path escapes $HOME: {}",
                t.id,
                t.rel
            );
        }
    }

    #[test]
    fn default_categories_are_a_subset_and_exclude_app() {
        for c in DEFAULT_CLEAN_CATEGORIES {
            assert!(ALL_CATEGORIES.contains(c));
        }
        assert!(!DEFAULT_CLEAN_CATEGORIES.contains(&"app"));
    }

    #[test]
    fn target_by_id_roundtrips() {
        for t in TARGETS {
            assert_eq!(target_by_id(t.id).unwrap().id, t.id);
        }
        assert!(target_by_id("no-such-target").is_none());
    }

    #[test]
    fn human_formats_sizes() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(512), "512 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1536), "1.5 KB");
        assert_eq!(human(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("sweepmac-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn clean_target_clear_contents_keeps_the_directory() {
        let dir = scratch_dir("clear");
        fs::write(dir.join("f.txt"), "x").unwrap();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/g.txt"), "y").unwrap();

        let t = Target {
            id: "test",
            category: "dev",
            desc: "",
            rel: "unused",
            clear_contents: true,
        };
        clean_target(&t, &dir).unwrap();
        assert!(dir.exists(), "directory itself must survive");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clean_target_remove_deletes_the_directory() {
        let dir = scratch_dir("remove");
        fs::write(dir.join("f.txt"), "x").unwrap();

        let t = Target {
            id: "test",
            category: "dev",
            desc: "",
            rel: "unused",
            clear_contents: false,
        };
        clean_target(&t, &dir).unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn clean_target_missing_path_is_ok() {
        let t = Target {
            id: "test",
            category: "dev",
            desc: "",
            rel: "unused",
            clear_contents: false,
        };
        let missing = env::temp_dir().join("sweepmac-test-definitely-missing");
        assert!(clean_target(&t, &missing).is_ok());
    }

    // --- selection model -------------------------------------------------

    fn sample() -> Vec<Selectable> {
        vec![
            Selectable::new("cache-a", 1_000, Risk::Regenerable),
            Selectable::new("cache-b", 2_000, Risk::Regenerable),
            Selectable::new("build-cache", 4_000, Risk::Conditional),
            Selectable::new("img-in-use", 8_000, Risk::Conditional).locked(true),
            Selectable::new("vol-data", 16_000, Risk::Irreversible),
            Selectable::new("vol-mounted", 32_000, Risk::Irreversible).locked(true),
        ]
    }

    #[test]
    fn summarize_totals_only_selected_unlocked_items() {
        let items = sample();
        let selected = ["cache-a", "build-cache"];
        let s = summarize(&items, |k| selected.contains(&k));
        assert_eq!(
            s,
            SelectionSummary {
                count: 2,
                bytes: 5_000
            }
        );
    }

    #[test]
    fn summarize_ignores_locked_items_even_if_selected() {
        let items = sample();
        // A stale key for a now-in-use image must not inflate the total.
        let s = summarize(&items, |k| k == "img-in-use" || k == "cache-b");
        assert_eq!(
            s,
            SelectionSummary {
                count: 1,
                bytes: 2_000
            }
        );
    }

    #[test]
    fn summarize_empty_selection_is_empty() {
        let s = summarize(&sample(), |_| false);
        assert!(s.is_empty());
        assert_eq!(s.bytes, 0);
    }

    #[test]
    fn bulk_selection_never_includes_irreversible_or_locked() {
        let keys = bulk_selectable_keys(&sample());
        assert_eq!(keys, vec!["cache-a", "cache-b", "build-cache"]);
        // The data-bearing volumes are the whole point of this rule.
        assert!(!keys.iter().any(|k| k.starts_with("vol-")));
        assert!(!keys.iter().any(|k| k == "img-in-use"));
    }

    #[test]
    fn bulk_selection_of_only_irreversible_items_is_empty() {
        let items = vec![
            Selectable::new("vol-a", 1, Risk::Irreversible),
            Selectable::new("vol-b", 2, Risk::Irreversible),
        ];
        assert!(bulk_selectable_keys(&items).is_empty());
    }

    #[test]
    fn risk_bulk_selectability() {
        assert!(Risk::Regenerable.bulk_selectable());
        assert!(Risk::Conditional.bulk_selectable());
        assert!(!Risk::Irreversible.bulk_selectable());
    }

    #[test]
    fn action_is_enabled_only_with_a_selection_and_no_work_in_flight() {
        let empty = SelectionSummary::default();
        let some = SelectionSummary {
            count: 1,
            bytes: 10,
        };
        assert!(!action_enabled(empty, false), "nothing selected");
        assert!(!action_enabled(empty, true));
        assert!(!action_enabled(some, true), "busy cleaning");
        assert!(action_enabled(some, false));
    }

    // --- tray/app glyph --------------------------------------------------

    #[test]
    fn broom_glyph_is_a_dense_readable_mark() {
        // At menu-bar size the template must have real coverage (not hairlines)
        // but still leave breathing room — a fully-filled square would read as
        // a block, an almost-empty one as noise.
        for size in [18u32, 36] {
            let buf = broom_rgba(size, true);
            assert_eq!(buf.len() as u32, size * size * 4);
            let px = (size * size) as f32;
            let opaque = buf.chunks(4).filter(|p| p[3] > 128).count() as f32;
            let frac = opaque / px;
            assert!(
                (0.12..0.55).contains(&frac),
                "template coverage at {size}px was {frac:.3}"
            );
            // Template images must be pure white + alpha so macOS can tint them.
            for p in buf.chunks(4).filter(|p| p[3] > 0) {
                assert_eq!((p[0], p[1], p[2]), (255, 255, 255));
            }
        }
    }

    #[test]
    fn rmi_refs_untag_multi_tagged_images_by_name() {
        let multi = DockerImage {
            id: "abc123".into(),
            name: "node:20, node:latest".into(),
            tags: vec!["node:20".into(), "node:latest".into()],
            size: 1,
            in_use: false,
        };
        assert_eq!(multi.rmi_refs(), vec!["node:20", "node:latest"]);

        let single = DockerImage {
            id: "def456".into(),
            name: "redis:7".into(),
            tags: vec!["redis:7".into()],
            size: 1,
            in_use: false,
        };
        assert_eq!(single.rmi_refs(), vec!["def456"]);

        let dangling = DockerImage {
            id: "ffff00".into(),
            name: "<none>:<none>".into(),
            tags: vec!["<none>:<none>".into()],
            size: 1,
            in_use: false,
        };
        assert_eq!(dangling.rmi_refs(), vec!["ffff00"]);
    }

    /// The bundled template assets must decode at every scale the tray asks
    /// for, be square, and carry a real alpha mask (macOS tints the alpha, so
    /// an all-transparent or all-opaque asset would show nothing or a block).
    #[cfg(feature = "icons")]
    #[test]
    fn tray_template_assets_decode_at_every_scale() {
        for px in [18u32, 36, 54] {
            let (rgba, w) = tray_template_rgba(px);
            assert_eq!(w, px, "asset for {px}px reported the wrong width");
            assert_eq!(rgba.len() as u32, px * px * 4);
            let opaque = rgba.chunks(4).filter(|p| p[3] > 128).count() as f32;
            let frac = opaque / (px * px) as f32;
            assert!(
                (0.05..0.60).contains(&frac),
                "template coverage at {px}px was {frac:.3}"
            );
        }
    }

    #[test]
    fn app_icon_tile_is_opaque_in_the_centre() {
        let size = 64u32;
        let buf = broom_rgba(size, false);
        let mid = ((size / 2 * size + size / 2) * 4) as usize;
        assert_eq!(buf[mid + 3], 255, "tile centre must be opaque");
    }
}
