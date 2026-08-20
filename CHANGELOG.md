# Changelog

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
