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
- Every successful room open carries a required `RoomAccessProjection`,
  including explicit `Local` for G1 rooms. Hydration decodes it off `GET
  /v1/rooms/persistent/{key}/snapshot`, which is the route an open now reads —
  the unpaged `GET /v1/rooms/persistent/{key}` answers the same field and is no
  longer what the surface opens with. Surface `None` means loading or no open
  room; it is never a local-room discriminator.
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
  builder's write verbs (`POST /v1/agents`, `GET`/`PUT`/`DELETE
  /v1/agents/{name}`) are registered explicitly, and an unregistered verb
  answers with an empty body
  that the surface can only report as a JSON decode error. Adding an
  `/v1/agents/{name}` route requires the `has_dot_segment` guard —
  percent-encoding does not neutralise `..`, because `.` is unreserved.
- Agent participants are selected from daemon-owned `/v1/agents` identities and
  remain subject to daemon authorization and admission. The surface never mints
  a participant from free text or calls the legacy bare `add_agent` path: a
  package becomes a local first-agent participant only through the atomic
  operator-authenticated `POST .../agents/bootstrap` response, and becomes room
  execution authority only through that response's daemon-derived room/preview,
  digest-bound owner ceremony, and durable binding. Never restore the local
  unauthenticated participant POST as agent bootstrap. Activation, context,
  grants, and `none`/`room` memory scope are operator choices in that ceremony;
  the daemon remains authoritative for their accepted intersection.
- Browser-PWA Room-agent mutations are available only through the server-side
  proxy host; every non-loopback host remains login-gated. The proxy injects
  its mode-0600 operator key only on the exact
  bootstrap, authorize, reauthorize, suspend, resume, and revoke routes;
  inspection and package preview stay credential-free. Browser
  `X-Ocean-Operator`, Cookie, Origin, and Referer headers never cross that
  boundary. In auth-off mode, a mutation carrying Origin or Referer must name
  the exact loopback Host; reject it before credential lookup otherwise, while
  retaining headerless localhost CLI clients. Auth-off startup is refused on
  non-loopback binds. The Tauri shell now owns the equivalent privileged
  transport this rule required: its `daemon_operator_request` command takes a
  method and a PATH (never a URL, never a header), re-checks both against a
  mirror of the same six-route allowlist, reads the same `operator.key` under
  the same five-condition custody check, supplies the daemon origin itself from
  `OCEAN_DAEMON_URL`, and returns only the daemon's status and body. Tauri 2
  capabilities do not gate `generate_handler!` commands, so that allowlist is
  the boundary. Dot segments are judged AFTER percent-decoding, because the
  URL parser normalises `%2e%2e` into `..` and would otherwise carry the
  credential to a route the allowlist approved a different string for; the
  built URL is then re-parsed and refused unless its path still equals the
  approved one, which makes the whole normalisation class inert. The shell
  does not build for a non-unix target: its custody contract is POSIX, and a
  ceremony rendered writable over a credential that cannot be read is the
  opposite of "absence, not errors". The extension host has no shell and no proxy and REMAINS
  read-only. On the surface side every privileged mutation leaves
  `room_agent_authorization.rs` through one seam addressed by an
  `AuthorityRoute` the four route builders alone construct; the credential
  enters no host's WASM bundle.
- Binding reads carry server-derived owner eligibility. Surface does not infer
  owner authority from a local participant projection. With no binding, a
  resolved Human already in a Local roster may see only the bootstrap
  affordance; the operator-authenticated daemon establishes or refuses the
  durable owner role and returns the authoritative updated room plus package
  preview. Lost-response retries for binding status changes reuse the exact
  decision id until the operation settles. Agent mention candidates require an
  Active binding (and federated `local_binding_available=true`); historical
  roster rows remain visible.
- Local rosters and mention ids come from `Room.participants`; every non-Local
  roster and mention id comes only from the safe access member projection.
  Composer writes are enabled only for `Local` and `Live` access.
- Federated outbox items render outside the confirmed transcript. Pending items
  are informational; only failed items expose the daemon retry action, and the
  returned access projection applies immediately behind the room-generation
  guard before any duplicate SSE projection arrives.
- `room_invite.rs` owns minting: `POST /v1/rooms/persistent/{key}/invites`
  answers 201 with the invite RAW — no `{ok:true}` envelope, unlike artifacts,
  attachments and the workspace lane. Success is settled first, on the status
  and a present `code`; only a reply that is not a success is asked what its
  top-level `error` means. A decoder copied from those neighbours reads a
  minted invite as a malformed reply, or as whatever its characters spell.
- Minting from a `Local` room BOOTSTRAPS federation, permanently and
  irreversibly from this surface, so the first click only ARMS the control and
  states what firing it will do. A 503 `federation_unavailable` is the
  deployment describing itself, not a fault.
- `room_redeem.rs` owns joining: `POST /v1/rooms/persistent/invites/redeem`
  answers 200 with a flattened `RoomAccessProjection` plus `room_key`.
  `room_key` is decoded OPTIONAL on purpose — bundle and daemon roll forward
  independently, and requiring it would make a redemption that ALREADY
  succeeded unreadable on an older daemon. Absent it, `newly_joined_key` diffs
  the room list and opens a room only when exactly one appeared. The panel
  mounts in the left rail, because someone holding a code has no room open and
  may have no rooms at all.
- An invite code is a bearer grant to the room. A minted one arrives in the
  RESPONSE body and lives in one signal and the open panel's DOM; a redeemed
  one goes in the REQUEST body. Never a log line, never an error sentence, and
  never the rail — `rail_line` is deliberately code-free because the rail is on
  screen for as long as the room is and the panel is not — and never past the
  room it was minted for. No fixture in this repo may carry a real one. The
  onboarding link EMBEDS the code, so it is the same grant in a longer form and
  gets the same discipline.
- A room's `workspace_root` is the folder its agent turns run in, resolved on
  the DAEMON's host — not the browser's, which cannot see that filesystem, so
  nothing here pre-validates a path and the daemon's canonicalizing
  `400 invalid_workspace_root` is the only verdict. Unrelated to the SESSION
  workspace root the rest of this crate means by that name. It rides the create
  body (`key`, `name`, `trigger_policy?`, `workspace_root?`) and
  `PATCH /v1/rooms/persistent/{key}`, where absent leaves the binding unchanged
  and an explicit `null` unbinds — so the unbind body must NOT skip `None`, and
  the policy and workspace PATCHes each send their own field alone rather than
  clobbering the other's. An unbound room is not a cosmetic gap: every
  room-bound agent turn in it is refused `503 workspace_unavailable` before the
  agent sees the message, so the surface states that in words wherever the
  trigger toggles render. That refusal is NOT `room_repo.rs`'s
  `workspace_unavailable`, which is the compute lane saying Bedrock is
  unreachable; do not share wording between them.
- Rooms G1 is daemon-native text collaboration. LiveKit controls stay outside
  the room join, leave, roster, and transcript lifecycle until explicitly
  reintroduced behind a reviewed platform contract.
- The rooms browser is the left rail of `rooms_workspace.rs`, a flex column;
  `.rooms-workspace__left-list` keeps `min-height: 0` with vertical overflow so
  long room lists scroll instead of pushing the create field and status line
  outside the viewport. (It was `.rooms-panel__list` in `styles/panels.css`
  until that never-rendered panel's CSS was deleted.)
- Channel/thread drafts, mention state, and pending-send confirmation are
  scoped to the exact open-room generation. A room switch or close clears them
  synchronously so content and a stale `Sending…` gate cannot cross rooms.
- An empty hydrated transcript has no resume cursor. Surface omits
  `after_seq` until it owns a real room sequence, preserving the daemon's
  zero-based first row.
- Mention notifications are raised from the LIVE TAIL only, by the same
  `room_markdown` tokeniser that paints the highlight, so what notifies is what
  shows. Hydration and the load-older backfill never notify — history arriving
  is not someone talking to you now. A notification is suppressed only when the
  reader is demonstrably looking at that room: focused, Rooms on screen, and
  that room open. `open_key` alone is NOT "on screen" — it and the tail both
  outlive the workspace unmounting behind Direct messages. Consequently a
  mention in a room you do not have OPEN does not raise an OS notification:
  only the open room has a tail, and standing up a tail per room to change that
  would contradict the one-room rule above. The room-list response now carries
  a daemon-derived, identity-scoped sparse `attention` projection for every
  selected page. Surface uses it for unopened-room unread/mention badges and
  never scans message text or opens N `EventSource`s to synthesize attention.
  An absent projection means an older daemon and falls back to legacy sequence
  unread state; a present empty projection is authoritative zero.

## Agent Builder Contract

- `agents.rs` owns the package write layer; its form mounts inside the Room-agent
  authorization ceremony and reuses `Rooms::available_agents` as the ceremony's
  package selector and the builder's edit-target list. No parallel agent list
  and no bare participant picker.
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

**Files:** `crates/ocean-surface-ui/src/agents.rs`,
`room_agent_authorization.rs`, `rooms_workspace.rs` (mount), `rooms.rs`
(`models` handle, `pub(crate) encode`),
`crates/ocean-surface-proxy/src/main.rs` (allowlist),
`styles/rooms-workspace.css`.

**Frozen gates:** the same seven listed under File Preview Deep-Link, plus
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

## Repository Ledger

Root `events.md` is this repo's append-only chronological ledger. Record
meaningful work there; a PR touching `crates/`, `styles/`, `scripts/`, `ops/`,
`deploy/`, `extension/`, `vscode-extension/`, `.github/` or `index.html` must
carry its entry in the same diff, which the `ledger` job in
`.github/workflows/ci.yml` reports on without blocking the merge.

Parallel branches each append their OWN entry — never settle an `events.md`
merge by dropping the other branch's. `.gitattributes` gives the file
`merge=union` so those appends stop conflicting at EOF, which they did the last
time two surface slices landed in one wave (#174, hand-resolved).

**Close an entry with a separator that carries the entry's own identity:** 81
underscores, a space, the entry's `HH:MM`, and the `worktree:` it was written on
when it has one — `______ 23:52 loop/my-slice`. Union emits a line both sides
added only ONCE, so two entries that end with the same line cannot be kept
apart: while every entry closed with the same bare rule, each extra parallel
append ate one separator and the next entry's `time:` header landed directly
under the previous entry's prose, FUSING two entries into one. #181 folded onto
#180 twelve minutes after #180 landed the check that catches it. `HH:MM` alone
is not the identity — it is minute resolution, and two slices in one wave land
in the same minute often enough to have done it; the worktree is what the clock
cannot give, and two parallel appends are by definition on two different
branches. An entry with no worktree was written on the main checkout, where
there is one writer and nothing to race, so its minute alone is enough.

The bare rule stays valid forever — the 276 entries written before this
convention all close with one, and `events.md` is append-only — so the check
accepts both forms and NEVER asserts that a separator is unique or
identity-bearing; requiring the new form would red every historical entry and
every entry a slice in flight is writing right now. What it asserts is that each
entry is CLOSED before the next one starts, which is what a fold destroys. **Run
`node scripts/check-ledger.mjs events.md` on any change to `events.md`, and again
on either side of a rebase carrying one; the two verdicts must match.** It runs in
CI on PRs and on pushes to main, and
its exit codes are 0 clean / 1 an entry is open / 2 the check could not run at
all. `--fix` closes what it finds by insertion only and writes the identity form,
so a repair does not hand the next merge the same shared line — never in CI,
because that is a non-append edit to a file under `merge=union` and the last cost
below applies to it. `scripts/events-merge-driver.test.mjs` proves a three-way
parallel append keeps all four rules and fuses nothing, but it reproduces the
merge in a scratch repo and never reads THIS file, so CI's `guards` job running
it proves the driver, not the ledger.

**Run `node scripts/check-ledger-order.mjs events.md` beside it.** The checker
never reads a `time:` header past the word, so five entries sat at the top of
this ledger newest-first for months and it called the file clean. The order
check reads the clock and reds any entry more than a day out of merge order —
a prepend, a backdate — while descents of hours, which is how parallel slices
land, pass. Same exit codes, no `--fix`: moving an entry is a decision, made
once by hand and recorded in the ledger. `scripts/check-ledger.mjs` itself is
one of three copies (bedrock, os, surface) and carries a code stamp; its test
recomputes the digest, so an edit that forks it from bedrock's copy is red
until the fork is written down.

Three things the identity separator does NOT buy:

- **An entry owns its rule, not the blank line after it.** That is a ruling, not
  an omission: a blank line cannot be given an identity, so it is the one part
  of the format no convention can protect from union. A merged append can land
  its `time:` header flush against the previous rule — the three-way fixture in
  `scripts/events-merge-driver.test.mjs` loses the blank at every join, while
  wave 52's real rebase kept this repo's and ate the sibling's, so it turns on
  where xdiff anchors rather than on anything worth asserting. The entry
  boundary survives either way. Cosmetic; close it up by hand if you mind, and
  never make the check red for it.
- **It saves an entry's TAIL, not its HEAD.** Two appends written in the same
  minute open with two identical lines (`time:` and `agent:`) and union folds
  those the same way, so the second entry can arrive without its header while
  its rule survives — and the check reads the survivor as one closed entry and
  exits 0. Eyeball the head of a merged entry when two slices share a minute;
  the fix belongs to the entry schema, not to the separator.
- **union only fails safe for append/append.** A NON-append change to
  `events.md` — a correction, a redaction, a repaired separator — lands in the
  same tail hunk a concurrent append touches, and union settles it by keeping
  both sides, silently restoring the line the change removed. Any merge
  carrying one must be eyeballed; that it came back clean is not evidence that
  it is right.

Merged entries may also INTERLEAVE rather than land in strict wall-clock order,
since union emits the current branch's lines before the merged branch's. That
one is cosmetic: every entry carries its own `time:` field.

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
--target wasm32-unknown-unknown -- -D warnings`, `cargo clippy -p
ocean-surface-ui --all-targets -- -D warnings`, `cargo check -p
ocean-surface-ui --target wasm32-unknown-unknown`, `cargo check -p
ocean-surface-proxy`, `cargo test -p ocean-surface-ui --target
wasm32-unknown-unknown --no-run`, `cargo test -p ocean-surface-ui`.

