# Ocean GPUI Surface — Deep State Analysis

H3 2026-07-03 | Read-only exploration wave | `crates/ocean-gui/`

---

## 1. Architecture Map

```
bin/ocean_gui.rs (production GPUI app entry)
  └─ main.rs: OceanGuiApp { workspace: Workspace, ocean: Entity<OceanGuiShell> }
       ├─ workspace.rs: Obsidian-vault workspace (files, tabs, search, outline)
       │    └─ model.rs: ShellState — pure-data markdown editor state machine (2.3K LOC)
       └─ lib.rs / app.rs: GPUI application scaffold
            └─ view.rs: OceanGuiShell — THE monolith (~11K LOC)
                 ├─ 14 background message pumps (spawn_*_task functions)
                 ├─ Agent transcript rendering (streaming SSE → AgentTurn blocks)
                 ├─ Markdown editor with rich text shaping, cursor, line numbers
                 ├─ Canvas pane (native GPUI OceanCanvasView OR tldraw wry webview)
                 ├─ LiveKit presence panel (voice/video roster, local mic/camera)
                 ├─ Persistent rooms panel (OCEAN-109)
                 ├─ Permission approval cards
                 ├─ Session picker, model picker, project picker
                 ├─ Command palette (fuzzy search)
                 └─ ~1,400 LOC of tests (~500 lines)
```

**Key architectural decisions:**

- **Monolithic entity:** `OceanGuiShell` owns ALL runtime state (~40 fields) as one GPUI entity. Every interaction routes through it. This is intentional for the prototype but will grow unwieldy.
- **Background message pumps:** 14 `spawn_*_task` free functions bridge blocking daemon HTTP → main-thread GPUI updates via `mpsc` channels. Pattern is consistent: spawn a thread, send results over channel, GPUI task receives on main thread.
- **Three rendering surfaces for agent transcript:** (a) Native GPUI `EditorSurfaceElement` (~1,300 LOC, markdown parsing + text shaping), (b) Legacy `SurfaceLedger` for tldraw projection (demoted in OCEAN-168), (c) Native `OceanCanvasView` for the authoritative canvas ledger.
- **Canvas split personality:** The `SurfaceRenderTarget` enum (`view.rs:8413-8418`) switches between `Native` (GPUI canvas) and `Tldraw` (wry webview). Default is native.

---

## 2. Real vs Stubbed — Per Subsystem

