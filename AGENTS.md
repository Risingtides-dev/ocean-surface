# Ocean Surface Agent Guide

Ocean Surface is the product-surface repo. The canonical UI is one Leptos/WASM
app (`crates/ocean-surface-ui`) built once via Trunk and shipped to two hosts:
the browser PWA (served by `crates/ocean-surface-proxy`) and the Tauri native
shell (`crates/ocean-tauri`, macOS first). A Chrome extension wraps the same
bundle. The GPUI desktop app (`crates/ocean-gui`) is ABANDONED (2026-07-21) —
DO NOT work on it: no fixes, no features, no scouting for bugs, no tickets. It
is dead source, frozen for reference only; touching it is wasted effort.

The sibling repo `../ocean-os` owns runtime authority: daemon, agent loop,
tools, providers, permissions, projects, workspaces, sessions, and events.
`ocean-bedrock` is a private, optional authenticated service for authorized team
deployments; Surface must not require it or expose Bedrock credentials to
browser state. Cross-repo routing and ownership map:
`docs/OCEAN_PROJECT_MAP.md`.

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

The public proxy login contract is username/password to an HttpOnly,
SameSite=Strict session cookie. Ordinary browsers and devices must not be
rejected by Origin, Host, forwarded-header, Cloudflare Access, Tailscale, or
device-posture gates. Public HTTPS deployments set
`OCEAN_SURFACE_COOKIE_SECURE=on`; that setting controls only the cookie's
Secure attribute.

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

Ocean Floor is the read-only Observatory mode. Its isometric pixel scene,
semantic list, inspector, and replay rail consume only the typed reducer over
metadata-safe `/v1/observatory/{snapshot,events,replay}` responses; no mock
executions, transcript content, inferred activity, or observer write controls.
Every reducer execution owns one constant-footprint cubicle with a stable slot:
snapshot rows preserve server admission order, and a live admission appends the
next slot without resizing or repacking existing modules. Modules pack
grid-adjacent into one connected facility — corridors, doorway partitions, and
the tall boundary envelope derive from present slots only. Furnishings are
static architecture; actor, tool, attention, status, room lighting, and
topology treatments come only from real reducer state. Animation is truthful:
typing/blink/wave and screen activity render recorded phases, the one-shot
walk-in visualizes a live admission event, and reduced motion stops the loop.
The web proxy reads the daemon-minted mode-0600 observer token immediately
before each upstream request and injects it server-side. Never expose that
token to browser code, bundle it, or cache it in client storage.

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
- Opening the sessions panel refreshes both the session list and project
  catalog. Project-section render identity includes the visible label so a
  daemon-side project rename cannot leave stale client chrome behind.
- `New Session` is lazy on the web surface. It resets local transcript/turn
  state and waits for the first prompt to POST a daemon session; do not re-add
  eager session creation that litters 0-turn drafts. The bounded exception is
  confirmed Voice Planner: the exact `Create draft` or `Create & start` click
  is itself the explicit product action that creates one session.
- The pre-session Voice Planner remains non-executing: its Realtime tool set is
  limited to bounded `list_workspace` / `read_workspace_file` reads whose
  normalized and daemon-canonicalized targets stay under the validated workspace
  plus `propose_handoff`; only the exact `Create draft` or `Create & start` click
  creates a session or starts a turn.
- Ordinary realtime Voice chat receives workspace inspection only when the
  daemon's secret response includes the canonical root resolved from the active
  session. Freeze that root into the connection config and fulfill only bounded
  relative `list_workspace` / `read_workspace_file` calls whose daemon-resolved
  targets remain under it. Never derive authority from model arguments or a
  browser-selected path; older/project-less responses keep render + handoff only.
- Ordinary realtime Voice chat streams OpenAI `response.output_audio_transcript.*`
  deltas into one local assistant turn keyed by output item, then replaces the
  accumulated text with the authoritative done transcript. Keep this projection
  conversation-only (Planner has no chat transcript), and do not represent it as
  daemon-persisted history: a later session refresh remains authoritative.
- Idle web/extension headers stay single-bar: project/session context may stay
  visible, but call/join affordances live behind overflow until intentionally
  opened or actively connected.
