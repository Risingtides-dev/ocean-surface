#!/usr/bin/env bash
# Install + supervise the Ocean Surface proxy under launchd (OCEAN-161 / OCEAN-385).
#
# Idempotent. Safe to re-run after a pull/rebuild. What it does:
#   1. Builds the proxy binary (release) from MAIN and ensures a valid wasm bundle.
#   2. Copies the LaunchAgent plist into ~/Library/LaunchAgents/.
#   3. By default PRINTS the bootstrap/kickstart commands for you to run.
#      Pass --bootstrap to actually touch the live launchd on this box.
#
# The live bootstrap is OPT-IN. Without --bootstrap this script only builds and
# stages the plist; it does not start, stop, or restart anything. Mirrors
# ocean-os's ops/install-ocean-daemon.sh (build-from-main guard + idempotency).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LABEL="dev.risingtides.ocean-surface-proxy"
PLIST_SRC="$REPO/deploy/$LABEL.plist"
PLIST_DST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

# --bootstrap (off by default) opts in to touching the live launchd domain.
BOOTSTRAP=0
for arg in "$@"; do
  case "$arg" in
    --bootstrap) BOOTSTRAP=1 ;;
    -h|--help)
      sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      echo "unknown arg: $arg (use --bootstrap or --help)" >&2
      exit 2 ;;
  esac
done

export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"

# --- build-from-main guard (operator rule) -------------------------------------
# The supervised service runs a PREBUILT binary; that binary must be built from
# main. Warn loudly if the repo isn't on main.
branch="$(git -C "$REPO" rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"
if [[ "$branch" != "main" ]]; then
  echo "WARNING: repo at $REPO is on branch '$branch', not 'main'." >&2
  echo "         Operator rule: build/deploy from MAIN only." >&2
  echo "         Continuing in 3s — Ctrl-C to abort and 'git checkout main' first." >&2
  sleep 3
fi

echo "==> [1/3] building proxy + wasm bundle (release) from '$branch'"
# Build the binary the service runs.
( cd "$REPO" && cargo build -p ocean-surface-proxy --release )
BIN="$REPO/target/release/ocean-surface-proxy"
if [[ ! -x "$BIN" ]]; then
  echo "FATAL: build did not produce an executable at $BIN" >&2
  exit 1
fi
# Ensure a servable bundle exists. If trunk is available and no bundle is present,
# build it; otherwise assume run-surface.sh / CI already produced dist/.
shopt -s nullglob
wasm_files=( "$REPO"/dist/*_bg.wasm )
shopt -u nullglob
if (( ${#wasm_files[@]} == 0 )); then
  if command -v trunk >/dev/null 2>&1; then
    echo "    no dist/*_bg.wasm — running 'trunk build --release'"
    ( cd "$REPO" && trunk build --release )
  else
    echo "FATAL: no dist/*_bg.wasm and 'trunk' not on PATH. Build the bundle first." >&2
    exit 1
  fi
fi

echo "==> [2/3] installing plist -> $PLIST_DST"
mkdir -p "$HOME/Library/LaunchAgents"
cp "$PLIST_SRC" "$PLIST_DST"
plutil -lint "$PLIST_DST"

if (( BOOTSTRAP == 0 )); then
  echo
  echo "==> [3/3] plist staged. Live bootstrap is OPT-IN — not touching launchd."
  echo "    Re-run with --bootstrap to start supervision, or run these yourself:"
  echo
  echo "        launchctl bootout   $DOMAIN/$LABEL 2>/dev/null || true"
  echo "        launchctl bootstrap $DOMAIN \"$PLIST_DST\""
  echo "        launchctl enable    $DOMAIN/$LABEL"
  echo "        launchctl kickstart -k $DOMAIN/$LABEL"
  echo
  echo "    Then check it's listening:  lsof -nP -iTCP:8790 -sTCP:LISTEN"
  echo "    Tail logs:                  tail -f /private/tmp/ocean-surface-proxy.log"
  exit 0
fi

echo "==> [3/3] (re)bootstrapping launchd job $LABEL in $DOMAIN"
# Tear down any previous instance so this is a clean (re)install.
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST_DST"
launchctl enable "$DOMAIN/$LABEL"
# Force an immediate (re)start so we don't wait for the next event.
launchctl kickstart -k "$DOMAIN/$LABEL"

echo
echo "==> done. status:"
launchctl print "$DOMAIN/$LABEL" 2>/dev/null | grep -E 'state|pid|program|path =' | sed 's/^/    /' || true
echo
echo "    Check it's listening:   lsof -nP -iTCP:8790 -sTCP:LISTEN"
echo "    Tail logs:              tail -f /private/tmp/ocean-surface-proxy.log"
