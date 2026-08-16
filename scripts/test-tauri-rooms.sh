#!/usr/bin/env bash
# Isolated macOS Stage0 acceptance runner for the compact native Rooms workspace.
# --validate performs static/config checks only and never starts a daemon or GUI.
set -Eeuo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
TAURI_DIR="$ROOT/crates/ocean-tauri"
TEST_DIR="$ROOT/test/tauri"
MODE=${1:-stage0}
RESULT_LABEL="Tauri Rooms Stage0 acceptance"
TMP_ROOT=
DAEMON_PID=
DAEMON_PORT=
WEBDRIVER_PORT=
W3C_PID=
APP_PID=
ACCEPTANCE_BIN=
DIAGNOSTICS_DIR=
FINAL_STATUS=1

fail() { echo "acceptance error: $*" >&2; return 1; }

port_is_closed() {
  ! /usr/bin/nc -z 127.0.0.1 "$1" >/dev/null 2>&1
}

wait_port_closed() {
  local port=$1 _
  for _ in {1..50}; do
    port_is_closed "$port" && return 0
    sleep 0.1
  done
  return 1
}

copy_diagnostic_nofollow() {
  python3 -I - "$1" "$2" "$3" <<'PY_DIAGNOSTIC_COPY'
import os
import stat
import sys

source_dir, destination_dir, name = sys.argv[1:]
if not name or name in (".", "..") or "/" in name:
    raise ValueError("diagnostic name must be one basename")
flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
source_directory_fd = os.open(source_dir, flags)
destination_directory_fd = os.open(destination_dir, flags)
source_fd = destination_fd = None
try:
    source_fd = os.open(
        name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=source_directory_fd
    )
    if not stat.S_ISREG(os.fstat(source_fd).st_mode):
        raise ValueError("diagnostic source is not a regular file")
    destination_fd = os.open(
        name,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
        0o600,
        dir_fd=destination_directory_fd,
    )
    while True:
        chunk = os.read(source_fd, 1024 * 1024)
        if not chunk:
            break
        view = memoryview(chunk)
        while view:
            written = os.write(destination_fd, view)
            view = view[written:]
finally:
    if destination_fd is not None:
        os.close(destination_fd)
    if source_fd is not None:
        os.close(source_fd)
    os.close(destination_directory_fd)
    os.close(source_directory_fd)
PY_DIAGNOSTIC_COPY
}