| Subsystem | Status | Evidence (file:line) |
|---|---|---|
| **Agent transcript (SSE)** | **REAL, working** | `view.rs:5109-5156`: `connect_agent_events()` opens session-scoped SSE, `apply_agent_event()` (L5335) filters by `session_id`. Full streaming markdown rendering with turn blocks, tool calls, statuses. Tests at L9366-10798. |
| **Turn submission** | **REAL, working** | `view.rs:4988-5076`: Creates session if none exists (`POST /v1/agent/sessions`), then `POST /v1/agent/turns` with `client_type: "surface-gpui"`. Correct session scoping. |
| **Daemon client** | **REAL, complete** | `daemon.rs:582-923`: Full `reqwest::blocking` HTTP client. 25+ endpoints: health, models, projects, sessions, turns, events (SSE), permissions, component events, LiveKit tokens, rooms CRUD. `#[cfg(test)]` at L1247-1883 covers SSE parsing edge cases. |
| **Native canvas ledger** | **REAL, well-tested** | `canvas/` module: 9 files, ~5K LOC total. `CanvasLedger` with component/edge state, collision avoidance, revision bumping, patch log. `patch.rs` has 14 convergent-merge property tests. `persistence.rs` has crash-safe write ordering + 455 LOC of tests. `templates.rs` has 17 tests including 2 E2E ledger-integration tests. `hit_test.rs` is GPUI-free and headlessly testable. |
| **Native canvas renderer** | **REAL, working** | `canvas/render.rs: OceanCanvasView` — full GPUI element drawing components, edges (Bezier curves), viewport pan/zoom, grid, selection handles, multi-canvas tabs (OCEAN-279), presence markers (OCEAN-280). Component drag, resize, keyboard nudging, template content rendering (storyboard frames, tally rows, proposal cards). |
| **Canvas persistence** | **REAL, working** | `canvas/persistence.rs`: `CanvasStore` with `~/.ocean/canvases/{session}/{canvas_id}/` layout. Snapshot + patch-log rotation, crash-safe write ordering, symlink hardening. |
| **Canvas ledger set (multi-canvas)** | **REAL, working** | `canvas/ledger_set.rs`: Two-pin LRU eviction, multi-canvas switching (OCEAN-257/278/380). 32 canvases max. |
| **tldraw adapter** | **WRITTEN, UNWIRED** | `tldraw_adapter.rs:34`: `#![allow(dead_code)]`. `ledger_to_tldraw_commands` only called in tests (L271). `import_snapshot_into_ledger` only called in tests (L375). The adapter code is correct but nothing in `view.rs` calls it — the projection/import seam exists in code but has zero runtime activation. |
| **Canvas-web (tldraw bundle)** | **REAL, PARTIALLY WIRED** | `canvas-web/`: React 19 + tldraw 5.0.1 + Vite. Dual modes: `@tldraw/sync` for multiplayer (sync_uri present) or standalone local. `oceanBridge.ts` has IPC bridge: `postMessage` outbound, `window.oceanSurfaceApplyCommand` inbound. Commands supported: `load_canvas`, `upsert_component`, `focus_component`. Events emitted: `canvas_ready`, `ledger_snapshot`, `selection_changed`, `canvas_error`. |
| **Canvas-web IPC (Rust host)** | **WRITTEN, WIRED for host→webview only** | `surface_host.rs:222`: `with_ipc_handler` receives raw JSON events from webview. `view.rs:4735-4747`: `push_event_json` receives them, `pop_event` drains them. But the processing only handles `CanvasError` (L4749) — `LedgerSnapshot` and `SelectionChanged` events are received but never acted upon in the view. The `sync_command` path (host→webview) is fully wired for `LoadCanvas` and `UpsertComponent`. |
| **LiveKit real session** | **REAL, feature-gated** | `surface_livekit_session.rs`: Full `livekit` 0.7 SDK integration behind `#[cfg(feature = "livekit")]`. Room join/publish mic/publish camera (OCEAN-97), decode remote video tracks to BGRA frames, roster building with mic/camera/speaking flags, disconnect handling. ~280 LOC of session logic. |
| **LiveKit microphone** | **REAL, working** | `surface_livekit_session.rs:468-531`: `reconcile_microphone` creates `LocalAudioTrack` from platform mic, publishes/unpublishes. Uses real `PlatformAudio` device enumeration. |
| **LiveKit camera (outbound)** | **REAL skeleton, no frames** | `surface_livekit_session.rs:554-614`: `reconcile_camera` creates `NativeVideoSource` + `LocalVideoTrack` + publishes with `TrackSource::Camera`. But `_source` is held with no capture loop feeding it — OCEAN-97 gap. Remote peers see the publication but a black tile. |
| **LiveKit remote video (inbound)** | **REAL, working** | `surface_livekit_video.rs`: I420→BGRA decode. `surface_livekit_session.rs:353-423`: per-track video decode tasks that spawn/abort on TrackSubscribed/TrackUnsubscribed. BGRA frames flow to GPUI shell via `SurfaceLiveKitClientEvent::RemoteVideoFrame`. |
| **LiveKit default build (no feature)** | **STUB, intentional** | `surface_livekit_client.rs:278-295`: `spawn_surface_livekit_client` returns `Failed` event with "voice not built in" status. `surface_livekit_session.rs` is NOT compiled. Default `cargo build -p ocean-gui` stays free of native WebRTC. |
| **LiveKit presence metadata** | **REAL, working** | `surface_livekit.rs:422-441`: `SurfaceRoomMetadata` and `SurfaceCanvasRoomMetadata` carry compact pointers (session, active canvas, panes, mode). Published via LiveKit room metadata and participant attributes. Receive side parses metadata to build roster with `active_canvas_id` for per-canvas presence scoping. |
| **Persistent rooms** | **REAL, complete** | `rooms.rs` + `daemon.rs:317-518`: Full rooms state machine (OCEAN-109) with room list, open room, transcript polling (2.5s interval), composer, trigger policy (on_message/on_schedule/on_component_event), agent roster management. `view.rs` renders full rooms panel. |
| **Permissions** | **REAL, working** | `daemon.rs:200-231`: `ControlEvent` enum with `PermissionRequest`/`PermissionRevoked` from global `/v1/events` stream. `view.rs:5207-5253`: `apply_control_event` filters by active `session_id`. Approval/deny via `POST /v1/permissions/{permission_id}/decision`. |
| **Command palette** | **REAL, working** | `view.rs:8230-8310`: `CommandPaletteState` with fuzzy search over 40+ commands. |
| **Vault file watcher** | **REAL, working** | `watcher.rs` + `view.rs:8815-8839`: Filesystem watch task with 160ms poll, 128-event batch limit. |
| **Theme/design** | **REAL, minimal** | `theme.rs`: CSS-like constants for colors, spacing, typography. Dark palette. Functional but not polished — see WebDesignCritic's report for UI quality. |
| **Canvas multiplayer sync** | **NOT IMPLEMENTED** | The canonical native canvas has NO multiplayer path. Human edits (LedgerSink, view.rs:377-384) land in a LOCAL `Arc<Mutex<Option<CanvasLedger>>>` cell. They are NEVER posted to daemon, NEVER broadcast to peers. Agent patches are one-way daemon→client. LiveKit carries compact metadata pointers but NOT canvas CRDT state. The `@tldraw/sync` path exists only in the demoted tldraw projection pane — disconnected from the authoritative native ledger. Two humans CANNOT converge on the canonical canvas. |

