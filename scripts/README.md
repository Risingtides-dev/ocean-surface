# scripts/

Utility scripts for the ocean-surface repo.

## prune-cargo-caches.sh — cargo cache pruning rail

Reclaims disk from cargo `target/` caches, which pile up fast across the two
canonical checkouts and the constantly-churning worktree lanes under
`~/.worktrees/`. This machine has hit ENOSPC from ~170GB of accumulated
caches; this script keeps that from happening again.

**It is dry-run by default. Nothing is deleted unless you pass `--apply`.**

```bash
scripts/prune-cargo-caches.sh              # report what WOULD be freed
scripts/prune-cargo-caches.sh --apply      # actually delete
scripts/prune-cargo-caches.sh --days 3     # keep lane caches warm for 3 days (default 2)
scripts/prune-cargo-caches.sh --force      # run even while a build is in flight
```

Requires **bash >= 4** (associative arrays). macOS `/bin/bash` is 3.2 — the
shebang picks up Homebrew bash via `PATH`; the script also aborts with a clear
message if it somehow runs under 3.2.

### What it does (and its safety tiers)

1. **Worktree lanes under `~/.worktrees/`.** Ownership is decided by each
   dir's `.git` gitdir pointer, *not* by name:
   - A lane owned by an ocean canonical repo but no longer in `git worktree
     list` (an **orphan**) is deleted whole.
   - A **live** ocean lane keeps its `target*/` dirs unless they are older than
     `--days` (default 2) — active lanes stay warm.
   - A lane owned by **any other repo** (syzygy, Horus, Thoth, …), a standalone
     repo, or a plain dir (`backups/`) is reported and **skipped** — never
     touched.
2. **The two canonical checkouts** (`~/dev/ocean-surface`, `~/dev/ocean-os`):
   - `target/debug` is deleted only when older than 7 days.
   - Stray `target-*/` dirs (from `CARGO_TARGET_DIR`) are deleted only when
     older than 7 days.
   - **`target/release` is never touched** — the live daemon and proxy binaries
     run from there. `target/` as a whole is never removed either.
3. **`~/.cargo/registry` is never touched.**

Every deletion goes through a single primitive that hard-refuses any protected
path (the release dirs, canonical `target/` roots, `~`, `~/.cargo`, `/`), so a
logic bug cannot reach them.

### Safety guards

- **Dry-run by default** — `--apply` required to delete.
- **Build guard** — refuses to run while `cargo`, `rustc`, `rustdoc`, or
  `trunk` is running, unless `--force`.
- **Idempotent** — safe to run repeatedly; it only removes what currently
  qualifies.

### Scheduling (weekly LaunchAgent)

`prune-cargo-caches.plist` is a `launchd` template
(label `dev.risingtides.cargo-cache-prune`) that runs the script with `--apply`
every Sunday at 05:00, logging to `/tmp/cargo-cache-prune.log`. It invokes
`/opt/homebrew/bin/bash` explicitly. Install:

```bash
cp scripts/prune-cargo-caches.plist \
   ~/Library/LaunchAgents/dev.risingtides.cargo-cache-prune.plist
launchctl bootstrap gui/$(id -u) \
   ~/Library/LaunchAgents/dev.risingtides.cargo-cache-prune.plist
# uninstall:
launchctl bootout gui/$(id -u)/dev.risingtides.cargo-cache-prune
```
