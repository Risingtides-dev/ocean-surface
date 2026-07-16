# Ocean Surface Agent Guide

Ocean Surface is the product-surface repo. The canonical UI is one Leptos/WASM
app (`crates/ocean-surface-ui`) built once via Trunk and shipped to two hosts:
the browser PWA (served by `crates/ocean-surface-proxy`) and the Tauri native
shell (`crates/ocean-tauri`, macOS first). A Chrome extension wraps the same
bundle. The legacy GPUI desktop app (`crates/ocean-gui`) is soft-deprecated —
source retained for mining into the Tauri Rust backend, not for new feature work.

The sibling repo `../ocean-os` owns runtime authority: daemon, agent loop,
tools, providers, permissions, projects, workspaces, sessions, and events.
Cross-repo routing and ownership map: `docs/OCEAN_PROJECT_MAP.md`.

Do not put provider calls, agent reasoning, session storage, permission policy,
or tool execution authority in this repo. Surface code should render state,
collect intent, attach to sessions, and call the daemon.

## Platform Contract (binding — web + desktop + mobile)

`docs/OCEAN_PLATFORM_CONTRACT.md` is the cross-team alignment layer; the
desktop feature split lives in `docs/OCEAN_DESKTOP_NORTH_STAR.md` (Surface
Capability Matrix). The load-bearing rules:

- One core, many shells: `crates/ocean-surface-ui` is the product everywhere;
  shells (proxy/PWA, extension, Tauri desktop, future Tauri mobile) stay thin.
- `src/host.rs` is the ONLY seam for platform capability; every fn no-ops off
  its platform, and UI mounts platform features conditionally — absence, not
  errors.
- Sorting rule: "does the phone version need this?" Yes → shared core (daemon
  HTTP/SSE only, compact/touch rendering in the same slice). No, it's about
  the machine → ocean-tauri command + host.rs wrapper + conditional mount.
- The web core is the bones of the mobile app (Tauri 2 iOS/Android; PWA is
  mobile v0; `compact.css` is the mobile stylesheet; no hover-only
  affordances).
- Shared-file discipline (`app.rs`): smallest hunks, committed promptly;
  NEVER reference an uncommitted module from a shared file — `mod x;` +
  usage lands only when `x` compiles with passing tests.
- Main must build standalone: verify the pushed tree in a detached worktree
  before pushing (GitButler lanes split hunks across sessions).

## Current Build Focus

```sh
# Web/PWA (canonical UI; builds Trunk bundle + release proxy):
./run-surface.sh
# Native desktop (Tauri; builds and loads the same dist/ bundle):
./run-tauri.sh
```

`run-surface.sh` requires `OCEAN_SURFACE_USER` and `OCEAN_SURFACE_PASS` for its
default LAN/tailnet bind. For trusted localhost diagnostics only, bind
`127.0.0.1` and set `OCEAN_SURFACE_AUTH=off`. Direct `cargo tauri dev` does not
rebuild `dist/`; use `run-tauri.sh` whenever freshness matters.

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

## Web Surface Design System

All visual work on the Leptos web surface follows
`docs/OCEAN_WEB_SURFACE_DESIGN.md`. Non-negotiables:

- Identity is the OCEAN depth ramp (the TUI splash banner, deep indigo →
  bright aqua) and the accepted circular neumorphic wave mark in
  `public/brand/master-1024.png`. No Rising Tides
  magenta/purple (rejected 2026-07-04), no legacy teal-mint, no gradient-clip
  text, and no prompt/cursor/code-glyph logo direction. The ramp paints solid
  colors on discrete elements or vector fills inside the circular mark.
- Stylesheets are split per domain under `styles/` (tokens → base → chrome →
  transcript → components → composer → panels → call → canvas → compact →
  float, in cascade order). Colors live ONLY in `styles/tokens.css`.
  `extension/sidepanel.html` and `scripts/build-extension.sh` enumerate the
  same files — adding a stylesheet touches all three places.
- Control density is a design defect: conditional rendering over permanent
  chrome, one header overflow (`⋯`) for secondary actions, ghost triggers for
  idle features (dialer, join call), reveal-on-intent for power knobs.

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

Web surface session UI:

- The sessions panel is project-first: prefer daemon-provided `owning_project`,
  fall back to exact `workspace_root`/`cwd` matches against the project catalog,
  and put everything else in an explicit `Other` bucket.