---

## 3. Canvas + LiveKit Integration Maturity

### The Collaboration Bet — Verdict: NOT YET REALIZED

The design spec (OCEAN_GPUI_CANVAS_LIVEKIT_SPEC.md) defines three planes:
1. **Canvas Plane** (tldraw multiplayer sync) — NOT connected to authoritative canvas
2. **Transport + Presence Plane** (LiveKit) — voice/video/roster IS real, but carries no canvas state
3. **Reasoning Plane** (daemon) — fully functional, one-way agent→client for canvas patches

**What works:**
- LiveKit voice/video presence: real room join, real mic, real remote video decode, real roster with mic/cam/speaking flags. Camera outbound has the publish skeleton but lacks frame capture (OCEAN-97 — needs AVFoundation/nokhwa bridge).
- Agent→canvas one-way rendering: daemon `surface_patch` events flow into `CanvasLedger.apply_remote_patch()` and the native renderer draws them. Works end-to-end.
- Presence scoping: collaborators are filtered to the active canvas via `presence_on_canvas()` (surface_livekit.rs:105-117). Presence markers render on the native canvas (view.rs:8575-8611).

**What's missing (the critical gaps):**
1. **No human→human sync on the authoritative canvas.** The native canvas ledger is local-only. There is no daemon endpoint to post surface patches, no broadcast from `LedgerSink`, no peer-to-peer CRDT. The whole "two humans converge on a shared canvas" story is unrealized.
2. **Canvas state never flows through LiveKit data channels.** The spec says "RPC/data messages" but the LiveKit room metadata carries only compact pointers (active canvas ID, mode), never the canvas document.
3. **@tldraw/sync is on the wrong canvas.** The tldraw webview CAN sync via `@tldraw/sync` when a `sync_uri` is configured, but this is the DEMOTED projection pane. The authoritative native ledger is disconnected from it. The tldraw adapter exists in code (`tldraw_adapter.rs`) but is only called in tests — never wired into the view's render/paint loop.
4. **Human edits stay local.** The `LedgerSink` closure at `view.rs:377-384` writes to an in-memory mutex. No HTTP, no WebSocket, no broadcast. A human's drag/resize on the native canvas affects only their own process.
5. **No collaboration auth/token flow.** The spec's Slice 9 item ("Add real collaboration auth/token flow through the daemon/proxy") is untouched.

### Maturity Scorecard

| Component | Maturity | Notes |
|---|---|---|
| Native CanvasLedger | ★★★★☆ | Excellent data model, well-tested. Missing only multiplayer plumbing. |
| Native Canvas Renderer | ★★★★☆ | Full rendering, interaction, templates. GPUI-free testable core. |
| LiveKit Voice | ★★★★★ | Real, working, production-ready. |
| LiveKit Video Inbound | ★★★★☆ | Real decode pipeline. Working. |
| LiveKit Video Outbound | ★★★☆☆ | Skeleton exists. Needs capture loop. OCEAN-97. |
| LiveKit Presence Roster | ★★★★☆ | Real, working, canvas-scoped. |
| Canvas Multiplayer Sync | ★☆☆☆☆ | Not implemented. The central product bet is unrealized. |
| tldraw Adapter | ★★☆☆☆ | Written, tested, zero runtime activation. |
| Canvas-Web IPC | ★★☆☆☆ | Host→webview works. Webview→host events received but unhandled. |

---

## 4. Session-Contract Correctness

### Verdict: **PASS** with minor observations

**Correct behavior (evidence):**

