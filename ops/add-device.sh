#!/usr/bin/env bash
# Add one device to a person's entry in the surface roster.
#
#   ops/add-device.sh <username> <device-name> <daemon-url> [options]
#
# A "device" is a machine whose Ocean daemon that login may attach to. After a
# device is added, signing in at the public surface offers it in the picker and
# selecting it lands the browser in THAT machine's sessions — no second login,
# and no URL typed in a browser. See ops/README.md for the full recipe.
#
# Options:
#   --observer-token <path>  mode-0600 observatory token minted by THAT daemon
#   --operator-key <path>    mode-0600 room-operator key minted by THAT daemon
#   --default                make this the device a fresh session lands on
#   --allow-public           accept a daemon_url that is not on the tailnet
#   --users-file <path>      roster to edit (default ~/.config/ocean-surface/users.json)
#
# The roster holds every teammate's password, so this script only ever writes it
# through a 0600 temp file in the same directory followed by a rename: a crash
# mid-write cannot leave a truncated or world-readable roster behind, and a
# reader never sees a half-written file.
#
# The daemon has NO authentication of its own — reachability is the whole
# boundary — so a non-tailnet daemon_url is refused unless you say --allow-public
# in as many words. Read the warning in ops/README.md before you do.
set -euo pipefail

USERS_FILE="${OCEAN_SURFACE_USERS_FILE:-$HOME/.config/ocean-surface/users.json}"
OBSERVER_TOKEN=""
OPERATOR_KEY=""
MAKE_DEFAULT=0
ALLOW_PUBLIC=0

die() {
  echo "add-device: $*" >&2
  exit 1
}

usage() {
  sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit "${1:-1}"
}

[ $# -ge 1 ] || usage 1
case "$1" in -h | --help) usage 0 ;; esac
[ $# -ge 3 ] || usage 1

USERNAME="$1"
DEVICE_NAME="$2"
DAEMON_URL="$3"
shift 3

while [ $# -gt 0 ]; do
  case "$1" in
    --observer-token)
      [ $# -ge 2 ] || die "--observer-token needs a path"
      OBSERVER_TOKEN="$2"
      shift 2
      ;;
    --operator-key)
      [ $# -ge 2 ] || die "--operator-key needs a path"
      OPERATOR_KEY="$2"
      shift 2
      ;;
    --users-file)
      [ $# -ge 2 ] || die "--users-file needs a path"
      USERS_FILE="$2"
      shift 2
      ;;
    --default) MAKE_DEFAULT=1; shift ;;
    --allow-public) ALLOW_PUBLIC=1; shift ;;
    *) die "unknown option '$1'" ;;
  esac
done

[ -n "$USERNAME" ] || die "username must not be empty"
[ -n "$DEVICE_NAME" ] || die "device name must not be empty"
[ -f "$USERS_FILE" ] || die "no roster at $USERS_FILE"

command -v python3 >/dev/null 2>&1 || die "python3 is required to edit JSON safely"

# The proxy refuses a group/world-readable roster at startup; refuse to write
# one here too rather than leaving the operator a surface that will not boot.
# python3 rather than stat(1), whose mode flag differs between BSD and GNU —
# and a stat that silently reports something else is worse than no check.
MODE="$(python3 -c 'import os,sys;print("%03o" % (os.stat(sys.argv[1]).st_mode & 0o777))' "$USERS_FILE")"
case "$MODE" in
  600 | 400) ;;
  *) die "$USERS_FILE is mode $MODE; it holds credentials and must be 0600" ;;
esac

case "$DAEMON_URL" in
  http://* | https://*) ;;
  *) die "daemon_url must be an absolute http:// or https:// URL, got '$DAEMON_URL'" ;;
esac

# Tailnet check. Tailscale hands out addresses from the 100.64/10 CGNAT range
# and names under *.ts.net; loopback is the daemon's own default. Anything else
# means the daemon is listening somewhere its lack of auth is not covered by a
# tailnet ACL, which is the one thing this recipe must not do quietly.
HOST="${DAEMON_URL#*://}"
HOST="${HOST%%/*}"
HOST="${HOST##*@}"
case "$HOST" in
  \[*\]*) HOST="${HOST%%]*}]" ;;
  *:*) HOST="${HOST%%:*}" ;;
