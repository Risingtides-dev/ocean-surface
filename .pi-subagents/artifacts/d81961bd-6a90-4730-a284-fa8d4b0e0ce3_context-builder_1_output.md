# Stable session snapshot / SSE cursor architecture brief

**Status:** implementation-ready, read-only analysis (2026-07-16)  
**Consumer:** Ocean Surface  
**Scope:** `GET /v1/sessions/{id}` plus session-scoped `GET /v1/agent/events`; persistence/event ordering needed to make their hydration seam correct. No repository files were modified.

## Executive decision

The requested guarantee is **not implementable from the current response and replay behavior**, even though the existing bus correctly closes the narrower replay-to-live subscription seam.

The stable contract should be:

1. Every persisted session version carries an opaque **agent-event commit cursor** identifying the last session event whose transcript effects are represented by that exact atomic session-file version.
2. `GET /v1/sessions/{id}` returns that persisted transcript and cursor together.
3. Surface fetches the snapshot first, then opens `GET /v1/agent/events?session_id=<id>` with `Last-Event-ID: <snapshot cursor>`; it does **not** use `replay=1` for hydration.
4. The SSE route atomically subscribes and replays only events strictly after the cursor, then tails live events. It must explicitly reject expired, foreign-session, malformed, future, or wrong-daemon-epoch cursors instead of silently attaching live-only.
5. Permission request/decision lifecycle must be added, additively, to the session-scoped `AgentTurnEvent` rail. The legacy `/v1/events` copies remain for compatibility, but Surface uses the agent rail as the single session-state writer.

The least ambiguous cursor is a versioned opaque value containing a daemon epoch and monotonic bus sequence, for example `v1.<epoch-uuid>.<u64>`. Keep the existing random UUID envelope id as a temporary compatibility alias if needed, but new snapshot/replay correctness must use the ordered cursor. A UUID-only cursor cannot distinguish expired from unknown/future and cannot describe an empty-stream boundary.

## Required wire semantics

### Additive snapshot field

Add an optional field to `ocean_core::SessionDetail` (or, less desirably, `SessionResponse`):

```text
agent_event_cursor?: string
```

Recommended semantics:

- Present for all current daemon-produced snapshots after rollout.
- Opaque to clients; clients only store and replay it verbatim.
- Identifies the greatest session event whose transcript effect is included in `transcript`, `tool_context`, and `messages` in this response.
- Does **not** claim that ephemeral UI projections (`active_requests`, `pending_permissions`, request state) are persisted transcript rows. Surface must not use those projections as a second append-only event source.
- Legacy session files deserialize without it. The daemon establishes a current-epoch baseline cursor when first serving/mutating a legacy file; see migration below.

A clearer long-term DTO name would be `SessionSnapshot`, but adding the field to `SessionDetail` preserves the existing route and consumers.

### Cursor ordering and scope

A cursor is bound to:

- daemon boot epoch;
- one agent-event sequence boundary;
- the requested session (validated by stored commit metadata or a signed/lookup association).

Replay means `event.sequence > cursor.sequence`, filtered to the same session, in emission order. A cursor is allowed to be a boundary with no corresponding client-visible event, which solves empty/new/restarted sessions without inventing transcript text.

### SSE failure contract

Current behavior (“unknown/aged out => empty replay, attach live”) can lose data and must not be used by the new contract.

Recommended HTTP responses before an SSE 200 is committed:

- `400 cursor_invalid`: malformed cursor.
- `409 cursor_session_mismatch`: cursor belongs to another session.
- `409 cursor_epoch_mismatch`: daemon restarted; refetch snapshot.
- `410 cursor_expired`: at least one deliverable event after the cursor is no longer replayable (count/byte eviction or an individually oversized non-retained event); refetch snapshot. Include no transcript/event payload in the error.
- `409 cursor_ahead`: cursor sequence is newer than the bus high-water mark.

On `broadcast::Receiver::Lagged`, emit the existing error frame and terminate the stream. Continuing after a known gap cannot satisfy exactly-once delivery. Surface then refetches a snapshot and starts from its new cursor.

