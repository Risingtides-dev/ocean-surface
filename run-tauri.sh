#!/usr/bin/env bash
# Launch Ocean Surface as a Tauri native desktop app: build the wasm bundle
# via Trunk, verify it, then boot the Tauri shell which loads that SAME dist/
# bundle as its frontend. Parity with the browser PWA — one bundle, two hosts.
#
# The Tauri crate at crates/ocean-tauri/ is standalone (not a workspace member)
# and managed by `cargo tauri`. This script orchestrates: trunk build → wasm
# magic guard → cargo tauri dev.
#
# Optional overrides:
#   export OCEAN_DAEMON_URL=http://<host>:4780   # default 127.0.0.1:4780
set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:$PATH"
export RUSTUP_HOME="$HOME/.rustup"
export CARGO_HOME="$HOME/.cargo"

DAEMON="${OCEAN_DAEMON_URL:-http://127.0.0.1:4780}"

echo "==> building wasm bundle (trunk)"
( cd "$REPO" && trunk build --release )

# Build-guard (OCEAN-121): trunk can report success yet leave a corrupt,
# all-zero wasm in dist/. Assert every dist *_bg.wasm starts with the wasm
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
  magic="$(head -c 4 "$w" | xxd -p)"
  if [[ "$magic" != "0061736d" ]]; then
    echo "FATAL: $w is corrupt — first 4 bytes are '$magic', expected '0061736d' (wasm magic)." >&2
    exit 1
  fi
  echo "    ok: $(basename "$w") ($magic)"
done

echo "==> daemon: $DAEMON"
echo "==> launching Tauri (loads $REPO/dist as frontendDist)"

# The Tauri crate is standalone — run from its own dir so its Cargo.lock applies.
cd "$REPO/crates/ocean-tauri"
exec env \
  OCEAN_DAEMON_URL="$DAEMON" \
  cargo tauri dev
