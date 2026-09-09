//! sweepmac CLI — find and clear regenerable macOS caches.
//!
//! Safe by default: bare `sweepmac` just scans. Deletion needs `--clean` and a
//! confirmation (skip with `-y`). See `--help`.

use std::collections::BTreeMap;
use std::io::{self, Write};

use sweepmac::{
    clean_build_dir, clean_target, extras, find_git_repos, git_available, home, human,
    persist_pty_limit, pty_status, raise_pty_limit, remove_worktree, run_docker_prune,
    run_simctl_prune, scan, target_by_id, GitRepo, PtyLevel, Worktree, WorktreeKind,
    ALL_CATEGORIES, DEFAULT_CLEAN_CATEGORIES, TARGETS,
};

const PTY_TARGET: u64 = 999;

struct Options {
    clean: bool,
    assume_yes: bool,
    docker: bool,
    simulators: bool,
    categories: Option<Vec<String>>, // None = use defaults
    all: bool,
    fix_pty: bool,
    persist: bool,
    /// Path of a single worktree to remove (individual opt-in — never bulk).
    remove_worktree: Option<String>,
}

fn main() {
    let opts = match parse_args() {
        Ok(o) => o,
        Err(code) => std::process::exit(code),
    };

    if opts.fix_pty {
        run_fix_pty(opts.persist);
        return;
    }

    if let Some(path) = opts.remove_worktree.clone() {
        run_remove_worktree(&path, opts.assume_yes);
        return;
    }

    // Which categories are in play.
    let active: Vec<&str> = if opts.all {
        ALL_CATEGORIES.to_vec()
    } else if let Some(c) = &opts.categories {
        c.iter().map(|s| s.as_str()).collect()
    } else if opts.clean {
        DEFAULT_CLEAN_CATEGORIES.to_vec()
    } else {
        // A bare scan shows every cache, but still does not walk repositories:
        // `git` is opt-in via `-c git` or `--all`.
        ALL_CATEGORIES
            .iter()
            .copied()
            .filter(|c| *c != "git")
            .collect()
    };

    let rows = match scan(&active) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    println!("\n  sweepmac — scanning {} cache locations…\n", rows.len());
    print_table(&rows);

    let total: u64 = rows.iter().map(|r| r.size).sum();
    println!("  {}", "─".repeat(58));
    println!("  {:<40} {:>15}", "reclaimable from caches", human(total));

    // Repositories are only looked at when the `git` category is asked for —
    // scanning them at all is a departure from the default promise.
    let repos = if active.contains(&"git") {
        scan_git_repos()
    } else {
        Vec::new()
    };
    let git_build: u64 = repos.iter().map(|r| r.build_bytes()).sum();
    if !repos.is_empty() {
        print_git(&repos);
        println!("  {}", "─".repeat(58));
        println!(
            "  {:<40} {:>15}",
            "reclaimable from git build dirs",
            human(git_build)
        );
    }

    print_extras();
    print_pty();

    if !opts.clean {
        println!(
            "\n  This was a dry run. To actually delete, re-run with --clean\n  (see --help for category and docker/simulator options).\n"
        );
        return;
    }

    // --- Clean mode ---
    let nonempty = rows.iter().filter(|r| r.size > 0).count();
    if nonempty == 0 && git_build == 0 && !opts.docker && !opts.simulators {
        println!("\n  Nothing to clean — caches are already empty.\n");
        return;
    }

    println!(
        "\n  About to clean {nonempty} location(s), reclaiming ~{}.",
        human(total)
    );
    if opts.docker {
        println!("  + prune Docker build cache and unused images");
    }
    if opts.simulators {
        println!("  + delete unavailable iOS simulators");
    }
    if git_build > 0 {
        println!(
            "  + delete {} of regenerable build dirs in {} repo(s)",
            human(git_build),
            repos.len()
        );
    }

    if !opts.assume_yes && !confirm("\n  Proceed? [y/N] ") {
        println!("  Aborted. Nothing was deleted.\n");
        return;
    }

    let mut freed: u64 = 0;
    for row in &rows {
        if row.size == 0 {
            continue;
        }
        let Some(t) = target_by_id(row.id) else {
            continue;
        };
        match clean_target(t, &row.path) {
            Ok(()) => {
                freed += row.size;
                println!("  ✓ cleared {:<24} ({})", row.id, human(row.size));
            }
            Err(e) => eprintln!("  ✗ {:<24} {}", row.id, e),
        }
    }

    // Build dirs only. Removing a worktree is never part of a bulk clean —
    // it needs --remove-worktree, one at a time.
    for repo in &repos {
        for wt in &repo.worktrees {
            for b in &wt.build {
                match clean_build_dir(&b.path) {
                    Ok(()) => {
                        freed += b.size;
                        println!("  ✓ removed {:<24} ({})", b.label, human(b.size));
                    }
                    Err(e) => eprintln!("  ✗ {:<24} {}", b.label, e),
                }
            }
        }
    }

    if opts.docker {
        println!("\n  Pruning Docker…\n{}", indent(&run_docker_prune()));
    }
    if opts.simulators {
        println!("\n  {}", run_simctl_prune());
    }

    println!("\n  Done. Reclaimed ~{} from caches.\n", human(freed));
}

