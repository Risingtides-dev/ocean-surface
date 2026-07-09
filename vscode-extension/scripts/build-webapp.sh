#!/usr/bin/env bash
# Build the Ocean Leptos WASM surface and copy the bundle into the extension so
# the "Ocean: Open Web Surface (full app)" command can host it in a webview.
# Requires: rustup + wasm32-unknown-unknown target + trunk.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EXT_DIR="$REPO_ROOT/vscode-extension"

echo "Building WASM surface (trunk build --release) from $REPO_ROOT ..."
( cd "$REPO_ROOT" && trunk build --release )

mkdir -p "$EXT_DIR/media/webapp"
rm -f "$EXT_DIR"/media/webapp/*
cp "$REPO_ROOT"/dist/ocean-surface-ui-*.js \
   "$REPO_ROOT"/dist/ocean-surface-ui-*_bg.wasm \
   "$REPO_ROOT"/dist/style-*.css \
   "$EXT_DIR/media/webapp/"
echo "Copied bundle into $EXT_DIR/media/webapp/:"
ls -1 "$EXT_DIR/media/webapp/"
