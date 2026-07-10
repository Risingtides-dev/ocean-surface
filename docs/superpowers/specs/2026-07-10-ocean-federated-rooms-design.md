# Ocean federated Rooms design

Date: 2026-07-10
Status: ready for user review
Scope: cross-repo design for Ocean Rooms as a federated, sovereign-agent collaboration surface. Touches `ocean-os`, `ocean-bedrock`, `ocean-agents`, and `ocean-surface` as a system. This document is a design specification, not an implementation plan.

## Purpose

Ocean "Rooms" is one word covering three unrelated things today: (1) durable persistent collaboration rooms under `/v1/rooms/persistent/*`, (2) Track-0 projection lenses over global runtime state, and (3) LiveKit media containers. Only (1) is the peer-and-agent collaboration surface, and it is real: a single daemon owns a SQLite-backed room with a roster, an append-only transcript with monotonic `seq`, and a genuine `@mention` → agent-turn → reply-posted-back convene loop that honors `Project -> Workspace -> Session -> Turns -> Events`.

What that working nucleus does **not** have is federation. There is no authenticated identity binding a human to a participant, no cross-machine transport, no "each person runs their own sovereign agent that joins the room," and no durable shared record two machines agree on. A second human can read and write the same local room, but only because both surfaces hit one loopback daemon with self-asserted participant ids.

This spec separates the two products cleanly and designs the second:

- **Gate 1 — the honest local-room nucleus.** Make the single-machine, two-human, multiple-named-agent room truthful and live: real named local agents (not the default assistant under name tags), room-scoped realtime instead of 2.5-second polling, a roster labeled honestly as durable membership with no fake online claims, no misleading audio surface, proven on two clients.
- **Gate 2 — sovereign cross-machine federation.** Two humans on two machines, each running their own Ocean and their own agents, convening into one shared room through an authenticated Bedrock-mediated record, where each `ocean-os` executes only the agents it locally owns.

The approved architecture is Bedrock-mediated (see **Architecture decision**). The hardest net-new piece—authenticated, durable, cross-machine realtime fanout—is owned by `ocean-bedrock` and specified under **The net-new hardest piece: Bedrock authenticated realtime fanout**. The existing ceremony WebSocket is a frame-shape reference only.

## Problem statement (honest baseline)

The following is true of the system today, grounded in the live code:

**Real and durable (single-machine).** Persistent rooms survive restart (`SqliteRoomStore`, `ocean-store/src/lib.rs`; proven by `persistent_rooms_survive_store_reopen`). Full CRUD, roster join/leave, transcript with `seq`, and one-read hydrate snapshot are live under `/v1/rooms/persistent/*` (`ocean-daemon/src/main.rs:1612-1659`, handlers `6185-6952`). `room_post_message` (`main.rs:6351`) is a real agent-join engine: it parses `@mentions`, evaluates `RoomTriggerPolicy` against `RoomTriggerEvent::Mention`, resolves the named participant against the room's own roster (`resolve_agent_participant`, `main.rs:6527`), and spawns a genuine `state.runtime.prompt()` on a deterministic per-(room,agent) UUIDv5 session that resumes across mentions (`spawn_room_agent_turn`, `main.rs:6592`; session id `room_agent_session_id`, `main.rs:6540`). The reply is posted back into the transcript with `author_kind = Agent`. Anti-loop is real: triggers are never evaluated on `author_kind == Agent` messages (`main.rs:6404`). Distinct agent participants already yield distinct sessions.

**Surface is wired and makes real calls.** `crates/ocean-surface-ui/src/rooms.rs` lists, creates, enters, joins, leaves, posts, and tails the transcript over `gloo-net` against `/v1/rooms/persistent/*`. It is a pure render + intent + subscribe client over one daemon.

**Absent or misleading.**

1. **No per-participant agent identity.** `spawn_room_agent_turn` builds a `PromptRequest` with no `agent`/`assistant`/`model` selector tied to the participant id — every convened "agent" is the daemon's single default assistant wearing a display name. The rich `AgentConfig`/`AgentDef` folder-as-agent resolution (`ocean-agent/src/agentdir.rs:32-89, 154-171`, `resolve` at `:226`) and the `AgentTurnRequest.agent` selection path (`ocean-daemon/src/main.rs:9397-9417`) are live but are **not joined** to room convene.
2. **No live fanout for transcript writes.** `append_message` emits no bus/SSE event. The only emission in the message path is the `room_trigger` convene *notice* (`main.rs:6436`, `scope: None` → reaches only the `?all=1` firehose, carries no reply body, is not room-filterable). A second client sees new lines only via the 2.5-second transcript poll (`TRANSCRIPT_POLL_MS`, `rooms.rs:40`). `/v1/rooms/persistent/{key}/events` is a poll endpoint, not SSE.
3. **No truth in roster/presence.** Roster is durable membership only — there is no "who is here now" signal. Two clients both joined is indistinguishable from one client joined and one gone.
4. **Misleading LiveKit audio.** On join, the surface builds `livekit_token_path = /v1/rooms/{key}/livekit-token` from the persistent key and flips `show_livekit_controls = true`. That route lives on the Track-0/call namespace (`room_livekit_token`, `main.rs:5314`), which is fully decoupled from the persistent `Room` store: no roster↔participant binding, no identity binding, publish off unless the operator holds `OCEAN_LIVEKIT_PUBLISH_TOKEN`, no agent joins audio. It presents as a room upgrade but is an ungoverned parallel channel.
5. **No authenticated identity.** `room_join` takes self-asserted `{id, display_name, kind}` off the wire (`main.rs:6292`). The daemon's HTTP/SSE API has no per-connection principal — authz is CORS + loopback only (`main.rs:1281-1328`, with the `OCEAN_BIND` off-loopback warning at `:5288-5291`). Fine for a single-operator local slice; blocking for any multi-person room.
6. **No cross-machine federation.** The daemon binds loopback (`127.0.0.1:4780` + `[::1]`); every client dials the daemon (client-initiated); the daemon never initiates outbound connections to peers and has no daemon-to-daemon transport, peer discovery, or NAT traversal. "Federation" appears in `ocean-os` only as the department enum (`Dev/Sales/Content/Campaign/Commons`), and cross-daemon coordination is explicitly deferred in `docs/LONGHOUSE_ORCHESTRATION.md`.

