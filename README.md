# sweepmac

A tiny macOS cleanup helper. Finds regenerable caches (the kind that rebuild
themselves on next use), reports how much space each is holding, and clears the
ones you pick.

**Safe by default** — running it just *scans*. Nothing is deleted unless you
pass `--clean` (CLI) or confirm in the dialog (GUI). Documents, projects, and
git repos are never scanned.

Ships as three front-ends over one shared core (`src/lib.rs`):

- **CLI** (`sweepmac`) — zero dependencies, Rust std only.
- **GUI** (`sweepmac-gui`) — a native window built with
  [egui/eframe](https://github.com/emilk/egui), opt-in behind the `gui` feature.
- **Menu bar** (`sweepmac-tray`) — a 🧹 icon in the macOS top-right status bar
  with a dropdown of one-click actions, built with
  [tray-icon](https://github.com/tauri-apps/tray-icon) + tao, opt-in behind the
  `tray` feature.

Both GUI and tray are feature-gated so a plain `cargo install` keeps the CLI
dependency-free.

## Install

```bash
# CLI only (tiny, no deps):
cargo install --path .

# CLI + GUI:
cargo install --path . --features gui

# Everything (CLI + GUI + menu-bar app):
cargo install --path . --features "gui tray"
# binaries land in ~/.cargo/bin/{sweepmac,sweepmac-gui,sweepmac-tray}
```

## Menu bar (top-right)

```bash
sweepmac-tray
```

Adds a 🧹 icon to the macOS menu bar. Click it for a dropdown showing
reclaimable space (total + safe subset) and one-click actions: clean safe
caches, clean all (incl. app caches), prune Docker, delete old simulators,
rescan, or open the full window. Scans/cleans run off the main thread so the
menu never blocks.

> Run straight from the terminal it works, but also shows a Dock icon. To make
> it a pure background menu-bar app (no Dock icon) and launch it at login, wrap
> it in a minimal `.app` bundle with `LSUIElement = true` in its Info.plist and
> add it to System Settings → General → Login Items.

## GUI

```bash
sweepmac-gui
```

A single window: caches grouped by category with checkboxes and sizes,
default-safe categories pre-checked, app caches left unchecked. A running
"Selected" total, checkboxes to also prune Docker / delete old simulators, and a
red **Clean selected** button that asks for confirmation before deleting.
Scanning and cleaning run on background threads so the window never freezes.

## Usage

```bash
sweepmac                      # dry run: scan everything and report sizes
sweepmac --clean              # clean default categories (macos, dev, xcode)
sweepmac --clean --all        # also clean app caches (Brave/Spotify/Ableton — they re-download)
sweepmac --clean -c dev,xcode # clean only the given categories
sweepmac --clean --docker     # also: docker builder/image prune -af
sweepmac --clean --simulators # also: xcrun simctl delete unavailable
sweepmac --clean -y           # skip the confirmation prompt
sweepmac --help
```

## Categories

| Category | Targets |
|----------|---------|
| `macos`  | "Cleanup At Startup" temp staging |
| `dev`    | Poetry, pip, Yarn, pnpm, Playwright, Homebrew caches |
| `xcode`  | DerivedData (build cache) |
| `app`    | Brave, Spotify, Ableton caches *(opt-in — these re-download)* |

`macos`, `dev`, and `xcode` are cleaned by default. `app` is opt-in via
`--category app` or `--all`.

## Extras

Docker/Colima and iOS simulators are reported in the scan but cleaned through
their own tooling (they need it):

- `--docker` runs `docker builder prune -af` and `docker image prune -af`.
- `--simulators` runs `xcrun simctl delete unavailable`.

> The Colima VM disk image (`~/.colima`) doesn't shrink just from pruning inside
> it. To fully reclaim it when Docker is empty: `colima stop && colima delete`,
> remove any leftover `~/.colima/_lima/_disks/*`, then `colima start` to recreate
> a fresh small VM. sweepmac intentionally does *not* automate this, since it
> stops Docker.

## Adding a target

Edit the `TARGETS` array in `src/main.rs` — each entry is a path relative to
`$HOME`, a category, a description, and whether to clear the directory's
contents or remove it entirely.
