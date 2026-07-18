# Ocean Dynamic Island — Interaction and Runtime Contract

> Status: active implementation contract, superseding the combined
> Activity-plus-session-list popover explored in the first prototype.
>
> Companion delivery plan:
> [`OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md`](OCEAN_DYNAMIC_ISLAND_BUILD_PLAN.md).

## Product correction

The first prototype proved titlebar placement, daemon snapshots, session focus,
keyboard routing, and native geometry. It did **not** yet prove the product.
Opening the capsule produced a conventional popover containing a search field,
an Activity accordion, and a long session list. That reads as a list with
drawers, not a dynamic agent object.

The Island now has three distinct jobs. They share one titlebar object but never
render as sections in one combined dashboard:

1. **Agent** — direct interaction with the focused agent or the single work item
   currently needing attention.
2. **Sessions** — metadata browse and fuzzy focus switching.
3. **Recall** — daemon-owned search inside prior user and assistant turns.

At most one expanded mode exists in the DOM at a time.

## Non-negotiable principles

### One object that changes shape

The expanded surface begins behind the compact capsule and grows from its titlebar
position. It is not a detached centered modal.

- no dimmed full-window scrim;
- outside click collapses the object;
- Escape collapses it and restores invoking focus;
- per-mode width and height are bounded;
- reduced motion removes interpolation without changing state;
- the compact capsule remains the visual anchor while expanded.

### One intent per mode

The Agent mode never appends a session catalogue. Sessions never prepend
Activity. Recall never returns session metadata rows or commands. A mode may link
to another mode, but changing modes replaces the current content.

### Agent work is an object, not an accordion

A running request, permission, failure, or cancellation is presented directly as
one current work object. Its status, context, and available action are facets of
that object. The primary action is never hidden behind a disclosure drawer.

When several authoritative work items exist, the Island shows one at a time with
a bounded position/stepper control. It does not render an Activity feed.

### Daemon truth only

Surface code may project and render daemon state. It does not infer background
work from timestamps, execute tools, hold provider credentials, index transcript
history, or weaken permission authority.

The binding ownership remains:

```text
Surface intent
  -> daemon HTTP/SSE authority
  -> session/runtime/permission/tool authority
```

## State model

```rust
pub enum IslandMode {
    Closed,
    Agent,
    Sessions,
    Recall,
}
```

`app.rs` owns the mode because global shortcuts, native commands, sibling
overlays, and Escape ordering are application concerns. The Island owns each
mode's local query, selection, and focus state.

Transitions:

```text
Closed -- click capsule ----------> Agent
Closed -- Cmd/Ctrl+P -------------> Sessions
Closed -- Cmd/Ctrl+Shift+F -------> Recall
Agent -- Sessions ----------------> Sessions
Agent -- Recall ------------------> Recall
Any expanded mode -- Escape ------> Closed
Any expanded mode -- outside click > Closed
Any mode -- Cmd/Ctrl+K ------------> Closed, then command palette
```

Changing between Agent, Sessions, and Recall replaces content in place. It does
not close and spawn a different modal.

## Compact state

The quiet capsule contains:

```text
[state mark] focused session [optional project] · Ready
```

`Ready` is not a recent-session count. Recent sessions belong to the Sessions
mode, not the living agent state.

Authoritative compact states:

- one or more permissions/waits: `N need you`;
- otherwise one or more running requests: `N running`;
- otherwise: `Ready`.

Clicking the capsule opens Agent mode. `aria-label` names the focused session and
current authoritative state.

## Agent mode

### Purpose

Answer: **What is Ocean doing, what does it need, and what can I do next?**

### Idle

When there is no authoritative active work item:

- show focused session identity;
- show a short ready/status line;
- show one compact `Ask Ocean…` field;
- Enter submits through `Daemon::send_prompt` to the focused/lazily-created
  session and collapses the Island.

The full composer remains the rich authoring surface. The Island field is a
fast steering path, not a second transcript.

### Active work

Projection inputs:

- `GET /v1/requests`;
- `GET /v1/permissions`;
- `GET /v1/agent/sessions` for display identity only.

Ordering:

```text
NeedsHuman -> Failed -> Cancelling -> Running
```

Permissions replace their matching waiting request. Completed and cancelled
requests are omitted. Errored history remains bounded to the newest three.

One selected work object shows:

- authoritative state;
- owning session;
- concise bounded detail;
- project/tool/time metadata when present;
- direct, currently-authorized actions.

### Permission action boundary

Approve/Deny appears only when all are true:

1. the global permission snapshot identifies the request;
2. the permission belongs to the focused session;
3. the focused SSE stream contains the same pending permission;
4. this Surface owns the active decision token.

Otherwise the object is read-only and may offer `Open session`.

### Cancellation boundary

Stop appears only for daemon state `running`. While the POST is in flight it
reads `Stopping…`; lifecycle state still comes from the next daemon snapshot.

### Background reply