cleanup() {
  local incoming=$1 cleanup_failed=0
  trap - EXIT INT TERM

  if [[ -n "$W3C_PID" ]] && kill -0 "$W3C_PID" 2>/dev/null; then
    kill "$W3C_PID" 2>/dev/null || cleanup_failed=1
    for _ in {1..50}; do
      kill -0 "$W3C_PID" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$W3C_PID" 2>/dev/null; then
      kill -KILL "$W3C_PID" 2>/dev/null || cleanup_failed=1
    fi
  fi
  [[ -z "$W3C_PID" ]] || wait "$W3C_PID" 2>/dev/null || true
  if [[ -n "$W3C_PID" ]] && kill -0 "$W3C_PID" 2>/dev/null; then
    echo "acceptance error: tracked W3C client survived teardown" >&2
    cleanup_failed=1
  fi

  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || cleanup_failed=1
    for _ in {1..50}; do
      kill -0 "$APP_PID" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$APP_PID" 2>/dev/null; then
      kill -KILL "$APP_PID" 2>/dev/null || cleanup_failed=1
    fi
    wait "$APP_PID" 2>/dev/null || true
  fi
  if [[ -n "$APP_PID" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    echo "acceptance error: tracked acceptance app survived teardown" >&2
    cleanup_failed=1
  fi

  if [[ -n "$DAEMON_PID" ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
    kill "$DAEMON_PID" 2>/dev/null || cleanup_failed=1
    for _ in {1..50}; do
      kill -0 "$DAEMON_PID" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$DAEMON_PID" 2>/dev/null; then
      kill -KILL "$DAEMON_PID" 2>/dev/null || cleanup_failed=1
    fi
    wait "$DAEMON_PID" 2>/dev/null || true
  fi

  [[ -z "$DAEMON_PORT" ]] || wait_port_closed "$DAEMON_PORT" || cleanup_failed=1
  [[ -z "$WEBDRIVER_PORT" ]] || wait_port_closed "$WEBDRIVER_PORT" || cleanup_failed=1
  if [[ $incoming -ne 0 || $cleanup_failed -ne 0 || $FINAL_STATUS -ne 0 ]] && [[ -n "$TMP_ROOT" ]]; then
    DIAGNOSTICS_DIR=$(umask 077; mktemp -d "${TMPDIR:-/tmp}/ocean-rooms-stage0-diagnostics.XXXXXX") \
      || cleanup_failed=1
    if [[ -n "$DIAGNOSTICS_DIR" ]]; then
      chmod 700 "$DIAGNOSTICS_DIR" || cleanup_failed=1
      for log_name in daemon.log app.log w3c.log; do
        if [[ -e "$TMP_ROOT/$log_name" ]]; then
          copy_diagnostic_nofollow "$TMP_ROOT" "$DIAGNOSTICS_DIR" "$log_name" \
            || cleanup_failed=1
        fi
      done
      printf 'stage0_exit=%s\ncleanup_failed=%s\nfinal_status=%s\n' \
        "$incoming" "$cleanup_failed" "$FINAL_STATUS" >"$DIAGNOSTICS_DIR/summary.txt" \
        || cleanup_failed=1
    fi
  fi
  if [[ -n "$TMP_ROOT" ]]; then
    rm -rf "$TMP_ROOT" || cleanup_failed=1
  fi
  if [[ $incoming -ne 0 || $cleanup_failed -ne 0 || $FINAL_STATUS -ne 0 ]] && [[ -z "$DIAGNOSTICS_DIR" ]]; then
    DIAGNOSTICS_DIR=$(umask 077; mktemp -d "${TMPDIR:-/tmp}/ocean-rooms-stage0-diagnostics.XXXXXX") || true
    if [[ -n "$DIAGNOSTICS_DIR" ]]; then
      chmod 700 "$DIAGNOSTICS_DIR" || true
      printf 'stage0_exit=%s\ncleanup_failed=%s\nfinal_status=%s\n' \
        "$incoming" "$cleanup_failed" "$FINAL_STATUS" >"$DIAGNOSTICS_DIR/summary.txt" || true
    fi
  fi
  [[ -z "$DIAGNOSTICS_DIR" ]] || echo "ARTIFACTS: $DIAGNOSTICS_DIR" >&2

  if [[ $incoming -eq 0 && $cleanup_failed -eq 0 && $FINAL_STATUS -eq 0 ]]; then
    echo "PASS: $RESULT_LABEL"
    exit 0
  fi
  echo "FAIL: $RESULT_LABEL"
  [[ $incoming -eq 0 ]] && exit 1
  exit "$incoming"
}
trap 'cleanup $?' EXIT
trap 'exit 130' INT TERM

validate_static() {
  command -v python3 >/dev/null || fail "python3 is required"
  command -v jq >/dev/null || fail "jq is required"
  command -v curl >/dev/null || fail "curl is required"
  command -v trunk >/dev/null || fail "trunk is required for Stage0"
  command -v cargo >/dev/null || fail "cargo is required for Stage0"
  command -v git >/dev/null || fail "git is required for source staging"
  command -v tar >/dev/null || fail "tar is required for source staging"
  jq -e '.app.windows[0].visible == true and .app.windows[0].width <= 650 and .app.windows[0].x < 0' \
    "$TAURI_DIR/tauri.rooms-acceptance.conf.json" >/dev/null
  ! grep -q 'source = "git+' "$ROOT/Cargo.lock" "$TAURI_DIR/Cargo.lock" \
    || fail "offline Stage0 does not permit git-sourced Cargo dependencies"
  [[ ! -d "$ROOT/.cargo" && ! -d "$TAURI_DIR/.cargo" ]] \
    || fail "Stage0 must explicitly review Cargo config before copying the crate"
  python3 -I -c 'import ast,pathlib,sys; ast.parse(pathlib.Path(sys.argv[1]).read_text())' \
    "$TEST_DIR/rooms_stage0.py"
  python3 -I -c 'import ast,pathlib,sys; text=pathlib.Path(sys.argv[1]).read_text(); helper=text.split("<<'"'"'PY_DIAGNOSTIC_COPY'"'"'\n",1)[1].split("\nPY_DIAGNOSTIC_COPY",1)[0]; ast.parse(helper)' \
    "$0"
}

if [[ "$MODE" == "--validate" ]]; then
  RESULT_LABEL="Tauri Rooms static validation"
  validate_static
  : # No npm/Node dependency graph: Python stdlib drives W3C directly.
  FINAL_STATUS=0
  exit 0
fi
[[ "$MODE" == "stage0" ]] || fail "usage: $0 [--validate]"
[[ $(uname -s) == Darwin ]] || fail "Stage0 is macOS-only (use --validate elsewhere)"
[[ ${OCEAN_STAGE0_ALLOW_FOCUS_STEAL:-} == CI_DEDICATED_LOGIN ]] \
  || fail "live Stage0 steals focus; run only in CI/a dedicated login with OCEAN_STAGE0_ALLOW_FOCUS_STEAL=CI_DEDICATED_LOGIN"
validate_static
[[ -x /usr/bin/osascript ]] || fail "/usr/bin/osascript is required for native keyboard acceptance"

DAEMON_BIN=${OCEAN_TEST_DAEMON_BIN:-}
[[ -n "$DAEMON_BIN" && "$DAEMON_BIN" == /* ]] || fail "OCEAN_TEST_DAEMON_BIN must be explicit and absolute"
[[ -f "$DAEMON_BIN" && -x "$DAEMON_BIN" ]] || fail "OCEAN_TEST_DAEMON_BIN is not an executable file"
SOURCE_CARGO_HOME=${CARGO_HOME:-$HOME/.cargo}
SOURCE_REGISTRY="$SOURCE_CARGO_HOME/registry"
[[ -d "$SOURCE_REGISTRY" && ! -L "$SOURCE_REGISTRY" ]] \
  || fail "public crates.io registry cache is absent or symlinked"

TMP_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/ocean-rooms-stage0.XXXXXX")
mkdir -p "$TMP_ROOT/home" "$TMP_ROOT/config" "$TMP_ROOT/cwd" "$TMP_ROOT/build-dist" \
  "$TMP_ROOT/web-target" "$TMP_ROOT/tauri-target" "$TMP_ROOT/tauri-crate" \
  "$TMP_ROOT/cargo-home/registry" "$TMP_ROOT/xdg-cache" "$TMP_ROOT/web-src"

# Trust an exact cache id only when its own regular config.json identifies the
# canonical crates.io download and API origins. No basename prefix is trusted.
CRATES_IO_CACHE_IDS=()
[[ -d "$SOURCE_REGISTRY/index" && ! -L "$SOURCE_REGISTRY/index" ]] \
  || fail "registry index root is absent or symlinked"
while IFS= read -r -d '' registry_config; do
  registry_index_dir=$(dirname "$registry_config")
  [[ ! -L "$registry_config" && -d "$registry_index_dir" && ! -L "$registry_index_dir" ]] \
    || fail "crates.io registry metadata must be regular and non-symlinked"
  jq -e '.dl == "https://static.crates.io/crates" and .api == "https://crates.io"' \
    "$registry_config" >/dev/null || continue
  CRATES_IO_CACHE_IDS+=("$(basename "$registry_index_dir")")
done < <(find "$SOURCE_REGISTRY/index" -mindepth 2 -maxdepth 2 -type f -name config.json -print0)
[[ ${#CRATES_IO_CACHE_IDS[@]} -gt 0 ]] \
  || fail "no registry cache id has canonical crates.io metadata"

# Materialize only the exact metadata-approved ids from index/cache/src.
for registry_id in "${CRATES_IO_CACHE_IDS[@]}"; do
  for registry_part in index cache src; do
    source_part="$SOURCE_REGISTRY/$registry_part/$registry_id"
    destination_part="$TMP_ROOT/cargo-home/registry/$registry_part"
    [[ -d "$source_part" && ! -L "$source_part" ]] \
      || fail "approved crates.io cache id is incomplete for $registry_part"
    ! find "$source_part" -type l -print -quit | grep -q . \
      || fail "approved crates.io cache contains a symlink"
    mkdir -p "$destination_part"
    cp -R "$source_part" "$destination_part/$registry_id" \
      || fail "could not materialize approved crates.io $registry_part cache"
  done
done
! find "$TMP_ROOT/cargo-home" -type l -print -quit | grep -q . \
  || fail "isolated Cargo home unexpectedly contains a symlink"

if ! git -C "$ROOT" archive HEAD | tar -x -C "$TMP_ROOT/web-src"; then
  fail "could not stage committed web source under the isolated temporary root"
fi
[[ -f "$TMP_ROOT/web-src/Trunk.toml" && -f "$TMP_ROOT/web-src/Cargo.toml" \
   && -f "$TMP_ROOT/web-src/Cargo.lock" \
   && -f "$TMP_ROOT/web-src/crates/ocean-surface-ui/Cargo.toml" ]] \
  || fail "staged HEAD is missing required Trunk/Cargo web source files"
config_ancestor="$TMP_ROOT/web-src"
while :; do
  [[ ! -e "$config_ancestor/.cargo/config" && ! -L "$config_ancestor/.cargo/config" \
     && ! -e "$config_ancestor/.cargo/config.toml" && ! -L "$config_ancestor/.cargo/config.toml" ]] \
    || fail "staged web source has a Cargo config in its ancestor chain"
  [[ "$config_ancestor" == / ]] && break
  config_ancestor=$(dirname "$config_ancestor")
done

read -r DAEMON_PORT WEBDRIVER_PORT < <(python3 - <<'PY'
import socket
ports = []
while len(ports) < 2:
    sock = socket.socket()
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
    if port != 4780 and port not in ports:
        ports.append(port)
    sock.close()
print(*ports)
PY
)
[[ "$DAEMON_PORT" != 4780 && "$WEBDRIVER_PORT" != 4780 && "$DAEMON_PORT" != "$WEBDRIVER_PORT" ]] \
  || fail "unsafe allocated ports"
port_is_closed "$DAEMON_PORT" && port_is_closed "$WEBDRIVER_PORT" || fail "allocated port was claimed before launch"
DAEMON_URL="http://127.0.0.1:$DAEMON_PORT"

(
  cd "$TMP_ROOT/cwd"
  exec env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$TMP_ROOT/home" TMPDIR="$TMP_ROOT" \
    OCEAN_UNSUPERVISED=1 OCEAN_BIND="127.0.0.1:$DAEMON_PORT" \
    OCEAN_CONFIG_DIR="$TMP_ROOT/config" OCEAN_DB_PATH="$TMP_ROOT/ocean.sqlite3" "$DAEMON_BIN"
) >"$TMP_ROOT/daemon.log" 2>&1 &
DAEMON_PID=$!

HEALTH_BODY="$TMP_ROOT/health.json"
for _ in {1..100}; do
  kill -0 "$DAEMON_PID" 2>/dev/null || fail "isolated daemon exited; failure logs will be copied to diagnostics"
  curl --fail --silent --max-time 1 "$DAEMON_URL/health" >"$HEALTH_BODY" 2>/dev/null && break
  sleep 0.1
done
[[ -s "$HEALTH_BODY" ]] && jq -e 'type == "object"' "$HEALTH_BODY" >/dev/null \
  || fail "isolated daemon health did not return a JSON object"
kill -0 "$DAEMON_PID" 2>/dev/null || fail "daemon PID died after health response"
/usr/sbin/lsof -nP -a -p "$DAEMON_PID" -iTCP:"$DAEMON_PORT" -sTCP:LISTEN >/dev/null \
  || fail "health response was not served by the tracked daemon PID"

ROOM_REQUEST_NAME="Stage0 Room $(date +%s)-$RANDOM"
ROOM_REQUEST_KEY=$(printf '%s' "$ROOM_REQUEST_NAME" | tr '[:upper:]' '[:lower:]' | tr -cs 'a-z0-9' '-' | sed 's/^-//;s/-$//')
ROOM_RESPONSE="$TMP_ROOT/room.json"
jq -n --arg key "$ROOM_REQUEST_KEY" --arg name "$ROOM_REQUEST_NAME" '{key:$key,name:$name}' |
  curl --fail --silent --show-error -H 'content-type: application/json' \
    --data-binary @- "$DAEMON_URL/v1/rooms/persistent" >"$ROOM_RESPONSE"
jq -e '.ok == true and (.room | type == "object")' "$ROOM_RESPONSE" >/dev/null \
  || fail "room seed API did not return a successful canonical room"
ROOM_KEY=$(jq -er '.room.id // .room.key' "$ROOM_RESPONSE")
ROOM_NAME=$(jq -er '.room.name' "$ROOM_RESPONSE")
ROOM_COUNT=$(curl --fail --silent "$DAEMON_URL/v1/rooms/persistent" | jq '[.rooms[]] | length')
[[ -n "$ROOM_KEY" && -n "$ROOM_NAME" && "$ROOM_COUNT" == 1 ]] || fail "fresh fixture must contain one canonical room"

if ! (
  cd "$TMP_ROOT/web-src"
  env -u RUSTC_WRAPPER -u RUSTC_WORKSPACE_WRAPPER -u SCCACHE_DIR \
    HOME="$TMP_ROOT/home" CARGO_HOME="$TMP_ROOT/cargo-home" \
    CARGO_TARGET_DIR="$TMP_ROOT/web-target" \
    CARGO_NET_OFFLINE=true CARGO_BUILD_LOCKED=true XDG_CACHE_HOME="$TMP_ROOT/xdg-cache" \
    OCEAN_DAEMON_URL="$DAEMON_URL" trunk build --release --offline --locked \
    --dist "$TMP_ROOT/build-dist"
); then
  fail "offline locked Trunk build failed; public crates.io registry cache may be incomplete"
fi
WASM_FILE=$(find "$TMP_ROOT/build-dist" -type f -name '*.wasm' -print -quit)
[[ -n "$WASM_FILE" ]] || fail "acceptance Trunk build produced no WASM"
[[ $(od -An -tx1 -N4 "$WASM_FILE" | tr -d ' \n') == 0061736d ]] || fail "invalid WASM magic: $WASM_FILE"
# Build-time presence of the random URL plus the later native app assertion
# that exactly this unique seeded room rendered is the authoritative proof that
# the acceptance webview routes only to the isolated daemon fixture.
grep -aR -F "$DAEMON_URL" "$TMP_ROOT/build-dist" >/dev/null \
  || fail "acceptance bundle does not contain its isolated random daemon endpoint"

# Compile in a private crate copy so generate_context! reads a private config
# and private frontendDist; the tracked acceptance-dist is never renamed/written.
cp "$TAURI_DIR"/{Cargo.toml,Cargo.lock,build.rs,Info.plist,tauri.rooms-acceptance.conf.json} "$TMP_ROOT/tauri-crate/"
cp -R "$TAURI_DIR"/{src,capabilities,gen,icons} "$TMP_ROOT/tauri-crate/"
[[ ! -e "$TMP_ROOT/tauri-crate/.cargo/config" \
   && ! -L "$TMP_ROOT/tauri-crate/.cargo/config" \
   && ! -e "$TMP_ROOT/tauri-crate/.cargo/config.toml" \
   && ! -L "$TMP_ROOT/tauri-crate/.cargo/config.toml" ]] \
  || fail "copied Tauri crate unexpectedly contains Cargo configuration"
mv "$TMP_ROOT/build-dist" "$TMP_ROOT/tauri-crate/acceptance-dist"
if ! (
  cd "$TMP_ROOT/tauri-crate"
  env -u RUSTC_WRAPPER -u RUSTC_WORKSPACE_WRAPPER -u SCCACHE_DIR \
    HOME="$TMP_ROOT/home" CARGO_HOME="$TMP_ROOT/cargo-home" \
    CARGO_TARGET_DIR="$TMP_ROOT/tauri-target" \
    CARGO_NET_OFFLINE=true CARGO_BUILD_LOCKED=true XDG_CACHE_HOME="$TMP_ROOT/xdg-cache" \
    cargo build --locked --manifest-path Cargo.toml \
    --features rooms-acceptance --bin ocean-rooms-acceptance
); then
  fail "offline locked Tauri build failed; public crates.io registry cache may be incomplete"
fi
ACCEPTANCE_BIN="$TMP_ROOT/tauri-target/debug/ocean-rooms-acceptance"
[[ -x "$ACCEPTANCE_BIN" ]] || fail "dedicated acceptance binary was not built"

port_is_closed "$WEBDRIVER_PORT" || fail "WebDriver port was claimed before app launch"
(
  cd "$TMP_ROOT/cwd"
  exec env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$TMP_ROOT/home" TMPDIR="$TMP_ROOT" \
    OCEAN_DAEMON_URL="$DAEMON_URL" TAURI_WEBDRIVER_PORT="$WEBDRIVER_PORT" \
    "$ACCEPTANCE_BIN"
) >"$TMP_ROOT/app.log" 2>&1 &
APP_PID=$!

for _ in {1..100}; do
  kill -0 "$APP_PID" 2>/dev/null || fail "acceptance app exited; failure logs will be copied to diagnostics"
  if ! port_is_closed "$WEBDRIVER_PORT"; then
    /usr/sbin/lsof -nP -a -p "$APP_PID" -iTCP:"$WEBDRIVER_PORT" -sTCP:LISTEN >/dev/null \
      || fail "WebDriver port listener is not the tracked acceptance app PID"
    break
  fi
  sleep 0.1
done
port_is_closed "$WEBDRIVER_PORT" && fail "embedded WebDriver port never opened; app log will be copied to diagnostics"

python3 -I "$TEST_DIR/rooms_stage0.py" --port "$WEBDRIVER_PORT" \
  --room-key "$ROOM_KEY" --room-name "$ROOM_NAME" --app-pid "$APP_PID" \
  >"$TMP_ROOT/w3c.log" 2>&1 &
W3C_PID=$!
set +e
wait "$W3C_PID"
W3C_STATUS=$?
set -e
W3C_PID=
[[ $W3C_STATUS -eq 0 ]] || fail "stdlib W3C acceptance failed; client log will be copied to diagnostics"
FINAL_STATUS=0