The baseline is therefore: a strong, honest local nucleus that is missing its live layer, its named-agent binding, and its truthfulness cleanup — plus an entirely unbuilt federation story. Gate 1 closes the first; Gate 2 closes the second.

## Product success definition

A federated Room succeeds when all of the following hold end to end:

1. **Two humans on two machines.** Alice and Bob each run their own Ocean daemon and surface, on separate machines, behind separate NATs. Neither daemon is exposed to inbound cross-machine connections.
2. **Two sovereign local agents.** Alice's agent and Bob's agent each run only on their owner's machine, resolved from that machine's `agents_root`, executing with that machine's provider credentials, tools, and workspace. No agent's secrets or execution ever cross the machine boundary.
3. **Invitation.** Alice creates a room and invites Bob via a scoped, expiring, single-use invite that is redeemable on Bob's machine out-of-band. After joining, Bob's authenticated daemon registers Bob's locally-owned agent participants under Bob's principal.
4. **Ordered real-time transcript.** Both humans see every message, agent reply, and membership change in a single agreed total order, delivered live (push), not polled.
5. **Reconnect.** If either daemon drops its connection (network, sleep, restart), it rejoins the room, replays exactly the events it missed in order, and resumes the live tail — with no gaps and no duplicates.
6. **Attribution.** Every line carries truthful authorship: which human, which agent, which machine's principal backs it.
7. **Revocation.** Alice can revoke Bob's membership; Bedrock terminates Bob's already-open subscription within one heartbeat window, and replay, reads, and writes are denied immediately thereafter.
8. **No inbound daemon exposure.** At no point does any daemon accept an inbound connection from another machine. All cross-machine traffic is brokered by the authenticated Bedrock substrate; daemons only dial outbound.

## Architecture decision

### Rejected: creator-host

In the creator-host shape, Alice's `ocean-os` daemon is the single source of truth for the room; Bob's surface subscribes to Alice's daemon over a channel brokered for reachability and auth. This maximally reuses the existing event-envelope/replay/reconnect/scope stack (`AgentEventBus::subscribe_with_replay`, `Last-Event-ID` replay, `should_emit_agent_event`, `RoomCanvasLedger`/`bind_room_session`) and changes ocean-os's invariants the least.

It is rejected for product reasons, not technical ones:

- It makes Alice's machine a load-bearing dependency for the entire room. If Alice's laptop sleeps, the room is gone for everyone, including Bob's view of prior history.
- It conflates "who hosts the record" with "who runs the agents." Bob's sovereign agent turns would depend on Alice's daemon being reachable at convene time, breaking sovereignty: Bob cannot run his agent unless Alice is up.
- It requires exposing Alice's daemon to inbound cross-machine connections (or a per-daemon tunnel), which the daemon is structurally not built for and which the product success criteria forbid.

### Rejected: direct mesh

In a direct-mesh shape, daemons talk daemon-to-daemon. This reuses nothing in ocean-os: there is no daemon-to-daemon client, no peer discovery, no NAT/firewall traversal, no tunnel/relay client, and no multi-writer convergence model. The cross-daemon non-interference contract (Longhouse "capped external mark weight," deferred in `LONGHOUSE_ORCHESTRATION.md`) is explicitly unbuilt. It would require inventing transport, cross-peer identity trust, ordering/merge across independent authorities, and Sybil/collusion handling. It is the highest-risk, most-greenfield shape and is rejected.

### Approved: Bedrock-mediated shared room record

The approved architecture is **Bedrock-mediated**. `ocean-bedrock` owns the authenticated, durable, shared room record — membership, the ordered append-only event timeline, invite/token identity, and realtime fanout. Each `ocean-os` daemon:

- dials **outbound** to Bedrock (never accepts inbound cross-machine connections);
- subscribes to its room's realtime fanout and posts locally-authored events into the shared record;
- merges inbound shared events into its local transcript projection;
- **executes only the agents it locally owns**, resolving them from its own `agents_root` and running them through its own runtime with its own provider credentials.

This satisfies sovereignty (each machine runs only its own agents), availability (the shared record outlives any single daemon; a daemon offline does not erase history), and the no-inbound-exposure rule (every daemon is a client of Bedrock). It confines the two things ocean-os genuinely lacks — cross-machine reachability and authenticated identity — to `ocean-bedrock`, which already owns authenticated cross-machine collaboration.

```text
                  ┌─────────────────────────── ocean-bedrock ───────────────────────────┐
                  │  auth_tokens / auth_invites  (invite → token, scoped, revocable)     │
                  │  longhouse.room_members      (who is in which room)                  │
                  │  longhouse.ledger_events     (ordered, hash-chained, append-only)    │
                  │  GET /api/v1/ledger/subscribe (authenticated durable realtime SSE)   │
                  │  after_sequence cursor replay                                         │
                  └─────────▲───────────────────────────────────────▲────────────────────┘
        outbound subscribe   │                           outbound subscribe   │
   + post events (outbound)  │                      + post events (outbound) │
                            │                                           │
        ┌───────────────────┴──────────┐               ┌─────────────────┴────────────┐
        │        ocean-os (Alice)      │               │       ocean-os (Bob)          │
        │  local Room + transcript     │               │  local Room + transcript      │
        │  private participant→AgentDef│               │  private participant→AgentDef │
        │  runs Alice's agents ONLY    │               │  runs Bob's agents ONLY       │
        │  outbound room bridge        │               │  outbound room bridge         │
        └────────────▲─────────────────┘               └──────────────▲────────────────┘
                     │ loopback :4780                                 │ loopback :4780
                ┌────┴─────┐                                     ┌────┴─────┐
                │  surface │                                     │  surface │
                │ (render/ │                                     │ (render/ │
                │ intent/  │                                     │ intent/  │
                │ subscribe)                                     │ subscribe)│
                └──────────┘                                     └──────────┘
```

Bedrock is the only cross-machine authority. Daemons stay loopback-bound. Surfaces never speak to anything but their own local daemon.

## Four-repository responsibility split

