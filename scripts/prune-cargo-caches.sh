#!/usr/bin/env bash
#
# prune-cargo-caches.sh — reclaim disk from cargo target/ caches, safely.
#
# Background: ocean-surface + ocean-os accumulate enormous cargo target/
# directories (this machine hit ENOSPC at ~170GB of caches). Worktree lanes
# under ~/.worktrees/ are created and pruned constantly by agents; their
# target*/ dirs (and stray CARGO_TARGET_DIR dirs like target-task52/) linger
# after the lane is gone. This script reclaims that space.
#
# It is DRY-RUN by default. Pass --apply to actually delete.
#
# Safety model (three tiers, most conservative for the live checkouts):
#   1. Worktree lanes under ~/.worktrees/ owned by an ocean canonical repo:
#        - orphaned lane dir (owned by ocean, no longer a live worktree)  -> delete whole dir
#        - live lane dir                                                  -> delete target*/ older than --days (default 2)
#      Dirs owned by a DIFFERENT repo (syzygy, Horus, ...) or with no git
#      ownership are reported and SKIPPED — never touched.
#   2. The two canonical checkouts (~/dev/ocean-surface, ~/dev/ocean-os):
#        - target/debug deleted only when older than 7 days
#        - stray target-*/ dirs deleted only when older than 7 days
#        - target/release is NEVER touched (live daemon + proxy run from there)
#        - target/ as a whole is NEVER removed
#   3. ~/.cargo/registry is NEVER touched.
#
# Refuses to run while cargo/rustc/rustdoc/trunk are building (unless --force).
#
set -euo pipefail

# Requires bash >= 4 (associative arrays). macOS /bin/bash is 3.2 — this
# script's shebang picks up Homebrew bash via PATH; fail loudly otherwise.
if [ "${BASH_VERSINFO:-0}" -lt 4 ]; then
  echo "FATAL: needs bash >= 4 (found ${BASH_VERSION:-unknown}). Run with /opt/homebrew/bin/bash." >&2
  exit 1
fi

# ----------------------------------------------------------------------------
# Configuration (absolute, user 501)
# ----------------------------------------------------------------------------
readonly HOME_DIR="/Users/risingtidesdev"
readonly WT_ROOT="${HOME_DIR}/.worktrees"
readonly CANON_SURFACE="${HOME_DIR}/dev/ocean-surface"
readonly CANON_OS="${HOME_DIR}/dev/ocean-os"
readonly CANON_REPOS=("${CANON_SURFACE}" "${CANON_OS}")

# Paths that must NEVER be deleted, whatever the logic decides.
readonly PROTECTED_PATHS=(
  "/"
  "${HOME_DIR}"
  "${HOME_DIR}/.cargo"
  "${HOME_DIR}/.cargo/registry"
  "${CANON_SURFACE}"
  "${CANON_OS}"
  "${CANON_SURFACE}/target"
  "${CANON_OS}/target"
  "${CANON_SURFACE}/target/release"
  "${CANON_OS}/target/release"
)

LANE_DAYS=2          # live-lane target*/ must be older than this to prune
readonly CANON_DAYS=7 # canonical target/debug + stray target-*/ threshold
APPLY=0
FORCE=0

# ----------------------------------------------------------------------------
# Accounting
# ----------------------------------------------------------------------------
TOTAL_KB=0

log()  { printf '%s\n' "$*"; }
warn() { printf 'WARN: %s\n' "$*" >&2; }
die()  { printf 'FATAL: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<EOF
Usage: ${0##*/} [--apply] [--days N] [--force]

  --apply     Actually delete. Without it, the script only reports (dry-run).
  --days N     Age threshold in days for live-lane target*/ dirs (default: ${LANE_DAYS}).
  --force      Run even if cargo/rustc/rustdoc/trunk are currently building.
  -h, --help  This help.

Dry-run by default. Canonical target/release dirs are always protected.
EOF
}

# Kilobytes of a path (0 if missing).
size_kb() {
  local p="$1"
  [ -e "$p" ] || { echo 0; return; }
  du -sk "$p" 2>/dev/null | awk '{print $1}'
}

human() {
  local kb="$1"
  awk -v kb="$kb" 'BEGIN{
    split("KB MB GB TB", u, " ");
    v=kb; i=1;
    while (v>=1024 && i<4){ v/=1024; i++ }
    printf("%.1f%s", v, u[i]);
  }'
}

