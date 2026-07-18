## Context handoff

### Current implementation

The titlebar Island is currently a Tauri-only mount, but its implementation combines all three requested concerns in one component:

- `crates/ocean-surface-ui/src/app.rs:273-274`
  - Global `IslandMode` and focus-request signals.
- `app.rs:344-354`
  - One `open_island` callback enforces overlay mutual exclusion.
- `app.rs:466-472`
  - Tauri-only “Search Ocean…” command opens `IslandMode::Search`.
- `app.rs:778-806`
  - Cmd/Ctrl+P is Tauri-only; browser/PWA Print remains untouched.
- `app.rs:1035-1041`
  - `<Island>` mounts only when `running_in_tauri()`.
- `app.rs:1043-1047`
  - Web and extension retain the existing Sessions modal.

`crates/ocean-surface-ui/src/island.rs` is presently a single large module:

- `island.rs:18-23`: `IslandMode::{Closed, Browse, Search}`.
- `island.rs:32-49`: session projection/search result types.
- `island.rs:51-99`: ambient attention types.
- `island.rs:147-237`: derives permission/request attention cards.
- `island.rs:318-383`: projects daemon sessions into Island rows.
- `island.rs:388-465`: fuzzy metadata search over sessions.
- `island.rs:502 onward`: one component owns:
  - ambient chip,
  - polling,
  - session search,
  - activity cards,
  - permission/cancel actions,
  - session switching,
  - focus trap and keyboard navigation.

This coupling is the primary implementation risk: query state, selection, attention disclosure, polling, focus restoration, browsing, and actions all share one reactive scope.

### Existing daemon-backed authority

Relevant DTOs and methods are in `crates/ocean-surface-ui/src/daemon.rs`:

- `daemon.rs:768-799`
  - `RequestSnapshot` from `GET /v1/requests`.
- `daemon.rs:802-823`
  - `PermissionSnapshot` from `GET /v1/permissions`.
- `daemon.rs:1343-1362`
  - session, request, permission, and cancellation signals.
- `daemon.rs:2639 onward`
  - `fetch_attention()` polls the two authoritative registries concurrently and preserves the last good snapshot on failure.
- `daemon.rs:2859 onward`
  - `halt()` and `cancel_request()` use `POST /v1/requests/{id}/cancel`.
- Following `cancel_request()`
  - `switch_session()` clears focused local presentation state, hydrates session detail, and reconnects a session-scoped SSE stream.

Important authority constraint:

- Cross-session permission snapshots are mostly read-only.
- Island approval/denial is enabled only when the permission belongs to the focused session, appears in `pending_permissions`, and the surface owns the active decision token (`island.rs:260-280`).
- Cross-session request cancellation is allowed through the daemon request ID.
- This distinction must survive refactoring; a snapshot’s existence does not grant mutation authority.

### Session UI patterns to reuse

`crates/ocean-surface-ui/src/sessions.rs` is the canonical project-first browsing implementation:

- `sessions.rs:1-14`: daemon `owning_project` first, path-based fallback, explicit Other bucket.
- `sessions.rs:24-65`: `session_root()` and longest component-boundary `project_for_root()`.
- `sessions.rs:132 onward`: `group_for_panel()` retains every daemon-returned session, including zero-turn drafts.
- `sessions.rs:430 onward`: origin, relative-time, and recency helpers.
- `SessionsPanel` remains the full web/extension browse and project-management surface.

The Island should not grow a second project/grouping policy. A safe migration would move shared pure session-catalog projection helpers into a neutral module rather than making Island depend indefinitely on presentation helpers from `sessions.rs`.

### Search and palette patterns

- `crates/ocean-surface-ui/src/search.rs:7`
  - shared deterministic `fuzzy_score()` for case-insensitive subsequence matching.
- `crates/ocean-surface-ui/src/palette.rs:10`
  - palette reuses that scorer.
- `palette.rs:49-73`
  - command registry is typed and host-gated.
- `palette.rs:110-138`
  - one dispatch path refuses disabled commands.
- `island.rs:388-465`
  - current “Search Ocean” only searches session metadata:
    title, project, cwd, branch, origin, focus/recency, and turn count.
  - It does **not** search transcript/history content and is not semantic search.