| Repo | Owns for Rooms | Does not own |
| --- | --- | --- |
| `ocean-os` | Local room state, roster, transcript `seq` ordering, trigger evaluation, local agent execution, the room-scoped SSE fanout to local surfaces, the outbound room bridge (subscribe to Bedrock fanout + post local events to the shared ledger + merge inbound events into the local transcript), the private participant→AgentDef binding, local sovereignty check (convene only locally-owned agents). | Cross-machine identity, invite/auth, the durable shared record, realtime fanout across machines, agent profile/persona content. |
| `ocean-bedrock` | The authenticated shared room record: invite/token identity and revocation, scoped authorization, `longhouse.room_members`, the ordered hash-chained ledger timeline keyed per room, `after_sequence` cursor replay, and the authenticated durable realtime fanout endpoint. | Agent execution, local transcript ownership, surface rendering, agent persona content. |
| `ocean-agents` | Agent profile/persona content (the `agent.toml`/`instructions.md` folders, assistant manifests, surface profiles) that the public descriptor summarizes and the local resolver reads. | Runtime enforcement, provider credentials, daemon-owned session/transcript storage. |
| `ocean-surface` | Render room state, durable membership, derived presence when the daemon provides it, confirmed transcript order, and a separate pending outbox; collect user intent (create/invite/join/message/`@mention`); subscribe to the local daemon's room-scoped SSE. | Any federation logic, identity minting or verification, invite/auth authority, provider calls, agent execution, cross-machine transport. |

### Strict authority boundaries

- **The surface holds no federation logic.** A surface talks to exactly one local daemon over the already-resolved origin (loopback for extension/Tauri, same-origin proxy for web). It never opens a second endpoint, never switches daemons, never mints or verifies identity, and never contacts Bedrock. It does render additive federated metadata, confirmed global order, invite/member projections, and the daemon-owned pending outbox; receiving new SSE variants still requires the test-guarded `AGENT_EVENT_NAMES` allow-list update in `daemon.rs`.
- **Each daemon executes only its own agents.** A daemon convenes a mentioned agent participant if and only if that participant is present in its private local binding. A remote agent is rendered in the roster and `@mention`-able, but is never resolved or executed on this machine.
- **Bedrock is transport + record, never an executor.** Bedrock stores, orders, authenticates, and fans out. It never runs an agent turn, never holds a provider credential, never touches a local filesystem.
- **Secrets stay local.** `AgentDef.root` (local fs path), resolved provider credentials (`ResolvedCredential`/`SecretString`, redacted in Debug/Display), `subprocess_capability.env` (may hold API keys), the `yolo` permission posture, and per-turn permission secrets never leave the machine that owns them. Only the non-secret public agent descriptor defined under **Identifiers and data models** travels into the shared room.
- **Identity is minted by Bedrock and consumed by the daemon.** For federated rooms, the surface's unauthenticated `localStorage` identity (`ROOM_IDENTITY_KEY`, `rooms.rs:45`) is replaced by an authenticated member projection supplied by the local daemon. Bedrock tokens remain daemon-only and never reach browser storage.

## Identifiers and data models

The model splits into the **local nucleus** (Gate 1, `ocean-os`) and the **federated record** (Gate 2, `ocean-bedrock`). Existing types are reused verbatim; only additive, wire-compatible fields are introduced.

### Local nucleus (ocean-os, backward-compatible extensions)

- **`Room`** (`ocean-core/src/lib.rs:695`) — `{ id: RoomKey, name, participants: Vec<RoomParticipant>, created_at, updated_at, trigger_policy: Option<RoomTriggerPolicy>, workspace_root: Option<String> }`. Unchanged.
- **`RoomParticipant`** (`ocean-core/src/lib.rs:662`) — `{ id: String, kind: RoomParticipantKind, display_name: String }`. Unchanged structurally; the doc-comment field "capabilities, transport, and agent profiles are future work" is resolved by the *adjacent private binding*, not by growing this struct. Per the agent-identity boundary, **no owner/sovereignty field is added to `ocean-core::RoomParticipant`**; sovereignty is derived from Bedrock's authenticated principal→participant mapping, not encoded in the local participant.
- **`RoomParticipantKind`** (`:674`) — `Human | Agent | Bot | Tool | System`. Unchanged.
- **`RoomMessage`** (`:766`) — retains `{ seq: u64 (room-scoped, store-assigned), author_id, author_kind, kind: RoomMessageKind, body, created_at }` for local rooms and adds a serde-defaulted `federated: Option<FederatedMessageMeta>` for confirmed federated rows. `FederatedMessageMeta` carries `{ ledger_event_id, global_sequence, source_id, source_sequence, client_event_id, origin_principal_id, origin_member_id }`. Confirmed transcript rows are ordered by `global_sequence`; the additive option keeps existing local-room payloads wire-compatible.
- **`RoomOutboxItem`** — a separate daemon-owned projection for a locally-authored federated event not yet confirmed by Bedrock: `{ client_event_id, source_id, source_sequence, author_member_id, event_type, payload, mention_member_ids, state: pending|failed }`. It is rendered in a distinct pending area and is never inserted into the confirmed transcript before Bedrock assigns a global sequence.
- **`RoomMessageKind`** (`:747`) — `Message | ParticipantJoined | ParticipantLeft | System`. Unchanged.
- **`RoomTriggerPolicy`** (`:642`) — `{ on_mention, on_thread_reply, on_component_event, on_schedule }`. Unchanged.
- **Private local agent binding** — a daemon-private map `participant.id -> local AgentDef`, resolved at convene time via the existing `agentdir::resolve(agents_root(), name)` (`agentdir.rs:226`) and executed through the existing `AgentTurnRequest.agent` path (`ocean-agent-sdk/src/lib.rs:292-299`; live selection at `main.rs:9397-9417`). This is the single load-bearing local addition that joins named agents to room convene. It never serializes `AgentDef.root`, credentials, subprocess env, or `yolo`.

### Federated record (ocean-bedrock, reuse plus additive contracts)