A conservative global-ring `410` is correct but can force resets because unrelated sessions evicted entries. Prefer tracking per-session replay floors/gaps while retaining the existing global 2,048-event / 32-MiB memory limits. Correctness is more important than avoiding a reset.

### Exactly-once meaning

Transport reconnection can redeliver an event when the connection fails after receipt but before the client durably records its id. Therefore the achievable contract is **at-least-once transport plus exactly-once reducer application by cursor/id**:

- daemon emits each sequence once and replays strictly after the supplied cursor;
- client records/applies each SSE id at most once;
- client advances its in-memory cursor only after applying the frame;
- snapshot replacement resets reducer state and seeds its cursor atomically.

Do not promise network-level exactly once.

## Why current behavior is insufficient

### What already works

`AgentEventBus` records an envelope into history before broadcast and takes the same history lock across `tx.subscribe()` plus replay snapshot. This correctly ensures an event is in replay or the new live receiver, never in neither (`crates/ocean-daemon/src/bus.rs:195-318`, especially `emit` at 231-269 and `subscribe_with_replay` at 285-304). The SSE handler replays first, dedupes replay ids against live, and then tails the broadcast (`crates/ocean-daemon/src/main.rs:6599-6716`). Session filtering is applied identically to replay and live.

### The unresolved snapshot seam

`GET /v1/sessions/{id}` reads the session file independently and does not acquire the per-session turn lock or the bus history lock (`crates/ocean-daemon/src/main.rs:5107-5143`). It then separately enriches request/permission state (`5145-5190`). There is no cursor in `SessionDetail`/`SessionResponse` (`crates/ocean-core/src/lib.rs:270-405`).

Session saves are individually atomic and durable (temp sibling, file fsync, rename) at `crates/ocean-agent/src/session/mod.rs:359-390`, so a reader sees a coherent old or new file. Atomic-file visibility alone does not tie that version to an SSE boundary.

The runtime parent currently forwards each `AgentEvent` to the daemon sink **before** matching it and saving a `TurnCheckpoint` (`crates/ocean-agent/src/lib.rs:1720-1773`). The sink is an unbounded channel. The daemon bridge is a separate task (`crates/ocean-daemon/src/main.rs:5829-6120`). Consequently:

- save may complete while earlier forwarded deltas/tool events are still queued and have no bus ids yet;
- a snapshot can contain text/tool rows for events that will be emitted only after the read;
- choosing the bus tail after that read can skip queued events, while choosing it before can duplicate transcript text.

At final completion, ocean-agent saves before returning (`crates/ocean-agent/src/lib.rs:1795-1811`), then the daemon awaits bridge drain and only afterward emits `TurnFinished` (`crates/ocean-daemon/src/main.rs:6200-6312`). The per-session turn lock has already been released before bridge drain, so merely exposing that lock to the GET route would still leave a release-to-drain race.

### Existing replay fallback is lossy

- No `Last-Event-ID` means no replay unless `?replay=1` is requested.
- `?replay=1` replays the entire **bounded global** history, session-filtered. A resumed client that already hydrated persisted text duplicates deltas, which is why the TUI uses a fresh-vs-resumed heuristic (`crates/ocean-tui/src/shell/client.rs:205-230`).
- Unknown, malformed, or aged-out UUIDs become `None`/empty replay and live-only, with no client-visible reset requirement (`crates/ocean-daemon/src/main.rs:553-560`; `crates/ocean-daemon/src/bus.rs:285-304`; tests at `main.rs:8282-8290`, `9051-9067`).
- Agent replay retention is globally bounded at 2,048 events and 32 MiB; oversized individual events are live but not retained (`crates/ocean-daemon/src/bus.rs:24-33,195-269`).
- UUID v4 ids encode no order or daemon epoch.

Thus full replay is a race workaround, not a snapshot cursor contract.

## Persistence and lock findings

### Current per-session serialization

`AgentRuntime` owns a process-lifetime registry of per-session Tokio mutexes (`crates/ocean-agent/src/lib.rs:280-283,389-399`). The primary/fallback transaction holds one lock across both attempts (`844-918`), and `run_prompt` documents that load/run/save occurs under it (`1447-1458`). Idle lock entries are pruned; active entries survive (`389-399`, tests `4234-4291`). External message append takes the same lock (`1275-1288`).

