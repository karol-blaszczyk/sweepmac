//! sweepmac CLI — find and clear regenerable macOS caches.
//!
//! Safe by default: bare `sweepmac` just scans. Deletion needs `--clean` and a
//! confirmation (skip with `-y`). See `--help`.

use std::collections::BTreeMap;
use std::io::{self, Write};

use sweepmac::{
    clean_target, extras, human, persist_pty_limit, pty_status, raise_pty_limit, run_docker_prune,
    run_simctl_prune, scan, target_by_id, PtyLevel, ALL_CATEGORIES, DEFAULT_CLEAN_CATEGORIES,
    TARGETS,
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

    // Which categories are in play.
    let active: Vec<&str> = if opts.all {
        ALL_CATEGORIES.to_vec()
    } else if let Some(c) = &opts.categories {
        c.iter().map(|s| s.as_str()).collect()
    } else if opts.clean {
        DEFAULT_CLEAN_CATEGORIES.to_vec()
    } else {
        ALL_CATEGORIES.to_vec() // scan with no filter shows everything
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
    if nonempty == 0 && !opts.docker && !opts.simulators {
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
    sweepmac fix-pty [--persist]    Raise the macOS pty limit (fixes \"out of
                                    pty devices\"); --persist survives reboots

OPTIONS:
    -c, --category <csv>   Comma-separated categories: {cats}
        --all              Include every category (adds app caches)
        --clean            Actually delete (otherwise it's a dry run)
    -y, --yes              Skip the confirmation prompt
        --docker           Run `docker builder/image prune -af`
        --simulators       Run `xcrun simctl delete unavailable`
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
        "\nSafe by default: only regenerable caches are touched, and nothing is\ndeleted without --clean. App caches are opt-in. Documents, projects, and\ngit repos are never scanned.\n"
    );
}
