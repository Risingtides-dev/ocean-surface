#!/usr/bin/env bash
# Install and supervise the Ocean Surface proxy plus automatic main deployment.
#
# Idempotent. Safe to re-run after a pull/rebuild. What it does:
#   1. Builds the proxy and WASM release from MAIN, then atomically promotes it.
#   2. Installs the proxy and auto-deploy LaunchAgent plists.
#   3. By default PRINTS the bootstrap/kickstart commands for you to run.
#      Pass --bootstrap to actually touch the live launchd on this box.
#
# The live bootstrap is OPT-IN. Without --bootstrap this script builds/promotes
# the release and stages both plists, but does not touch launchd. It HARD-FAILS
# on a non-main checkout (override with --allow-non-main). Mirrors ocean-os's
# ops/install-ocean-daemon.sh (build-from-main guard + idempotency).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROXY_LABEL="dev.risingtides.ocean-surface-proxy"
AUTO_LABEL="dev.risingtides.ocean-surface-auto-deploy"
PROXY_PLIST_SRC="$REPO/deploy/$PROXY_LABEL.plist"
AUTO_PLIST_SRC="$REPO/deploy/$AUTO_LABEL.plist"
PROXY_PLIST_DST="$HOME/Library/LaunchAgents/$PROXY_LABEL.plist"
AUTO_PLIST_DST="$HOME/Library/LaunchAgents/$AUTO_LABEL.plist"
LAUNCHER_DIR="$HOME/.config/ocean-surface/bin"
DOMAIN="gui/$(id -u)"

# --bootstrap (off by default) opts in to touching the live launchd domain.
# --allow-non-main (off by default) is the deliberate escape hatch for the build-
# from-main guard; without it the script HARD-FAILS on a non-main checkout.
BOOTSTRAP=0
ALLOW_NON_MAIN=0
for arg in "$@"; do
  case "$arg" in
    --bootstrap) BOOTSTRAP=1 ;;
    --allow-non-main) ALLOW_NON_MAIN=1 ;;
    -h|--help)
      sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      echo "unknown arg: $arg (use --bootstrap, --allow-non-main, or --help)" >&2
      exit 2 ;;
  esac
done

export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin:$PATH"

# --- auth-env preflight (prod contract) ----------------------------------------
# Creds live in a 0600 env file sourced by deploy/ocean-surface-proxy.sh — never
# in the plist. Fail early with an actionable message so a fresh install cannot
# stage a supervised proxy that then refuses to boot (auth on, no creds) or,
# worse, gets an ad-hoc AUTH=off override. This check does not print secrets.
AUTH_ENV="$HOME/.config/ocean-surface/proxy-auth.env"
if [[ ! -f "$AUTH_ENV" ]]; then
  echo "ERROR: missing $AUTH_ENV" >&2
  echo "       Prod auth requires a 0600 env file that exports:" >&2
  echo "         OCEAN_SURFACE_AUTH=on" >&2
  echo "         OCEAN_SURFACE_USER=..." >&2
  echo "         OCEAN_SURFACE_PASS=..." >&2
  echo "         OCEAN_SURFACE_COOKIE_SECURE=on  # public HTTPS tunnel" >&2
  echo "       Create it before installing (chmod 600). Password material" >&2
  echo "       typically lives in ~/.config/ocean-surface/proxy-basic-auth.txt." >&2
  echo "       There are no built-in operator credentials." >&2
  exit 1
fi
# shellcheck disable=SC1090
# Source in a subshell so we can validate without leaking into this script's env
# permanently beyond the checks below.
auth_mode="$(set -a; . "$AUTH_ENV"; set +a; printf '%s' "${OCEAN_SURFACE_AUTH:-}")"
auth_user="$(set -a; . "$AUTH_ENV"; set +a; printf '%s' "${OCEAN_SURFACE_USER:-}")"
auth_pass="$(set -a; . "$AUTH_ENV"; set +a; printf '%s' "${OCEAN_SURFACE_PASS:-}")"
if [[ -z "$auth_mode" || -z "$auth_user" || -z "$auth_pass" ]]; then
  echo "ERROR: $AUTH_ENV must export OCEAN_SURFACE_AUTH, OCEAN_SURFACE_USER, and OCEAN_SURFACE_PASS." >&2
  echo "       Refusing to install an incomplete auth config (values not printed)." >&2
  exit 1
fi
if [[ "$auth_mode" != "on" && "$auth_mode" != "off" ]]; then
  echo "ERROR: OCEAN_SURFACE_AUTH in $AUTH_ENV must be 'on' or 'off' (got a non-empty other value)." >&2
  exit 1
fi
if [[ "$auth_mode" == "off" ]]; then
  echo "WARNING: OCEAN_SURFACE_AUTH=off in $AUTH_ENV — trusted-localhost escape hatch only." >&2
fi
# Never echo USER/PASS.

# --- build-from-main guard (operator rule) -------------------------------------
# The supervised service runs a PREBUILT binary; that binary must be built from
# main. HARD-FAIL on a non-main checkout BEFORE any build/install — a warn+sleep
# enforces nothing in unattended/operator runs. The only way past is the explicit
# --allow-non-main flag. "On main" = the branch is `main`, OR HEAD is detached
# exactly at origin/main (e.g. a CI checkout of the merge commit).
branch="$(git -C "$REPO" rev-parse --abbrev-ref HEAD 2>/dev/null || echo '?')"
on_main=0
if [[ "$branch" == "main" ]]; then
  on_main=1