- Sessions-panel live state (OCEAN A1 / v7): abortable poll rail — every
  async fetch is wrapped in `abortable()` with the `AbortHandle` stored in
  `RwSignal<Option<AbortHandle>>`. Only `fetch_all_sessions().await` lives
  inside abortable. `poll_guard_write(current_gen, my_gen, panel_open)` is
  called before every `session_list.set()` — gen match + panel open required,
  so a settled stale fetch after close/reopen can never write. After matching
  all three outcomes, ONE unified `poll_release_in_flight` gen-gated guard
  releases `in_flight` (reads the actual `poll_in_flight.get_untracked()`, not
  a literal). Interval uses `leptos::prelude::set_interval_with_handle(_, Duration::from_secs(2))`
  → `RwSignal<Option<IntervalHandle>>` (thin i32 Copy+Send+Sync wrapper, no raw
  Closure/i32/forget leak). `handle.clear()` in `stop_polling`. The three
  production deciders (`poll_guard_write`, `poll_should_skip`,
  `poll_release_in_flight`) are file-scope fns; the Abortable test feeds old
  `Err(Aborted)` through the unified cleanup after the new task sets
  `in_flight=true` and proves the stale task does not clear it. The session
  dot renders `data-state` (permission → cancelling → running → recent → idle)
  with colour-keyed breathing/fade animations, `role="img"`, and
  `aria-label`; CSS tokens are `--fg-4` (idle), `--fg-3` (cancelling),
  `--accent`/`--warn`; `prefers-reduced-motion` sits
  after the state selectors so it wins. Unknown daemon variants are
  `#[serde(other)]` and render idle/recent.
- Transcript live-follow is intent-aware: follow streams only while the reader
  is at (or near) the bottom; scrolled-up history reading is never yanked, a
  quiet zero-height sticky `↓ latest` affordance returns and re-pins, and a
  session switch always re-pins so the new transcript opens at its latest
  turn. Do not re-add unconditional scroll-to-bottom on stream deltas.
- Session switches and reconnects commit the daemon's complete persisted
  transcript immediately, including while a turn is live, while preserving the
  daemon-owned running/Stop projection and continuing the scoped SSE tail.
  Never quarantine a live session behind client-side detail polling,
  `detail syncing…`, or a manual refresh control.
- Every complete session-list request—thin daemon spawner or A1 panel
  poll—claims the same daemon-owned generation ticket. Only the latest claimant
  may replace `session_list`; panel-local generation/open guards still protect
  lifecycle writes and cleanup. Pagination cursors are RFC 3986-encoded as one
  query-component value before daemon requests.
- A project-section `+ new session` always passes an explicit cwd: catalogue
  `workspace_root` first, otherwise the section's newest concrete session root,
  and the action stays absent when neither root exists. It must never inherit
  the previously active session's cwd or silently fall back to `/tmp`.
  Project-form focus refs belong on the actual first inputs (existing path and
  new-project name), not container elements.
- Reveal lifecycle is deterministic: opening Council closes every competing
  reveal; opening the Island closes every non-Island reveal; every peer reveal
  open (Council, Rooms, Sessions, Floor, deck, phone dialer, LiveKit controls)
  closes the Island. Window Escape closes exactly one topmost surface in visual
  z-order: Council → Island → Rooms → Sessions → Floor → deck → phone dialer →
  LiveKit.

## Rooms Contract

- Browser-hosted Rooms treat `/api/config` as the current-user authority and
  keep join/post unavailable until that identity resolves; never act under a
  previous tenant's browser storage. Explicit single-operator and direct
  extension/Tauri hosts use the stable `surface-operator` identity, and Room
  identity signals own `String` values rather than leaked process-lifetime
  string allocations.
- Every successful `GET /v1/rooms/persistent/{key}` carries a required
  `RoomAccessProjection`, including explicit `Local` for G1 rooms. Surface
  `None` means loading or no open room; it is never a local-room discriminator.
- Room transcripts hydrate once, then tail only the room-scoped SSE endpoint
  `GET /v1/rooms/persistent/{key}/events` with sequence resume. Subscribe to
  both `room_message` and `room_access` immediately: messages alone advance the
  room sequence cursor; access projections replace state without a sequence.
  Do not restore transcript polling or consume the global agent-event stream.
