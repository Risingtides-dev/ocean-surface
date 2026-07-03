# Ocean GPUI Canvas + LiveKit Spec

Status: active GPUI product direction.

This is the collaboration-surface spec for `crates/ocean-gui`. It is not the
old TUI room system, not a Papyrus note app, and not a wrapper around the web
chat. It is the native desktop space where humans and agents share a live
canvas, voice/video presence, and one Ocean session context.

## Product Shape

Ocean GUI is a desktop collaboration cockpit for remote work:

- humans join a shared working space,
- coworkers can optionally publish mic/camera,
- agents participate through Ocean daemon/Longhouse actions,
- the shared canvas persists visible working memory,
- turns can render cards, workflows, storyboards, proposals, maps, and other
  structured objects onto the canvas.

The chat transcript is table stakes. The canvas is the core product surface.

## Three Planes

Do not collapse these into one transport.

### 1. Canvas Plane

Use the native `CanvasLedger` for the multiplayer canvas.

- Humans manipulate the GPUI canvas directly.
- Agents render through structured canvas commands that become ledger patches.
- Convergence is owned by the OCEAN-270 merge state (`LamportClock`,
  `ComponentVersion`, `CanvasMergeState`), not by LiveKit or tldraw.
- Eligible `SurfacePatchEnvelope`s are encoded by
  `crates/ocean-gui/src/shell/canvas_sync.rs` and broadcast as reliable LiveKit
  data packets on topic `ocean.canvas.v1`.
- Late joiners request a targeted chunked snapshot, reassemble it, and merge the
  bulk state through `CanvasLedger::merge_snapshot`.
- tldraw remains the optional demoted sketch/import adapter from OCEAN-168, not
  the multiplayer source of truth.
- The canvas state is not stored as daemon-global chat state.

Current host path:

```text
GPUI native app
  -> native canvas renderer
  -> CanvasLedger
  -> canvas_sync.rs
  -> LiveKit reliable data packets
```

### 2. Transport + Presence Plane

Use LiveKit for realtime presence:

- audio,
- video,
- participant attributes,
- room metadata,
- RPC/data messages.

LiveKit's word "room" means the media/session container. It is not the old TUI
room concept. In product language, prefer "collaboration space", "hangout", or
"surface session" unless the code is specifically naming a LiveKit room.

The GPUI app should use the LiveKit Rust client SDK as a participant. The
Agents framework can remain a later Python/Node sidecar only if needed for
turn detection; the runtime/reasoning authority remains Ocean.

### 3. Reasoning Plane

Ocean daemon remains the authority:

```text
GPUI / web / extension / TUI
  -> ocean-daemon
  -> ocean-agent / ocean-runtime / providers / tools / Longhouse
```

Agents do not own the canvas transport. They receive surface context and emit
structured intent. The app applies that intent to the canvas ledger/tldraw doc.

## Session Model

Use the ecosystem contract:

```text
Project -> Workspace -> Session -> Turns -> Events
Surface -> Session
```

- A session is the reasoning root and transcript/event stream.
- A surface is a UI attached to a session.
- A canvas is a view/document attached to a session.
- A pane is UI layout only.

Do not bind agent memory to a pane. Multiple panes can show different canvases
or transcript views for the same session. Closing a pane must not kill the
session.

First-party surfaces must:

```text
POST /v1/agent/sessions
GET  /v1/agent/events?session_id=<id>
POST /v1/agent/turns { session_id, prompt, cwd, project_id?, client_type: "surface-gpui" }
```

The app may open the same session across GPUI, web, and extension by explicitly
attaching each surface to the same `session_id`. Different sessions must never
bleed events into each other.

## Canvas Ledger

The canvas needs a ledger so humans and agents share spatial memory.

Each visible component should have a durable record:

```json
{
  "id": "brief-1",
  "component_type": "brief_card",
  "x": 450,
  "y": 120,
  "width": 320,
  "height": 220,
  "content": {},
  "metadata": {},
  "connections": []
}
```

Ledger responsibilities:

- record what exists on the canvas,
- expose positions and sizes to the next turn,
- prevent agents from stomping existing work,
- support mode-specific layouts such as workflow builder, storyboard, campaign
  board, proposal review, or map planning,
- keep the canvas as persistent working memory.

The ledger should ride with the canvas/document state, not become a daemon
table. The GPUI app injects the relevant ledger summary into turn context.

## Agent Render Loop

Target loop:

```text
human speaks/types/clicks
  -> GPUI builds session + surface + canvas context
  -> POST /v1/agent/turns
  -> daemon streams AgentTurnEvents
  -> agent emits render commands / component events
  -> GPUI applies commands to the native canvas ledger
  -> all canvas participants converge through OCEAN-270 merge state plus LiveKit reliable data packets on `ocean.canvas.v1`; late joiners catch up through chunked snapshots
```

The agent should not emit arbitrary web code. It should emit trusted structured
commands such as:

```json
{
  "type": "canvas.render_component",
  "canvas_id": "main",
  "component_type": "proposal_card",
  "placement": { "strategy": "next_available", "near": "brief-1" },
  "content": {}
}
```

The app owns final rendering.

## Write Paths

All writes enter the native ledger first:

1. Human canvas gestures enter through the GPUI `LedgerSink`, become versioned
   `SurfacePatchEnvelope`s, and are broadcast over the LiveKit data channel when
   sync-eligible.
2. Agent render events arrive from the daemon SSE stream, apply to the same
   ledger, and are re-broadcast by the receiving surface so peers attached to
   other sessions still converge.
3. Remote LiveKit packets are decoded by `canvas_sync.rs` and applied to the
   named canvas ledger; full-state catch-up uses chunked snapshots merged by
   `CanvasLedger::merge_snapshot`.

The ledger rides with the canvas/document state and local persistence. The daemon
does not become a canvas relay, renderer, or table of CRDT state.

## Layout / Multiplexing

The layout goal is tmux-like UI plumbing:

- attach/detach canvases,
- split panes,
- show multiple canvases for one session,
- pop a canvas into another window later,
- keep the session as the root, not the pane.

Pane layout is orthogonal to the reasoning loop. The turn context should include
the full active surface topology, not just "the pane the user is in."

## Near-Term Slice Order

1. Keep session scoping correct: no global SSE, no session adoption.
2. Stabilize GPUI chrome and transcript rendering.
3. Stabilize the native GPUI canvas renderer and `CanvasLedger` write boundary.
4. Wire LiveKit data-channel patch broadcast/receive through `canvas_sync.rs`.
5. Add targeted chunked snapshots for late joiner convergence.
6. Feed ledger summary into `surface-gpui` turn context.
7. Add LiveKit join/presence controls with mic/camera toggles.
8. Use LiveKit metadata/attributes for compact surface/session presence.
9. Add real collaboration auth/token flow through the daemon/proxy.

## Hard Boundaries

- Do not implement this from old TUI room docs.
- Do not introduce "pods" or separate agent mesh concepts in the GUI.
- Do not store canvas state in LiveKit — data packets are a transient courier; the ledger + local persistence remain the source of truth.
- Do not make the daemon the canvas renderer.
- Do not rely on GPUI overlays above the webview.
- Do not let global SSE pick the active session for a surface.