This prevents two turns from last-writer-wins corruption, but:

- the lock is private to ocean-agent;
- ordinary `session_detail` does not take it (`1272`; `session/mod.rs:505-507`);
- daemon-side `TurnStarted`, event bridge drain, `TurnFinished`, and permission bus emissions are not covered by it.

**Conclusion:** current locks/order are not sufficient. They can be reused for persisted mutation, but a cursor-aware acknowledged event sink (or equivalent shared commit protocol) is required. Adding a second daemon per-session turn lock is risky and unnecessary; it would duplicate authority and can alter ACK/queuing behavior.

### Checkpoint order

Current durable boundaries are sound as transcript boundaries:

1. Accepted user row saved before provider/tool execution (`ocean-agent/src/lib.rs:1555-1563`).
2. Runtime emits `TurnCheckpoint` only for newly completed provider-valid rows (`ocean-runtime/src/agent_loop.rs:1218-1233`; type contract at `ocean-runtime/src/types.rs:294-301`).
3. Ocean-agent incrementally saves those rows (`ocean-agent/src/lib.rs:1760-1773`).
4. Final capped history is saved after a successful run (`1795-1811`).

Do not change the provider-valid checkpoint rule or persist orphan tool calls. Add cursor commitment to these same atomic saves.

## Recommended implementation design

### 1. Ordered, acknowledged event publication

Replace or augment `PromptControl`'s fire-and-forget `mpsc::UnboundedSender<AgentEvent>` with an acknowledged sink for daemon-owned turns. For each runtime event, the ocean-agent parent must be able to await an acknowledgement containing the current session event cursor after the daemon has:

1. translated the event;
2. inserted every resulting `AgentTurnEvent` into replay history;
3. broadcast it;
4. returned the last inserted cursor (filtered internal events return the current cursor without emitting).

FIFO must be preserved. This removes the queued-but-not-yet-identified event state.

For a `TurnCheckpoint`, ocean-agent obtains the acknowledged cursor **before** saving and writes that cursor into the same JSON session object/atomic rename as the corresponding completed rows. Do the same for:

- accepted-user save, seeded with the already-emitted `TurnStarted` cursor;
- each incremental checkpoint;
- final save.

Do not persist a cursor beyond an event whose transcript effect is absent from the saved messages.

An alternative direct synchronous callback is acceptable if it does not hold async locks across blocking I/O and retains parent-turn ownership/cancellation. A post-save `TranscriptCommitted` event without acknowledgement is insufficient: GET can observe the new file before the bridge processes the marker.

### 2. Cursor-aware AgentEventBus

In `crates/ocean-daemon/src/bus.rs`:

- add daemon `epoch` and monotonic `next_sequence`;
- have `emit` return the cursor;
- retain sequence on each `AgentEventEnvelope`;
- track global and per-session retained floors plus non-retained/evicted gaps;
- add a typed `subscribe_after(cursor, session_id)` result rather than overloading `Vec::new()` for all failure cases;
- subscribe and snapshot replay under the existing history lock;
- preserve count/byte bounds and live delivery of oversized events;
- expose no bus internals in ocean-core.

The sequence increment, history insertion, and live subscription snapshot remain under the existing bus synchronization. Do not hold the std mutex while serializing HTTP bodies or awaiting.

### 3. Persisted session cursor

In `crates/ocean-agent/src/session/mod.rs`, add an optional serde-defaulted cursor field to `Session`. It must be updated only alongside `replace_messages` at a proven checkpoint and written by the existing atomic save. Keep old files readable.

In `ocean-core`, add the optional wire field and a small opaque newtype if useful. Do not expose sequence arithmetic to clients.

On daemon restart/legacy file:

- the full persisted transcript is authoritative;
- establish a current-epoch baseline boundary before returning the snapshot;
- no pre-restart in-memory lifecycle can be recovered, so active requests are already gone and the snapshot is the reset point;
- subsequent events use the new epoch.

If baseline establishment requires rewriting the session file, do it under the existing session mutation lock and preserve compatibility. A daemon-memory mapping keyed by `(session id, updated_ms/file generation)` is acceptable only if GET cannot race a new save; persisting the cursor with the transcript is simpler and auditable.