# Absolute, symlink-resolved path (best effort; falls back to the input).
canonicalize() {
  local p="$1"
  if command -v realpath >/dev/null 2>&1; then
    realpath -m "$p" 2>/dev/null || echo "$p"
  else
    # Resolve parent, keep basename — enough for our fixed paths.
    local dir base
    dir="$(cd "$(dirname "$p")" 2>/dev/null && pwd -P)" || { echo "$p"; return; }
    base="$(basename "$p")"
    echo "${dir}/${base}"
  fi
}

is_protected() {
  local target rp p
  target="$(canonicalize "$1")"
  for p in "${PROTECTED_PATHS[@]}"; do
    rp="$(canonicalize "$p")"
    [ "$target" = "$rp" ] && return 0
    # Anything at or under a protected release dir is off-limits.
    case "$target" in
      "${rp}/"*)
        case "$rp" in
          */target/release) return 0 ;;
        esac
        ;;
    esac
  done
  # Belt and suspenders: never touch a */target/release subtree.
  case "$target" in
    "${CANON_SURFACE}/target/release"*|"${CANON_OS}/target/release"*) return 0 ;;
  esac
  return 1
}

# The one and only deletion primitive. Honors dry-run and protection.
remove_path() {
  local path="$1" label="$2" kb
  if [ -z "$path" ] || [ "$path" = "/" ]; then
    die "refusing to remove empty/root path"
  fi
  if is_protected "$path"; then
    die "refusing to remove PROTECTED path: $path"
  fi
  [ -e "$path" ] || return 0
  kb="$(size_kb "$path")"
  TOTAL_KB=$(( TOTAL_KB + kb ))
  if [ "$APPLY" -eq 1 ]; then
    log "  DELETE  [$(human "$kb")]  $label  ($path)"
    rm -rf -- "$path"
  else
    log "  WOULD-DELETE  [$(human "$kb")]  $label  ($path)"
  fi
}

# True if $path's directory mtime is strictly older than $days days.
older_than() {
  local path="$1" days="$2"
  [ -e "$path" ] || return 1
  [ -n "$(find "$path" -maxdepth 0 -mtime "+${days}" 2>/dev/null)" ]
}

# Guard: refuse while a build is in flight.
build_running() {
  local p
  for p in cargo rustc rustdoc trunk; do
    if pgrep -x "$p" >/dev/null 2>&1; then
      echo "$p"
      return 0
    fi
  done
  return 1
}

# Collect live worktree paths (canonical roots + lanes) from both repos.
# Populates the LIVE_WORKTREES associative array (path -> 1).
declare -A LIVE_WORKTREES
load_live_worktrees() {
  local repo line path
  for repo in "${CANON_REPOS[@]}"; do
    [ -d "$repo/.git" ] || [ -f "$repo/.git" ] || continue
    while IFS= read -r line; do
      case "$line" in
        "worktree "*)
          path="${line#worktree }"
          LIVE_WORKTREES["$(canonicalize "$path")"]=1
          ;;
      esac
    done < <(git -C "$repo" worktree list --porcelain 2>/dev/null)
  done
}

# Read the gitdir a worktree's .git file points at; empty if not a linked wt.
lane_gitdir() {
  local dir="$1"
  if [ -f "$dir/.git" ]; then
    sed -n 's/^gitdir: //p' "$dir/.git" 2>/dev/null | head -n1
  fi
}

# Which canonical repo owns this lane dir? Echoes the repo path or empty.
# Ownership is decided by the .git gitdir pointer, matched against
# "<canon>/.git/worktrees/" — this survives even a pruned admin entry, and
# it NEVER matches a lane belonging to some other repo.
lane_owner() {
  local dir="$1" gd repo
  gd="$(lane_gitdir "$dir")"
  [ -n "$gd" ] || return 1
  for repo in "${CANON_REPOS[@]}"; do
    case "$gd" in
      "${repo}/.git/worktrees/"*) echo "$repo"; return 0 ;;
    esac
  done
  return 1
}

# Prune target/ and target-*/ dirs inside a live lane, by age.
prune_lane_targets() {
  local lane="$1" t
  shopt -s nullglob
  for t in "$lane"/target "$lane"/target-*; do
    [ -d "$t" ] || continue
    if older_than "$t" "$LANE_DAYS"; then
      remove_path "$t" "live-lane cache (>${LANE_DAYS}d)"
    else
      log "  keep (warm, <=${LANE_DAYS}d)  [$(human "$(size_kb "$t")")]  $t"
    fi
  done
  shopt -u nullglob
}