**Both clippy invocations are required and neither replaces the other.** The
wasm32 one is the target the release lane actually denies warnings on, so it is
the one whose verdict can stop a bundle promoting. It builds the bin without
`cfg(test)`, which makes every `mod tests` block invisible to it — and this
crate's tests are where most changes land. The `--all-targets` one is the only
thing in this list that lints test code, and it runs on the host because
`--all-targets` on wasm32 has nothing to add: `cargo test --target
wasm32-unknown-unknown --no-run` already proves the test code compiles there.

## Guard Tests — Controls the Compiler Does Not Hold

`crates/ocean-surface-ui` is a BINARY crate (`src/main.rs` + `fn main()`, no
`[lib]`). An integration test in `tests/` therefore cannot import a single item
from it, cannot mount a component, and cannot press anything. Every guard in
that directory is a source scanner because that is the only lever available —
do not spend time looking for a way to call a component.

**The toolkit is shared: `tests/common/mod.rs`.** A subdirectory `mod.rs`, not
a top-level file, because cargo compiles each top-level `tests/*.rs` as its own
binary and a subdirectory is not a target. It carries `repo_root`, `read` (repo
root-relative, for `styles/`), `src` (crate `src/`-relative), `view_source`
(the half of a module a release build compiles), `without_whitespace`, and
`all_rust_src`. It is `#![allow(dead_code)]` because inclusion is per-binary:
each guard calls only some helpers, and an uncalled `pub fn` in a test binary
is a `dead_code` warning the gate's `-D warnings` turns into a failure. That is
the right trade here, but it is a trade: these helpers previously sat inline in
`dead_selector_removal.rs` and `ci_failure_trigger_control.rs` with no allow, so
one that lost its last caller announced itself. After the move, an orphaned
helper is silent forever — prune by reading, not by waiting for the gate.
Consumers: `ci_failure_trigger_control.rs`, `dead_selector_removal.rs`,
`unheld_room_controls.rs`.