### 4. Permission lifecycle on the agent rail

Today permission requests/decisions exist only as `OceanEvent` on `/v1/events` (`crates/ocean-daemon/src/main.rs:1722-1849,1899-2005`; TUI explicitly documents this at `crates/ocean-tui/src/shell/client.rs:297-346`). The agent bridge only synthesizes paired tool start/finish for a denial (`main.rs:5900-5950`). A snapshot plus agent cursor therefore cannot guarantee permission lifecycle.

Add additive `AgentTurnEvent` variants, with stable ids and session/turn/request correlation, for example:

- `PermissionRequested { session_id, turn_id/request_id, permission_id, tool, reason, args }`
- `PermissionDecided { session_id, turn_id/request_id, permission_id, allowed, reason }`

Emit them to `AgentEventBus` in the same order as legacy events, update the turn's current cursor, and retain legacy `/v1/events` emission unchanged. Never include `decision_token` (existing security tests around `main.rs:12345-12390` enforce this). Surface must render permission lifecycle only from the agent rail after adoption, preventing dual-rail duplicates.

`pending_permissions` in session detail is an ephemeral reconciliation projection, not a second ordered transcript. On snapshot reset it may seed current cards keyed by `permission_id`; replayed permission events must reduce idempotently by that same id.

### 5. GET route behavior

`GET /v1/sessions/{id}` stays the route for compatibility and returns the additive cursor with the exact persisted version. Enrichment may continue, but the cursor guarantee applies to persisted transcript/tool rows, not separately-read request-registry timestamps.

If a session file exists but its cursor cannot establish a replayable boundary for events not yet represented, return a typed retry/reset error rather than a cursor that can skip events. Reads of stable terminal sessions should not block on a running turn. The acknowledged commit protocol makes live reads possible without taking the whole-turn mutex.

### 6. Surface hydration algorithm

Target source from the project map: `ocean-surface/crates/ocean-surface-ui/src/daemon.rs` and its callers/reducers. That sibling checkout was not present in this worktree and was not modified.

Required client sequence:

1. Stop/ignore the prior subscription generation for the session.
2. Fetch `GET /v1/sessions/{id}`.
3. Atomically replace persisted transcript/tool projection and seed reducer cursor from `agent_event_cursor`.
4. Open scoped agent SSE with `Last-Event-ID` set to that cursor. Never use `replay=1` for this path.
5. Apply frames in order; dedupe by SSE id/cursor before appending text deltas or changing lifecycle maps.
6. Keep text assembly keyed by `turn_id`; tools by `call_id`; permissions by `permission_id`.
7. On `410`, epoch mismatch, or lag error: stop applying that stream generation, refetch snapshot, replace state, and reconnect. Do not merge a fresh full replay into an already-hydrated transcript.
8. Ignore legacy `origin: "agent"` transcript/tool mirrors and, after adoption, legacy permission mirrors as well.

This closes the fetch-then-subscribe race because any event after the persisted commit cursor is replayable, including events emitted between HTTP responses.

## Compatibility strategy

1. **Server additive phase:** optional `agent_event_cursor`; new agent permission variants; old UUID SSE id retained as alias if necessary; existing `replay=1`, UUID Last-Event-ID, and `/v1/events` unchanged.
2. **Surface opt-in:** use cursor only when field is present. On an old daemon, retain current behavior but label it best-effort; do not claim exact hydration.
3. **Other clients:** TUI/ACP continue existing rails. Unknown `AgentTurnEvent` kinds are already required to be ignored (`ocean-agent-sdk/src/lib.rs:534-540`). Update exhaustive Rust matches to ignore/handle new variants.
4. **Deprecation:** after all first-party clients adopt, deprecate `replay=1` as first-hydration recovery, not ordinary Last-Event-ID reconnect. Do not remove it in this change.
5. **Session files:** optional serde field, no destructive migration. Old files load; first current snapshot/save establishes the new baseline.