- **Federated room id** — a `RoomKey` mapped to a Bedrock ledger correlation. The shared timeline is keyed by `correlation_id = room_id` (and/or `virtual_path = /rooms/<room_id>`), reusing `longhouse.ledger_events` (`db/002_ocean_ledger.sql`) and `longhouse.ledger_correlations`. No new "room" table for the timeline; the ledger *is* the timeline.
- **Authenticated principal** — a Bedrock token identity, minted by the existing invite exchange and represented by `authenticate -> principal { id, name, role, scopes }` (`src/server.mjs`). A joined human/operator daemon holds one room-scoped `contributor` principal and may register human and agent members that it owns. The existing `agent` token role remains available for headless-agent credentials but is not required for the person-plus-agent flow. Scopes reuse the live path-prefix ACL (`longhouse.auth_tokens.scopes`).
- **Room member** — **net-new table `longhouse.room_members`**: `{ room_id, member_id, owner_principal_token_id, owner_member_id?, actor_type (user|agent), role_in_room, display_name, public_agent_descriptor?, joined_at, removed_at?, removed_by? }`, unique on `(room_id, member_id)`. Bedrock mints the opaque `member_id`; mentions, attribution, and authorization use it instead of a local folder name. A human row has no `owner_member_id`; an agent row points to its owning human member and shares that human's authenticated principal. The table stores durable membership only—never presence.
- **Public agent descriptor** — the non-secret subset of `GET /v1/agents` (`agents_list`, `main.rs:2963-2990`): `{ display_name, description, model_alias, skills_count, subagent_names }`, attached to an opaque Bedrock `member_id`. It lets other machines render and address the agent without receiving its local folder name/path, credentials, tool configuration, permission posture, or execution capability.
- **Ordered room event** — a `longhouse.ledger_events` row with `correlation_id = room_id`, `virtual_path = /rooms/<room_id>`, a Bedrock-minted event id, `event_type`, actor/member attribution, canonical `mention_member_ids`, payload, and the existing global `sequence`, `prev_hash`, and `hash`. The producer stream is unique: `source_id = room:<room_id>:member:<member_id>:producer:<stable_daemon_instance_id>` and `source_sequence` is a persisted monotonic counter within that stream. Retries reuse the same source tuple and `client_event_id`; a duplicate source tuple with different content must return `409 Conflict`, never the prior event.
- **Cursor** — two-level. Bedrock adds `after_sequence` to ledger reads and subscriptions; the local daemon SSE retains its existing `Last-Event-ID` replay over `AgentEventBus`. Each bridge persists the highest confirmed Bedrock global sequence per room and each producer's next source sequence.
- **Presence semantics** — presence is not membership. Gate 1 makes no online/away claim and labels the roster as durable members. Gate 2 derives human presence from an active authenticated Bedrock subscription lease for the owning principal; an agent is “available” only while that lease is active and the owning daemon advertises a valid private binding. Leases are volatile and expire after the heartbeat timeout; `room_members` stores no presence field.

### Data that must not leak

`AgentDef.root`, resolved provider credentials, `subprocess_capability.env`, `yolo`, and per-turn permission secrets are local-only by construction: they are never placed in the public descriptor, never serialized into a ledger `payload`, and never sent over any fanout. The public descriptor is intentionally a strict subset of `GET /v1/agents`.

## End-to-end flows

### Gate 1 flows (single machine, honest nucleus)

- **Create.** `POST /v1/rooms/persistent { key, name, workspace_root?, trigger_policy: { on_mention: true } }` (`room_create`, `main.rs:6185`). The room is bound to a real `workspace_root` (OCEAN-260) so agent turns run in an actual repo.
- **Add named agents.** A human adds Agent participants via the existing `POST /v1/rooms/persistent/{key}/participants { kind: Agent }` (`room_join`, `main.rs:6292`). The surface offers a discovery picker backed by the real `GET /v1/agents` so the human selects from real folder-as-agent names rather than typing free-form text. Each added agent id is recorded in the daemon's private local binding, resolving to a real `AgentDef`.
- **Join (two clients).** Two surface clients (two distinct local identities) each `POST .../participants { kind: Human }`. Both open the room-scoped SSE stream.
- **Message + `@mention` + live reply.** Client A posts `@researcher summarize the plan`. `room_post_message` appends the line (local `seq`), parses the mention, resolves `researcher` against the roster, evaluates `on_mention`, and — because `researcher` is in the local binding — spawns a real turn via `AgentTurnRequest.agent = "researcher"` (not the default assistant). The reply is appended as `author_kind = Agent`. The append emits a room-scoped `Extension { extension: "room_message", scope: <room> }` onto `state.agent_events`; the local SSE rail filters by `?room=<key>` so both clients receive it instantly, without polling.
- **Reconnect.** A client that drops reconnects to the room-scoped SSE carrying `Last-Event-ID`; the daemon replays events strictly after the anchor from the 2048-event ring, then resumes the live tail.

### Gate 2 flows (cross-machine federation)

- **Create + register.** Alice's daemon creates the local room (Gate 1 path) and registers the federated record in Bedrock: `correlation_id = room_id`, Alice's human `room_members` row, and any agent-member rows owned by Alice's principal. The local daemon stores the matching private `member_id -> AgentDef` bindings and its stable producer-stream ids.
- **Invite.** Alice's daemon asks Bedrock to mint one room invite (`POST /api/v1/invites`) scoped to `/rooms/<room_id>` with `role = contributor`, an expiry, and single-use semantics. The invite grants Bob's daemon authority to join Bob as a human and register agent members owned by Bob; it does not transfer any agent profile or secret.
- **Redeem.** Bob redeems the code on his machine (`POST /api/v1/invites/redeem`, public/no-auth endpoint). Bedrock validates the invite and atomically mints the scoped contributor token in the existing `SELECT ... FOR UPDATE` transaction. The consumed invite cannot be redeemed again.
- **Join.** Bob's daemon uses that token to create Bob's human member and Bob's agent member(s), each linked to Bob's `owner_principal_token_id`; it records private `member_id -> AgentDef` bindings only for agents it owns, hydrates its local roster, and opens the authenticated room subscription.
- **Message commit.** Alice's surface submits a line to her local daemon. The daemon writes a `RoomOutboxItem` and exposes it only in the separate pending area, then posts the Ordered Room Event with Alice's producer-scoped source tuple and canonical mention member ids. Bedrock validates active membership and actor ownership, assigns the global sequence, persists the event, and fans it out to every subscriber—including Alice. Each daemon appends that confirmed event to its local transcript in global-sequence order, removes any matching outbox item by `client_event_id`, and emits the confirmed row on its local room SSE. No provisional line is inserted into the confirmed transcript, so concurrent posts never require visible transcript reordering.
- **`@remote-agent`.** Mention addressing uses the target's opaque `member_id`, not a display name. Bedrock validates that every target is an active member. Daemons evaluate triggers only after ingesting the confirmed Bedrock event and journal `(ledger_event_id, target_member_id)` before execution. Alice's daemon finds no private binding for Bob's agent and does nothing; Bob's daemon finds the binding and convenes exactly once.
- **Reply.** Bob's agent runs on Bob's machine through `agentdir::resolve` and `AgentTurnRequest.agent`. Its reply follows the same outbox → Bedrock commit → confirmed fanout path under Bob's agent member id. Agent-authored messages retain the existing anti-loop rule and are never evaluated as new triggers.
- **Reconnect.** Bob's bridge reconnects with its persisted `after_sequence`. Bedrock performs the race-free snapshot/live cutover defined below, replaying every authorized event after the cursor before live delivery. Deduplication by ledger event id and producer source tuple makes replay idempotent. Bob's surface independently reconnects to the local room SSE via `Last-Event-ID`.
- **Revoke.** Alice removes Bob's active `room_members` records or revokes Bob's scoped token. Removing membership is distinct from revoking the already-consumed invite. Bedrock revalidates token and active membership on every room operation and at each subscription heartbeat, emits a terminal `revoked` frame, and closes Bob's open stream within one heartbeat. Replay, read, and write subsequently return 401/403; remaining members receive a `room.member_revoked` event.