1. **Session creation before first turn:** `view.rs:5044-5076` calls `POST /v1/agent/sessions` when no active session exists, gets back a `session_id`, attaches it to the `AgentTurnRequest`, then emits `SessionReady` which sets `agent.session_id`.

2. **Session-scoped agent events:** `view.rs:5109-5156`'s `connect_agent_events()` calls `agent_events_url(base_url, Some(session_id))` which adds `?session_id=<id>` to the SSE URL. `daemon.rs:936-951` confirms the query parameter.

3. **Events filtered by session:** `view.rs:5335-5343`'s `apply_agent_event()` drops events whose `session_id` doesn't match the active session.

4. **Control events filtered by session:** `view.rs:5207-5221`'s `apply_control_event()` drops permission requests/revocations from foreign sessions.

5. **No global-stream session adoption:** `view.rs:9226-9237`'s `gui_command_for_agent_event()` maps `SessionCreated`/`TurnStarted` to `OpenSession`/`SwitchSession` — but these arrive on the SESSION-SCOPED agent events stream, not the global control stream. They only fire for the session that was explicitly connected.

6. **Client type stamped correctly:** `view.rs:5060` sends `client_type: Some(client_type)` where `client_type = "surface-gpui"`.

**Minor observations:**
- The control events stream (`/v1/events`) IS global — it's not session-scoped. But this is the DESIGN: permissions span sessions, so a global listen + client-side filter is correct. The spec says "Never open a product transcript on the global stream" — the product transcript IS on the session-scoped stream, permissions are on the global stream. This is correct separation.
- Room transcript polling (view.rs:76-81) is explicit about being unscoped — the room trigger frame is council-wide and doesn't reach session-scoped streams, so the panel polls separately. This is correct.
- `switch_agent_session` (view.rs:6424-6467) reconnects the agent events stream with the new session ID, preventing stale-session bleed. Correct.

---

## 5. Ranked Focus Areas

### Priority 1: Multiplayer Canvas Sync (the collaboration bet)
**What's wrong:** The native canvas ledger is local-only. Human interactions write to an in-memory mutex, never posted to daemon, never broadcast to peers. There is zero path for two humans to converge on the authoritative canvas. The design spec's central product thesis ("humans and agents share a live canvas") is not realized.

**Why it matters:** This is the headline feature the entire `crates/ocean-gui/` crate exists to deliver. Without it, the app is a fancy local editor with agent turn streaming — not a collaboration cockpit.

**Rough scope:** Large. Needs:
- A daemon endpoint for surface patch posting (new `POST /v1/surface/patches` or similar)
- A daemon broadcast path (WebSocket or SSE push of remote patches to connected surfaces)
- OR a direct CRDT path between clients (via LiveKit data channels or a dedicated sync server)
- The LedgerSink must post, not just write locally
- Remote patches must arrive and apply without stomping local interactions
- This likely touches: `view.rs` (LedgerSink closure + new message pump), `daemon.rs` (new wire types), the daemon repo (new endpoint), and potentially a sync protocol layer.

**Leverage:** Maximum. Unblocks everything else: tldraw projection, collaboration presence, agent-in-the-loop canvas work.

### Priority 2: Wire the tldraw Adapter Into the View
**What's wrong:** `tldraw_adapter.rs` has correct, tested translation functions (`ledger_to_tldraw_commands`, `import_snapshot_into_ledger`) but `view.rs` never calls them. The entire module is `#![allow(dead_code)]`. The "toggle to tldraw projection" toggle exists in the UI but doesn't actually project the ledger.

**Why it matters:** The spec envisions tldraw as the human freehand sketching surface. Without the adapter wired, toggling to tldraw shows an empty canvas, and sketches made there never land in the authoritative ledger. This is the near-term path to "human doodles become agent context."

**Rough scope:** Medium. Wire `ledger_to_tldraw_commands` into the tldraw toggle handler. Handle `ledger_snapshot` inbound events and feed through `import_snapshot_into_ledger`. Handle `selection_changed` events. This is ~200-400 LOC of view.rs glue plus event wiring.

**Leverage:** High. Makes the tldraw pane functional. Enables the freehand→agent feedback loop.

### Priority 3: Camera Frame Capture (OCEAN-97)
**What's wrong:** LiveKit camera outbound publishes a `NativeVideoSource` with no frames fed to it. Remote peers see the camera publication (presence flag flips) but a black tile. The `_source` field is held exactly for a future capture loop but there is none.