Changing every SSE `id:` from UUID to a versioned cursor may affect clients that parse UUID specifically. Audit `ocean-tui`, `ocean-acp`, `ocean-heartbeat`, offshore remote tooling, service worker/proxy, and Surface before switching. TUI currently stores ids as strings (`ocean-tui/src/shell/client.rs:231-270,541+`), so it is likely compatible.

## Failure, eviction, and restart semantics

- **Count/byte eviction:** explicit `410 cursor_expired`; never empty replay/live-only.
- **Oversized event:** mark a replay gap at that sequence. Any older cursor that would require it gets `410`, even though live subscribers received it.
- **Slow subscriber lag:** error then close; client snapshot-resets.
- **Daemon restart:** epoch mismatch forces snapshot reset. Persisted transcript survives; in-memory SSE lifecycle does not.
- **Save failure:** do not advance persisted cursor. Turn already reports persistence failure through existing result handling; later replay may recover buffered events while retained.
- **Corrupt session:** preserve existing 500/no-fresh-session behavior (`session/mod.rs:420-462`).
- **Session TTL deletion:** existing 404; client discards local reducer/session selection.
- **Client duplicate frame:** ignore id already applied.
- **Cross-session cursor:** reject; never rely only on output filtering.
- **Advisors/extensions after turn:** they occur after persisted transcript cursor and replay normally. They are not transcript text.

## Exact implementation touch points

### Required production files

- `crates/ocean-core/src/lib.rs:270-405` — `SessionRunState`, `SessionDetail`, `SessionResponse`; add opaque/additive snapshot cursor.
- `crates/ocean-agent/src/session/mod.rs:1-75,359-507,909-1020` — persisted `Session`, atomic save/load, detail projection.
- `crates/ocean-agent/src/lib.rs:280-283,389-399,844-918,1420-1811` — per-session lock, accepted/checkpoint/final save order, acknowledged sink integration.
- `crates/ocean-runtime/src/types.rs:294-301` and `agent_loop.rs:1218-1233` — checkpoint contract; likely no semantic change, only sink plumbing if its type lives here.
- `crates/ocean-daemon/src/bus.rs:24-33,139-318` — ordered cursor, typed replay outcomes, gap/floor tracking, emit acknowledgement.
- `crates/ocean-daemon/src/main.rs:553-560,5107-5190,5685-5712,5829-6120,6200-6312,6575-6784,7430-7450` — header parsing, snapshot route, turn boundaries/bridge, SSE response errors, emission return cursor.
- `crates/ocean-agent-sdk/src/lib.rs:534-752` — additive permission lifecycle variants and `session_id()` coverage.
- Permission emission paths `crates/ocean-daemon/src/main.rs:1722-1849,1899-2005`.
- Downstream exhaustive matches, especially daemon relay classification/tests and TUI/ACP event reducers.

### Surface follow-up

Per `docs/OCEAN_PROJECT_MAP.md`, Ocean Surface owns UI hydration. Inspect/update the sibling repository's root/local `AGENTS.md`, `crates/ocean-surface-ui/src/daemon.rs`, session store/reducer, service worker/proxy SSE handling, and hydration/reconnect tests. Do not move persistence or permission authority into Surface.

## Targeted tests to add/update

### Bus and SSE (`ocean-daemon`)

- snapshot cursor at sequence N replays exactly N+1..live, with an emit racing subscribe;
- empty-session boundary then first event;
- cursor scoped to session A is rejected for B;
- malformed/future/unknown/epoch-mismatch typed failures;
- count eviction, byte eviction, and oversized-event gap return `410`;
- unrelated-session eviction does not falsely skip a deliverable session event (or conservatively resets, explicitly tested);
- replay/live seam applies each id once;
- lag frame terminates stream;
- new permission request/decision order and no token leakage;
- existing full replay and legacy UUID behavior remain compatible.

Existing characterization anchors: `main.rs:8238-8480`, `8573-8598`, `9051-9085`; bus pressure/oversize tests in `bus.rs:344-470`.

### Persistence (`ocean-agent`)