# ----------------------------------------------------------------------------
# Stages
# ----------------------------------------------------------------------------
stage_worktree_lanes() {
  log ""
  log "== Stage 1: worktree lanes under ${WT_ROOT} =="
  [ -d "$WT_ROOT" ] || { log "  (${WT_ROOT} does not exist — skipping)"; return; }

  local dir base owner rp
  shopt -s nullglob
  for dir in "$WT_ROOT"/*/; do
    dir="${dir%/}"
    base="$(basename "$dir")"
    rp="$(canonicalize "$dir")"

    if owner="$(lane_owner "$dir")"; then
      # Owned by an ocean canonical repo.
      if [ -n "${LIVE_WORKTREES[$rp]+x}" ]; then
        log "  [live ocean lane] $base  (owner: ${owner##*/})"
        prune_lane_targets "$dir"
      else
        log "  [ORPHAN ocean lane] $base  (owner: ${owner##*/}, not in worktree list)"
        remove_path "$dir" "orphaned ocean lane"
      fi
    else
      # Not owned by an ocean repo — another project's worktree, a plain
      # dir, or a standalone repo. Never destructive here.
      local gd; gd="$(lane_gitdir "$dir")"
      if [ -n "$gd" ]; then
        log "  [skip: other repo]  $base  (gitdir: $gd)"
      else
        log "  [skip: unmanaged]   $base  (no ocean worktree ownership)"
      fi
    fi
  done
  shopt -u nullglob
}

stage_canonical() {
  log ""
  log "== Stage 2: canonical checkouts (target/debug + stray target-*/, >${CANON_DAYS}d) =="
  local repo debug t
  shopt -s nullglob
  for repo in "${CANON_REPOS[@]}"; do
    [ -d "$repo" ] || { log "  (${repo} missing — skipping)"; continue; }
    log "  ${repo}"
    log "    target/release -> PROTECTED (never touched)"

    debug="$repo/target/debug"
    if [ -d "$debug" ]; then
      if older_than "$debug" "$CANON_DAYS"; then
        remove_path "$debug" "canonical target/debug (>${CANON_DAYS}d)"
      else
        log "    keep (fresh, <=${CANON_DAYS}d)  [$(human "$(size_kb "$debug")")]  $debug"
      fi
    fi

    for t in "$repo"/target-*; do
      [ -d "$t" ] || continue
      if older_than "$t" "$CANON_DAYS"; then
        remove_path "$t" "stray CARGO_TARGET_DIR (>${CANON_DAYS}d)"
      else
        log "    keep (fresh stray, <=${CANON_DAYS}d)  [$(human "$(size_kb "$t")")]  $t"
      fi
    done
  done
  shopt -u nullglob
}

# ----------------------------------------------------------------------------
# Main
# ----------------------------------------------------------------------------
main() {
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --apply) APPLY=1 ;;
      --force) FORCE=1 ;;
      --days)
        shift
        [ "${1:-}" ] || die "--days needs a value"
        case "$1" in (*[!0-9]*|'') die "--days must be a non-negative integer" ;; esac
        LANE_DAYS="$1"
        ;;
      --days=*)
        LANE_DAYS="${1#*=}"
        case "$LANE_DAYS" in (*[!0-9]*|'') die "--days must be a non-negative integer" ;; esac
        ;;
      -h|--help) usage; exit 0 ;;
      *) die "unknown argument: $1 (see --help)" ;;
    esac
    shift
  done

  if [ "$APPLY" -eq 1 ]; then
    log "MODE: APPLY (deletions are REAL)"
  else
    log "MODE: DRY-RUN (no deletions; pass --apply to delete)"
  fi
  log "lane age threshold: ${LANE_DAYS}d   canonical age threshold: ${CANON_DAYS}d"

  local proc
  if proc="$(build_running)"; then
    if [ "$FORCE" -eq 1 ]; then
      warn "'$proc' is running but --force given; proceeding."
    else
      die "'$proc' is currently running — refusing to prune (use --force to override)."
    fi
  fi

  load_live_worktrees
  stage_worktree_lanes
  stage_canonical

  log ""
  if [ "$APPLY" -eq 1 ]; then
    log "== Freed: $(human "$TOTAL_KB") ($TOTAL_KB KB) =="
  else
    log "== Reclaimable: $(human "$TOTAL_KB") ($TOTAL_KB KB) — run with --apply to reclaim =="
  fi
}

main "$@"