esac
tailnet=0
case "$HOST" in
  localhost | 127.* | '[::1]') tailnet=1 ;;
  *.ts.net) tailnet=1 ;;
  100.*)
    second="${HOST#100.}"
    second="${second%%.*}"
    if [ "$second" -ge 64 ] 2>/dev/null && [ "$second" -le 127 ] 2>/dev/null; then
      tailnet=1
    fi
    ;;
esac
if [ "$tailnet" -ne 1 ] && [ "$ALLOW_PUBLIC" -ne 1 ]; then
  die "'$HOST' is not a tailnet address (100.64/10, *.ts.net, or loopback).
The Ocean daemon has no auth of its own — whoever can reach it can drive it.
Bind it to the machine's tailnet address, or pass --allow-public if you have
another boundary in front of it and mean this."
fi

for path in "$OBSERVER_TOKEN" "$OPERATOR_KEY"; do
  [ -z "$path" ] && continue
  case "$path" in
    /*) ;;
    *) die "credential paths must be absolute, got '$path'" ;;
  esac
done

TMP="$(mktemp "${USERS_FILE}.XXXXXX")"
chmod 600 "$TMP"
trap 'rm -f "$TMP"' EXIT

USERNAME="$USERNAME" DEVICE_NAME="$DEVICE_NAME" DAEMON_URL="$DAEMON_URL" \
OBSERVER_TOKEN="$OBSERVER_TOKEN" OPERATOR_KEY="$OPERATOR_KEY" \
MAKE_DEFAULT="$MAKE_DEFAULT" USERS_FILE="$USERS_FILE" OUT="$TMP" \
  python3 - <<'PY'
import json, os, sys, urllib.parse

users_file = os.environ["USERS_FILE"]
username = os.environ["USERNAME"]
name = os.environ["DEVICE_NAME"]
url = os.environ["DAEMON_URL"]
make_default = os.environ["MAKE_DEFAULT"] == "1"

with open(users_file) as handle:
    roster = json.load(handle)
if not isinstance(roster, list):
    sys.exit("add-device: %s must contain a JSON array of users" % users_file)

entry = next((u for u in roster if u.get("username") == username), None)
if entry is None:
    sys.exit("add-device: no user '%s' in %s" % (username, users_file))

devices = entry.get("devices")
if not devices:
    # Fold the legacy single daemon into an explicit device named after its
    # host, exactly as the proxy does on load, so the two shapes never coexist
    # in one entry (which the proxy refuses).
    legacy = entry.pop("daemon_url", None)
    devices = []
    if legacy is None:
        # This entry inherited OCEAN_DAEMON_URL, whose value lives in the
        # proxy's environment and not in this file — so it cannot be written
        # down here without guessing. Say so: after this edit the person has
        # exactly the device being added.
        sys.stderr.write(
            "add-device: warning: '%s' had no daemon_url and was inheriting the proxy's "
            "OCEAN_DAEMON_URL.\nadd-device: after this edit their only device is '%s'; "
            "add the old one explicitly if they still need it.\n" % (username, name)
        )
    if legacy:
        host = urllib.parse.urlsplit(legacy).hostname or "default"
        if ":" in host:
            host = "[%s]" % host
        legacy_device = {"name": host, "daemon_url": legacy, "default": True}
        for key in ("observer_token_path", "operator_key_path"):
            if entry.get(key):
                legacy_device[key] = entry.pop(key)
        devices.append(legacy_device)
    entry["devices"] = devices

if any(d.get("name") == name for d in devices):
    sys.exit("add-device: '%s' already has a device named '%s'" % (username, name))

device = {"name": name, "daemon_url": url}
if os.environ["OBSERVER_TOKEN"]:
    device["observer_token_path"] = os.environ["OBSERVER_TOKEN"]
if os.environ["OPERATOR_KEY"]:
    device["operator_key_path"] = os.environ["OPERATOR_KEY"]
if make_default or not devices:
    for existing in devices:
        existing.pop("default", None)
    device["default"] = True
devices.append(device)

with open(os.environ["OUT"], "w") as handle:
    json.dump(roster, handle, indent=2)
    handle.write("\n")
PY

mv -f "$TMP" "$USERS_FILE"
trap - EXIT
chmod 600 "$USERS_FILE"

echo "add-device: '$USERNAME' can now attach to '$DEVICE_NAME' ($DAEMON_URL)"
echo "add-device: restart the proxy to pick it up —"
echo "  launchctl kickstart -k gui/\$(id -u)/dev.risingtides.ocean-surface-proxy"