- legacy file without cursor loads;
- accepted-user save and cursor round-trip;
- checkpoint file contains exactly provider-valid rows and matching acknowledged cursor;
- final save cursor does not advance past `TurnFinished` (which is emitted later);
- save failure leaves prior transcript/cursor pair intact;
- concurrent detail reads observe only old-pair or new-pair, never mixed;
- primary/fallback reuses accepted row and cursor under the same lock;
- restart baseline for legacy/current-epoch mismatch.

Reuse session tests around `ocean-agent/src/lib.rs:4168-4291,4336+,5032+,5466+` and session-module atomic save/load tests.

### Core/SDK and clients

- serde old/new `SessionDetail` compatibility;
- all `AgentTurnEvent` variants serialize/deserialize and unknown-event clients remain forward-compatible;
- Surface: hydrate snapshot, inject event between GET and subscribe, assert no lost/duplicated text;
- Surface: persisted assistant/tool rows plus later `TurnFinished` do not duplicate deltas;
- Surface: pending permission across hydration survives request/decision ordering exactly once;
- Surface: reconnect redelivery dedup;
- Surface: eviction/epoch/lag triggers replace-and-resubscribe, not merge/full replay;
- TUI/ACP compile and existing permission behavior remains unchanged.

### Validation commands

```text
cargo test -p ocean-core
cargo test -p ocean-agent session
cargo test -p ocean-agent
cargo test -p ocean-daemon bus::tests::
cargo test -p ocean-daemon agent_bus -- --nocapture
cargo test -p ocean-daemon permission_ -- --nocapture
cargo test -p ocean-daemon
cargo test -p ocean-tui
cargo test -p ocean-acp
cargo check --workspace --tests
```

Then run the owning Ocean Surface test/build commands from its local `AGENTS.md`, followed by `cargo xtask ci` at merge readiness. Protocol/logic changes require fresh reviewer acknowledgement under the repository contract.

## Implementation stop conditions

Stop and escalate rather than weakening the contract if any of these is true:

- persistence can become visible before all events through its cursor are assigned and replay-retained;
- the implementation cannot distinguish expired/gapped replay from “nothing happened”;
- a snapshot cursor can advance past an unpersisted text/tool event for that session;
- permission lifecycle remains only on the legacy global rail while claiming one-cursor hydration;
- Surface must combine `replay=1` with a persisted transcript;
- a new daemon-wide/session lock would be held across provider/tool execution solely to serve snapshots, changing turn admission/ACK behavior;
- legacy session deserialization or current SSE consumers require a breaking migration not explicitly approved;
- count/byte/oversized eviction can silently attach live-only;
- tests cannot deterministically inject the save/emit/subscribe races above.

Enough evidence exists to implement once the acknowledged sink/cursor representation is agreed. The product/API decision that still merits explicit approval is whether all SSE ids switch to the ordered cursor immediately or whether UUID ids remain as a compatibility alias during one release.

## Resolved questions and assumptions

- **Can current history-lock ordering solve hydration alone?** No; it solves replay-to-live, not disk-snapshot-to-replay.
- **Can the current per-session turn lock solve it alone?** No; GET does not take it and daemon bridge drain/terminal events are outside it.
- **Should snapshot reads block until turn end?** No. It would hang at permission gates and harm live Surface hydration; checkpoint cursoring supports live reads.
- **Should `replay=1` be the stable solution?** No; it duplicates persisted rows and silently fails after eviction.
- **Are tool calls represented in snapshots?** Completed provider-valid call/result rows are represented by `transcript`, `tool_context`, and raw `messages` (`session/mod.rs:909-1020`). In-flight lifecycle after the commit cursor must replay.
- **Are permissions represented today?** Pending ids are an ephemeral detail projection, but ordered request/decision events are only on `/v1/events`; the stable one-cursor contract requires additive agent-rail events.
- **Does “exactly once” include transport?** No; it means exactly-once reducer application using ordered ids atop replayable at-least-once transport.

## Meta-prompt handoff for implementation planner

**Goal:** Produce a bounded cross-crate implementation plan (and separate Ocean Surface follow-up) for an additive persisted-session/agent-SSE cursor contract that makes snapshot-first hydration race-free.

**Evidence:** Use the source anchors and ordering analysis above. Preserve atomic session saves, provider-valid checkpoint boundaries, the one per-session turn lock, bus history-before-broadcast and subscribe-under-history-lock behavior, current replay count/byte bounds, session scoping, and legacy rails.

