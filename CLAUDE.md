# Ocean Surface Agent Guide

Ocean Surface is the product-surface repo. The canonical UI is one Leptos/WASM
app (`crates/ocean-surface-ui`) built once via Trunk and shipped to two hosts:
the browser PWA (served by `crates/ocean-surface-proxy`) and the Tauri native
shell (`crates/ocean-tauri`, macOS first). A Chrome extension wraps the same
bundle. The legacy GPUI desktop app (`crates/ocean-gui`) is soft-deprecated —
source retained for mining into the Tauri Rust backend, not for new feature work.

The sibling repo `../ocean-os` owns runtime authority: daemon, agent loop,
tools, providers, permissions, projects, workspaces, sessions, and events.

Do not put provider calls, agent reasoning, session storage, permission policy,
or tool execution authority in this repo. Surface code should render state,
collect intent, attach to sessions, and call the daemon.

## Current Build Focus

```sh
# Web/PWA (canonical UI):
./run-surface.sh         # or: trunk serve --open
# Native desktop (Tauri, loads the same dist/ bundle):
./run-tauri.sh           # or: cd crates/ocean-tauri && cargo tauri dev
```

Native surface direction:

- Tauri 2.x shell (`crates/ocean-tauri`) loads `dist/` as `frontendDist` — the
  same Trunk-built Leptos WASM bundle the browser PWA ships.
- Rust commands in the Tauri backend replace the GPUI crate's `rfd` (folder
  dialogs) and `notify` (path watcher) native bits.
- Native LiveKit Rust client is a later phase behind a feature flag, not part
  of this batch.
- Ocean daemon stays the session/runtime authority.
- Canvas ledger/state injected into turns as surface context; the Leptos canvas
  already lives at `crates/ocean-surface-ui/src/canvas.rs`.
- `docs/OCEAN_GPUI_CANVAS_LIVEKIT_SPEC.md` is retained as a historical reference
  for the canvas+LiveKit design intent; the implementation surface moved to
  Leptos+Tauri.

## Session Contract

The ecosystem invariant is:

```text
Project -> Workspace -> Session -> Turns -> Events
Surface -> Session
```

First-party surfaces must create or choose a session before posting a turn:

```text
POST /v1/agent/sessions
GET  /v1/agent/events?session_id=<id>
POST /v1/agent/turns { session_id, prompt, cwd, project_id?, client_type }
```

Rules:

- Never open a product transcript on the global `/v1/agent/events` stream.
- Never adopt the active session from `SessionCreated` or `TurnStarted` on a
  global stream.
- Cross-surface sharing is explicit: two surfaces attach to the same
  `session_id`.
- Different sessions on different surfaces must not blend, switch each other,
  or race to become the active transcript.
- `client_type` only describes the surface medium (`surface-web`,
  `surface-extension`, `surface-tauri`; legacy: `surface-gpui`). It is not a
  session id or workspace id.

## Workspace Map

| Path | Role |
|---|---|
| `crates/ocean-surface-ui/` | Leptos WASM web/PWA/extension UI |
| `crates/ocean-tauri/` | Tauri 2.x native desktop shell; loads the same dist/ bundle as the browser PWA |
| `crates/ocean-surface-proxy/` | axum proxy for web bundle, STT/TTS (transitional), config, daemon reverse proxy |
| `extension/` | Chrome extension wrapper around the Leptos surface |
| `legacy-voice/` | reference voice code only; do not build new architecture here |
| `crates/ocean-gui/` | legacy GPUI native desktop app and tldraw canvas host (soft-deprecated, source retained for mining) |
| `crates/ocean-gui/canvas-web/` | web bundle loaded into the GPUI canvas webview (legacy) |

The proxy's xAI STT/TTS key handling is transitional — provider credentials
are moving to `ocean-os` (daemon-owned voice endpoints); the proxy keeps them
only until that migration lands. Do not extend it with new provider
credentials or provider calls.

## Build / Check

```sh
cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
cargo check -p ocean-surface-proxy
cd crates/ocean-tauri && cargo check   # native shell (standalone, not a workspace member)
```

The GPUI crate still builds but is not the active surface:
`cargo check -p ocean-gui`.

For local web/proxy work:

```sh
./run-surface.sh
trunk serve --open
```

Native desktop:

```sh
./run-tauri.sh                          # build dist/ + launch Tauri
```

The daemon must be running from `../ocean-os` for live agent behavior.