**`tests/unheld_room_controls.rs`** pins six room controls that measurement
proves nothing else holds. The failure it exists for: a reviewer deletes a
control, every gate stays green, and a landed daemon route goes back to being
unreachable — #165 deleted the create panel's CI-failure checkbox and the
Response Policy summary line and the full suite plus the wasm check said
nothing.

**Measure before you pin. This is the lane's discipline, not a suggestion.**
Some controls ARE compiler-held and a guard on one is maintenance that buys
nothing. Apply the deletion for real, run the gate, pin only what stays green.
Measured at `4ed9a7c`:

| Control | Result |
|---|---|
| `room_summary.rs` summary rail `open` button | GREEN — pinned |
| `room_repo.rs` unbind ARMING click | GREEN — pinned |
| `room_workspace_panel.rs` destroy ARMING click | GREEN — pinned |
| `room_workspace_panel.rs` exec purge-all ARMING click | GREEN — pinned |
| `rooms_workspace.rs` both rosters' remove ARMING click | GREEN — pinned |
| `room_redeem.rs` join button's `on:click` (markup kept) | GREEN — pinned |
| `room_summary.rs` summarize RUN button | RED — compiler-held |
| `room_workspace_panel.rs` `provision` button | RED — compiler-held |
| `room_redeem.rs` join button's MARKUP | RED — held by an in-file test |
| `room_workspace_panel.rs` `expose` button * | RED — compiler-held |
| `room_workspace_panel.rs` port row's `close` button * | RED — compiler-held |

