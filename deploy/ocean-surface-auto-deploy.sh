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

promote_bundle() {
  local source="$1"
  local revision="$2"
  [[ "$revision" =~ ^[0-9A-Za-z._-]+$ ]] || fail "unsafe revision: $revision"
  validate_bundle "$source"
  mkdir -p "$RELEASES_DIR"

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
  cargo fmt --all -- --check
  node scripts/surface-auto-deploy.test.mjs
  env -u NO_COLOR trunk build --release
  cargo build -p ocean-surface-proxy --release
)

promote_bundle "$worktree/dist" "$target"