**Why it matters:** Video presence is table stakes for a collaboration cockpit. The skeleton wired shows it's low-risk to complete — the publish path, presence metadata, and remote decode all work. Only the capture source is missing.

**Rough scope:** Small. Add a macOS AVFoundation capture loop via `nokhwa` or direct `CMSampleBuffer` bridge. Feed frames into `NativeVideoSource::capture_frame()`. All infrastructure downstream already works.

**Leverage:** High for effort ratio. Small code change, big product impact.

### Priority 4: Webview→Host IPC Event Handling
**What's wrong:** `view.rs:4735-4747` receives IPC events from the canvas webview (`push_event_json` + `pop_event` drain loop) but only handles `CanvasError`. `LedgerSnapshot` and `SelectionChanged` events are decoded but discarded — they reach `surface_host.rs` tests but are never dispatched to adapter functions in the view.

**Why it matters:** This is the feedback path for human interactions in the tldraw pane. Without it, sketching in tldraw is a dead end — shapes never reach the native ledger. Blocks Priority 2's import direction.

**Rough scope:** Small. Add match arms for `LedgerSnapshot` → `import_snapshot_into_ledger` and `SelectionChanged` → update selection state. ~50-100 LOC.

**Leverage:** Medium-high. Unblocks the human→agent sketch feedback loop.

### Priority 5: Collaboration Auth/Token Flow (Spec Slice 9)
**What's wrong:** The spec's final near-term slice ("Add real collaboration auth/token flow through the daemon/proxy") is untouched. LiveKit tokens are fetched from the daemon (`daemon.rs:984-990`) but there's no auth envelope around collaboration sessions.

**Why it matters:** Anyone who can reach the daemon can join a LiveKit room. For a product shipping multi-user collaboration, this is a blocker.

**Rough scope:** Medium. Needs daemon-side auth checks, token scoping, surface identity verification. Likely a cross-repo effort (daemon + GPUI surface).

**Leverage:** Medium. Blocked on Priority 1 (need multiplayer before auth matters).

### Priority 6: SurfaceHost Canvas IPC Wiring Cleanup
**What's wrong:** The `CanvasHostState.sync_command` (host→webview) path is wired for `LoadCanvas` and `UpsertComponent` but the host's outbound command queue (`pending: VecDeque<CanvasHostAction>`) is drained in the GPUI render loop — there's a tight coupling between frame rendering and webview command delivery that could cause missed frames or ordering issues.

**Why it matters:** Cosmetic for now but will cause visual glitches when the adapter is wired (Priority 2).

**Rough scope:** Small. Review the drain lifecycle, ensure commands aren't dropped when the webview pane is hidden.

**Leverage:** Low (pre-requisite cleanup).

### Priority 7: Vault/Editor Polish — Feature Parity With Web Surface
**What's wrong:** The markdown editor is functional but basic. No syntax highlighting beyond markdown runs (bold/italic/code), no LSP integration, no collaborative editing. The web surface has richer editing via Monaco/CodeMirror (in `ocean-surface-ui`).

**Why it matters:** The workspace is the "home base" for the operator. If it feels worse than the web surface, operators won't use the native app. Already flagged by user: "sloppy vibe-coded styling."

**Rough scope:** Large if full parity. Incremental if focused: theme polish, better typography, syntax highlighting.

**Leverage:** Medium. Product quality, not a blocker.

---

## Appendix: File Size Summary

| File | Lines | Role |
|---|---|---|
| `view.rs` | ~10,800 | Monolithic app shell |
| `model.rs` | ~2,300 | Workspace state machine |
| `daemon.rs` | ~1,883 | HTTP client + wire types |
| `canvas/render.rs` | ~2,400 | Native canvas rendering |
| `canvas/patch.rs` | ~865 | Patch types + merge |
| `canvas/persistence.rs` | ~1,066 | Disk storage |
| `canvas/templates.rs` | ~989 | Template content shapes |
| `canvas/ledger.rs` | ~1,200 | Core ledger data model |
| `canvas/ledger_set.rs` | ~780 | Multi-canvas management |
| `surface_livekit.rs` | ~1,031 | LiveKit state + room metadata |
| `surface_livekit_session.rs` | ~784 | Real LiveKit SDK session |
| `surface_host.rs` | ~448 | wry webview host |
| `rooms.rs` | ~730 | Persistent rooms state |
| `agent.rs` | ~866 | Agent event types + state |
| `gui_control.rs` | ~610 | Region/component registry |
| `tldraw_adapter.rs` | ~392 | Ledger↔tldraw translation |
