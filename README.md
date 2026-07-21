# Ocean Surface

The client face of [Ocean OS](https://github.com/Risingtides-dev/ocean-os).
One Rust + Leptos app, built once via Trunk and shipped to two hosts: the
browser PWA and the Tauri native shell — both load the same `dist/` WASM bundle.
A Chrome extension wraps that same bundle. Native macOS is delivered by the
Tauri 2.x shell (`crates/ocean-tauri`), not a separate GPUI app; the GPUI
desktop app (`crates/ocean-gui`) is ABANDONED (2026-07-21) — dead source, do
not work on it.

Cross-repo routing and ownership map: [`docs/OCEAN_PROJECT_MAP.md`](docs/OCEAN_PROJECT_MAP.md).
Ocean Surface requires only the public `ocean-os` daemon. The private
`ocean-bedrock` service is optional for authorized team deployments; surfaces
must not depend on it or place Bedrock credentials in browser state.

Release asset sources, licenses, and exact hashes are documented in
[`docs/ASSET_PROVENANCE.md`](docs/ASSET_PROVENANCE.md) and enforced by
`node scripts/check-asset-provenance.mjs`.

| Target | How | Why |
|---|---|---|
| Native desktop (Tauri) | `./run-tauri.sh` | Tauri 2.x shell loading a freshly built `dist/` Leptos bundle as the browser |
| Browser PWA | `./run-surface.sh` | builds the shared bundle and release proxy, then serves desktop/mobile browser access over the daemon API |
| Chrome extension | `extension/` wrapper | browser-side panel with explicit `surface-extension` context |
| Web proxy | `cargo run -p ocean-surface-proxy` | serves web bundle, config, STT/TTS, and daemon reverse proxy |

All targets are thin clients over `ocean-daemon`. None hold agent logic or
session authority, and none hold provider credentials. Voice STT/TTS
credentials are owned by ocean-os (daemon endpoints `/v1/voice/stt` and
`/v1/voice/tts`); the proxy forwards to the daemon transparently. They speak
the daemon's product agent API:

```
POST /v1/agent/sessions
GET  /v1/agent/events?session_id=<id>
POST /v1/agent/turns   { prompt, cwd, session_id, project_id?, client_type, ... }
```

Surfaces create or choose a session before posting turns. They do not adopt a
session from global SSE. Cross-surface sharing is explicit: attach both
surfaces to the same `session_id`.

## Daemon API

All surfaces drive the daemon over the same HTTP+SSE product agent API. The
wire shapes the surfaces send live in
[`crates/ocean-surface-ui/src/daemon.rs`](crates/ocean-surface-ui/src/daemon.rs);
the daemon side is `ocean-os`. The daemon listens on `127.0.0.1:4780` by
default (`OCEAN_BIND` to override); surfaces resolve it via `OCEAN_DAEMON_URL`.

### `POST /v1/agent/sessions`

Create (or reuse) a session before the first turn. Body
(`AgentSessionCreateRequest`):

| Field            | Type             | Notes                                                                                  |
| ---------------- | ---------------- | -------------------------------------------------------------------------------------- |
| `workspace_root` | `string`         | **Required** workspace anchor. (No serde alias for `cwd` — sending `cwd` fails to deserialize.) |
| `project_id`     | `string?`        | Optional project binding.                                                              |
| `client_type`    | `string?`        | The originating surface (`surface-web`, `surface-extension`, …).                       |

Returns the `session_id`. Surfaces then subscribe with
`GET /v1/agent/events?session_id=<id>` and send that `session_id` on every
turn.

### `POST /v1/agent/turns`

Start a turn. The POST returns once the turn completes but carries only
metadata — reply text, tool calls, and ids arrive over the SSE stream. Body
(`AgentTurnRequest`):

| Field            | Type             | Notes                                                                                                          |
| ---------------- | ---------------- | ------------------------------------------------------------------------------------------------------------- |
| `prompt`         | `string`         | The user/turn prompt.                                                                                          |
| `cwd`            | `string`         | Working directory for the turn. The web client sends `"/"` and relies on `project_id` for the real workspace. |
| `session_id`     | `string?`        | The session this turn belongs to. Omitted only when the daemon should mint one.                               |
| `project_id`     | `string?`        | Selected project. When set, the daemon binds the turn to the project's `workspace_root`.                      |
| `client_type`    | `string?`        | The originating surface, so the agent can adapt per surface (`surface-web`, `surface-extension`, …).           |
| `guidance`       | `string[]?`      | Optional guidance hints passed to the agent (e.g. active-tab context, `"focus on tests"`). Added in OCEAN-61. |
| `room_id`        | `string?`        | Optional room identifier for Track-0 room-scoped turns. Added in OCEAN-61. Not yet exposed in the web UI.     |
| `thinking_level` | `string?`        | Per-turn reasoning-effort override, serialized as the daemon's lowercase `ThinkingLevel` string. `None` leaves the daemon's global default in force. Added in OCEAN-61. Not yet exposed in the web UI. |
| `model_id`       | `string?`        | Per-session / per-turn model override (OCEAN-36). Mirrors the daemon's `model_id: Option<String>`. `None` leaves the session/daemon default model in force. Added to the surface wire shape in OCEAN-61. Not yet exposed in the web UI. |

All `Option` fields are `skip_serializing_if = "Option::is_none"`, so they are
omitted from the JSON body when unset rather than sent as `null`.

### `GET /v1/agent/events?session_id=<id>`

Session-scoped SSE stream of `AgentTurnEvent`s (assistant text, tool calls,
permission requests, completion). Surfaces must subscribe scoped to their own
`session_id` and must not adopt active sessions from the global SSE stream.

## Workspace

| Path                            | Role                                                                 |
| ------------------------------- | -------------------------------------------------------------------- |
| `crates/ocean-surface-ui/`      | canonical Leptos UI (CSR/WASM) for web/PWA/extension/Tauri.          |
| `crates/ocean-tauri/`           | Tauri 2.x native desktop shell; loads the same `dist/` bundle as the browser PWA. |
| `crates/ocean-surface-proxy/`   | axum service: forwards voice STT/TTS to the daemon, serves the WASM bundle. |
| `extension/`                    | Chrome extension wrapper around the Leptos surface.                  |
| `legacy-voice/`                 | Reference: the JS voice client (PR #22). Deleted once ported.        |
| `crates/ocean-gui/`             | ABANDONED GPUI desktop app (2026-07-21) — dead, do NOT work on it.   |
| `crates/ocean-gui/canvas-web/`  | ABANDONED (part of the dead GPUI app) — do NOT work on it.           |

## Dev loop

### One command (recommended)

The daemon must be running (in `../ocean-os`: `cargo run -p ocean-daemon --release`). Then:

```sh
# Tailnet or trusted LAN: Basic auth is required because the default bind is 0.0.0.0:8790.
export OCEAN_SURFACE_USER='<user>'
export OCEAN_SURFACE_PASS='<strong password>'
./run-surface.sh
# → open http://<this-host>:8790

# Trusted localhost only: explicitly disable auth and restrict the bind.
OCEAN_SURFACE_BIND=127.0.0.1:18790 OCEAN_SURFACE_AUTH=off ./run-surface.sh

# Or build dist/ + launch the Tauri native desktop shell (loads the same bundle):
./run-tauri.sh
```

`run-surface.sh` builds both the Trunk release bundle and
`target/release/ocean-surface-proxy` before serving. It fails before building if
Basic auth is enabled without nonblank credentials, or if auth is disabled on a
non-loopback bind. Tailnet traffic is encrypted; direct LAN HTTP should be used
only on a trusted network because Basic auth does not encrypt transport.
Override the daemon or voice profile with `OCEAN_DAEMON_URL` and
`OCEAN_VOICE_PROFILE`. Maps are optional: set a referrer-restricted
`GOOGLE_MAPS_API_KEY` to enable them. No organization-owned Maps key is bundled;
without the variable, the map component renders its unavailable notice.

### Verify before you open the browser

```sh
./smoke.sh        # health, /api/config, chat round-trip, + live STT/TTS (daemon must be running)
```

5/5 green means every wired path works; then the browser check is just UI/mic confirmation.

### Native desktop (Tauri)

```sh
./run-tauri.sh                        # build dist/ + launch the Tauri shell
```

Direct `cd crates/ocean-tauri && cargo tauri dev` does not rebuild `dist/`
because the Tauri config intentionally has no `beforeDevCommand`; use it only
when a current root `dist/` already exists. `./run-tauri.sh` is the canonical
fresh-build path.

Tauri loads the same `dist/` Leptos bundle the browser ships; its Rust backend
adds native folder dialogs and path watcher (replacing the GPUI crate's `rfd`/
`notify`). The canvas+LiveKit design intent is documented in
[`docs/OCEAN_GPUI_CANVAS_LIVEKIT_SPEC.md`](docs/OCEAN_GPUI_CANVAS_LIVEKIT_SPEC.md)
(now a historical reference; the implementation surface is Leptos+Tauri).

### Live-reload web dev

```sh
trunk serve --open                                    # → http://localhost:8080
OCEAN_DAEMON_URL=http://mac-mini.tailnet:4780 trunk serve --open   # remote daemon
```

Note: `trunk serve` serves the UI but NOT the proxy, so voice (`/api/stt`,
`/api/tts`) and `/api/config` need `run-surface.sh`. Text chat works under
both.

## Voice — daemon-owned

Voice STT/TTS provider credentials are owned by `ocean-os` (daemon endpoints
`/v1/voice/stt` and `/v1/voice/tts`). The proxy forwards `/api/stt` and
`/api/tts` to the daemon transparently; the browser never sees a credential.
Set `XAI_API_KEY` in the daemon's environment (or configure the credential
via ocean-os's provider auth file). The proxy no longer reads or stores an
xAI key.

`GET /api/config` reports `has_auth`; the UI fetches it on boot so no URL or credential is ever typed in the browser.

## Roadmap

- Done: web/PWA chat, SSE transcript, model picker, session picker, proxy,
  voice STT/TTS, Chrome extension bootstrap, provider-backed STT/TTS moved
  to daemon-owned voice endpoints in `ocean-os` (proxy no longer holds xAI key).
- In progress: Tauri native shell (loads the same `dist/` bundle), explicit
  session scoping, Leptos canvas, canvas ledger, LiveKit presence controls.
- Next: reliable canvas IPC for Leptos+Tauri, tldraw render commands, LiveKit
  mic/camera participation, surface-state injection into agent turns, native
  LiveKit Rust client behind a Tauri feature flag.

## Provenance

The voice work in `legacy-voice/` was originally proposed as PR #22 in `ocean-os`. Extracted here so the runtime repo stays Rust-only.

## License, brand, and credits

Ocean Surface code, project-authored documentation, and non-brand assets are
available under [MIT or Apache-2.0](LICENSE), at your option. Ocean names, logos,
wordmarks, application icons, and distinctive brand assets are excluded from
those grants; see [`TRADEMARKS.md`](TRADEMARKS.md) for truthful and nominative
use.

Third-party material remains under its own terms. See [`NOTICE.md`](NOTICE.md)
and the per-file [`docs/ASSET_PROVENANCE.md`](docs/ASSET_PROVENANCE.md) inventory,
and meet the people and projects behind the surface in
[`CREDITS.md`](CREDITS.md). Contribution terms are in
[`CONTRIBUTING.md`](CONTRIBUTING.md).
