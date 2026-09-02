#!/usr/bin/env bash
# Rebuild and install the Ocean desktop shell (TASK-87).
#
# WHY THIS EXISTS: the surface auto-deploy rail promotes web assets and can
# restart the shell, but it never RECOMPILES it. Changes to crates/ocean-tauri
# are Rust — a restart cannot pick them up. That gap shipped two native
# arbitrary-execution fixes (TASK-78, TASK-85) that stayed undeployed for hours
# while being reported as landed. The rail now writes
# ~/.config/ocean-surface/tauri-rebuild-required when the shell's sources move;
# this script is what clears it.
#
# Deliberately NOT run by the rail: a release bundle build takes minutes, and
# replacing a running .app underneath the operator is hostile. This is an
# operator-invoked step that refuses to do anything surprising.
set -euo pipefail

REPO="${OCEAN_SURFACE_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
STATE_DIR="${OCEAN_SURFACE_STATE_DIR:-$HOME/.config/ocean-surface}"
REBUILD_MARKER="$STATE_DIR/tauri-rebuild-required"
APP_DEST="${OCEAN_TAURI_APP_DEST:-/Applications/Ocean.app}"
BUNDLE="$REPO/crates/ocean-tauri/target/release/bundle/macos/Ocean.app"

log() { printf '%s\n' "$*"; }
fail() { printf 'FATAL: %s\n' "$*" >&2; exit 1; }

# --- refuse to clobber a running app ----------------------------------------
# Learned by doing this by hand: replacing the bundle under a live process
# leaves a half-broken app and loses the operator's session.
if pgrep -x "ocean-tauri" >/dev/null 2>&1; then
  fail "Ocean is running — quit it first (this script will not replace a live app)."
fi

# --- build ------------------------------------------------------------------
# frontendDist points at ../../dist, and `tauri::generate_context!` PANICS at
# compile time when it is missing — so a bare checkout fails confusingly.
[[ -f "$REPO/dist/index.html" ]] || fail "no dist/ — run 'trunk build --release' first."

log "==> building shell bundle (this takes a few minutes)"
( cd "$REPO/crates/ocean-tauri" && cargo tauri build --bundles app )
[[ -d "$BUNDLE" ]] || fail "build reported success but no bundle at $BUNDLE"

# --- verify the binary, not the exit code -----------------------------------
# A successful build is not evidence the fix is in the binary. Each of these
# strings is an error path added by a shell security boundary; if the bundle
# were stale or built from the wrong tree they would be absent.
#
# The third is the room-authority forwarder's allowlist refusal (spec 1.12).
# It is what makes the desktop authorization ceremony writable, and it is
# Rust: a running shell cannot pick it up by restarting, only by this rebuild.
# An installed app missing this string still shows the read-only notice, so
# the string is the difference between "shipped" and "reported as shipped".
BIN="$BUNDLE/Contents/MacOS/ocean-tauri"
[[ -x "$BIN" ]] || fail "no executable at $BIN"
for needle in "refusing to open an executable file" "path escapes workspace root" \
              "not a room authority mutation route"; do
  if ! strings "$BIN" 2>/dev/null | grep -qF "$needle"; then
    fail "built binary is missing an expected guard ('$needle') — refusing to install."
  fi
done
log "==> verified security guards present in the compiled binary"

# --- install ----------------------------------------------------------------
rm -rf "$APP_DEST"
cp -R "$BUNDLE" "$APP_DEST"
log "==> installed $APP_DEST"

# The mic entitlement lives in a plist FILE referenced from tauri.conf.json
# (an inline object silently fails this Tauri version's schema — TASK-63).
if /usr/libexec/PlistBuddy -c "Print :NSMicrophoneUsageDescription" \
     "$APP_DEST/Contents/Info.plist" >/dev/null 2>&1; then
  log "==> microphone usage description present"
else
  log "WARN: NSMicrophoneUsageDescription missing — voice capture will be denied by TCC"
fi

rm -f "$REBUILD_MARKER"
log "==> rebuild marker cleared; shell is current"