## Cross-cutting guarantees

- **Ordering.** Bedrock's global ledger sequence is the only confirmed display order for a federated room. Locally-authored events remain in the separate outbox until the authoritative event returns; only confirmed events enter the transcript. The Bedrock endpoint emits matching room events monotonically by global sequence (gaps are normal because the sequence is global across all ledger events), and each daemon appends those confirmed rows in that order. The local SQLite `seq` is only the projection's append position, never a federation idempotency key.
- **Idempotency.** Bedrock's live `(source_id, source_sequence)` unique index is reused with producer-scoped ids, not `room_id`. A producer persists its monotonic sequence and retries with the same tuple plus `client_event_id`. The same tuple and content returns the existing event; the same tuple with different content returns `409 Conflict`. Two daemons may both post local counter `1` because their `source_id` values differ.
- **Deduplication.** The ledger source tuple prevents duplicate append; each daemon deduplicates confirmed delivery by Bedrock event id/global sequence; the surface deduplicates local SSE frames by SSE event id. The hash chain detects history corruption but is not treated as a deduplication mechanism.
- **Authorization.** Every Bedrock room read, replay, subscribe, membership mutation, and write requires all three checks: valid token, path scope covering `/rooms/<room_id>`, and an active `room_members` row authorizing that operation. Writes additionally require the event's `member_id` to be owned by the caller's principal. `ledgerEventVisibleToPrincipal` remains the per-row scope/clearance filter but does not replace active membership.
- **Local execution ownership and exactly-once convene.** A daemon convenes an agent only when its private `member_id -> AgentDef` binding exists. Trigger evaluation occurs only on confirmed Bedrock events, and a durable processed-trigger journal keyed by `(ledger_event_id, target_member_id)` prevents replay or reconnect from running the same agent turn twice.
- **Anti-loop.** The outbound bridge posts only locally-authored outbox items and never reposts an ingested Bedrock event. Agent-authored events remain trigger-ineligible. Origin metadata and the processed-trigger journal prevent ingest → convene → repost amplification.
- **Backpressure.** Bedrock uses a bounded per-connection live queue, while the durable ledger—not an in-memory ring—is the recovery authority. Queue overflow sends `resync_required { after_sequence }` and closes the stream. The daemon pages durable events after its last confirmed sequence, then resubscribes; no frame is silently dropped.
- **Offline / reconnect.** The daemon persists its outbox, producer counters, and highest confirmed room sequence. Pending writes retry idempotently after restart. A reconnect uses the high-water snapshot/live protocol below; terminal auth or membership failure stops retry and marks the room access-revoked.
- **Observable failures.** Bedrock auth/membership failure becomes explicit `failed: auth`; producer-key conflict becomes `failed: conflict`; write/connectivity failure leaves an outbox item pending or failed with retry available; `resync_required` becomes a visible recovering state. No failure is rendered as an empty healthy room.

## The net-new hardest piece: Bedrock authenticated realtime fanout

The single hardest net-new component is **authenticated, durable, cross-machine realtime fanout on the `ocean-bedrock` server**. Everything else in Gate 2 is reuse or a thin additive: invite/token identity is live, scoped authorization is live, the ordered hash-chained ledger is live, the daemon's outbound-client posture is natural (every ocean-os client already dials a server). What does not exist is a realtime push endpoint on the authenticated Bedrock plane.

**Why the ceremony WebSocket is only a frame-shape reference.** The ceremony WS (`scripts/ocean-local-app.mjs:892-938`) is a working, self-tested protocol with exactly the right frame shape — `hello` → `snapshot` (events after an `after` seq) → live `event` frames → `heartbeat`, with reconnect via `?after=<seq>` and private-visibility stripping. But it is:

- **loopback-only** (`isLoopbackSocket` rejects non-`127.0.0.1`/`::1`/`::ffff:127.0.0.1`);
- **unauthenticated** (same-origin/loopback trust, no bearer token);
- **in-memory and non-durable** (an in-memory ring buffer of ~1000 renderable events, not backed by Postgres or the ledger);
- **single-machine** (one `ocean-local-app` process).

It is the onboarding transport, not the cross-machine authenticated substrate. Gate 2 must lift its *frame protocol and reconnect semantics* onto the authenticated Bedrock server, backed by the durable ledger, scoped by `ledgerEventVisibleToPrincipal`.

**Required Bedrock addition.** A new authenticated realtime endpoint—`GET /api/v1/ledger/subscribe?correlation_id=<room_id>&after_sequence=<n>` as SSE—that:

