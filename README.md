# sweepmac

[![ci](https://github.com/karol-blaszczyk/sweepmac/actions/workflows/ci.yml/badge.svg)](https://github.com/karol-blaszczyk/sweepmac/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A tiny macOS cleanup helper. Finds regenerable caches (the kind that rebuild
themselves on next use), reports how much space each is holding, and clears the
ones you pick.

**Safe by default** — running it just *scans*. Nothing is deleted unless you
pass `--clean` (CLI) or confirm in the review dialog (GUI). Documents and
projects are never scanned. Anything that would destroy data — Docker volumes —
is quarantined in its own confirmation path and is never bulk-selected.

**On git repositories**: earlier versions never looked at them at all. As of
v0.3.0 they can be scanned, but only when you ask — `-c git` / `--all` on the
CLI, a **Scan repositories** button in the GUI, **Scan git worktrees…** in the
menu bar. Nothing walks a repository on its own. That scan is strictly
read-only: it runs git plumbing (`rev-parse`, `diff-index --cached`,
`ls-files`, `log`) with `GIT_OPTIONAL_LOCKS=0` and never `git status`, which
against a foreign work-tree leaves a stale `.git/index.lock` that breaks the
repo. See [Git worktrees](#git-worktrees).

Ships as three front-ends over one shared core (`src/lib.rs`):

- **CLI** (`sweepmac`) — zero dependencies, Rust std only.
- **GUI** (`sweepmac-gui`) — a native window built with
  [egui/eframe](https://github.com/emilk/egui), opt-in behind the `gui` feature.
- **Menu bar** (`sweepmac-tray`) — a monochrome broom glyph in the macOS
  top-right status bar that reports what's reclaimable and hands cleanup to the
  window's review step, built with
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

Adds a monochrome broom glyph to the macOS menu bar — a proper template image,
so macOS tints it for light and dark menu bars automatically
([assets/tray](assets/tray)). Its dropdown is deliberately small:

- `{amount} safely reclaimable` and `{free} free of {total}` — status only.
- **Scan now** — measures; never deletes.
- **Review recommended cleanup…** — opens the window on its review step.
- **Scan git worktrees…** — opt-in; reports reclaimable build dirs and how many
  worktrees are safe to remove. Deliberately a separate line, not folded into
  the headline total.
- **Open sweepmac** / **Quit**.

The menu bar never deletes anything on its own: every cleanup goes through the
window's review step. Scanning runs off the main thread so the menu never
blocks.

> Run straight from the terminal it works, but also shows a Dock icon. To make
> it a pure background menu-bar app (no Dock icon) and launch it at login, wrap
> it in a minimal `.app` bundle with `LSUIElement = true` in its Info.plist and
> add it to System Settings → General → Login Items.

## GUI

```bash
sweepmac-gui
```

A single window, ordered by how risky each decision is:

1. **Storage summary** — the anchor: `4.7 GB safely reclaimable`, with
   `29.5 GB free of 460.4 GB` and a neutral capacity meter that shows free space
   in blue and what your selection would add in green.
2. **Recommended cleanup** — the regenerable caches a scan found, already
   selected. The list stays short; the rest fold into their group below.
3. **Cache groups** — Developer / System / App caches, collapsed, each showing
   its item count and total. App caches are never preselected (they re-download).
4. **Advanced developer cleanup** — collapsed: Docker build cache, unused
   images, stopped containers and networks, plus the `node_modules` finder and
   unavailable iOS simulators. Per-image removal has its own list, with in-use
   images locked.
5. **Git worktrees & build caches** — collapsed and empty until you press
   **Scan repositories**. Each worktree shows its branch, its size split into
   build output vs. source, and a verdict. Build directories are ordinary
   selectable rows; a worktree verified safe gets its own **Remove worktree…**
   button; an unsafe one is shown read-only with the reason.
6. **Irreversible cleanup** — a separate collapsed danger zone for Docker
   volumes, which hold real data. These are never included by Review & Clean or
   by any select-all, and deleting them needs its own confirmation.
7. **Recent activity** — collapsed until something happens, then it opens with
   the result and the full output behind a details toggle.

Rows have a checkbox and a size, not a button each. One persistent action bar at
the bottom reports `3 items selected · 4.7 GB` and offers **Review & Clean**,
which opens a review dialog listing exactly what will go, what it should
reclaim, and a final `Clean 4.7 GB`. With nothing selected it reads
`Select items to clean` and stays disabled.

Scanning and cleaning run on background threads, so the window never freezes and
a background rescan never greys out the controls. The window follows your macOS
light/dark appearance.

## Usage

```bash
sweepmac                      # dry run: scan everything and report sizes
sweepmac --clean              # clean default categories (system, macos, dev, xcode)
sweepmac --clean --all        # also clean app caches (Brave/Spotify/Ableton — they re-download)
sweepmac --clean -c dev,xcode # clean only the given categories
sweepmac --clean --docker     # also: docker builder/image prune -af
sweepmac --clean --simulators # also: xcrun simctl delete unavailable
sweepmac --clean -y           # skip the confirmation prompt
sweepmac -c git               # find git worktrees + their build dirs (opt-in)
sweepmac --clean -c git       # delete those build dirs (regenerable)
sweepmac --remove-worktree <path>   # remove ONE verified-safe worktree
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
| `git`    | Git worktrees and their regenerable build dirs *(opt-in — the only category that looks inside a repository)* |
| `app`    | Chrome, Firefox, Edge, Arc, Brave, Slack, Discord, VS Code, Zoom, Notion, Figma, Telegram, Spotify, Ableton caches *(opt-in — these re-download; apps you don't have scan as empty and are hidden)* |

`system`, `macos`, `dev`, and `xcode` are cleaned by default — note that
includes **emptying the Trash**. `app` and `git` are opt-in via `--category` or
`--all`, and you can narrow any run with `-c` (e.g. `-c dev,xcode` to leave
Trash and logs alone). A bare `sweepmac` scan reports every cache but still
does not walk repositories.

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

## Git worktrees

Agents (Claude Code and similar) park worktrees under `<repo>/.claude/worktrees/`
and `~/.claude-worktrees/`, and each one carries a full build directory — one
sweepmac worktree held 2.3 GB of Rust `target/` behind 200 KB of source. The
`git` category finds this, but only when asked (`-c git` / `--all` / the GUI's
**Scan repositories** button / the tray's **Scan git worktrees…**):

- **Discovery is read-only.** Every git call is plumbing (`rev-parse`,
  `diff-index --cached`, `diff-files`, `ls-files`, `log`) run with
  `GIT_OPTIONAL_LOCKS=0`, never `git status` — which against a foreign
  work-tree leaves a stale `.git/index.lock` that breaks the repo for its
  actual owner.
- **Build dirs are ordinary cleanup.** `target/`, `node_modules/`, `.next/`,
  `build/`, `dist/`, `__pycache__/`, `.venv/` are reported and selectable in
  *every* worktree, including the primary one — they're regenerable wherever
  they sit, even inside a worktree that must otherwise be kept.
- **A worktree is SAFE to remove only if both hold:** no uncommitted or
  untracked changes outside a build directory, and its branch tip's *tree*
  matches a commit reachable from the repository's default branch — or is an
  ancestor of it. Trees, not commit ancestry: a rebased or cherry-picked branch
  has a different SHA and identical content, and `git branch --merged` calls it
  unmerged. sweepmac walks the default branch's history comparing
  `<commit>^{tree}` against the tip's tree instead.
- **`prunable` is never trusted alone.** Git sets that flag when a worktree's
  gitdir pointer doesn't resolve — which also happens under a bind mount or a
  relocated repo, with the working directory still live. sweepmac checks the
  admin entry under `<repo>/.git/worktrees/<name>/` and whether the directory
  itself exists before drawing any conclusion.
- **Removal is never bulk.** A SAFE worktree gets its own **Remove worktree…**
  button (GUI) or `--remove-worktree <path>` (CLI) — one at a time, matching
  how Docker volumes are quarantined. Before deleting anything, sweepmac tags
  the branch tip as `archive/<branch>` so the commits stay reachable, *then*
  removes the directory, runs `git worktree prune`, and deletes the branch.
- **UNSAFE worktrees are shown, never selectable** — with the specific reason
  (uncommitted changes, unmerged content, missing directory, `prunable` but
  live, …), so you always know why one was left alone.

## Fixing "out of pty devices"

Long macOS uptimes with heavy terminal/tmux/IDE use can exhaust the default
pty limit. `sweepmac fix-pty` raises `kern.tty.ptmx_max` (prompts for an admin
password via the native macOS dialog); `--persist` also writes it to
`/etc/sysctl.conf` so it survives reboots.

## Adding a target

Edit the `TARGETS` array in [src/lib.rs](src/lib.rs) — each entry is a path
relative to `$HOME`, a category, a description, and whether to clear the
directory's contents or remove it entirely.

## Layout

```
src/lib.rs        shared core: catalogue, scan/clean, Docker, git, selection rules
src/main.rs       CLI (Rust std only, zero dependencies)
src/bin/gui.rs    window: state, workers, sections, review/danger dialogs
src/bin/ui/       presentation only — style.rs (design tokens), widgets.rs
src/bin/tray.rs   menu-bar front-end
assets/tray/      menu-bar template glyph (SVG master + 1x/2x/3x PNG masks)
```

The GUI and tray are feature-gated (`gui`, `tray`), so a default
`cargo build` produces the CLI with no dependencies at all. Selection rules —
what a bulk select may tick, when the primary action is live — live in the
library so they are unit-tested without a window.

## License

[MIT](LICENSE)