fn print_table(rows: &[sweepmac::ScanRow]) {
    println!("  {:<12} {:<34} {:>9}", "ID", "WHAT", "SIZE");
    println!("  {}", "─".repeat(58));
    // Empty caches (including apps that simply aren't installed) are hidden —
    // a row of dashes says nothing actionable.
    let hidden = rows.iter().filter(|r| r.size == 0).count();
    for row in rows.iter().filter(|r| r.size > 0) {
        println!(
            "  {:<12} {:<34} {:>9}",
            row.id,
            truncate(row.desc, 34),
            human(row.size)
        );
    }
    if hidden > 0 {
        println!(
            "  {:<12} {:<34} {:>9}",
            "",
            format!("({hidden} more empty or not installed)"),
            ""
        );
    }
}

/// Discover repositories under `$HOME`. Read-only: nothing here can take a
/// git index lock (see `find_git_repos`).
fn scan_git_repos() -> Vec<GitRepo> {
    if !git_available() {
        eprintln!("  note: git is not available — skipping the git category.");
        return Vec::new();
    }
    let Ok(home) = home() else {
        return Vec::new();
    };
    println!("\n  Looking for git repositories under {}…", home.display());
    find_git_repos(&home)
}

/// Report every repository's worktrees: what they hold, and whether removing
/// them is safe. UNSAFE ones are informational only.
fn print_git(repos: &[GitRepo]) {
    println!("\n  Git worktrees and build caches:\n");
    for repo in repos {
        let default = repo
            .default_branch
            .as_deref()
            .unwrap_or("(no default branch)");
        println!("  {}  [{}]", repo.label, default);
        for wt in &repo.worktrees {
            let kind = match wt.kind {
                WorktreeKind::Primary => "repo",
                WorktreeKind::Linked => "worktree",
            };
            let branch = wt.branch.as_deref().unwrap_or("(detached)");
            println!(
                "    {:<8} {:<30} {:<10} {:>9}",
                kind,
                truncate_left(&wt.label, 30),
                truncate(branch, 10),
                human(wt.total)
            );
            println!(
                "             HEAD {:<8}  build {:>9} · source {:>9}",
                short(&wt.head),
                human(wt.build_bytes),
                human(wt.source_bytes)
            );
            match &wt.safety {
                s if s.is_safe() => println!(
                    "             SAFE to remove — sweepmac tags archive/{} first\n             └ sweepmac --remove-worktree {}",
                    wt.branch.as_deref().unwrap_or("…"),
                    wt.path.display()
                ),
                s => println!("             KEEP — {}", s.reason().unwrap_or("")),
            }
        }
        println!();
    }
    println!(
        "  Build dirs ({}) are regenerable and cleaned by --clean.",
        sweepmac::BUILD_DIRS.join(", ")
    );
    println!("  Worktree removal is never bulk: use --remove-worktree <path>, one at a time.");
}