1. authenticates the token, verifies `/rooms/<room_id>` path scope, and requires an active `room_members` row;
2. establishes or buffers the live tail before snapshot completion and captures a room-visible high-water mark `H`;
3. sends `hello { room_id, snapshot_high_water: H }`, then exactly the authorized snapshot events with `after_sequence < sequence <= H`, then live events with `sequence > H`; no append racing the snapshot/live transition may be missed;
4. filters every snapshot and live event through `ledgerEventVisibleToPrincipal` plus active membership;
5. uses a bounded per-connection queue; on overflow sends `resync_required { after_sequence: last_delivered_sequence }` and closes so the daemon recovers from the durable ledger;
6. revalidates token and active membership at every heartbeat, sends terminal `revoked`, and closes an invalid subscriber within one heartbeat; and
7. accepts `after_sequence` (or `Last-Event-ID`) on reconnect and resumes from the last confirmed global sequence.

The core Bedrock additions are `longhouse.room_members`, `after_sequence` reads, and authenticated durable fanout, plus hardening existing ledger append so a reused producer tuple with different content returns conflict. The ceremony WebSocket supplies only the frame vocabulary; authorization, durable replay, race-free cutover, revocation, and backpressure are new production contracts.

## Gate 1 — honest local proof

Gate 1 makes the existing single-machine nucleus truthful and live. It is entirely within `ocean-os` and `ocean-surface` and requires no Bedrock.

### G1.1 Two real named local agents

Join room convene to the named-agent selection path. A `kind: Agent` participant added to a room must resolve to a real `AgentDef` via `agentdir::resolve(agents_root(), name)` and execute through `AgentTurnRequest.agent = <name>` (`main.rs:9397-9417`), applying the agent's `effective_tools()` allowlist, `config.model`, `subprocess_capabilities`, and `system_prompt()` — not the daemon default. Two distinct agent ids (e.g. an in-tree `researcher` plus a second configured agent) must produce two distinct, resumable per-(room,agent) sessions with their own tools/models. Acceptance: `@researcher` and `@<second>` produce turns whose tool/model/profile differ and are attributable to their folders, with no fallback to the default assistant when a named agent is resolvable.

### G1.2 Room-scoped realtime (replace 2.5s polling)

Every `append_message` that mutates a room transcript — human lines in `room_post_message` (`main.rs:6351`) and the agent-reply/audit appends in `spawn_room_agent_turn` (`main.rs:6592`) — emits `AgentTurnEvent::Extension { extension: "room_message", payload: { room, seq, author_id, author_kind, kind, body }, scope: <room-scoped> }` onto `state.agent_events`. The `agent_events` SSE (`main.rs:10905` / `should_emit_agent_event`, `:3453`) gains a `?room=<key>` filter arm so a subscriber tails exactly one room's `room_message` events, reusing the existing rail's `Last-Event-ID` replay, 3-second keepalive, lag metrics, and graceful-shutdown termination. The surface's `AGENT_EVENT_NAMES` allow-list gains `room_message` (one test-guarded line). Acceptance: a posted line and an agent reply appear on a second client via SSE within the keepalive window, with no transcript poll running.

### G1.3 Truthful membership, no fake presence

Gate 1 labels the roster as durable **Members** and makes no online/away claim because the local SSE is not authenticated to a participant. Closing a client does not remove membership or fabricate an away state. Live presence begins only in Gate 2, where Bedrock can associate an authenticated subscription lease with an owning principal. Acceptance: the local room shows who belongs to it and renders no online dot, “present,” or “away” label it cannot prove.

### G1.4 Remove or hide misleading Room LiveKit audio

The persistent-room LiveKit audio upgrade (`livekit_token_path_for_room` / `route_livekit_room_call` / `show_livekit_controls` in `rooms.rs`; `LiveKitPanel`) is decoupled from room membership, has no roster/identity binding, and targets the Track-0/call namespace for a persistent key. For persistent rooms it must be removed or hidden until audio is genuinely integrated with room membership. LiveKit remains a separate A/V plane and is explicitly listed under **Non-goals**. Acceptance: a persistent room renders no audio-upgrade control that implies room-integrated voice; the text room has no dead/misleading media affordance.

### G1.5 Two-client acceptance test

A deterministic two-client test: two distinct local identities open the same persistent room; both join as Human; one adds two named Agent participants; a human `@mention` of each agent produces a real distinct agent turn whose reply appears in **both** clients' transcripts live (via SSE, not poll), resumed across mentions, run in the bound `workspace_root`, with truthful Agent attribution. Acceptance: both clients converge to identical transcripts with no polling and no manual refresh.

## Gate 2 — cross-machine federation

Gate 2 builds the Bedrock-mediated federation on top of the Gate 1 nucleus. No Gate 2 work lands in the surface beyond additive wire fields and the allow-list line.

### G2.1 Bedrock room membership

Add `longhouse.room_members { room_id, member_id, owner_principal_token_id, owner_member_id?, actor_type, role_in_room, display_name, public_agent_descriptor?, joined_at, removed_at?, removed_by? }`. Bedrock mints opaque member ids; human and agent rows are linked through `owner_member_id`; membership is active only while `removed_at` is null. Every room read, replay, subscribe, membership mutation, and write checks valid token + path scope + active membership. No presence value is stored.

### G2.2 Invite scoping

Room invites reuse the live invite exchange (`POST /api/v1/invites`, `POST /api/v1/invites/redeem`) with `scopes = ['/rooms/<room_id>']` and `role = contributor`. The redeemed token backs one human/operator principal; that principal may register its own human and agent member rows. The invite endpoints are added to `docs/openapi.yaml` (currently absent). Redeem remains atomic (`FOR UPDATE`) and revoking membership remains distinct from consuming or revoking an invite.

### G2.3 `after_sequence` cursor replay

Add `after_sequence` to `readPostgresEvents` and `GET /api/v1/ledger/events`, filtered by `correlation_id`, `ledgerEventVisibleToPrincipal`, and active room membership. It returns exactly authorized events with `sequence > after_sequence`, in ascending order and pageably, so the bridge can persist a cursor and fill any gap.

### G2.4 Authenticated durable realtime fanout

The net-new hardest piece defined above: an authenticated SSE endpoint with the race-free high-water snapshot/live cutover, durable `after_sequence` recovery, bounded-queue overflow signaling, per-heartbeat token/membership revalidation, and terminal revocation behavior. The ceremony transport contributes frame names only.

