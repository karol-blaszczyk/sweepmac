# Security

sweepmac deletes files. This document states exactly what it will and will not
touch, so you can judge the blast radius before running it — and how to report a
problem if it ever does something outside these bounds.

## Supported versions

Only the latest release receives fixes. See
[releases](https://github.com/karol-blaszczyk/sweepmac/releases).

## What sweepmac deletes

Every deletion is one of these, and nothing else:

- **Catalogued caches** — a fixed list in `TARGETS` ([src/lib.rs](src/lib.rs)),
  each a path **relative to `$HOME`**. A unit test asserts every entry is
  relative, does not start with `/` or `~`, and contains no `..` component, so
  the catalogue cannot address anything outside your home directory.
- **Regenerable build directories** — `target/`, `node_modules/`, `.next/`,
  `build/`, `dist/`, `__pycache__/`, `.venv/`, matched by exact directory name.
- **Docker objects** — via `docker` itself (`builder`/`image`/`container`/
  `network prune`, `rmi`, `volume rm`), never by touching Docker's files.
- **A git worktree you removed individually**, after sweepmac verified it is
  safe (see below).

## What sweepmac never touches

- Documents, source files, and anything outside `$HOME`.
- Any path reached by following a symlink — the walkers skip symlinks and size
  them as zero rather than descending.
- A repository's primary checkout. Only its build directories are ever offered.
- Git history. Removing a worktree tags its branch tip as `archive/<branch>`
  **before** anything is deleted, so no commit is made unreachable.

## Destructive actions are gated, not bulk

- Nothing is deleted without `--clean` (CLI) or the review dialog (GUI).
- Docker **volumes** hold real data: they are quarantined in their own
  confirmation path, require an explicit acknowledgement checkbox, and are
  excluded from every select-all by the `Risk::Irreversible` rule.
- **Worktree removal is never bulk.** One at a time, via
  `--remove-worktree <path>` or a per-row button, and only for a worktree
  sweepmac classified SAFE.

## Read-only guarantee for git repositories

Repositories are only walked when you ask (`-c git`, `--all`, or the GUI/tray
button) — never as part of an ordinary scan. That walk is strictly read-only:

- Git plumbing only (`rev-parse`, `diff-index --cached`, `diff-files`,
  `ls-files`, `log`, `worktree list`), run with `GIT_OPTIONAL_LOCKS=0`.
- **`git status` is never used.** Run against a foreign work-tree it can leave a
  stale `.git/index.lock` behind, which breaks the repository for its owner. A
  test asserts no `index.lock` exists after a full discovery pass.

A worktree is classified SAFE to remove only if **both** hold: it has no
uncommitted or untracked changes outside a build directory, **and** its branch
tip's *tree* is already reachable from the default branch (or the tip is an
ancestor of it). Trees are compared rather than commit SHAs, so a rebased or
cherry-picked branch is correctly recognised as already-landed. The `prunable`
flag is never trusted on its own, because git also sets it for bind mounts and
relocated repos whose working directory is still live.

## Privilege

sweepmac runs entirely as your user. The **only** privileged operation is
`fix-pty`, which raises `kern.tty.ptmx_max` via the native macOS admin prompt
(`osascript … with administrator privileges`). It is never invoked implicitly —
you have to ask for it. sweepmac makes no network requests and collects no
telemetry.

## Reporting a vulnerability

Please report privately rather than opening a public issue:

**[Open a private security advisory](https://github.com/karol-blaszczyk/sweepmac/security/advisories/new)**

Useful things to include: your macOS version, the sweepmac version
(`sweepmac --help`), the exact command or UI action, and what was deleted or
modified that shouldn't have been.

Reports of *data loss outside the bounds described above* are treated as
security issues, not ordinary bugs.

## Note on distribution

Release DMGs are **not signed or notarized**. Gatekeeper will refuse them until
you allow the app explicitly or clear the quarantine flag — see the README. If
you would rather not do that, install via Homebrew, which builds from source on
your own machine.