/// Remove one SAFE worktree, after showing exactly what will happen.
fn run_remove_worktree(target: &str, assume_yes: bool) {
    let repos = scan_git_repos();
    let found: Option<&Worktree> = repos
        .iter()
        .flat_map(|r| r.worktrees.iter())
        .find(|w| w.path.ends_with(target.trim_end_matches('/')) || w.label == target);

    let Some(wt) = found else {
        eprintln!("error: no worktree found at '{target}'.");
        eprintln!("       Run `sweepmac -c git` to list what sweepmac can see.");
        std::process::exit(1);
    };

    if let Some(reason) = wt.safety.reason() {
        eprintln!("\n  Refusing to remove {}:\n    {reason}\n", wt.label);
        eprintln!(
            "  Its build dirs ({}) are still reclaimable with `sweepmac --clean -c git`.",
            human(wt.build_bytes)
        );
        std::process::exit(1);
    }

    println!("\n  Remove worktree {}", wt.label);
    println!(
        "    branch      {}",
        wt.branch.as_deref().unwrap_or("(detached)")
    );
    println!("    HEAD        {}", short(&wt.head));
    println!(
        "    reclaims    {} ({} build, {} source)",
        human(wt.total),
        human(wt.build_bytes),
        human(wt.source_bytes)
    );
    println!(
        "\n  sweepmac will tag {} at {} so the commits stay reachable,",
        wt.archive_tag(),
        short(&wt.head)
    );
    println!("  remove the directory, prune the admin entry, then delete the branch.");

    if !assume_yes && !confirm("\n  Proceed? [y/N] ") {
        println!("  Aborted. Nothing was removed.\n");
        return;
    }
    match remove_worktree(wt) {
        Ok(log) => println!(
            "\n{}\n\n  Done. Reclaimed ~{}.\n",
            indent(&log),
            human(wt.total)
        ),
        Err(e) => {
            eprintln!("\n  ✗ {e}\n");
            std::process::exit(1);
        }
    }
}

/// First 8 characters of a commit id.
fn short(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn print_extras() {
    let extras = extras();
    if extras.is_empty() {
        return;
    }
    println!("\n  Extras (need their own flags to clean):");
    for (label, _p, size) in extras {
        println!("  • {:<34} {:>9}", label, human(size));
    }
    println!("    └ Docker build cache/images: --docker · old simulators: --simulators");
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Show pseudo-terminal node usage, warning when near/at the kernel cap.
fn print_pty() {
    let Some(pty) = pty_status() else { return };
    let pct = if pty.cap > 0 {
        pty.nodes as f64 / pty.cap as f64 * 100.0
    } else {
        0.0
    };
    match pty.level {
        PtyLevel::Ok => {
            println!(
                "\n  pty terminals: {}/{} nodes ({:.0}%)",
                pty.nodes, pty.cap, pct
            );
        }
        PtyLevel::Warn => {
            println!(
                "\n  ⚠ pty terminals: {}/{} nodes ({:.0}%) — getting close to the cap.\n    Run `sweepmac fix-pty` to raise the limit.",
                pty.nodes, pty.cap, pct
            );
        }
        PtyLevel::Critical => {
            println!(
                "\n  ⚠ pty LIMIT REACHED: {}/{} nodes — new shells will fail with\n    \"Device not configured\". Run `sweepmac fix-pty` to fix instantly (no reboot).",
                pty.nodes, pty.cap
            );
        }
    }
}

/// Raise (and optionally persist) the pty node cap via a native admin prompt.
fn run_fix_pty(persist: bool) {
    if let Some(pty) = pty_status() {
        println!(
            "\n  pty terminals: {}/{} nodes (cap {})",
            pty.nodes, pty.cap, pty.cap
        );
    }
    println!(
        "  Raising kern.tty.ptmx_max to {PTY_TARGET}{}…",
        if persist {
            " and persisting to /etc/sysctl.conf"
        } else {
            ""
        }
    );
    println!("  (a macOS password dialog will appear)\n");

    let result = if persist {
        persist_pty_limit(PTY_TARGET)
    } else {
        raise_pty_limit(PTY_TARGET)
    };
    match result {
        Ok(_) => {
            println!("  ✓ kern.tty.ptmx_max is now {PTY_TARGET}.");
            if !persist {
                println!("    (resets on reboot — use `sweepmac fix-pty --persist` to make it permanent)");
            }
        }
        Err(e) if e == "cancelled" => println!("  Cancelled at the password prompt."),
        Err(e) => {
            eprintln!("  ✗ failed: {e}");
            std::process::exit(1);
        }
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

/// Truncate a path from the LEFT — the tail (worktree name) is what identifies
/// it, so that is the half worth keeping.
fn truncate_left(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    let mut out = String::from("…");
    out.extend(s.chars().skip(n - max.saturating_sub(1)));
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn parse_args() -> Result<Options, i32> {
    let mut opts = Options {
        clean: false,
        assume_yes: false,
        docker: false,
        simulators: false,
        categories: None,
        all: false,
        fix_pty: false,
        persist: false,
        remove_worktree: None,
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Err(0);
            }
            "scan" => {}
            "fix-pty" => opts.fix_pty = true,
            "--persist" => opts.persist = true,
            "--clean" => opts.clean = true,
            "-y" | "--yes" => opts.assume_yes = true,
            "--docker" => opts.docker = true,
            "--simulators" => opts.simulators = true,
            "--all" => opts.all = true,
            "--remove-worktree" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --remove-worktree needs a worktree path");
                    return Err(2);
                }
                opts.remove_worktree = Some(args[i].clone());
            }
            "--category" | "-c" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --category needs a value (e.g. dev,macos)");
                    return Err(2);
                }
                let cats: Vec<String> = args[i]
                    .split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect();
                for c in &cats {
                    if !ALL_CATEGORIES.contains(&c.as_str()) {
                        eprintln!(
                            "error: unknown category '{c}'. Valid: {}",
                            ALL_CATEGORIES.join(", ")
                        );
                        return Err(2);
                    }
                }
                opts.categories = Some(cats);
            }
            other => {
                eprintln!("error: unknown argument '{other}' (try --help)");
                return Err(2);
            }
        }
        i += 1;
    }
    Ok(opts)
}