The scorer should remain a shared pure helper. Palette UI/state should not be reused for history results because commands, sessions, and transcript hits have different result/action semantics.

### Styling and architecture constraints

- `styles/island.css:1-2` explicitly says selectors are inert off Tauri.
- `styles/island.css:23-127`: ambient chip.
- `styles/island.css:129-194`: scrim, popover, and search input.
- `styles/island.css:195-429`: attention cards/actions.
- `styles/island.css:430-527`: session result list.
- `styles/island.css:552-626`: titlebar/split-view responsiveness and reduced motion.

The stylesheet is included in all bundles:

- `index.html:43-57`
- `extension/sidepanel.html:9-24`
- `scripts/build-extension.sh:20-28`

Therefore new selectors may remain shared and inert, but stylesheet renames/additions require updating all enumerations.

Binding architecture:

- `docs/OCEAN_PLATFORM_CONTRACT.md`
  - one Leptos core for all hosts;
  - `host.rs` is the only platform-capability seam;
  - sessions, agent work, search authority, permissions, and provider behavior belong to the daemon;
  - capability absence renders as absence.
- `docs/OCEAN_WEB_SURFACE_DESIGN.md`
  - conditional rendering over permanent chrome;
  - no control sprawl;
  - touch/focus support;
  - colors only in `styles/tokens.css`;
  - reduced-motion handling;
  - same bundle must preserve compact behavior.

## Recommended Rust state model