### G2.5 Daemon outbound room bridge

In `ocean-os`, add a persistent outbound bridge per federated room: authenticated Bedrock subscriber, producer-scoped ledger poster, durable outbox, persisted per-producer counters and room cursor, confirmed-event merger, local room-SSE emitter, and processed-trigger journal. It never posts an ingested event, never inserts pending items into the confirmed transcript, and convenes only confirmed mentions whose target has a private local binding.

### G2.6 Public descriptor / private binding

A federated Agent member carries an opaque Bedrock `member_id`, its owning human/member link, and a public descriptor. The owning daemon keeps the private `member_id -> AgentDef` binding local; other daemons render and address the public member but cannot resolve or execute it. Local folder names are not federation identities.

### G2.7 Two-machine proof, reconnect, revocation

An end-to-end test across two isolated daemon runtimes backed by one Bedrock: create → invite → redeem → join; concurrent first posts from Alice and Bob both commit despite each local producer counter starting at `1`; Alice mentions Bob's opaque agent member id; Bob's agent runs only on Bob's daemon and replies; both confirmed transcripts converge in global order. Then race an append across snapshot/live cutover, force queue overflow and durable resync, restart a bridge with pending outbox work, and remove Bob while his subscription is open. Pass requires no gaps, duplicates, double-convene, or post-revocation delivery beyond one heartbeat.

## Component contracts

- **Surface (`ocean-surface`).** `Rooms` talks only to the local daemon. It renders durable members, derived presence when supplied, confirmed federated rows ordered by `global_sequence`, and a separate pending/failed outbox; it collects create/invite/join/message/mention intent without contacting Bedrock. Additive wire fields and SSE names are mirrored and guarded by `agent_event_names_cover_all_variants`. No second endpoint, token storage, remote-daemon switcher, or federation authority exists in the surface.
- **Daemon room service (`ocean-os`).** Owns local room state, local `seq`, named-agent convene, room-scoped SSE, private member→AgentDef bindings, confirmed transcript projection, pending outbox, and the outbound room bridge. It remains loopback-bound and accepts no cross-machine connection.
- **Outbound room bridge (`ocean-os`).** A persistent per-room component: authenticated Bedrock SSE subscriber, producer-scoped ledger poster, durable outbox, source counters, confirmed cursor, inbound merger, and processed-trigger journal. Restart and reconnect recover from persisted state; the bridge enforces actor ownership, anti-loop, exact-once convene, and pending-versus-confirmed separation.
- **Bedrock room API (`ocean-bedrock`).** Scoped invite create/redeem; active `room_members` CRUD and removal; idempotent ledger append with producer-conflict detection; membership-filtered `after_sequence` replay; race-free authenticated fanout; token/member revocation that closes active streams.
- **Agent profile (`ocean-agents`).** The `agent.toml`/`instructions.md` folders and manifests that the public descriptor summarizes and the local resolver reads. No runtime enforcement.

## Error handling

- **Auth or membership failures.** Bedrock 401/403 and terminal `revoked` are terminal for that room bridge. The daemon stops retrying, marks access revoked, preserves the local cached history as read-only, and the surface shows an explicit revoked/expired state rather than appearing live.
- **Write failures.** Network/5xx failures leave the item in the durable outbox and retry with the same producer tuple and `client_event_id`. A producer tuple reused with different content is a terminal conflict, not a retry. Pending/failed items remain separate from the confirmed transcript.
- **Fanout gaps and slow consumers.** The durable ledger has no correctness “replay window.” A `resync_required` close, transport error, or non-monotonic delivery makes the bridge page authorized room events after its last confirmed cursor, merge them in order, and only then resume live delivery.
- **Conflicting ownership.** If a daemon posts or convenes for a member not owned by its authenticated principal/private binding, Bedrock rejects the write or the local daemon refuses execution and surfaces a sovereignty error.
- **No silent empty states.** No daemon, no room, pending write, producer conflict, resync, auth failure, or revocation each renders a distinct visible state.

## Security and privacy invariants

1. **No inbound daemon exposure.** Daemons bind loopback only. All cross-machine traffic is daemon → Bedrock (outbound). No daemon accepts a connection from another machine at any gate.
2. **Secrets never leave the machine.** `AgentDef.root`, provider credentials, `subprocess_capability.env`, `yolo`, and per-turn permission secrets are local-only. The public descriptor is a strict, non-secret subset of `GET /v1/agents`.
3. **Identity is Bedrock-minted and revocable.** Each person/operator daemon is authenticated by a scoped Bedrock principal. Human and agent members are explicitly owned by that principal in `room_members`; agent profile secrets remain local. Removing membership or revoking the token terminates active delivery within one heartbeat.
4. **Scope and membership are both enforced.** `/rooms/<room_id>` path scope is necessary but insufficient. Every read, replay, subscribe, membership mutation, and write also requires an active member row; every write must name a member owned by the caller. `ledgerEventVisibleToPrincipal` continues to enforce scope and clearance per event.
5. **Sovereign execution.** A daemon convenes a confirmed agent mention only when the target member has a private local binding. Remote members are rendered and addressable but never resolved or executed off-machine.
6. **Tamper-evident history.** The shared timeline is an append-only hash-chained ledger guarded by `prevent_ledger_event_mutation`; corrections are new events, never mutation.
7. **No participant spoofing.** Federated author and mention identities are opaque Bedrock member ids validated against active ownership. Browser `localStorage` identity is accepted only in clearly local Gate 1 rooms.

## Test strategy

- **Unit.** Named-agent binding and selection; active membership authorization; invite redeem atomicity; producer tuple idempotency; different-content duplicate returns conflict; processed-trigger journal prevents replayed convene; pending-outbox confirmation removes exactly one matching item.
- **Gate 1 integration.** Two-client room-scoped SSE with no poll, two distinct named agents, truthful Members labeling, and the `agent_event_names_cover_all_variants` guard.
- **Bedrock protocol.** Cursor reads are ascending and membership-filtered; an append racing snapshot/live cutover is delivered exactly once; bounded-queue overflow produces `resync_required` and durable recovery; an active subscription closes within one heartbeat of membership/token revocation.
- **Federation.** Two isolated daemons against one Bedrock: create → invite → redeem → join → concurrent posts → remote-agent mention → reply → identical confirmed order. Restart with a pending outbox and a cursor gap; verify retry, replay, no duplicate, and no double-convene.
- **Property and honesty.** Two producer streams may both use source sequence `1` without collision; the same producer tuple cannot represent different content; confirmed order is total; ingest is idempotent; anti-loop holds. Persistent rooms render no fake presence or room-audio upgrade, and the surface opens no second endpoint.

