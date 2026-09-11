#!/usr/bin/env bash
# Keep the operator's live Ocean Surface pinned to a verified origin/main build.
#
# launchd runs this idempotently. A new main revision is built in a disposable
# detached worktree; the live `current` symlink and deployed-rev marker move only
# after every gate and bundle validation pass. Failures preserve the last-good
# release. `--promote DIR REV` exercises just the atomic promotion contract.
set -euo pipefail

REPO="${OCEAN_SURFACE_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
STATE_DIR="${OCEAN_SURFACE_STATE_DIR:-$HOME/.config/ocean-surface}"
RELEASES_DIR="$STATE_DIR/releases"
CURRENT_LINK="$STATE_DIR/current"
MARKER="$STATE_DIR/deployed-rev"
LOCK_DIR="$STATE_DIR/auto-deploy.lock"
WORKTREE_ROOT="${OCEAN_SURFACE_WORKTREE_ROOT:-/private/tmp/ocean-surface-auto-deploy}"
PROXY_LABEL="dev.risingtides.ocean-surface-proxy"
# The desktop shell (crates/ocean-tauri) serves this rail's promoted release
# from $STATE_DIR/current at request time (live_surface.rs), so a promote
# needs no shell restart: a hidden Ocean window reloads itself, a visible one
# receives `surface-updated` and Cmd+R re-reads the bundle. Only the shell's
# own Rust changes still need a rebuild — see note_tauri_rebuild_needed.
# TASK-87: live serving covers new web assets, but NOT changes to the shell's
# own Rust code — that needs `cargo tauri build`, which this rail
# deliberately does not run (it must never rebuild over a running app, and a
# release bundle build is minutes long). When the shell's source changes we
# therefore record a distinct marker: staleness that a restart CANNOT clear.
# This exists because TASK-78 and TASK-85 (both native exec fixes) landed and
# were announced while /Applications/Ocean.app kept running the old binary for
# hours — "landed" and "deployed" are different claims for this crate.
TAURI_REBUILD_MARKER="$STATE_DIR/tauri-rebuild-required"
DOMAIN="gui/$(id -u)"

export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