- `New Session` is lazy on the web surface. It resets local transcript/turn
  state and waits for the first prompt to POST a daemon session; do not re-add
  eager session creation that litters 0-turn drafts. The bounded exception is
  confirmed Voice Planner: the exact `Create draft` or `Create & start` click
  is itself the explicit product action that creates one session.
- Idle web/extension headers stay single-bar: project/session context may stay
  visible, but call/join affordances live behind overflow until intentionally
  opened or actively connected.

## Rooms Contract

- Room transcripts hydrate once, then tail only the room-scoped SSE endpoint
  `GET /v1/rooms/persistent/{key}/events` with sequence resume. Do not restore
  transcript polling or consume the global agent-event stream for Rooms.
- The browser PWA proxy must forward `/v1/agents` as JSON and stream room SSE
  unbuffered while preserving `Last-Event-ID`; Tauri reaches the same daemon
  endpoints directly.
- Agent participants are selected from daemon-owned `/v1/agents` identities
  and remain subject to daemon join validation. Free-text agent creation does
  not belong in the surface.
- Rooms G1 is daemon-native text collaboration. LiveKit controls stay outside
  the room join, leave, roster, and transcript lifecycle until explicitly
  reintroduced behind a reviewed platform contract.

## Workspace Map

| Path | Role |
|---|---|
| `crates/ocean-surface-ui/` | Leptos WASM web/PWA/extension UI |
| `crates/ocean-tauri/` | Tauri 2.x native desktop shell; loads the same dist/ bundle as the browser PWA |
| `crates/ocean-surface-proxy/` | axum proxy for web bundle, STT/TTS relay to the daemon, config, daemon reverse proxy |
| `extension/` | Chrome extension wrapper around the Leptos surface |
| `vscode-extension/` | Cursor/VS Code ACP client surface with sidebar, bottom-panel, editor-tab, and status-bar launch modes |
| `legacy-voice/` | reference voice code only; do not build new architecture here |
| `crates/ocean-gui/` | legacy GPUI native desktop app and tldraw canvas host (soft-deprecated, source retained for mining) |
| `crates/ocean-gui/canvas-web/` | web bundle loaded into the GPUI canvas webview (legacy) |

The proxy holds no provider credentials. `/api/stt` and `/api/tts` forward to
the daemon's `/v1/voice/stt` and `/v1/voice/tts`, where `ocean-os` resolves the
xAI key per-request (env `XAI_API_KEY` / auth.json `xai` block). Do not add
provider credentials or direct provider calls to the proxy.

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
OCEAN_SURFACE_BIND=127.0.0.1:18790 OCEAN_SURFACE_AUTH=off ./run-surface.sh
```

For LAN/tailnet access, keep the default bind and provide both Basic auth
environment variables. `trunk serve` exercises the bundle alone, not the
release proxy or daemon reverse-proxy path.

`localhost:8790` is the operator's live surface and serves the immutable release
selected by `~/.config/ocean-surface/current`, not the repo's `dist/`. The
`dev.risingtides.ocean-surface-auto-deploy` LaunchAgent polls `origin/main`,
builds and gates it in a disposable detached worktree, then atomically advances
`current` and `deployed-rev`; failures preserve the last-known-good release.
Still verify the served WASM hash and UI contracts on `:8790` itself—never
substitute a private proxy when reporting the live app's state.
Loopback origins must not retain a service worker or `ocean-shell-*` cache;
offline PWA interception is reserved for deployed non-loopback origins.

Native desktop:

```sh
./run-tauri.sh                          # build dist/ + launch Tauri
```

The daemon must be running from `../ocean-os` for live agent behavior.

## Cursor / VS Code Extension UI Contract

- Keep `vscode-extension/` transcript-first: do not add command decks, fake
  logos, sparkle/AI ornament, or rows of location/action buttons unless the
  operator explicitly asks for that UI.
- The operator did not request a new control-heavy UI for the extension; when
  repairing it, preserve a minimal chat surface and expose extra actions
  through Cursor commands/status entry points instead of visible button sprawl.
- If applying Kami to the extension, use it as restraint only: warm neutrals,
  sparse ink-blue accents, clean type hierarchy, and no copied document or
  landing-page chrome.
- For richer agent-thread affordances, use the installed Codex/ChatGPT
  extension as a product reference for patterns such as compact composers,
  inline tool activity, and file/diff rows. Do not copy proprietary source or
  styling verbatim; adapt the interaction pattern with Ocean-owned code.
