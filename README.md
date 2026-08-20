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

### Homebrew (recommended)

The repo doubles as a Homebrew tap ([Formula/sweepmac.rb](Formula/sweepmac.rb)).
It builds from source on your machine, so there are no Gatekeeper warnings and
no signing required:

```bash
brew tap karol-blaszczyk/sweepmac https://github.com/karol-blaszczyk/sweepmac
brew install sweepmac        # installs sweepmac, sweepmac-gui, sweepmac-tray
```

Update later with `brew upgrade sweepmac` (or `brew install --HEAD sweepmac`
to track `main`).

### DMG

Each [release](https://github.com/karol-blaszczyk/sweepmac/releases) attaches a
`sweepmac.dmg` with a drag-to-install `sweepmac.app` (the menu-bar app, with the
CLI and GUI binaries bundled inside). The DMG is **not signed or notarized**, so
Gatekeeper will refuse to open it at first: allow it under
**System Settings → Privacy & Security → "Open Anyway"**, or clear the
quarantine flag:

```bash
xattr -d com.apple.quarantine /Applications/sweepmac.app
```

If that puts you off (fair!), use Homebrew — locally built binaries are never
quarantined.

### From a checkout

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
"Selected" total and a red **Clean selected** button that asks for confirmation
before deleting. Scanning and cleaning run on background threads so the window
never freezes.

Beyond the cache categories, the window also has:

- **Docker panel** — per-image and per-volume lists with checkbox multi-select
  delete (in-use images/volumes are locked), plus one-click build-cache prune.
- **node_modules finder** — recursively finds `node_modules` folders under your
  home directory (skipping Library, Trash, media folders), sorted by size, with
  bulk delete.
- **Extras** — iOS simulator cleanup and other space reported outside the cache
  catalogue.

## Usage

```bash
sweepmac                      # dry run: scan everything and report sizes
sweepmac --clean              # clean default categories (system, macos, dev, xcode)
sweepmac --clean --all        # also clean app caches (Brave/Spotify/Ableton — they re-download)
sweepmac --clean -c dev,xcode # clean only the given categories
sweepmac --clean --docker     # also: docker builder/image prune -af
sweepmac --clean --simulators # also: xcrun simctl delete unavailable
sweepmac --clean -y           # skip the confirmation prompt
sweepmac fix-pty              # raise the macOS pty limit (fixes "out of pty devices")
sweepmac fix-pty --persist    # …and make it survive reboots (/etc/sysctl.conf)
sweepmac --help
```

## Categories

| Category | Targets |
|----------|---------|
| `system` | User logs + diagnostic reports (`~/Library/Logs`), Trash |
| `macos`  | "Cleanup At Startup" temp staging |
| `dev`    | Poetry, pip, Yarn, pnpm, npm, Playwright, Homebrew, Gradle, Maven, Cargo, Go build, JetBrains caches |
| `xcode`  | DerivedData (build cache), iOS DeviceSupport symbols |
| `app`    | Brave, Spotify, Ableton caches *(opt-in — these re-download)* |

`system`, `macos`, `dev`, and `xcode` are cleaned by default — note that
includes **emptying the Trash**. `app` is opt-in via `--category app` or
`--all`, and you can narrow any run with `-c` (e.g. `-c dev,xcode` to leave
Trash and logs alone).

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

## Fixing "out of pty devices"

Long macOS uptimes with heavy terminal/tmux/IDE use can exhaust the default
pty limit. `sweepmac fix-pty` raises `kern.tty.ptmx_max` (prompts for an admin
password via the native macOS dialog); `--persist` also writes it to
`/etc/sysctl.conf` so it survives reboots.

## Adding a target

Edit the `TARGETS` array in [src/lib.rs](src/lib.rs) — each entry is a path
relative to `$HOME`, a category, a description, and whether to clear the
directory's contents or remove it entirely.

## License

[MIT](LICENSE)
