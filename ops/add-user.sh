#!/usr/bin/env bash
# Add one person to the surface roster, bound to their own machine.
#
#   ops/add-user.sh <username> <device-name> <daemon-url> [options]
#
# The device is required: an entry with no devices falls back to the proxy's
# OCEAN_DAEMON_URL — the host's own daemon — which would hand a new login the
# host's sessions. The device goes through ops/add-device.sh (tailnet check
# included) against a staged copy, so the live roster only ever sees the
# finished entry. The password is generated and printed once — hand it over
# privately.
#
# Options:
#   --password-stdin         read the password from stdin instead of generating one
#   --allow-public           passed through to add-device.sh
#   --create                 create the roster if it does not exist yet
#   --users-file <path>      roster to edit (default ~/.config/ocean-surface/users.json)
#
# --create matters: with no roster the proxy runs single-operator off
# OCEAN_SURFACE_USER/PASS, and the moment a roster exists that pair stops
# working. Add yourself first when creating one.
set -euo pipefail

USERS_FILE="${OCEAN_SURFACE_USERS_FILE:-$HOME/.config/ocean-surface/users.json}"
PASSWORD_STDIN=0
CREATE=0
DEVICE_ARGS=()

die() {
  echo "add-user: $*" >&2
  exit 1
}

usage() {
  sed -n '2,21p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit "${1:-1}"
}

[ $# -ge 1 ] || usage 1
case "$1" in -h | --help) usage 0 ;; esac
[ $# -ge 3 ] || usage 1

USERNAME="$(printf '%s' "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//')"
DEVICE_NAME="$2"
DAEMON_URL="$3"
shift 3

while [ $# -gt 0 ]; do
  case "$1" in
    --password-stdin) PASSWORD_STDIN=1; shift ;;
    --allow-public) DEVICE_ARGS+=(--allow-public); shift ;;
    --create) CREATE=1; shift ;;
    --users-file)
      [ $# -ge 2 ] || die "--users-file needs a path"
      USERS_FILE="$2"
      shift 2
      ;;
    *) die "unknown option '$1'" ;;
  esac
done

[ -n "$USERNAME" ] || die "username must not be empty or only whitespace"
case "$USERNAME" in
  *[[:cntrl:]]*) die "username must not contain control characters" ;;
esac
command -v python3 >/dev/null 2>&1 || die "python3 is required to edit JSON safely"

if [ -f "$USERS_FILE" ]; then
  MODE="$(python3 -c 'import os,sys;print("%03o" % (os.stat(sys.argv[1]).st_mode & 0o777))' "$USERS_FILE")"
  case "$MODE" in
    600 | 400) ;;
    *) die "$USERS_FILE is mode $MODE; it holds credentials and must be 0600" ;;
  esac
elif [ "$CREATE" -ne 1 ]; then
  die "no roster at $USERS_FILE (pass --create to start one; read the note in --help first)"
fi

if [ "$PASSWORD_STDIN" -eq 1 ]; then
  IFS= read -r PASSWORD || true
  [ -n "$(printf '%s' "$PASSWORD" | tr -d '[:space:]')" ] || die "password from stdin is empty"
else
  PASSWORD="$(python3 -c 'import secrets;print(secrets.token_urlsafe(18))')"
fi

TMP="$(mktemp "${USERS_FILE}.XXXXXX")"
chmod 600 "$TMP"
trap 'rm -f "$TMP"' EXIT

USERNAME="$USERNAME" PASSWORD="$PASSWORD" USERS_FILE="$USERS_FILE" OUT="$TMP" \
  python3 - <<'PY'
import json, os, sys

users_file = os.environ["USERS_FILE"]
username = os.environ["USERNAME"]

roster = []
if os.path.exists(users_file):
    with open(users_file) as handle:
        roster = json.load(handle)
if not isinstance(roster, list):
    sys.exit("add-user: %s must contain a JSON array of users" % users_file)

# The proxy refuses duplicate usernames at startup; refuse them here instead.
if any((u.get("username") or "").strip() == username for u in roster):
    sys.exit("add-user: '%s' already exists in %s" % (username, users_file))

roster.append({"username": username, "password": os.environ["PASSWORD"]})

with open(os.environ["OUT"], "w") as handle:
    json.dump(roster, handle, indent=2)
    handle.write("\n")
PY

# Attach the device on the staged copy; any refusal leaves the live roster as it was.
# Its "inheriting OCEAN_DAEMON_URL" warning is about existing entries, so its
# output is shown only when it refuses.
if ! DEVICE_OUT="$("$(dirname "${BASH_SOURCE[0]}")/add-device.sh" "$USERNAME" "$DEVICE_NAME" \
  "$DAEMON_URL" --users-file "$TMP" --default ${DEVICE_ARGS[@]+"${DEVICE_ARGS[@]}"} 2>&1)"; then
  printf '%s\n' "$DEVICE_OUT" >&2
  exit 1
fi

mv -f "$TMP" "$USERS_FILE"
trap - EXIT
chmod 600 "$USERS_FILE"

echo "add-user: added '$USERNAME' on '$DEVICE_NAME' ($DAEMON_URL)"
[ "$PASSWORD_STDIN" -eq 1 ] || echo "add-user: password (shown once): $PASSWORD"
echo "add-user: restart the proxy to pick it up —"
echo "  launchctl kickstart -k gui/\$(id -u)/dev.risingtides.ocean-surface-proxy"
