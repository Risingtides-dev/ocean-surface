#!/usr/bin/env bash
# Launch Ocean Surface: build the WASM bundle and release proxy, then serve
# both from one process. Point your browser (or phone) at the printed URL.
#
# Operator login is enabled by default because the default bind is reachable
# over the LAN/tailnet. Set OCEAN_SURFACE_USER and OCEAN_SURFACE_PASS, or use
# OCEAN_SURFACE_AUTH=off only with a trusted localhost bind:
#   OCEAN_SURFACE_BIND=127.0.0.1:18790 OCEAN_SURFACE_AUTH=off ./run-surface.sh
#
# Optional overrides:
#   export OCEAN_DAEMON_URL=http://<host>:4780   # default 127.0.0.1:4780
#   export OCEAN_VOICE_PROFILE=leo              # daemon voice profile
#   export OCEAN_SURFACE_BIND=0.0.0.0:8790      # LAN/tailnet; default below
set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"
export RUSTUP_HOME="$HOME/.rustup"
export CARGO_HOME="$HOME/.cargo"

BIND="${OCEAN_SURFACE_BIND:-0.0.0.0:8790}"
DAEMON="${OCEAN_DAEMON_URL:-http://127.0.0.1:4780}"
AUTH="${OCEAN_SURFACE_AUTH:-on}"
BIND_HOST="${BIND%:*}"

if [[ "$AUTH" == "off" ]] && [[ "$BIND_HOST" != "127.0.0.1" && "$BIND_HOST" != "[::1]" ]]; then
  echo "FATAL: OCEAN_SURFACE_AUTH=off is allowed only with a loopback bind." >&2
  echo "       Use OCEAN_SURFACE_BIND=127.0.0.1:18790, or enable operator login." >&2
  exit 1
fi

USER_COMPACT="${OCEAN_SURFACE_USER:-}"
USER_COMPACT="${USER_COMPACT//[[:space:]]/}"
PASS_COMPACT="${OCEAN_SURFACE_PASS:-}"
PASS_COMPACT="${PASS_COMPACT//[[:space:]]/}"
if [[ "$AUTH" != "off" ]] && { [[ -z "$USER_COMPACT" ]] || [[ -z "$PASS_COMPACT" ]]; }; then
  echo "FATAL: operator login is enabled, but OCEAN_SURFACE_USER / OCEAN_SURFACE_PASS are not both set." >&2
  echo "       Set both for tailnet or trusted-LAN access, or use this trusted-localhost command:" >&2
  echo "       OCEAN_SURFACE_BIND=127.0.0.1:18790 OCEAN_SURFACE_AUTH=off ./run-surface.sh" >&2
  exit 1
fi

echo "==> building wasm bundle (trunk)"
( cd "$REPO" && trunk build --release )

echo "==> building surface proxy (release)"
# Keep the launcher and executable path deterministic even when the caller has
# a custom CARGO_TARGET_DIR in their shell.
( cd "$REPO" && cargo build -p ocean-surface-proxy --release --target-dir "$REPO/target" )

# Build-guard (OCEAN-121): trunk can report "✅ success" yet leave a corrupt,
# all-zero wasm in dist/ (e.g. when its wasm-opt step fails). Shipping that
# serves a blank page. Assert every dist *_bg.wasm starts with the WebAssembly
# magic word "00 61 73 6d" and fail loudly otherwise — never ship a dead bundle.
echo "==> verifying dist wasm magic word"
shopt -s nullglob
wasm_files=( "$REPO"/dist/*_bg.wasm )
shopt -u nullglob
if (( ${#wasm_files[@]} == 0 )); then
  echo "FATAL: no dist/*_bg.wasm produced by trunk build --release" >&2
  exit 1
fi
for w in "${wasm_files[@]}"; do
  # Read the first 4 bytes as hex; must equal the wasm magic "0061736d".
  magic="$(head -c 4 "$w" | xxd -p)"
  if [[ "$magic" != "0061736d" ]]; then
    echo "FATAL: $w is corrupt — first 4 bytes are '$magic', expected '0061736d' (wasm magic)." >&2
    echo "       trunk likely failed wasm-opt while reporting success; refusing to serve a blank bundle." >&2
    exit 1
  fi
  echo "    ok: $(basename "$w") ($magic)"
done

if [[ "$AUTH" == "off" ]]; then
  echo "==> auth: DISABLED (trusted-localhost use only)"
else
  echo "==> auth: username/password session login enabled"
fi
echo "==> daemon: $DAEMON"
echo "==> serving on http://$BIND   (open this in your browser)"

exec env \
  OCEAN_SURFACE_BIND="$BIND" \
  OCEAN_SURFACE_DIST="$REPO/dist" \
  OCEAN_DAEMON_URL="$DAEMON" \
  "$REPO/target/release/ocean-surface-proxy"