fn print_help() {
    let mut by_cat: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for t in TARGETS {
        by_cat.entry(t.category).or_default().push(t.id);
    }
    println!(
        "sweepmac — tiny macOS cleanup helper

USAGE:
    sweepmac [scan]                 Dry run: scan and report sizes (default)
    sweepmac --clean                Clean default categories ({defaults})
    sweepmac --clean --all          Also clean app caches (re-download)
    sweepmac --clean -c dev,xcode   Clean only the given categories
    sweepmac --clean --docker       Also prune Docker build cache + images
    sweepmac --clean --simulators   Also delete unavailable iOS simulators
    sweepmac -c git                 Find git worktrees + their build dirs
    sweepmac --clean -c git         Delete those build dirs (regenerable)
    sweepmac --remove-worktree <p>  Remove one SAFE worktree (archives it first)
    sweepmac fix-pty [--persist]    Raise the macOS pty limit (fixes \"out of
                                    pty devices\"); --persist survives reboots

OPTIONS:
    -c, --category <csv>   Comma-separated categories: {cats}
        --all              Include every category (adds app caches AND git)
        --clean            Actually delete (otherwise it's a dry run)
    -y, --yes              Skip the confirmation prompt
        --docker           Run `docker builder/image prune -af`
        --simulators       Run `xcrun simctl delete unavailable`
        --remove-worktree <path>
                           Remove a single SAFE git worktree. Tags its tip as
                           archive/<branch> first, so no commit is ever lost.
        --persist          With fix-pty: also write to /etc/sysctl.conf
    -h, --help             Show this help

CATEGORIES:",
        defaults = DEFAULT_CLEAN_CATEGORIES.join(", "),
        cats = ALL_CATEGORIES.join(", "),
    );
    for (cat, ids) in by_cat {
        println!("    {cat:<8} {}", ids.join(", "));
    }
    println!(
        "    {:<8} git worktrees + regenerable build dirs (opt-in)",
        "git"
    );
    println!(
        "\nSafe by default: only regenerable caches are touched, and nothing is\ndeleted without --clean. App caches and the git category are opt-in, and\nrepositories are only looked at when you ask for `-c git` (or --all).\nEven then, --clean only deletes regenerable build dirs; removing a worktree\nis always one explicit --remove-worktree at a time, and only when sweepmac\nhas verified its content is already on the default branch.\n"
    );
}