\* Measured on the commit that ADDED these two controls, not at `4ed9a7c`
where neither existed. Same method, later tree.

The split is not where intuition puts it, and the pairs are the lesson. In one
panel `provision` is held (`variant Provision is never constructed`) while
`destroy`'s arm is not. In one component the summarize RUN button is held
(deleting it takes `SummarizeRequest`, `summarize_url`, `classify_summarize`
and `SummarizeOutcome` dead with it) while the `open` button that is the only
door to it is not.

**The shape that hides** is the arming half of a two-click confirm. Deleting
the arm leaves the confirm branch standing, so the enum variant is still
constructed, the fire method is still called, and the signal is still read and
reset. Nothing is unreferenced; nothing warns. What is gone is the only way to
reach the confirm — the destructive verb stays fully implemented and
permanently unpressable.

**Two rules a guard here must satisfy.**

1. *Name the CALL SITE, not the literal.* A bare-literal assert is satisfied by
   the module's own test module quoting it. Measured in this tree:
   `state.confirm_destroy.set(true)` occurs 5x in `room_workspace_panel.rs` and
   once outside `#[cfg(test)]`; `state.confirm_purge.set(Some(PurgeTarget::
   All))` occurs 3x and once. So every needle carries its `on:click=` prefix
   AND every scan runs over `view_source`. Either alone is insufficient.
2. *Verify with a RENAME as well as a deletion.* A guard that only catches
   deletion lets the control be renamed to anything. All six here were checked
   both ways.

**Budget the mutation runs; they are not free.** The guard reads `src/` at
RUNTIME, but cargo builds every bin target of the package before it will run an
integration test — it has to, for `CARGO_BIN_EXE_*` — so every `cargo test
--test unheld_room_controls` against a mutated `src/` pays a full `Compiling
ocean-surface-ui`, measured at ~30s in this tree. The mutation therefore MUST
compile: append `this is not rust at all !!!` to any module and the run dies at
`error: could not compile ocean-surface-ui (bin "ocean-surface-ui")` having
executed no test at all. That same rebuild is what makes the RED rows above
legible — a compiler-held control announces itself as a build error, not as a
guard that passed.

**A control measured as compiler-held is a finding, not a failure.** Record it
and move on — but record it, because the hold has a shelf life. The rail rows
in `ci_failure_trigger_control.rs` were compiler-held until a flag table
elsewhere started constructing the same variants; that guard exists because the
hold evaporated.

**Frozen gates:** the same seven listed under File Preview Deep-Link.