Background inline reply is deferred until the daemon provides a request-scoped
turn contract whose SSE, decision token, cancellation, and session focus cannot
blend with the focused transcript. The Surface must not fake this by switching
sessions or reusing focused turn state invisibly.

## Sessions mode

### Purpose

Answer: **Which session should own the center transcript?**

Entry points:

- `Cmd/Ctrl+P` in Tauri;
- `Switch Session…` in the command registry;
- `Sessions` from Agent mode.

The mode contains only:

- a metadata search field;
- up to eight initial sessions;
- up to twenty filtered matches;
- title/project/path/branch/recency/turn metadata.

The local fuzzy scorer may search existing session catalogue metadata. Empty
query preserves focused-first derivation order. Arrow keys move selection;
Enter or click calls the existing `Daemon::switch_session(id, title)` and
collapses the Island.

The full Sessions panel remains the deep project-management browser. The Island
switcher does not duplicate project creation, deletion, grouping management, or
unbounded history.

## Recall mode

### Purpose

Answer: **Where did we discuss, decide, or change this?**

Entry points:

- `Cmd/Ctrl+Shift+F` in Tauri;
- `Recall History…` in the command registry;
- `Recall` from Agent mode.

Recall searches transcript content, not session metadata.

### Daemon contract

```http
GET /v1/agent/history/search?q=<query>&limit=<n>
```

- default limit: 20;
- clamp: 1 through 50;
- persisted display transcript text only;
- user and assistant roles only;
- no raw provider messages or tool payloads;
- no provider call or embedding performed by the Surface.

Response:

```json
{
  "ok": true,
  "query": "permission mode",
  "hits": [
    {
      "hit_id": "stable-id",
      "session_id": "session-id",
      "session_title": "title",
      "role": "assistant",
      "excerpt": "bounded matching excerpt",
      "timestamp_ms": 0,
      "workspace_root": "/workspace",
      "score": 1.0,
      "match_kind": "exact"
    }
  ],
  "error": null
}
```

Initial ranking is truthful fuzzy/lexical recall:

```text
exact phrase -> all-token lexical -> deterministic fuzzy subsequence
```

`match_kind` is `exact`, `lexical`, or `fuzzy`. Semantic ranking may later be
fused by daemon/Bedrock authority and must be labeled honestly when added.

Each result renders a bounded excerpt plus session, role, workspace, and match
provenance. Enter or click opens the source session. Exact turn scrolling waits
for a stable daemon-provided turn/message anchor.

A generation guard prevents a slow older query replacing a newer result set.

## Keyboard contract

| Key | Behavior |
| --- | --- |
| `Cmd/Ctrl+P` | Tauri-only Sessions mode |
| `Cmd/Ctrl+Shift+F` | Tauri-only Recall mode |
| `Cmd/Ctrl+K` | close Island, open command palette |
| `Escape` | collapse current mode and restore invoking focus |
| `ArrowUp/Down` | Sessions/Recall selection |
| `Enter` | switch/open selected Sessions/Recall result |
| `ArrowLeft/Right` | previous/next Agent work object |
| `Enter` in idle Agent field | submit focused-session prompt |

IME composition bypasses all global and local shortcut handling.

## Host contract

- The compact/expanded Island mounts only in Tauri.
- Browser PWA and extension retain their current Sessions entry points.
- Tauri-only shortcuts must not intercept browser Print or Find off-Tauri.
- `src/host.rs` remains the only platform capability seam.
- Shared `styles/island.css` selectors stay inert when the Island is absent.

## Geometry

- compact capsule remains centered clear of traffic lights and right controls;
- compact width is capped below every expanded mode, so opening always grows the object rather than shrinking it;
- while expanded, the compact control becomes the stage's transparent top rail instead of remaining a raised pill over a second surface;
- Agent mode is the shallowest surface (target maximum about 330px);
- Sessions and Recall are independently scroll-bounded;
- minimum supported width remains 720px;
- workspace-open geometry centers over the remaining transcript column;
- nested stage/content radii use concentric spacing:
  `inner radius = outer radius - padding`.

## Explicit non-goals for this slice

- full transcript inside the Island;
- mixed result lists;
- Activity feed or accordion;
- project management;
- raw tool arguments or logs;
- background prompt submission without a daemon contract;
- client-side transcript fan-out/indexing;
- client-side embeddings/provider calls;
- fabricated semantic labeling;
- exact turn jump without stable anchors.

## Acceptance

1. Click the compact Island: Agent mode opens with no session list.
2. Running/needs-human state renders as one direct work object, not a drawer.
3. Idle Agent mode can submit a focused-session prompt.
4. `Cmd/Ctrl+P` opens Sessions only; initial results are bounded.
5. `Cmd/Ctrl+Shift+F` opens Recall only and searches transcript content.
6. `Cmd/Ctrl+K`, Escape, and outside click close exactly one topmost surface.
7. Wide and 720px screenshots read as one object changing shape.
8. Permission and cancellation authority tests remain green.
9. Web/extension behavior and browser Print/Find remain unchanged.
10. Rust tests, WASM check, Tauri check, and extension packaging pass.
