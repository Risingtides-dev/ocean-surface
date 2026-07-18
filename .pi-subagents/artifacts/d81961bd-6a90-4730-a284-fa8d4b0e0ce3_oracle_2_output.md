## Inherited decisions

- `ocean-os` remains the session, persistence, permission, tool, and event authority; Ocean Surface stays a thin projection client.
- Product transcripts are session-scoped through `/v1/agent/events?session_id=...`.
- Agent replay is currently a global in-memory ring bounded to 2,048 events and 32 MiB.
- Session persistence is incremental:
  - accepted user input is saved before provider execution;
  - tool rounds are saved only after calls have ordered results;
  - the final transcript is saved at turn completion.
- Permissions currently travel on the separate global `/v1/events` rail.
- Advisory-only scope is binding: no files were modified and no implementation was launched.

## Diagnosis

The race is real in `ocean-surface-ui/src/daemon.rs`:

1. `switch_session` clears state.
2. It spawns `GET /v1/sessions/{id}` hydration.
3. It independently opens `/v1/agent/events`.
4. Live events may be applied before the snapshot response.
5. Hydration then replaces the whole transcript, erasing those events.

The current SSE ID dedupe cannot fix this because it only deduplicates events against other events. Snapshot rows carry no event boundary.

A client-only subscribe-and-buffer algorithm is also insufficient. It cannot determine whether an event received during hydration was:

- already included in the persisted snapshot;
- emitted before its corresponding persistence checkpoint;
- not persistable at all, such as thinking/browser lifecycle state;
- represented indirectly, such as components reconstructed from completed tool calls.

Therefore it cannot prove either no loss or no duplication.

### Existing daemon properties that prevent a simple cursor patch

- `AgentEventBus` assigns random UUIDs independently of session persistence.
- Sampling the bus cursor **after** reading the file can skip events emitted between the read and cursor sample.
- Sampling it **before** reading can replay events already incorporated into the file.
- Runtime events are sent toward the daemon bridge before `TurnCheckpoint` persistence completes.
- The daemon bridge drains after `AgentRuntime::prompt` returns and after its per-session turn lock is released.
- `TurnFinished` is emitted later still.
- An unknown/evicted `Last-Event-ID` currently falls through to an empty replay and live attachment, silently losing the gap.
- The replay ring is global, so traffic from unrelated sessions can evict the selected session’s anchor.
- Permissions use another bus and therefore cannot share the agent-event cursor today.

## Drift / contradiction check

Several current comments describe `/v1/agent/events` as a live-only tail, but the daemon now supports bounded replay. Surface reconnects nevertheless recreate `EventSource` without supplying a retained anchor, so that replay is not a correctness boundary.

The following proposed shortcuts would conflict with the required exactly-once projection:

1. **Add the current bus UUID to the snapshot response.**  
   Rejected: it is not atomic with snapshot persistence.

2. **Subscribe first, buffer, then hydrate.**  
   Rejected: there is no proof telling buffered events already represented by the snapshot from missing ones.

3. **Use `AgentRuntime`’s existing session turn lock.**  
   Rejected:
   - detail reads do not take it;
   - bridge emission occurs outside it;
   - holding it through a whole active turn would block session switching, potentially while the operator must answer a permission request.

4. **Add a short per-session mutex around file read and bus emission.**  
   Rejected: persistence writes occur in a different layer and are not covered, so the same ambiguity remains.

5. **Keep silently attaching live when a cursor was evicted.**  
   Rejected: this converts a detectable recovery condition into permanent projection loss.

## Recommendation

### Chosen design: daemon-owned, session-scoped materialized projection journal

Do not make the session JSON file plus the current broadcast ring pretend to be an atomic event log. Introduce a daemon-owned projection coordinator whose durable session journal and materialized projection are the authority for Surface attachment.

A rooms-style model is the safest precedent already present in this repository: durable ordered rows are authoritative; broadcast is only a wake hint.

### Concrete API contract

#### Snapshot

Either add an explicit attach route:

```http
GET /v1/sessions/{session_id}/projection
```

or add the same fields additively to `GET /v1/sessions/{id}` once that handler is backed by the coordinator:

```json
{
  "ok": true,
  "session": { "...existing fields...": "..." },
  "projection": {
    "version": 1,
    "cursor": "sc1:<epoch>:<session-id>:<seq>",
    "turns": [],
    "components": [],
    "pending_permissions": [],
    "active_turn": null,
    "browser_active": false
  }
}
```

Cursor meaning:

> The projection equals the deterministic fold of every committed projection record for this session through and including `seq`.

The snapshot and cursor must be read in one database transaction or one linearizable coordinator operation.

#### Tail

```http
GET /v1/agent/events?session_id=<id>&after_cursor=<cursor>
```

Rules:

- `after_cursor` is exclusive.
- Every new session-scoped frame receives a contiguous per-session sequence.
- SSE `id:` becomes the versioned cursor for new protocol clients.
- The daemon subscribes to wake notifications before paging committed records after the cursor.
- Broadcast lag triggers journal catch-up, not silent loss.
- Cursor/session mismatch, epoch mismatch, compaction, or an unretained oversized record produces:

```text
event: projection_reset
data: {"reason":"cursor_expired"}
```

and closes the stream. It must never silently continue live.
- If both `Last-Event-ID` and `after_cursor` are supplied and differ, reject the request rather than guessing.
- Existing UUID `Last-Event-ID` parsing remains available during migration.

### Server-side linearization

For each session, a short-lived coordinator operation performs:

1. Assign next `seq`.
2. Insert the immutable event record.
3. Fold it into the materialized projection.
4. Commit both atomically.
5. Publish a wake hint only after commit.