Keep long-lived daemon state in `Daemon`; add Island-local presentation state as explicit, concern-specific models:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandSurface {
    Closed,
    Activity,
    Sessions,
    History,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionBrowseState {
    pub query: String,
    pub selected: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistorySearchState {
    pub query: String,
    pub selected: usize,
    pub request_generation: u64,
    pub cursor: Option<String>,
    pub phase: SearchPhase,
    pub results: Vec<HistoryHit>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityViewState {
    pub expanded_item: Option<String>,
    pub reply_target: Option<ActivityTarget>,
    pub reply_draft: String,
    pub reply_phase: ReplyPhase,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IslandViewState {
    pub surface: IslandSurface,
    pub sessions: SessionBrowseState,
    pub history: HistorySearchState,
    pub activity: ActivityViewState,
}
```

Guidance:

- Ambient chip remains visible while `IslandSurface::Closed`; “Activity” is the expanded focused-agent/attention view.
- Do not encode daemon lifecycle as local Island state. `Running`, permission waits, cancellation, and failures continue to derive from daemon snapshots.
- Search transport phases should distinguish `Idle`, `Debouncing`, `Loading`, `Ready`, and `Failed`.
- Use a monotonically increasing search generation so stale async results cannot replace a newer query.
- Keep reply transport state keyed by target request/session, not a single global boolean.
- Preserve focus restoration separately as DOM/controller state, not domain state.

## Component boundaries

Suggested module layout:

```text
island/
  mod.rs                 # thin Island shell and surface routing
  state.rs               # pure enums/state transitions
  activity.rs            # focused/live agent projection and actions
  session_browser.rs     # session catalogue filtering and switch intent
  history_search.rs      # semantic/fuzzy history query and hit actions
  accessibility.rs       # focus loop/active-descendant helpers if warranted
```

Supporting extraction:

```text
session_catalog.rs       # session_root, project resolution, normalized metadata
search.rs                # fuzzy_score only, plus generic ranking helpers if needed
daemon.rs                # HTTP DTOs/signals/methods; remains authority adapter
```

Responsibilities:

1. **Ambient/activity**
   - Derive chip and attention cards from daemon snapshots.
   - Approve/deny only with existing focused decision-token authority.
   - Cancel through request IDs.
   - Open a related session through the existing switch path.
   - Inline reply only after a daemon-owned contract exists.

2. **Session browser**
   - Browse daemon-catalogued sessions.
   - Local fuzzy filtering is acceptable for catalogue metadata.
   - Enter/click switches via `Daemon::switch_session`.
   - No transcript search or activity action logic.

3. **History search**
   - Query daemon-owned historical content.
   - Render typed `HistoryHit` rows with session/turn/message identity and excerpt.
   - Selecting a hit switches/hydrates the owning session, then locates the turn/message if the detail DTO provides stable identifiers.
   - Local fuzzy scoring may rank cached/returned hits or provide an explicitly labeled metadata fallback; it must not be presented as semantic search.

4. **Shell/controller**
   - Own overlay exclusion, Escape, focus capture/restore, and surface transitions.
   - Avoid one global combobox spanning attention controls and heterogeneous result kinds.

## Daemon contract gaps

Repository inspection found no daemon endpoint for semantic transcript/history search.

A daemon-owned contract is needed, preferably:

```text
POST /v1/agent/history/search
{
  "query": "...",
  "limit": 20,
  "cursor": "...",
  "project_id": "...",
  "session_id": "...",
  "mode": "hybrid"
}
```

Suggested response:

```rust
struct HistorySearchResponse {
    ok: bool,
    hits: Vec<HistoryHit>,
    next_cursor: Option<String>,
}

struct HistoryHit {
    hit_id: String,
    session_id: String,
    session_title: String,
    turn_id: Option<String>,
    message_id: Option<String>,
    role: HistoryRole,
    excerpt: String,
    updated_at: String,
    project: Option<OwningProjectRef>,
    score: f32,
    match_kind: HistoryMatchKind, // lexical | semantic | hybrid
}
```

Contract requirements:

- Daemon performs indexing, embeddings/provider selection, ACL/project scoping, pagination, and ranking.
- Stable turn/message identifiers are required for navigation.
- Query and excerpt bounds must be specified.
- Empty-query semantics must be explicit.
- Errors must distinguish unsupported semantic search from transport failure.
- Surface must not create embeddings, hold provider keys, or scrape every session detail client-side.

### Inline reply gap

`POST /v1/agent/sessions/{id}/messages` exists in the proxy and is documented as a voice-agent handoff-note append. It should not be assumed to run an agent turn or provide interactive reply semantics.

The existing `POST /v1/agent/turns` can target a session, but the current Surface `Daemon` stores focused SSE, active decision token, active turn ID, and transcript state globally. Using it for a background inline reply would risk blending focused and background state.

Before adding inline reply, daemon and surface need one of:

- a dedicated session-scoped reply endpoint returning a request ID while execution remains observable through `/v1/requests`, or
- an explicitly documented background-safe use of `/v1/agent/turns`, including independent decision-token correlation and no requirement to make that session the focused SSE transcript.

Required semantics:

- target `session_id`;
- prompt/reply body and `client_type`;
- authoritative returned `request_id`;
- idempotency/double-submit behavior;
- permission-token ownership for that request;
- cancellation;
- whether replying changes session focus—it should not;
- how the resulting turn becomes discoverable without opening one SSE stream per background session.

Until resolved, activity cards should offer **Open session**, not a fake inline reply box.

## Migration strategy

1. Add pure state-transition and projection types while retaining current markup and behavior.
2. Extract shared session-catalog helpers from `sessions.rs`; keep both SessionsPanel and Island behavior identical.
3. Split attention derivation/actions into `activity.rs`; preserve polling and decision-token gates exactly.
4. Split session browsing and local fuzzy metadata filtering.
5. Replace `Browse/Search` ambiguity with explicit `Activity/Sessions/History`.
6. Keep app-level overlay mutual exclusion and Tauri-only shortcut/mount unchanged.
7. Add daemon history DTO/method only after the ocean-os endpoint is approved and implemented.
8. Add history UI behind capability/response availability; unsupported means clean absence or a lexical metadata fallback clearly identified as such.
9. Add inline reply only after background-turn authority and token correlation are defined.
10. Refactor CSS selectors in small, behavior-preserving groups; retain inert shared inclusion for web/extension.
11. Run detached-tree build validation before integration because `app.rs`, `main.rs`, and module declarations are shared-ground hazards.

## Focused tests

### Pure unit tests

- Surface transition matrix: Escape and opener behavior.
- Activity derivation:
  - permission replaces matching waiting request;
  - terminal successes/cancellations omitted;
  - failure history remains bounded;
  - permission actions require focused token ownership;
  - stop only appears for running requests.
- Session browser:
  - owning project wins;
  - longest path fallback;
  - Other bucket;
  - zero-turn drafts retained;
  - deterministic fuzzy ties and metadata matching.
- History:
  - DTO drift fixtures;
  - stale generation responses ignored;
  - pagination merge/deduplication;
  - semantic/lexical match-kind rendering;
  - hit navigation requires stable session/turn identity.
- Inline reply:
  - cannot submit without daemon-supported target;
  - double-submit suppression;
  - background reply does not mutate focused `session_id`, turns, SSE generation, or active decision token.

### Interaction/accessibility tests

- Chip opens Activity, not session browsing by accident.
- Cmd/Ctrl+P remains Tauri-only and opens the intended surface.
- Browser/PWA Print behavior remains unchanged.
- Cmd/Ctrl+K closes Island before palette opens.
- One Escape closes one topmost surface.
- Focus returns to prior control/chip.
- Activity disclosure native Enter/Space does not switch sessions.
- Session/history arrow navigation remains scoped to its own combobox.
- Tab remains trapped inside the active dialog.
- `aria-activedescendant` never references a removed result.

### Validation commands

```sh
cargo test -p ocean-surface-ui
cargo check -p ocean-surface-ui --target wasm32-unknown-unknown
cargo check -p ocean-surface-proxy
cd crates/ocean-tauri && cargo check
```

Also validate the extension build when stylesheet/module wiring changes:

```sh
./scripts/build-extension.sh
```

## Risks

- Background inline turns can corrupt focused transcript/token/permission state if routed through the current monolithic `Daemon` mutation path.
- Semantic search could accidentally move provider/index authority into WASM.
- Session detail fanout would be expensive and violates the intended daemon search boundary.
- Splitting files while concurrently editing `app.rs`/`main.rs` can leave main referencing uncommitted modules.
- Existing working tree is already heavily modified and includes untracked Island/search files; implementation must avoid overwriting unrelated lane work.
- Polling ownership can duplicate intervals if both shell and activity component poll.
- Separate result lists need separate keyboard/ARIA state; reusing one selected index causes invalid active descendants.
- Renaming or adding stylesheets requires synchronized index, extension HTML, and build-script changes.
- History-hit navigation is incomplete without stable turn/message IDs and a transcript scroll/highlight contract.

## Meta-prompt handoff

**Goal:** Refactor the Tauri titlebar Island so ambient agent activity, session browsing, and history search have independent typed state and components, while retaining daemon authority and unchanged web/extension behavior.

**Evidence:** Current coupling is concentrated in `island.rs:502 onward`; authoritative request/permission snapshots and actions live in `daemon.rs:768-823` and `2639 onward`; canonical project grouping lives in `sessions.rs`; fuzzy scoring is shared through `search.rs`; Tauri-only mounting/shortcut behavior is in `app.rs:344-354`, `778-806`, and `1035-1047`.

**Hard constraints:** Do not move provider, indexing, session, permission, or execution authority into the Surface. Do not treat `/sessions/{id}/messages` as an agent reply endpoint. Do not let background replies mutate focused transcript/SSE/token state. Preserve web/extension Sessions behavior and browser Print. Keep platform detection through existing app/host seams.

**Success criteria:** Three concerns have separate state and component ownership; local fuzzy session filtering remains deterministic; semantic history uses an approved daemon contract; inline reply is absent until authority semantics exist; permission and cancellation gates remain unchanged; accessibility and host behavior tests pass.

**Validation:** Run Surface unit tests, WASM check, proxy check, standalone Tauri check, and extension build when bundle wiring changes. Review keyboard/focus behavior manually.

**Stop/escalation:** Stop before implementing semantic search or inline reply if daemon endpoint shape, indexing ownership, stable hit IDs, background-turn correlation, or permission-token semantics remain unapproved.

**Resolved assumptions:** The Island remains Tauri-only for now. The shared CSS bundle may contain inert Island selectors. The existing SessionsPanel remains canonical for web/extension and project management. Local session metadata matching is fuzzy search, not semantic history search.