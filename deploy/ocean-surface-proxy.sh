#!/usr/bin/env bash
# Supervised launcher for the Ocean Surface proxy (OCEAN-161).
#
# This is the entrypoint launchd execs (via dev.risingtides.ocean-surface-proxy.plist).
# Unlike run-surface.sh, it does NOT run a trunk build — a supervised service must
# respawn fast and deterministically, so the wasm bundle is built once at install
# time (see ops/install-surface-proxy.sh) and this script only serves it. Prod
# serves the dedicated dist-prod dir (~/.config/ocean-surface/dist-prod), which
# dev rebuilds (trunk serve / run-surface.sh) never touch — so a dev loop can
# never clobber the public site the way the shared repo dist dir once did.
#
# It exec's the prebuilt proxy binary with the production env. The xAI key is NOT
# baked in here; the binary resolves it from ~/.config/ocean-surface/xai.key (or
# env XAI_API_KEY). Override any of the vars below by exporting them before launch
# or by editing the EnvironmentVariables block in the plist.
set -euo pipefail

# Repo root = parent of this deploy/ dir, resolved absolutely (symlink-safe).
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Toolchain on PATH (launchd starts with a minimal PATH).
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:$PATH"

BIN="$REPO/target/release/ocean-surface-proxy"
if [[ ! -x "$BIN" ]]; then
  echo "FATAL: $BIN not found or not executable." >&2
  echo "       Build it first:  cargo build -p ocean-surface-proxy --release" >&2
  echo "       (ops/install-surface-proxy.sh does this for you.)" >&2
  exit 127
fi

# Production dist dir — the dedicated dist-prod bundle that dev rebuilds never
# touch (see header). Honored from env/plist, defaulting to the prod dir. Set
# BEFORE the magic-word guard below so the guard and the serve path can never
# diverge: the glob, the magic check, and what the proxy actually serves all
# read this one variable.
export OCEAN_SURFACE_DIST="${OCEAN_SURFACE_DIST:-$HOME/.config/ocean-surface/dist-prod}"

# Guard: never serve a corrupt/all-zero wasm (OCEAN-121). Assert the bundle's
# magic word before binding, so a bad build fails loudly instead of serving blank.
shopt -s nullglob
wasm_files=( "$OCEAN_SURFACE_DIST"/*_bg.wasm )
shopt -u nullglob
if (( ${#wasm_files[@]} == 0 )); then
  echo "FATAL: no *_bg.wasm present in $OCEAN_SURFACE_DIST — copy a committed release bundle into ~/.config/ocean-surface/dist-prod (dev rebuilds are not served)." >&2
  exit 1
fi
for w in "${wasm_files[@]}"; do
  magic="$(head -c 4 "$w" | xxd -p)"
  if [[ "$magic" != "0061736d" ]]; then
    echo "FATAL: $w is corrupt (first 4 bytes '$magic', expected '0061736d'). Refusing to serve a dead bundle." >&2
    exit 1
  fi
done

# Production env. These mirror run-surface.sh; override via the plist or env.
# (OCEAN_SURFACE_DIST is exported above, before the guard.)
export OCEAN_SURFACE_BIND="${OCEAN_SURFACE_BIND:-0.0.0.0:8790}"
export OCEAN_DAEMON_URL="${OCEAN_DAEMON_URL:-http://127.0.0.1:4780}"

echo "==> ocean-surface-proxy: bind=$OCEAN_SURFACE_BIND dist=$OCEAN_SURFACE_DIST daemon=$OCEAN_DAEMON_URL"
exec "$BIN"