- Decoded room-tail frames mutate state only after one shared generation+room
  admission check. Opening and closing a room share one synchronous reset path
  so stale transcript, access, and tail state cannot leak across room identity.
- The browser PWA proxy must forward `/v1/agents` as JSON and stream room SSE
  unbuffered while preserving `Last-Event-ID`; Tauri reaches the same daemon
  endpoints directly. The proxy is an ALLOWLIST, not a passthrough: the agent
  builder's write verbs (`POST /v1/agents`, `GET`/`PUT /v1/agents/{name}`) are
  registered explicitly, and an unregistered verb answers with an empty body
  that the surface can only report as a JSON decode error. Adding an
  `/v1/agents/{name}` route requires the `has_dot_segment` guard —
  percent-encoding does not neutralise `..`, because `.` is unreserved.
- Agent participants are selected from daemon-owned `/v1/agents` identities and
  remain subject to daemon join validation. The surface never mints a
  participant from free text: a name typed into the agent builder becomes a
  participant only after the DAEMON creates the folder and the identity comes
  back in `/v1/agents`.
- Local rosters and mention ids come from `Room.participants`; every non-Local
  roster and mention id comes only from the safe access member projection.
  Composer writes are enabled only for `Local` and `Live` access.
- Federated outbox items render outside the confirmed transcript. Pending items
  are informational; only failed items expose the daemon retry action, and the
  returned access projection applies immediately behind the room-generation
  guard before any duplicate SSE projection arrives.
- Invite and redeem UI remains absent until daemon-owned outbound routes exist.
- Rooms G1 is daemon-native text collaboration. LiveKit controls stay outside
  the room join, leave, roster, and transcript lifecycle until explicitly
  reintroduced behind a reviewed platform contract.
- The rooms browser is a flex column; `.rooms-panel__list` keeps
  `min-height: 0` with vertical overflow so long room lists scroll instead of
  pushing status/actions outside the viewport.
- Channel/thread drafts, mention state, and pending-send confirmation are
  scoped to the exact open-room generation. A room switch or close clears them
  synchronously so content and a stale `Sending…` gate cannot cross rooms.
- An empty hydrated transcript has no resume cursor. Surface omits
  `after_seq` until it owns a real room sequence, preserving the daemon's
  zero-based first row.

## Agent Builder Contract

- `agents.rs` owns the agent write layer; the form mounts inside the members
  rail's existing `+ agent` disclosure (`rooms_workspace.rs`) and reuses
  `Rooms::available_agents` as both the add picker and the edit-target list.
  No parallel agent list.
- The model picker is built from the daemon's `/v1/models` catalogue shared
  through `Rooms::models` — never a hardcoded list, never a second fetch.
  `model_options` must always include the current value, because `/v1/models`
  resolves asynchronously and a missing option silently rewrites a pinned model
  to "inherit default" on save.
- Tools is free text until the daemon publishes a tool catalogue. There is no
  `/v1/tools` route; a hardcoded dropdown would rot on the next tool added.
- Prefill reads `agent.config.tools`, never the merged `AgentDef.tools` — the
  latter includes `tools/` filename stems, and round-tripping it writes
  filesystem-derived names into `agent.toml`.
- A write body is the WHOLE `agent.toml`: the daemon rebuilds the file from the
  spec it is handed, so `capabilities` and `yolo` are round-tripped verbatim
  even though the form does not render them. `[[subprocess_capability]]` cannot
  be expressed in the write API's spec, so an agent declaring one is refused
  (`blocks_save`) rather than saved lossily.
- An agent IS its folder: identity comes from the PUT path, the name field is
  read-only while editing, and renaming is a move on disk, not a form edit.
- Form state lives in `AgentBuilderState` at `RoomsWorkspace` scope. The
  members-rail closure re-runs on every `rooms.access` change, so state created
  inside it is destroyed by unrelated roster traffic.
- Every pre-dispatch decision is a pure function with a native test. The daemon
  stays the authority on all of them; the client copies exist to save a
  round-trip, never to replace one.
