#!/usr/bin/env bash
# Stop + unsupervise the Ocean Surface proxy and automatic deploy watcher.
# Leaves the repo, built artifacts, and immutable deployed releases untouched.
set -euo pipefail

PROXY_LABEL="dev.risingtides.ocean-surface-proxy"
AUTO_LABEL="dev.risingtides.ocean-surface-auto-deploy"
DOMAIN="gui/$(id -u)"

for label in "$AUTO_LABEL" "$PROXY_LABEL"; do
  plist="$HOME/Library/LaunchAgents/$label.plist"
  echo "==> booting out $DOMAIN/$label"
  launchctl bootout "$DOMAIN/$label" 2>/dev/null || echo "    (job was not loaded)"
  echo "==> removing $plist"
  rm -f "$plist"
done

echo "==> done. The proxy and deploy watcher are no longer supervised."
echo "    Verify it stopped:  lsof -nP -iTCP:8790 -sTCP:LISTEN   (should print nothing)"
