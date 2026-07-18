# Code Context

## Files Retrieved
1. `crates/ocean-surface-ui/src/app.rs` (lines 271-374, 780-832, 1014-1681) - top-level Tauri gates, overlay policy, shortcuts, composer, utility controls, and mount points.
2. `crates/ocean-surface-ui/src/island.rs` (lines 1-260, 830-1215) - Dynamic Island session search/activity/permission/stop UI and selectors.
3. `crates/ocean-surface-ui/src/voice/mod.rs` (lines 350-640, 740-1050) - voice orb/menu, mic capture modes, STT upload, and user-facing state.
4. `crates/ocean-surface-ui/src/voice/realtime.rs` (lines 450-610) - realtime mic acquisition and daemon realtime-call request.
5. `crates/ocean-surface-ui/src/call.rs` (lines 1-130, 350-504, 500-700) - call SSE reducer, lifecycle, transcript, summary, tasks, wake/barge-in UI.
6. `crates/ocean-surface-ui/src/place_call.rs` (lines 1-360) - outbound dialer validation, POST, credential-blocked/error states, selectors.
7. `crates/ocean-surface-ui/src/sessions.rs` (lines 721-1055, 1114-1144) - session/project overlay and row selectors.
8. `crates/ocean-surface-ui/src/rooms.rs` (lines 1039-1452) - room browser/create/policy, stage/join/leave/agent/message controls.
9. `crates/ocean-surface-ui/src/palette.rs` (lines 300-465) - command filtering and keyboard/click interactions.
10. `crates/ocean-surface-ui/src/workspace.rs` (lines 658-867, 948-1047) - Tauri workspace tabs/tree/filter/file preview.
11. `crates/ocean-surface-ui/src/council.rs` (lines 123-290) - long-running council mutation and stage rendering.

## Key Code

### Prioritized real-WebView smoke matrix

| Pri | Surface / quickest smoke | Obvious selector + event | Expected | Requirements / side effects |
|---|---|---|---|---|
| P0 | Dynamic Island open/search/select | `.island-chip` click; `.island-search__input` input; `.island-result` click; Tauri `Cmd/Ctrl+P` | popover opens, filtering updates, result focuses daemon session and closes | Daemon/session catalog for meaningful rows; selection changes active session |
| P0 | Island activity expansion and safe open | `.island-attention__summary` click, `.island-attention__open` click | details disclose; open focuses owning session | Daemon request snapshot; session focus mutation only |
| P0 | Overlay exclusivity / Escape | open Island then Sessions/Rooms/Council/Palette; press `Escape` | latest surface wins; one Escape closes one top surface | Safe. Policy is centralized in `app.rs:284-374, 809-832`; test all entry paths, especially native menu |
| P0 | Composer basics | `.ocean-composer__input` input, form `.ocean-composer` submit; `.ocean-composer__send` click; `.ocean-composer__halt` click | prompt submits, empty ignored, send/halt visibility tracks turn | **POSTs real agent turn / halt**; daemon, selected/lazy-created session, model/provider credentials; can execute tools |
| P0 | Voice menu + Off (safe portion) | `[aria-label="voice settings"]` click; `.voice-menu [role=menuitemradio]` click Off; `[aria-label="voice input"]` click while Off | menu toggles; Off active; orb click opens menu; no recording | Safe only if staying Off |
| P0 | Dictate / push-to-talk lifecycle | choose row by `.voice-menu__item-label` text; pointerdown/up on `[aria-label="voice input"]` | recording → transcribing; Dictate inserts text into composer; PTT sends transcript according to callback | **Mic permission + `/api/stt` + daemon xAI key/network**; PTT can submit agent work. Use a harmless phrase and Dictate first |
| P0 | Hands-free exclusivity | choose Hands-free; inspect `.voice-live-chip`, orb classes; then choose Realtime/Off | open-mic indicator; only one mic owner; Off stops capture | **Continuous mic streaming/STT**, xAI/network; privacy-sensitive. Stop promptly |
| P0 | Dialer validation only | overflow `.ocean-more__item` text `Dial phone`; `.ocean-place-call__input` input; `.ocean-place-call__btn`; `.ocean-place-call__hide` | invalid hint and disabled Call; valid E.164 enables; hide closes | Validation/hide safe. **Do not click enabled Call** without explicit approval: real PSTN side effect |
| P1 | Realtime voice start/stop/error | first `.voice-menu [role=menuitem]` (label Live conversation); re-open and click to stop | Connecting/Live orb state, label changes to end action; errors reopen menu | **Mic + daemon realtime endpoint + LiveKit/xAI credentials/network**; may join/create live audio room |
| P1 | Live call rendering | `.ocean-call[aria-label="live call"]`, `.ocean-call__phase`, `.__transcript`, `.__summary`, `.__tasks`, `.__wake`, `.__barge` | hidden until `call_started`; interim revises; Ocean rows, summary/tasks/wake/phase update; hides on end | Needs actual or injected daemon `/v1/events` call frames. Real call is unsafe/cost-bearing |
| P1 | Sessions/project grouping | web header trigger (not Tauri) or palette `Toggle Sessions`; `.sessions-item` click; group head toggle; close/scrim | project-first groups, focus selection, collapse/close | Daemon required; selecting changes active session. Creating/adding project mutates filesystem/catalog |
| P1 | Workspace (Tauri-only) | `.ocean-workspace-toggle`; `.workspace-tab`; `.workspace-tree-filter__input`; `.workspace-tree-row--dir/file`; resize pointer drag | collapse persists, filter/tree expand, file preview/tab/close, resize | Native host/daemon filesystem reads; generally safe, avoid sensitive files |
| P1 | Palette | `Cmd/Ctrl+K`; `.palette-input` input/ArrowUp/Down/Enter/Escape; `.palette-row` click | filters, selection moves, Enter runs and closes, Escape closes only palette | Command-dependent; `New Session` is locally lazy/safe, Council/Rooms/project commands can mutate |
| P2 | Rooms browser/stage | overflow Rooms; `.rooms-panel__create-input`; `.rooms-item`; `.room-stage__back`; `.room-stage__join/leave`; `.rooms-composer` | browse/open/back; roster/transcript; mode replaces chat surface | Browsing safe; **create/join/leave/add-agent/message/policy POST/DELETE mutations**, LiveKit join may need token/network/mic |
| P2 | Council stage | overflow Council; close button; inspect empty/existing topics | opens modal exclusively, polls snapshot, closes | Viewing safe. **Do not submit `.ocean-council-convene`** casually: daemon awaits 45s+ multi-worker/provider deliberation, cost/network |