- Requires `ocean-os` `feat/agent-crud` on main. Against an older daemon the
  write verbs answer 405 and `write_error_message` says so in words.

**Files:** `crates/ocean-surface-ui/src/agents.rs`, `rooms_workspace.rs`
(mount), `rooms.rs` (`models` handle, `pub(crate) encode`),
`crates/ocean-surface-proxy/src/main.rs` (allowlist),
`styles/rooms-workspace.css`.

**Frozen gates:** the same six listed under File Preview Deep-Link, plus
`cargo test -p ocean-surface-proxy`.

## Workspace Map

| Path | Role |
|---|---|
| `crates/ocean-surface-ui/` | Leptos WASM web/PWA/extension UI |
| `crates/ocean-tauri/` | Tauri 2.x native desktop shell; loads the same dist/ bundle as the browser PWA |
| `crates/ocean-surface-proxy/` | axum proxy for web bundle, STT/TTS relay to the daemon, config, daemon reverse proxy |
| `extension/` | Chrome extension wrapper around the Leptos surface |
| `vscode-extension/` | Cursor/VS Code ACP client surface with sidebar, bottom-panel, editor-tab, and status-bar launch modes |
| `legacy-voice/` | reference voice code only; do not build new architecture here |
| `crates/ocean-gui/` | ABANDONED GPUI desktop app (2026-07-21) — dead, do NOT work on it |
| `crates/ocean-gui/canvas-web/` | ABANDONED (part of the dead GPUI app) — do NOT work on it |

The proxy holds no provider credentials. `/api/stt` and `/api/tts` forward to
the daemon's `/v1/voice/stt` and `/v1/voice/tts`, where `ocean-os` resolves the
xAI key per-request (env `XAI_API_KEY` / auth.json `xai` block). Do not add
provider credentials or direct provider calls to the proxy. Google Maps is
optional and enabled only by an explicit non-empty `GOOGLE_MAPS_API_KEY`; never
commit an organization-owned browser key or restore a compiled-in default.

## Build / Check

```sh
cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
cargo check -p ocean-surface-proxy
cd crates/ocean-tauri && cargo check   # native shell (standalone, not a workspace member)
```

The GPUI crate is ABANDONED — do not work on it, do not scout it for fixes.

For local web/proxy work:

```sh
OCEAN_SURFACE_BIND=127.0.0.1:18790 OCEAN_SURFACE_AUTH=off ./run-surface.sh
```

For LAN/tailnet access, keep the default bind and provide both operator-login
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

## File Preview Deep-Link (Lane D)

**Contract:** FileTreeView captures `daemon.cwd` at mount, creates an
`on_file_click` callback that calls `resolve_file_tree_path` (unified resolver
with explicit-vs-absent provenance branch), and sets `preview_file_intent`. The
app producer Effect reads the intent and dispatches `WorkspaceFocus::Preview`:
on Tauri it clears the intent after dispatch (FilesPanel never mounts); on web
it leaves the intent set for the FilesPanel consumer. The consumer (web-only)
re-reads intent, resolves the path, fetches on cache miss, and clears. On Tauri
the workspace receives `WorkspaceFocus::Preview` and routes through `open_file`
→ `open_or_focus` → Preview tab + fetch-on-cache-miss.

**Resolver:** `resolve_file_tree_path(entry_path, ancestor_prefix, name,
workspace_root, cwd)` — explicit absolute/home-relative passthrough; explicit
relative cwd-authoritative (no starts_with guard); absent path assembles
resolved-root + ancestor + name where relative roots resolve against cwd.
`resolve_file_path` exists for non-file_tree callers and follows the same
cwd-authoritative rule.

**Files:** daemon.rs, app.rs, workspace.rs, components.rs, deck/files.rs,
styles/deck.css.

**Frozen gates:** `cargo fmt --check`, `cargo clippy -p ocean-surface-ui
--target wasm32-unknown-unknown -- -D warnings`, `cargo check -p
ocean-surface-ui --target wasm32-unknown-unknown`, `cargo check -p
ocean-surface-proxy`, `cargo test -p ocean-surface-ui --target
wasm32-unknown-unknown --no-run`, `cargo test -p ocean-surface-ui`.
