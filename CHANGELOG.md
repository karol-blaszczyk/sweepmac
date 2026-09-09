# Changelog

## Unreleased

- New opt-in `git` category: finds git worktrees under a root (agents park
  them under `.claude/worktrees/` and `~/.claude-worktrees/`, each carrying a
  full build directory) and reports per-worktree branch, HEAD, size, and a
  build-vs-source breakdown for `target/`, `node_modules/`, `.next/`,
  `build/`, `dist/`, `__pycache__/`, `.venv/`.
  - A worktree is classified SAFE to remove only when it has no uncommitted
    or untracked changes outside a build directory AND its tip's *tree*
    (not commit SHA) is already reachable from the repo's default branch —
    so rebased/cherry-picked branches are recognized correctly.
  - Discovery is strictly read-only: plumbing only, `GIT_OPTIONAL_LOCKS=0`,
    never `git status`, so it can't leave a stale `.git/index.lock` behind.
  - Removing a SAFE worktree tags its tip as `archive/<branch>` first, then
    deletes the directory, prunes the admin entry, and deletes the branch —
    never bulk, one at a time, via `--remove-worktree <path>` (CLI) or
    **Remove worktree…** (GUI).
  - Regenerable build dirs are selectable cleanup wherever they sit,
    including inside the primary worktree and inside worktrees that must be
    kept.
  - This is a departure from the previous "git repos are never scanned"
    promise — `git` is opt-in (`-c git` / `--all`), not in
    `DEFAULT_CLEAN_CATEGORIES`, and off by default everywhere.
  - Wired into all three front-ends: CLI (`-c git`, `--remove-worktree`),
    GUI (a new "Git worktrees & build caches" section), and the menu bar
    (**Scan git worktrees…**, reported separately from the headline total).

## v0.2.0 — 2026-08-26

- Added 11 popular app caches: Chrome, Firefox, Edge, Arc, Slack, Discord,
  VS Code, Zoom, Notion, Figma, Telegram (31 cache locations total).
- CLI now hides empty and not-installed rows, collapsing them into a single
  "(N more empty or not installed)" line.
- Fixed row overlap and an unreadable checkbox state in the GUI.
- Fixed clippy/rustc 1.98 lints across the binaries.
- Release workflow now has `contents: write` so the DMG attaches to the release.

## v0.1.0 — 2026-08-20

Initial release.

- CLI (`sweepmac`): scan-by-default cache cleaner for regenerable macOS caches
  (`system`, `macos`, `dev`, `xcode` by default; `app` opt-in), plus `--docker`,
  `--simulators`, and a `fix-pty` subcommand.
- GUI (`sweepmac-gui`): category checkboxes, Docker images/volumes panel with
  multi-select delete, node_modules finder with bulk delete.
- Menu bar (`sweepmac-tray`): 🧹 status-bar dropdown with one-click actions.
- Homebrew tap (repo doubles as tap: `Formula/sweepmac.rb`) and a
  drag-to-install DMG built by `packaging/make-app.sh` (unsigned — see README).