validate_bundle() {
  local dir="$1"
  [[ -f "$dir/index.html" ]] || fail "bundle has no index.html: $dir"

  local wasm_files=()
  while IFS= read -r -d '' wasm; do
    wasm_files+=("$wasm")
  done < <(find "$dir" -maxdepth 1 -type f -name '*_bg.wasm' -print0)
  (( ${#wasm_files[@]} == 1 )) || fail "bundle must contain exactly one *_bg.wasm: $dir"

  local magic
  magic="$(od -An -tx1 -N4 "${wasm_files[0]}" | tr -d '[:space:]')"
  [[ "$magic" == "0061736d" ]] || fail "bundle wasm is corrupt (magic=$magic): ${wasm_files[0]}"

  if grep -qE '127\.0\.0\.1:8080|localhost:8080|__trunk_address__' "$dir/index.html"; then
    fail "release bundle contains a Trunk development endpoint: $dir/index.html"
  fi
}

restart_proxy() {
  [[ "${OCEAN_SURFACE_NO_RESTART:-0}" == "1" ]] && return 0
  launchctl kickstart -k "$DOMAIN/$PROXY_LABEL"
}

# The desktop shell reads the promoted release live; no restart is owed for a
# web-only promote. Say so in the log so a reader does not go looking for a
# launchd job that was never there.
note_tauri_live_surface() {
  local revision="${1:-}"
  echo "TAURI: no restart needed — the shell serves $STATE_DIR/current live ($revision)"
}

# TASK-87: record when the shell's own source changed since the last deploy, so
# an owed `cargo tauri build` is VISIBLE instead of silent. Compares the
# previously deployed revision against the new one; on the first deploy (no
# prior marker) it says nothing rather than crying wolf.
note_tauri_rebuild_needed() {
  local prev="$1" revision="$2"
  [[ -n "$prev" ]] || return 0
  [[ "$prev" == "$revision" ]] && return 0
  # Only ask for a rebuild when the shell's own sources moved. Frontend-only
  # changes reach the shell through live serving of $STATE_DIR/current.
  local changed
  changed="$(git -C "$REPO" diff --name-only "$prev" "$revision" -- crates/ocean-tauri 2>/dev/null || true)"
  if [[ -n "$changed" ]]; then
    printf '%s\n' "$revision" > "$TAURI_REBUILD_MARKER"
    echo "TAURI: crates/ocean-tauri changed ($prev -> $revision) — REBUILD REQUIRED"
    echo "TAURI: a restart will NOT pick this up; run scripts/rebuild-tauri-app.sh"
  fi
}

rebuild_extension() {
  local deployed_dist="$1"
  local ext_dir="$REPO/extension"
  [[ -d "$ext_dir" && -f "$ext_dir/sidepanel.html" ]] || return 0

  local ext_dist="$ext_dir/dist"
  rm -rf "$ext_dist"
  mkdir -p "$ext_dist"
  # Trunk --release produces hashed names (ocean-surface-ui-HASH.js etc.);
  # map them to the stable names sidepanel.html expects.
  local js_file wasm_file
  js_file="$(ls "$deployed_dist"/ocean-surface-ui*.js 2>/dev/null | grep -v '_bg.wasm' | head -1 || true)"
  wasm_file="$(ls "$deployed_dist"/ocean-surface-ui*_bg.wasm 2>/dev/null | head -1 || true)"
  if [[ -z "$js_file" || ! -f "$js_file" ]] || [[ -z "$wasm_file" || ! -f "$wasm_file" ]]; then
    echo "EXTENSION: skipping — no wasm-bindgen files in $deployed_dist"
    return 0
  fi
  cp "$js_file"   "$ext_dist/ocean-surface-ui.js"
  cp "$wasm_file" "$ext_dist/ocean-surface-ui_bg.wasm"
  # Trunk --release produces hashed CSS names (tokens-HASH.css etc.);
  # sidepanel.html references stable names (tokens.css etc.). Strip the hash.
  local css base stable
  for css in "$deployed_dist"/*.css; do
    [[ -f "$css" ]] || continue
    base="$(basename "$css")"
    # tokens-b44329ae8bc1c369.css -> tokens.css (strip hash suffix)
    stable="${base%-*}.css"
    cp "$css" "$ext_dist/$stable"
  done
  if [[ -d "$deployed_dist/fonts" ]]; then
    mkdir -p "$ext_dist/fonts"
    cp "$deployed_dist/fonts"/* "$ext_dist/fonts/"
  fi
  for f in "$deployed_dist"/*.png "$deployed_dist"/*.webmanifest; do
    [[ -e "$f" ]] && cp "$f" "$ext_dist/" || true
  done
  echo "EXTENSION: rebuilt from $deployed_dist"
}

promote_bundle() {
  local source="$1"
  local revision="$2"
  # TASK-87: capture the OUTGOING revision before $MARKER is overwritten below.
  # Reading it afterwards always yields the incoming one, which silently
  # disables the shell-rebuild detector — caught by actually running a promote,
  # not by the source-assertion test, which passed while it was broken.
  local previous_revision=""
  [[ -f "$MARKER" ]] && previous_revision="$(cat "$MARKER" 2>/dev/null || true)"
  [[ "$revision" =~ ^[0-9A-Za-z._-]+$ ]] || fail "unsafe revision: $revision"
  validate_bundle "$source"
  mkdir -p "$RELEASES_DIR"

  # Inject freshness marker before atomic promotion so every release carries its
  # own identity. Surfaces read /.deploy-sha to detect staleness.
  printf '%s\n' "$revision" > "$source/.deploy-sha"

  local release="$RELEASES_DIR/$revision"
  local staged="$RELEASES_DIR/.${revision}.$$"
  local next_link="$STATE_DIR/.current.$$"
  local next_marker="$STATE_DIR/.deployed-rev.$$"
  rm -rf "$staged"
  rm -f "$next_link" "$next_marker"

  if [[ ! -d "$release" ]]; then
    mkdir -p "$staged"
    rsync -a --delete "$source/" "$staged/"
    validate_bundle "$staged"
    mv "$staged" "$release"
  else
    validate_bundle "$release"
  fi

  ln -s "releases/$revision" "$next_link"
  python3 -c 'import os,sys; os.replace(sys.argv[1], sys.argv[2])' "$next_link" "$CURRENT_LINK"
  printf '%s\n' "$revision" > "$next_marker"
  mv -f "$next_marker" "$MARKER"
  restart_proxy

  # Sync the deployed dist back to the canonical repo so Tauri (which loads
  # frontendDist = ../../dist) picks up the current bundle on next restart.
  # Also rebuild the extension from the deployed dist.
  local repo_dist="$REPO/dist"
  rm -rf "$repo_dist"
  mkdir -p "$repo_dist"
  rsync -a --delete "$source/" "$repo_dist/"
  validate_bundle "$repo_dist"
  rebuild_extension "$source"
  note_tauri_rebuild_needed "$previous_revision" "$revision"
  note_tauri_live_surface "$revision"

  echo "DEPLOYED: $revision -> $CURRENT_LINK"
}

if [[ "${1:-}" == "--promote" ]]; then
  (( $# == 3 )) || fail "usage: $0 --promote BUNDLE_DIR REVISION"
  mkdir -p "$STATE_DIR"
  promote_bundle "$2" "$3"
  exit 0
fi
(( $# == 0 )) || fail "usage: $0 [--promote BUNDLE_DIR REVISION]"

mkdir -p "$STATE_DIR"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  owner_pid="$(cat "$LOCK_DIR/pid" 2>/dev/null || true)"
  if [[ "$owner_pid" =~ ^[0-9]+$ ]] && kill -0 "$owner_pid" 2>/dev/null; then
    echo "SKIP: surface deployment already running as pid $owner_pid"
    exit 0
  fi
  rm -rf "$LOCK_DIR"
  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    echo "SKIP: another surface deployment acquired the lock"
    exit 0
  fi
fi
printf '%s\n' "$$" > "$LOCK_DIR/pid"

worktree=""
cleanup() {
  local status=$?
  if [[ -n "$worktree" ]]; then
    git -C "$REPO" worktree remove --force "$worktree" >/dev/null 2>&1 || true
  fi
  rm -rf "$LOCK_DIR"
  exit "$status"
}
trap cleanup EXIT INT TERM

if [[ -n "${OCEAN_SURFACE_TARGET_REV:-}" ]]; then
  target="$OCEAN_SURFACE_TARGET_REV"
else
  git -C "$REPO" fetch origin main --quiet
  target="$(git -C "$REPO" rev-parse origin/main)"
fi

if [[ -f "$MARKER" && "$(tr -d '[:space:]' < "$MARKER")" == "$target" && -L "$CURRENT_LINK" ]]; then
  current_release="$STATE_DIR/$(readlink "$CURRENT_LINK")"
  validate_bundle "$current_release"
  echo "CURRENT: $target"
  exit 0
fi

worktree="$WORKTREE_ROOT-$target"
git -C "$REPO" worktree remove --force "$worktree" >/dev/null 2>&1 || true
rm -rf "$worktree"
git -C "$REPO" worktree add --detach "$worktree" "$target" --quiet

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO/target}"
(
  cd "$worktree"
  cargo test -p ocean-surface-proxy
  cargo clippy -p ocean-surface-proxy --all-targets -- -D warnings
  cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
  cargo test -p ocean-surface-ui
  cargo clippy -p ocean-surface-ui --target wasm32-unknown-unknown -- -D warnings
  cargo clippy -p ocean-surface-ui --all-targets -- -D warnings
  cargo fmt --all -- --check
  node scripts/surface-auto-deploy.test.mjs
  env -u NO_COLOR trunk build --release
  cargo build -p ocean-surface-proxy --release
)

promote_bundle "$worktree/dist" "$target"