**Success criteria:** The exact race/eviction/restart/permission tests listed above pass; a snapshot cursor never skips an unrepresented session event; resumed transcripts do not reapply text deltas; tool and permission lifecycle converge; cursor gaps reset explicitly; old files and clients remain compatible.

**Hard constraints:** No second session authority; no client-side persistence authority; no silent replay failure; no decision-token exposure; no orphan tool-call persistence; no removal of legacy routes/replay in this change; fresh reviewer gate required.

**Suggested approach:** First define the cursor/replay result types and acknowledged publication protocol, then wire cursor into atomic session saves, then expose GET/SSE behavior, then add permission variants, then update Surface hydration. Keep server compatibility and Surface adoption in separately reviewable commits if possible.

**Validation:** Run the narrow crate commands above, deterministic race tests, workspace tests, then Surface tests and the canonical merge gate.

**Escalate/stop:** Use the stop conditions above. In particular, do not substitute full replay or whole-turn snapshot blocking for a commit cursor.

# Acceptance report

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Produced the requested read-only, implementation-ready architecture brief without modifying repository source or widening into implementation."
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "Brief identifies exact routes, types, ordering, locks, replay/eviction behavior, source anchors, compatibility, failure semantics, deterministic tests, validation, residual decision, and stop conditions."
    }
  ],
  "changedFiles": [],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short && git branch --show-current",
      "result": "passed",
      "summary": "Repository had no reported staged or unstaged files; branch was feat/session-snapshot-sse-cursor-20260716."
    },
    {
      "command": "repository read/grep inspection across ocean-agent, ocean-runtime, ocean-core, ocean-agent-sdk, ocean-daemon, ocean-tui, and docs",
      "result": "passed",
      "summary": "Located and inspected session atomic saves/checkpoints/locks, session GET projection, bus emission/replay, SSE Last-Event-ID/full replay, permission rails, tests, and client reconnect behavior."
    },
    {
      "command": "git ls-remote https://github.com/Risingtides-dev/ocean-surface.git HEAD",
      "result": "passed",
      "summary": "Confirmed the Ocean Surface repository is reachable at HEAD 590ff25363aeb37b1b1231afbcffcfa8390590dd; it was not cloned or modified."
    }
  ],
  "validationOutput": [
    "Read-only analysis: no build or test execution was necessary or performed.",
    "Current bus atomically closes replay/live seam, but current disk snapshot has no commit cursor and can race the asynchronous event bridge.",
    "Current unknown/expired Last-Event-ID silently yields empty replay/live-only, which cannot satisfy lossless hydration.",
    "Permission request/decision lifecycle currently rides the legacy global event rail, so it must be additively unified onto the session-scoped agent rail for a one-cursor contract."
  ],
  "residualRisks": [
    "Ocean Surface source was not locally available and the tool budget stopped further browsing; its exact reducer/service-worker call sites must be confirmed in the sibling repository before implementation.",
    "Approval is still needed on immediate ordered SSE-id replacement versus a one-release UUID compatibility alias.",
    "A global bounded ring may cause conservative resets under unrelated-session pressure unless per-session replay floors are tracked."
  ],
  "noStagedFiles": true,
  "diffSummary": "No repository diff; only the required external architecture-brief artifact was written.",
  "reviewFindings": [
    "blocker: crates/ocean-daemon/src/main.rs:5107 and crates/ocean-agent/src/lib.rs:1720 - session detail and asynchronous event publication have no shared persisted commit cursor, so snapshot hydration can skip or duplicate deltas.",
    "blocker: crates/ocean-daemon/src/bus.rs:285 - unknown/expired Last-Event-ID is indistinguishable from no missed events and silently attaches live-only.",
    "blocker: crates/ocean-daemon/src/main.rs:1899 - permission lifecycle is absent from the session-scoped AgentEventBus contract.",
    "review gate required after implementation for protocol, logic, and cross-client behavior."
  ],
  "manualNotes": "The output artifact is /private/tmp/ocean-session-cursor-context-20260716.md. Repository files were intentionally left unchanged."
}
```