else
  head_sha="$(git -C "$REPO" rev-parse HEAD 2>/dev/null || echo 'x')"
  origin_main_sha="$(git -C "$REPO" rev-parse origin/main 2>/dev/null || echo 'y')"
  [[ "$head_sha" == "$origin_main_sha" ]] && on_main=1
fi
if (( on_main == 0 )); then
  if (( ALLOW_NON_MAIN == 1 )); then
    echo "WARNING: repo at $REPO is on '$branch', not main — proceeding due to --allow-non-main." >&2
  else
    echo "ERROR: repo at $REPO is on branch '$branch', not 'main'." >&2
    echo "       Operator rule: build/deploy the proxy from MAIN only." >&2
    echo "       Refusing to build/install from a feature branch. Either:" >&2
    echo "         git checkout main && git pull        # then re-run, or" >&2
    echo "         re-run with --allow-non-main          # deliberate override." >&2
    exit 1
  fi
fi

deploy_sha="$(git -C "$REPO" rev-parse HEAD)"
echo "==> [1/3] building proxy + wasm bundle (release) from '$branch' at $deploy_sha"
( cd "$REPO" && cargo build -p ocean-surface-proxy --release )
BIN="$REPO/target/release/ocean-surface-proxy"
if [[ ! -x "$BIN" ]]; then
  echo "FATAL: build did not produce an executable at $BIN" >&2
  exit 1
fi
if ! command -v trunk >/dev/null 2>&1; then
  echo "FATAL: 'trunk' is required to build the release bundle." >&2
  exit 1
fi
( cd "$REPO" && env -u NO_COLOR trunk build --release )
OCEAN_SURFACE_NO_RESTART=1 \
  "$REPO/deploy/ocean-surface-auto-deploy.sh" --promote "$REPO/dist" "$deploy_sha"

echo "==> [2/3] installing stable launchers and launch agents"
mkdir -p "$LAUNCHER_DIR" "$HOME/Library/LaunchAgents"
cp "$REPO/deploy/ocean-surface-proxy.sh" "$LAUNCHER_DIR/ocean-surface-proxy.sh"
cp "$REPO/deploy/ocean-surface-auto-deploy.sh" "$LAUNCHER_DIR/ocean-surface-auto-deploy.sh"
chmod 755 "$LAUNCHER_DIR/ocean-surface-proxy.sh" "$LAUNCHER_DIR/ocean-surface-auto-deploy.sh"
cp "$PROXY_PLIST_SRC" "$PROXY_PLIST_DST"
cp "$AUTO_PLIST_SRC" "$AUTO_PLIST_DST"
plutil -lint "$PROXY_PLIST_DST"
plutil -lint "$AUTO_PLIST_DST"

if (( BOOTSTRAP == 0 )); then
  echo
  echo "==> [3/3] plists staged. Live bootstrap is OPT-IN — not touching launchd."
  echo "    Re-run with --bootstrap to start the proxy and automatic main deployment."
  echo
  echo "        launchctl bootout   $DOMAIN/$PROXY_LABEL 2>/dev/null || true"
  echo "        launchctl bootout   $DOMAIN/$AUTO_LABEL 2>/dev/null || true"
  echo "        launchctl bootstrap $DOMAIN \"$PROXY_PLIST_DST\""
  echo "        launchctl bootstrap $DOMAIN \"$AUTO_PLIST_DST\""
  echo "        launchctl enable    $DOMAIN/$PROXY_LABEL"
  echo "        launchctl enable    $DOMAIN/$AUTO_LABEL"
  echo "        launchctl kickstart -k $DOMAIN/$PROXY_LABEL"
  echo
  echo "    Then check it's listening:  lsof -nP -iTCP:8790 -sTCP:LISTEN"
  echo "    Tail proxy logs:             tail -f /private/tmp/ocean-surface-proxy.log"
  echo "    Tail deploy logs:            tail -f /private/tmp/ocean-surface-auto-deploy.log"
  exit 0
fi

bootstrap_job() {
  local plist="$1"
  local attempt
  for attempt in 1 2 3 4 5; do
    if launchctl bootstrap "$DOMAIN" "$plist"; then
      return 0
    fi
    (( attempt == 5 )) && return 1
    sleep 1
  done
}

echo "==> [3/3] (re)bootstrapping launchd jobs in $DOMAIN"
launchctl bootout "$DOMAIN/$PROXY_LABEL" 2>/dev/null || true
launchctl bootout "$DOMAIN/$AUTO_LABEL" 2>/dev/null || true
bootstrap_job "$PROXY_PLIST_DST"
bootstrap_job "$AUTO_PLIST_DST"
launchctl enable "$DOMAIN/$PROXY_LABEL"
launchctl enable "$DOMAIN/$AUTO_LABEL"
launchctl kickstart -k "$DOMAIN/$PROXY_LABEL"

echo
echo "==> done. status:"
launchctl print "$DOMAIN/$PROXY_LABEL" 2>/dev/null | grep -E 'state|pid|program|path =' | sed 's/^/    proxy: /' || true
launchctl print "$DOMAIN/$AUTO_LABEL" 2>/dev/null | grep -E 'state|pid|program|path =' | sed 's/^/    deploy: /' || true
echo
echo "    Check it's listening:   lsof -nP -iTCP:8790 -sTCP:LISTEN"
echo "    Tail proxy logs:        tail -f /private/tmp/ocean-surface-proxy.log"
echo "    Tail deploy logs:       tail -f /private/tmp/ocean-surface-auto-deploy.log"