### Permission/stop caution
- `.island-attention__approve` and `.__deny` only render when the permission belongs to the focused session and a decision token is present (`island.rs:218-244, 1056-1134`). Approval can authorize tool execution; smoke only with a deliberately harmless fixture. Deny is lower risk but still changes the request.
- `.island-attention__stop` appears only for Running requests (`island.rs:246-252, 1078-1093`) and cancels real work. Test only against a disposable request.

## Architecture
`app.rs` owns shared UI state and mounts Tauri-only Island/workspace versus common composer, voice, call, rooms, palette, and council surfaces. The Island derives sessions plus global request/permission snapshots from `Daemon`. Composer and most collaboration actions call daemon HTTP APIs. Classic voice records browser `MediaStream`, uploads to `/api/stt`, and optionally speaks replies; realtime voice separately owns the mic and daemon/LiveKit setup. `CallPanel` independently subscribes to named `call_*` SSE events on `/v1/events`; `PlaceCallControl` only initiates `POST /v1/calls/place`, after which SSE drives the panel.

## Likely static bugs / review findings
1. **medium:** `crates/ocean-surface-ui/src/call.rs:381-397, 609-630` - barge-in sets `CallPhase::Interrupted` but there is no timer/state transition back to Listening. If no subsequent transcript/summary frame arrives, the UI can remain permanently “Interrupted” despite comments calling it transient.
2. **medium:** `crates/ocean-surface-ui/src/call.rs:596-644` - wake/summary animations claim counters “re-fire” animation, but the rendered elements retain the same DOM identity and class; merely reading a signal/re-rendering does not reliably restart a CSS animation. Repeated wake/summary events may not visibly pulse after the first.
3. **low:** `crates/ocean-surface-ui/src/call.rs:448-495` - call SSE decode and subscription errors are silently discarded. A wire drift or failed named subscription yields an invisible/stale call panel with no user status or diagnostics.
4. **low:** `crates/ocean-surface-ui/src/palette.rs:382-398` - palette input has no explicit `aria-label` or dialog semantics; placeholder is the only accessible name visible statically.

## Start Here
Open `crates/ocean-surface-ui/src/app.rs:271-374` first: it defines platform gates and the overlay exclusivity contract, then follow the component mount points at `app.rs:1036-1681`.

## Residual Risks
- This was static, read-only inspection; no real Tauri WebView, daemon, microphone, SSE call, or credentialed network behavior was executed.
- Native menu/deep-link entry paths could bypass assumptions despite the reactive exclusivity guard; test them in the packaged Tauri shell.
- Real voice/call behavior depends on sibling `ocean-os` wire contracts and credentials not inspected here.