## Migration and cutover

- **Gate 1 is additive and non-breaking.** Local rooms keep their current SQLite transcript and routes. The room-scoped SSE is a new emission + filter; the surface keeps its poll fallback until the SSE is confirmed, then drops the poll. Named-agent convene changes the *selection* inside an existing turn, not the turn contract. Hiding the LiveKit control is a surface-only removal. Existing rooms deserialize unchanged.
- **Gate 2 is opt-in per room.** A room federates only after Bedrock registration. The daemon adds a federated metadata projection, durable outbox, producer counters, confirmed cursor, member bindings, and trigger journal; local-only rooms remain Gate 1. Bedrock's table, cursor, append-conflict validation, and fanout are additive to existing ledger/auth behavior.
- **Identity cutover.** A federated room ignores browser-minted identity. The daemon authenticates to Bedrock, maps the resulting principal to Bedrock member ids, and exposes only safe member projections to the surface; the bearer token remains daemon-only. Local rooms may retain explicitly local self-asserted identity.
- **No partial shims.** Every producer, bridge, local transcript consumer, and surface mirror migrates together for federated metadata and SSE variants. The 2.5-second poll is removed only after room SSE is verified; no deprecated federation path remains.

## Risks

- **Bedrock realtime fanout is the long pole.** It is the only component with no existing authenticated, durable precedent in the repo. It must be built and load-tested independently before the two-machine proof.
- **Ordering and local responsiveness.** Showing a provisional line inline would let concurrent clients disagree until reflow. The design avoids that by keeping pending work outside the confirmed transcript; the UX must make that separation clear without making sends feel lost.
- **Producer identity and exact-once execution.** Lost producer counters, unstable daemon instance ids, or an unjournaled trigger can drop, conflict, or duplicate work. The outbox, counters, stable producer id, confirmed cursor, and processed-trigger journal are one durability unit.
- **Anti-loop under federation.** A bridge that reposts ingested events or triggers before confirmation can amplify traffic or run an agent twice. Only outbox-originated events are posted; only confirmed non-agent events are evaluated; origin and trigger journals are mandatory.
- **Bedrock as a dependency.** Federation shifts liveness for the *shared record* onto Bedrock (already the case for all Bedrock data). Local Gate 1 rooms remain fully functional offline; only federated rooms require Bedrock reachability.
- **Scope creep into canvas/voice.** Multi-writer canvas convergence and LiveKit audio are explicitly deferred under **Non-goals**; they must not be pulled into the rooms slice.

## Non-goals

- Voice/video (LiveKit audio) integrated with room membership.
- Multi-writer / P2P canvas convergence and `RoomUiState` shared surfaces.
- Cross-daemon quorum / marks / "capped external mark weight" federation (Longhouse Kaswentha) — deferred.
- Git-worktree-per-room/turn execution isolation.
- Track-0 projection rooms (`pm`/`writers`/`orch_mesh`/`review`) as federated surfaces.
- Non-`on_mention` triggers wired end-to-end (`on_thread_reply`, `on_component_event`, `on_schedule`).
- A person/account entity in Bedrock (identity remains a named, scoped, revocable token).
- Daemon-to-daemon direct transport or NAT traversal.
- Any new identity/invite/profile/auth machinery inside `ocean-os` or `ocean-surface`.

## Acceptance criteria

### Gate 1 — honest local proof

- G1-AC1. Two named local agents in one room each run as their own `AgentDef` (distinct tools/model/profile via `AgentTurnRequest.agent`), with no fallback to the default assistant when resolvable.
- G1-AC2. A posted message and an agent reply reach a second client over room-scoped SSE within the keepalive window, with no transcript poll active.
- G1-AC3. The roster is labeled as durable Members and renders no online/present/away state it cannot prove.
- G1-AC4. A persistent room renders no audio-upgrade control implying room-integrated voice.
- G1-AC5. The two-client test passes: two identities, two named agents, live `@mention` → distinct real replies visible to both clients, resumed across mentions, run in the bound workspace, with truthful Agent attribution.

### Gate 2 — cross-machine federation

- G2-AC1. `longhouse.room_members` links opaque human and agent member ids to an owning authenticated principal; active membership plus path scope is required for every read, replay, subscribe, mutation, and write.
- G2-AC2. A scoped, expiring, single-use contributor invite is redeemed atomically on a second machine; double-redeem fails, and the joined principal can register only members it owns.
- G2-AC3. `GET /api/v1/ledger/events?after_sequence=<n>` returns exactly the authorized room events after the cursor, ascending and pageably.
- G2-AC4. Authenticated fanout has a race-free high-water snapshot/live cutover, heartbeat, bounded queue, `resync_required`, durable cursor recovery, and active membership/token revalidation.
- G2-AC5. Producer-scoped idempotency allows two daemons to post source sequence `1` without collision; a retry returns the same event, while different content for one tuple returns conflict.
- G2-AC6. Each daemon persists outbox, source counters, confirmed cursor, and processed-trigger journal; pending rows never enter the confirmed transcript, and both confirmed transcripts converge by global sequence.
- G2-AC7. A remote agent is rendered through its public member descriptor and addressed by opaque member id, but executes exactly once and only on the owning daemon through its private binding.
- G2-AC8. Two-machine proof passes: create → invite → redeem → join → concurrent posts → remote-agent mention → reply, with identical confirmed order and no inbound daemon exposure.
- G2-AC9. Reconnect and restart fill the exact durable gap, retry pending writes idempotently, and produce no duplicate event or agent turn.
- G2-AC10. Removing membership or revoking the token stops an already-open subscription within one heartbeat; replay/read/write then fail, while cached local history is visibly read-only.
- G2-AC11. The surface contacts only its local daemon, stores no Bedrock token, and contains no federation authority; its changes are limited to rendering daemon-projected member/provenance/presence/outbox state and collecting intent.
