# Tauri Rooms acceptance harness

Repository-owned, test-only acceptance for the compact native Rooms workspace
in the macOS Tauri shell (`crates/ocean-tauri`). Tracks issue #110 /
OCEAN-390.

Stage 0 proves: a real WKWebView in the Tauri shell, isolated daemon routing,
the `<=650px` compact Rooms layout by computed CSS, the visible room-list
toggle opening the real drawer, exactly the seeded room rendering, and native
Tab/Enter/Escape ordering (first Escape closes the drawer and restores toggle
focus; second Escape dismisses Rooms). Later stages — real thread open/reply
with reload persistence, room-scoped SSE fanout across isolated clients,
roster/presence, and the imported daemon-owned agent picker — are gated
behind a reviewed native Stage 0 run and are not implemented yet.

## Pieces

| Path | Role |
|---|---|
| `scripts/test-tauri-rooms.sh` | the one repository-owned command (live Stage 0 runner and `--validate`) |
| `test/tauri/rooms_stage0.py` | stdlib-only W3C client driving the embedded WebDriver server; no npm/WDIO dependency graph |
| `crates/ocean-tauri/src/bin/rooms_acceptance.rs` | dedicated acceptance binary (`required-features = ["rooms-acceptance"]`) |
| `crates/ocean-tauri/tauri.rooms-acceptance.conf.json` | alternate Tauri context: identifier `dev.ocean.surface.rooms-acceptance`, 640px offscreen-visible window |
| `crates/ocean-tauri/capabilities/rooms-acceptance.json` | empty-permission capability for the acceptance window |
| `crates/ocean-tauri/acceptance-dist/index.html` | tracked placeholder frontendDist; the runner builds into a private crate copy instead |

## Isolation model

- The embedded WebDriver server (`tauri-plugin-wdio-webdriver = "=1.3.0"`)
  is an optional dependency wired only into the `rooms-acceptance` feature,
  and `src/lib.rs` fails any release build that enables the feature with a
  `compile_error!`. Ordinary debug/release binaries carry neither the
  dependency nor the endpoint; production `run()` and the default capability
  are untouched.
- `OCEAN_DAEMON_URL` is compiled into the WASM bundle, so the runner starts
  and health-checks an isolated daemon first (explicit reviewed binary,
  neutral temporary cwd, `OCEAN_UNSUPERVISED=1`, random loopback port,
  temporary config/DB, one seeded room via the public API), then performs a
  fresh offline+locked Trunk build with exactly that URL and asserts the
  built bundle contains it.
- Keyboard events are real macOS key codes sent through System Events at the
  asserted-frontmost acceptance PID — never plugin action synthesis — because
  a hidden WKWebView cannot prove native focus delivery. The acceptance
  window is visible but offscreen (`x: -10000`).
- Builds run against a private Cargo home materialized from only
  metadata-approved crates.io cache ids, in private target directories, from
  a `git archive HEAD` staging of the web source — never the operator's
  checkout state, ambient PATH, or Cargo config.
- Cleanup tracks every owned PID and port, deletes the W3C session,
  terminates owned processes, verifies the endpoints are gone, then removes
  temporary data while preserving the original exit status. It never touches
  the operator's installed Ocean app, Chrome, or Safari, and never uses a
  broad `pkill`.

## Local command (live Stage 0 — macOS only)

The live run steals focus and must not run in an operator's active login.
Run it in CI or a dedicated macOS login where System Events Accessibility is
already granted to the runner, with an explicitly reviewed daemon binary:

```sh
OCEAN_STAGE0_ALLOW_FOCUS_STEAL=CI_DEDICATED_LOGIN \
OCEAN_TEST_DAEMON_BIN=/absolute/path/to/ocean-daemon \
./scripts/test-tauri-rooms.sh
```

Prerequisites: `python3`, `jq`, `curl`, `trunk`, `cargo`, `git`, `tar`,
`/usr/bin/osascript`, and a populated public crates.io registry cache (both
builds run offline and locked).

## Static validation (any OS)

```sh
./scripts/test-tauri-rooms.sh --validate
```

Static/config checks only — never starts a daemon or a GUI. CI runs this on
every PR (`tauri-acceptance` job) together with resolve-only dependency-graph
gates proving the default `ocean-tauri` build excludes the WebDriver plugin
and that the `rooms-acceptance` feature pins exactly `v1.3.0`.

## Exact pass/fail output

- Success prints exactly one stdout line and exits `0`:
  - `PASS: Tauri Rooms Stage0 acceptance` (live run)
  - `PASS: Tauri Rooms static validation` (`--validate`)
- Failure prints diagnostics to stderr (typically `acceptance error:
  <reason>`), then `FAIL: <same label>` on stdout, and exits non-zero,
  preserving the first failing status.
- On any failure — including cleanup trouble after an otherwise-green run —
  the runner prints `ARTIFACTS: <dir>` to stderr: a fresh mode-0700
  directory containing at most `daemon.log`, `app.log`, `w3c.log`, and
  `summary.txt` (`stage0_exit=` / `cleanup_failed=` / `final_status=`
  lines). Logs are copied with a symlink-refusing helper; the temporary
  workspace itself is always removed.