The lock/transaction must never span provider calls, tools, permission waits, or network awaits.

The persisted agent JSON may remain provider-history authority initially, but it must not be used to manufacture the projection cursor. Record stable history revision/hash checkpoints and reconcile on startup. If strict crash atomicity between provider history and visible projection is required, both ultimately need one transactional store; two independently renamed files cannot provide that guarantee.

### Projection coverage

One cursor must cover every event that mutates the session UI:

- canonical user and assistant transcript state;
- stable turn IDs and tool-call IDs;
- tool started/chunk/finished state in original order;
- component render, replace, unmount, and pinned registry;
- full pending permission cards and decisions;
- active turn/model/status and terminal token statistics;
- browser-active current state;
- canvas/surface ledger state if `SurfacePatch` remains part of session attachment.

Permissions should be dual-published temporarily:

- new session projection journal: authoritative for new Surface;
- legacy `/v1/events`: compatibility only.

The new Surface must not let both rails write the same permission state.

Thinking may either be included in the active-turn projection or explicitly classified as transient and cleared on hydration. That semantic must be decided before claiming that the snapshot covers the complete UI projection.

### Surface switch algorithm

1. Increment the local session generation and close old session streams.
2. Fetch the daemon projection snapshot.
3. Recheck generation and session ID.
4. Apply the snapshot exactly once.
5. Open SSE with `after_cursor=<snapshot.cursor>`.
6. Track the last applied sequence:
   - duplicate/older cursor: ignore;
   - next contiguous cursor: apply;
   - gap or `projection_reset`: discard the generation and restart from step 2.

Do not open the new session stream concurrently with the initial snapshot request.

### Proof sketch

Let `P(s, n)` be the materialized projection for session `s` after folding journal rows `1..n`.

- Snapshot atomically returns `(P(s, n), cursor(s, n))`.
- Every subsequent mutation commits row `n+1` and updates the projection in the same transaction before any wake is sent.
- Tail replay returns exactly the committed rows with sequence `> n`, in sequence order.
- Subscribe-before-page plus durable catch-up means a wake can be duplicated or lost without losing the row.
- Client sequence checking applies each row at most once.
- Consequently, after applying rows through `m`, client state is `P(s, m)` by induction.
- If rows required for that induction were compacted, the daemon explicitly forces a fresh snapshot instead of claiming continuity.

This proves exactly-once **logical projection** despite physical SSE redelivery.

### Replay and compaction

Use per-session ordered journals with a global storage/memory budget, not an unconstrained 32 MiB allocation per session.

Recommended policy:

- durable projection row always retained;
- journal rows may be compacted through a projection checkpoint;
- active attach cursors receive a short lease to prevent immediate eviction between snapshot and stream connection;
- stale cursors reset to a fresh snapshot;
- oversized events may be delivered live, but any later cursor crossing an unretained oversized event must reset;
- subscriber lag pages the journal and only resets if the required range has been compacted.

### Tool and component ordering

The journal writer must preserve the current runtime channel order:

```text
ToolCallStarted
ComponentRender / ComponentUnmount, when emitted
ToolCallFinished
persistence checkpoint
TurnFinished
```

Hydration must expose stable canonical IDs rather than regenerating synthetic turn or tool IDs. Successful persisted component calls may seed recovery, while failed component calls must not create phantom components. Component replacement/unmount must remain deterministic under replay.

### Backwards-compatible migration plan

1. **Characterization tests**
   - deterministically pause hydration while events arrive;
   - demonstrate current transcript overwrite;
   - cover tool/component/permission ordering.

2. **Daemon projection store**
   - add per-session sequence, materialized projection, journal, cursor parsing, and explicit reset semantics;
   - preserve existing routes and buses.

3. **Route event producers**
   - route every Surface-relevant event through the projection coordinator;
   - dual-publish legacy agent and permission events after projection commit.

4. **Add snapshot/cursor API**
   - old clients ignore additive fields;
   - old UUID replay continues unchanged.

5. **Surface feature detection**
   - use the new attach protocol only when `projection.version == 1`;
   - stop the permission rail from being a second reducer in this mode.

6. **Old-daemon behavior**
   - do not label the old path race-free;
   - safest fallback is snapshot-only/degraded live attachment with an upgrade indication, rather than unverifiable buffering.

7. **Validation**
   - race barriers at every snapshot/publish interleaving;
   - reconnect, duplicate, lag, compaction, epoch restart, oversized-event, permission, component, and multi-session eviction tests;
   - cross-repo integration test proving the final client projection matches the daemon projection.

## Risks

- Durable projection introduces a new schema and reducer that must not drift from Surface semantics.
- `SurfacePatch`, Slack Canvas operations, and extension events require explicit classification:
  - state projection;
  - idempotent command;
  - or non-replayable side effect.
- Replaying side-effecting bridge commands requires stable operation IDs and durable idempotency, not merely UI sequence dedupe.
- Two-file crash consistency between agent history JSON and a projection database needs revision/hash reconciliation unless session history migrates into the same transaction.
- A global projection storage budget and cursor lease duration still require operational values.
- New Surface against an old daemon cannot receive exactly-once live attachment; compatibility can be functional or strict, but not both without server support.

## Need from main agent

No decision is required to complete this advisory. Before implementation, product/architecture owners must settle:

1. whether thinking is snapshot state or explicitly transient;
2. whether `SurfacePatch` is included in this projection or remains under a separate ledger cursor;
3. whether old-daemon Surface attachment degrades to snapshot-only or remains best-effort;
4. retention and cursor-lease limits.

## Suggested execution prompt

No executor handoff is warranted from this advisory-only task. Implementation should not begin until the projection coverage and old-daemon fallback decisions above are accepted.