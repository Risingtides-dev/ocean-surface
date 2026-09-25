# Ocean Rooms — Product Contract

Ocean Rooms is a persistent, multi-human + multi-agent collaboration surface.
One room = one named space with a participant roster, an append-only transcript,
and durable state owned by the daemon. The web and desktop surfaces render the
same room data through the same API contract.

**Reference only, never dependencies:** Buzz's VISION_SOVEREIGN + VISION_AGENT
docs, Stitchpad's daemon-side room loop, Slack/Discord interaction patterns.

---

## Room Lifecycle

### 1. Creating a Room

A human creates a room from the surface:

```
POST /v1/rooms/persistent
{ "key": "my-room", "name": "My Room", "trigger_policy": { ... }, "workspace_root": "/path/to/project" }
```

That is `CreateRoomBody` in `rooms.rs`; `trigger_policy` and `workspace_root`
are omitted when unset. The `key` is a free-form identifier (lower-kebab by
convention).

`workspace_root` is the folder the room's work happens in, and it is resolved on
the machine running the **daemon** — not in the browser, which cannot see that
filesystem. It must be an absolute path that already exists there; the daemon
canonicalizes it and refuses anything else with `400 { "ok": false, "error":
"invalid_workspace_root" }`. The surface's create form carries a field for it,
and leaving that field empty creates the room **unbound**.

An unbound room is not a room with a missing convenience: **agent turns in it
fail closed.** The daemon resolves a room-bound turn's project and `cwd` from
the room's `workspace_root`, and with none stored it refuses every turn with
`503 workspace_unavailable` before the agent sees the message — so an @mention
in an unbound room does nothing, however its trigger policy is set.

A room can also be bound, rebound, or unbound after creation:

```
PATCH /v1/rooms/persistent/{key}
{ "workspace_root": "/path/to/project" }   # bind or rebind
{ "workspace_root": null }                 # unbind
```

An absent field leaves the binding unchanged, so a rename can never silently
unbind a working room. The surface renders this beside the room's trigger
toggles, with an explicit notice while the room is unbound, because that is the
condition which makes every trigger above it inert. The bind control requires an
`ocean-os` daemon carrying `workspace_root` on `RoomUpdateRequest`; create-time
binding works against any daemon that has the field on `RoomCreateRequest`.

The daemon responds with a `Room` entity including the full participant roster,
timestamps, trigger policy, and `workspace_root`.

### 2. Joining a Room

The rooms rail lists every room the daemon serves, so a human opens a room first
and then joins it. `join_open` in `crates/ocean-surface-ui/src/rooms.rs` posts
the signed-in identity as a human participant of the OPEN room:

```
POST /v1/rooms/persistent/{key}/participants
{ "id": "smaths", "display_name": "smaths", "kind": "human" }
```

That is `JoinBody` in `rooms.rs`; the surface only ever sends `kind: "human"`
on this route. Leaving is `DELETE /v1/rooms/persistent/{key}/participants/{id}`
(`leave_open`, `remove_participant`); removing a federated member is
`DELETE /v1/rooms/persistent/{key}/members/{member_id}`, which answers the
refreshed access projection directly rather than the `{ok, room}` envelope. When
the live tail delivers a join or leave row, the surface re-reads the room record
(`GET /v1/rooms/persistent/{key}`) for the roster — that is the one remaining use
of the unpaged room GET, and it is not how a room opens.

Reaching a room on ANOTHER daemon goes through invites (below, "Inviting Another
Human"), not through knowing the key.

### 3. Opening a Room

A surface opens a room through the paged snapshot route, anchored at the newest
page (`open_room` → `room_snapshot_url` in `rooms.rs`):

```
GET /v1/rooms/persistent/{key}/snapshot?before_seq=18446744073709551615&limit=1000
-> { ok, room, transcript, access, last_seq, prev_seq, has_more, closed, agent_owners, ... }
```

`before_seq` is `u64::MAX` (`HYDRATION_TAIL_CURSOR`), which pages BACKWARD from
past every stored seq and so names the room's tail; `limit` is the store's
1000-row ceiling (`HYDRATION_TRANSCRIPT_LIMIT`) because the route's own default
is 200. `after_seq` is never sent beside `before_seq` — the daemon answers that
pair with a 400 `conflicting_transcript_cursors`. The snapshot keys are the ones
ocean-os publishes in `docs/contracts/room-wire.json` (`snapshot_keys`).

Older history pages backward from the oldest painted row:

```
GET /v1/rooms/persistent/{key}/snapshot?before_seq=<oldest painted seq>&limit=200
```

The page's `prev_seq` is the next `before_seq` and `has_more` says whether
older rows exist (on a backward page, `has_more` means OLDER rows, not "the room
has more messages"). Hydration walks at most 5 such pages unasked
(`MAX_TRANSCRIPT_CATCHUP_PAGES`, `BACKFILL_TRANSCRIPT_PAGE_LIMIT = 200`); past
that, the `↑ Load older messages` press in `rooms_workspace.rs` reads one page
per press until `has_more` is false. `before_seq=0&limit=1` is a terminal empty
page the surface uses as a roster-only re-read for `agent_owners`.

`last_seq` on the snapshot is where the live tail resumes. The tail is the
room-scoped SSE endpoint (`start_live_tail` in `rooms.rs`):

```
GET /v1/rooms/persistent/{key}/events?after_seq=<last_seq>
```

A room with no messages omits `after_seq`. The surface subscribes to three
frames: `room_message`, `room_access` and `room_read_cursor` — the same three
`sse_events` in `room-wire.json`. Messages advance the resume cursor; access
projections replace state without a sequence. The surface must **never**
consume the global agent-event stream for room data. A soft-closed room
(`closed: true` on the snapshot) is a frozen audit view: it opens no tail and
its composer is shut.

When the tail is down, a successful post falls back to a forward catch-up read
(`refresh_open_transcript`):

```
GET /v1/rooms/persistent/{key}/transcript?after_seq=<resume seq>
-> { ok, transcript, next_seq, has_more, closed }
```

Here `next_seq` is the next `after_seq`; the walk stops when `has_more` is false
or after 5 pages, and the live tail owns everything beyond that.

### Access States

Every successful open carries a required `access` projection
(`RoomAccessProjection` in `rooms.rs`). Its `state` is exactly one of the five
values of `RoomAccessState`, which match `access_states` in ocean-os
`docs/contracts/room-wire.json`:

- `local` — a room on this daemon only, with no federation. Writable.
- `connecting` — federated, link coming up. Banner "Connecting to federated
  room…"; the composer is held.
- `live` — federated and connected. Writable.
- `recovering` — federated, link dropped and coming back. Banner "Recovering
  connection…"; the composer is held.
- `revoked` — the operator was removed. Banner "Access revoked"; the composer
  and the room's local-store controls are held.

The composer gate is `access_allows_writes` and `composer_writes_allowed` in
`crates/ocean-surface-ui/src/rooms_workspace.rs`: writable only for `local` and
`live`, and never on a soft-closed room. `local_store_write_gate` is the looser
gate for writes that land only in this daemon's store (trigger policy,
summaries, artifacts, attachments): everything except `revoked`. The banner
strings come from `access_banner`. Before a projection arrives the surface
holds no access state at all — that absence means "loading or no open room" and
is never a stand-in for a local room.

### 4. Sending Messages

A participant sends a chat message into an open room (`post_message` →
`PostMessageBody` in `rooms.rs`):

```
POST /v1/rooms/persistent/{key}/messages
{ "author_id": "smaths", "author_kind": "human", "body": "@builder fix the map component", "thread_parent_seq": 42 }
```

`thread_parent_seq` is omitted for a root message. There is no mention list on
the wire: a mention is the `@id` text in `body`, inserted by the composer's
mention picker, and the daemon derives who was mentioned. The daemon appends the
row and broadcasts a `room_message` frame; after a successful POST the surface
also runs the forward catch-up read above so the row lands even if the tail is
down.

Federated rooms additionally carry an `outbox` on the access projection: items
waiting for Bedrock to confirm. They render outside the confirmed transcript as
`.rooms-workspace__outbox-item` rows labelled "Pending" or "Failed"; only a
failed item offers "Retry", which posts
`POST /v1/rooms/persistent/{key}/outbox/retry { "client_event_id": ... }` and
applies the returned access projection behind the room-generation guard.

### 5. Leaving / Closing

Closing a room on the surface (`close_room`) bumps the room generation and
clears transcript, access and tail state in one synchronous reset. The daemon
keeps the room. Leaving the roster is the participants DELETE above.

---

## Agent Participation

### Binding an Agent to a Room

Agents are selected from the daemon-owned `/v1/agents` identity catalog — never
created as free text, and the surface never posts an agent through the
participants route. An agent becomes a room participant and gets execution
authority through the operator-authenticated binding ceremony in
`crates/ocean-surface-ui/src/room_agent_authorization.rs`:

```
GET    /v1/rooms/persistent/{key}/agents/preview/{package}   # daemon-owned preview + digest
POST   /v1/rooms/persistent/{key}/agents/bootstrap          # first local agent
POST   /v1/rooms/persistent/{key}/agents                    # authorize a binding
POST   /v1/rooms/persistent/{key}/agents/{member}/reauthorize|suspend|resume
DELETE /v1/rooms/persistent/{key}/agents/{member}           # revoke
GET    /v1/rooms/persistent/{key}/agents                    # inspect bindings
```

Every mutation leaves through one transport seam (`send_authority_mutation`);
the browser PWA's same-origin proxy or the Tauri shell injects the operator key,
and browser code never holds it.

### Agent Wake Policy

Each room carries an optional `RoomTriggerPolicy` (`rooms.rs`), all flags off
by default:

```json
{
  "on_mention": true,
  "on_thread_reply": true,
  "on_component_event": false,
  "on_build_failure": false,
  "on_ci_failure": false
}
```

Four triggers are live. The daemon evaluates `on_mention` and `on_thread_reply`
per non-agent-authored transcript message, and `on_build_failure` /
`on_ci_failure` per workspace ledger row. `on_component_event` and `on_schedule`
are unwired: nothing fires them, and the daemon's write routes answer a 400
`trigger_unwired` for `on_component_event: true` or a set `on_schedule`. The
surface PATCHes the policy WHOLESALE
(`PATCH /v1/rooms/persistent/{key} { "trigger_policy": {...} }`), so it always
sends the complete object.

### Agent Turns in Rooms

The surface does not post agent turns for rooms. Posting a message is the whole
client-side path: the daemon evaluates the room's trigger policy against the new
row and, when an agent should wake, runs the turn itself with `cwd` and project
resolved from the room's `workspace_root`. An unbound room refuses every such
turn with `workspace_unavailable`. The agent's reply arrives like any other row,
as a `room_message` frame on the room's own event stream.

---

## Surface Contract

### Browsing Rooms

The rooms rail is a flex column listing rooms ONE PAGE at a time:

```
GET /v1/rooms/persistent            -> { ok, rooms, read_states, attention, next_cursor, has_more }
GET /v1/rooms/persistent?cursor=<room key>
```

The daemon has paged this route since OCEAN-250. It orders rooms
`updated_at DESC, id ASC` and answers at most `limit` of them —
the surface sends no `limit`, so it takes the store default of 100 —
with `has_more` and a `next_cursor` that is the KEY of the last room on the
page. Replaying that key as `?cursor=` returns the rooms strictly after it in
that order. Both fields are decoded with serde defaults, so a daemon predating
the route (which sends neither) reads as a single complete page and the rail
behaves exactly as it did before.

Each room row shows:
- Room name, behind a `#` channel glyph
- A compact unread count, or `@N` when the authenticated reader has unread
  mentions, from the list's daemon-derived `attention` projection
- Open-room selection state (`aria-selected`, roving tabindex across the rows)

`attention` is sparse, ordered with and bounded to the returned room page. Each
row carries `{room_id, latest_seq, read_seq, unread_count, mention_count}` and
is derived by the daemon from its credential-bound local member plus durable
transcript/read-cursor state. Surface does not parse message text to guess who
was mentioned. Omission from a present projection is authoritative zero; an
absent projection means an older daemon, where the rail falls back to
`read_states` for a binary unread indicator and claims no mention knowledge.

**The end of the loaded list carries a `Load more rooms` press.** It renders on
the parked cursor and on nothing else: a rail already holding every room the
daemon will address parks `None` and grows no control, so the row's presence is
itself the statement that there are more rooms. The press fetches ONE page,
appends the rooms the rail does not already list, and re-parks. Every press
either adds rooms or removes the affordance — a page that adds nothing (which is
what the daemon's fallback to page one produces when the cursor names a room
that has since closed) ends the paging rather than re-offering itself.

**Unread refresh polls one page, not every page.** The rail re-reads the list
every 8 seconds to keep the unread dots honest. That poll issues exactly one
request no matter how many pages are loaded: the daemon's order puts every room
with new activity on the first page, so the first page is where unread changes
are. On a rail that has paged, the fresh first page leads and the pages already
loaded are kept behind it, minus any room the fresh page just promoted. The
trade is that a room closed on the daemon while it sits below the fold stays on
screen until an interactive refresh (opening the panel, creating a room,
redeeming an invite) replaces the rail with a fresh first page.

**The paging boundary is re-derived on every retaining poll, never replayed.** A
cursor is a room KEY, and the daemon resolves its place in the order from that
room's *current* `updated_at` — it looks the anchor row up per request. So a
message arriving in the room the cursor names moves that room to the front of
`updated_at DESC` and takes the boundary with it: a press replaying the old key
would ask for the rooms behind the *newest* room, get back a page of rooms
already on screen, and — since a page that adds nothing retires the affordance —
strand every room past the real boundary until an interactive refresh. The rail's
own last row is the boundary instead, and it survives exactly the event that
moves the parked key: a room with new activity is by definition in the fresh
first page, deduped out of the tail, and the row behind it becomes the last.

The rail is the left column of `rooms_workspace.rs`. Its list is
`.rooms-workspace__left-list` (`styles/rooms-workspace.css`), which keeps
`min-height: 0` with `overflow-y: auto` — long room lists scroll instead of
pushing the create field and status line outside the viewport.

### Roster

The members rail lists the open room's participants: avatar, display name, and
a kind badge, with a two-step confirm behind every remove. A federated room's
rail is the access projection's safe member list instead, carrying role, actor
type, a presence dot, and a `yours` chip on agents the caller owns.

**Agent ownership renders (2026-09-02).** Each agent row in the LOCAL roster
says which worker owns it — `owned by <name>`, with the rail's own presence dot
for whether that worker is still in the room — or `unclaimed` when no ownership
row names it. The rows come from `agent_owners` on
`GET /v1/rooms/persistent/{key}/snapshot` (ocean-os#437), decoded as an optional
array with a serde default so a daemon that predates the field still opens rooms
AND stays distinguishable from one that answers an empty list. A closed room's
audit view shows the same ownership, because closing retains the roster and the
ownership rows and the snapshot IS that audit view — a frozen room still says
who owned what and whether they were present when it froze.

`unclaimed` is only ever said on the daemon's authority. An `agent_owners` array
that is present but empty is the daemon answering that nobody owns anything
here; an ABSENT array — a daemon predating ocean-os#437, which may hold durable
ownership rows it cannot project — is no answer at all, and the rail renders no
ownership line rather than badging every agent in every room. The same silence
covers the moment after a binding mutation, before the re-read lands.

Ownership is re-read after the mutations that change it. The daemon's store
inserts an ownership row as part of creating an agent participant, so a
first-agent bootstrap or an authorization leaves the room owned in the database
and stale on screen; both now trigger a roster-only re-read
(`/snapshot?before_seq=0&limit=1`, which the contract defines as a terminal
empty page while the daemon still resolves `agent_owners` from the room's own
lock). It invalidates before it asks, so a re-read that never answers degrades
to silence rather than to a stale claim, and it costs no transcript — the
operator's loaded history is not thrown away to learn who owns an agent.

Two further limits are deliberate. Presence is the daemon's `owner_present`
narrowed by the roster on screen: join, leave and remove replace the room record
from routes that carry no `agent_owners`, so a worker who left after hydration is
never badged present while the rail no longer shows them. And the FEDERATED rail
renders no ownership at all — the daemon joins ownership rows to local
`participants` ids, while a federated row's `member_id` is a bedrock-minted
binding id in a different namespace, so matching one against the other would
mark every federated agent unclaimed rather than say nothing.

### Transcript Rendering

Messages render as a scrolling transcript in `rooms_workspace.rs`, with the
density rules in `crates/ocean-surface-ui/src/room_messages.rs`:
- Timestamps are the viewer's LOCAL wall clock, `HH:MM` 24-hour
  (`local_clock_time`), using the browser's offset for each message's own
  instant so a DST change between two rows is handled. A value that is not
  canonical RFC 3339 shows the raw wire string rather than a guessed time.
- Day separators (`.rooms-workspace__day-separator`) open the transcript and mark
  every change of the viewer's local day (`local_day_key`,
  `day_separator_label`), labelled "Today", "Yesterday" or the `YYYY-MM-DD` date
  (`humanize_day_label`).
- Consecutive messages from the same author within 5 minutes on the same local
  day group under one header (`is_grouped`, `.rooms-workspace__msg--grouped`); a
  silence of more than 15 minutes earns a time header (`needs_gap_header`).
- Join, leave and system rows render as compact single-line rows
  (`is_compact_system_row`).
- Message bodies render as markdown (`room_markdown.rs`); `@id` mentions render as
  `.rooms-md__mention` pills.
- Federated outbox items render below the confirmed transcript (see "Sending
  Messages").

**Live-follow is intent-aware:** the transcript follows new messages while the
reader is at/near the bottom. Scrolled-up history reading is never yanked; a
jump-to-latest affordance returns and re-pins.

### Composer

The message composer is enabled only when `composer_writes_allowed` holds —
`local` or `live` access on a room that is not soft-closed. It supports:
- @-mention autocomplete from the room's roster (`.rooms-workspace__mention-pop`),
  accepted with Enter or Tab
- One-level thread replies (`thread_parent_seq`)
- The federated outbox's Pending/Failed state with retry on failure

### Federated Rooms

A room becomes federated when an invite is minted on it or redeemed into it (see
below); from then on its access state is one of `connecting`, `live`,
`recovering` or `revoked`. The members rail then shows the access projection's
member list instead of the local roster, and confirmed rows carry `federated`
ledger metadata. The access projection returned by an outbox retry applies
immediately behind the room-generation guard.

---

## Onboarding Flow (Human)

### First Join — Web Surface

1. Operator navigates to the Ocean surface PWA (or desktop app) and signs in.
   Browser-hosted Rooms keep join and post unavailable until `/api/config`
   resolves the current user.
2. Operator opens the rooms workspace; the rail calls
   `GET /v1/rooms/persistent` and renders the first page.
3. Operator clicks a room → `GET /v1/rooms/persistent/{key}/snapshot?before_seq=…`
   hydrates the room, its access projection and its newest transcript page.
4. The live tail opens on `GET /v1/rooms/persistent/{key}/events?after_seq=…`.
5. The transcript renders; the composer enables if access is `local` or `live`
   and the room is open.
6. Operator sends → `POST .../messages`, and the row arrives on the tail.

### First Join — Desktop (Tauri)

Same flow, but the Tauri shell loads the identical `dist/` bundle and reaches
the daemon's room endpoints directly rather than through the PWA proxy.

### Inviting Another Human

Invites and redemption are live. Minting is `room_invite.rs`:

```
POST /v1/rooms/persistent/{key}/invites
{ "recipient_name": "Ada", "ttl_minutes": 60 }      # both optional; TTL defaults to 1440, max 10080
-> 201 { code, expires_at, onboard_url?, ... }        # the invite RAW, no {ok} envelope
```

A daemon without federation configured answers 503 `federation_unavailable`,
which the panel shows as a state rather than an error. On a `local` room the
mint registers the room with Bedrock and federates it permanently, so the first
click only arms the control and says so. The code is a bearer grant: it lives in
one signal and the open panel, never in a log line or the rail.

The other human redeems it from their rail's "Invite code…" field
(`room_redeem.rs`):

```
POST /v1/rooms/persistent/invites/redeem
{ "code": "…" }
-> 200 { state, ..., room_key }                        # a RoomAccessProjection plus the joined room's key
```

The surface opens the room named by `room_key`; against a daemon too old to send
it, it diffs the room list before and after and opens the room only when exactly
one appeared. A 403 `invite_forbidden` means the code is spent or refused; other
refusals leave the redemption pending, and re-sending the same code resumes it.

---

## Onboarding Flow (Agent)

### Porting an Existing Agent

An agent package the daemon can preview becomes a room participant only through
the binding ceremony in `room_agent_authorization.rs` (see "Binding an Agent to
a Room"): the daemon previews the package and its digest, the operator makes the
owner choices, and the first agent in a room goes through
`POST .../agents/bootstrap`. The daemon remains authoritative for admission.
Ownership then shows on the roster from the snapshot's `agent_owners`.

### Configuring Agent Wake Behavior

1. Human opens the room's trigger toggles in the workspace rail.
2. Human flips `on_mention`, `on_thread_reply`, `on_build_failure` or
   `on_ci_failure`.
3. Surface PATCHes the COMPLETE trigger policy.
4. Subsequent matching events wake the room's agents.

### Agent's First Turn in a Room

1. Human writes a message containing "@builder review this diff".
2. The daemon sees the mention and, with `on_mention` on, wakes the agent.
3. The turn runs in the room's `workspace_root`; an unbound room refuses it with
   `workspace_unavailable`.
4. The reply lands as a room message from the agent's participant id, on the
   same stream as human messages.

---

## What Rooms Are NOT (G1)

- **Not a replacement for the chat/PTY session surface.** Rooms are additive —
  a collaboration layer for teams. The solo agent chat session remains the
  primary coding surface.
- **Not a real-time voice/video space.** LiveKit controls stay outside the room
  lifecycle until explicitly reintroduced behind a reviewed platform contract.
- **Not federated by default.** A room starts `local` to one daemon. It
  federates through Bedrock only when an invite is minted on it or redeemed
  into it, and that step is permanent.
- **Not a general file-sharing surface.** Room context files are attachments
  (`attachments.rs`: `GET`/`POST /v1/rooms/persistent/{key}/attachments`,
  downloads always served as `application/octet-stream`), and summaries and
  artifacts have their own routes; room messages themselves carry text.
- **Not a project management tool.** No kanban, no issue tracker, no sprints.
  Rooms carry conversation and agent turns. Workflows that need structured
  tracking belong in the Longhouse or the agent session surface.

---

## Daemon API Summary

The routes this surface calls, with the module that calls them. Workspace,
repo, summary, artifact and attachment routes live in `room_workspace_panel.rs`,
`room_repo.rs`, `room_summary.rs`, `room_artifacts.rs` and `attachments.rs` and
are not repeated here.

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/v1/rooms/persistent?cursor=<key>` | List rooms, one page (`rooms.rs`) |
| `POST` | `/v1/rooms/persistent` | Create a room (`rooms.rs`) |
| `PATCH` | `/v1/rooms/persistent/{key}` | Workspace binding or trigger policy (`rooms.rs`) |
| `GET` | `/v1/rooms/persistent/{key}/snapshot?before_seq=&limit=` | Open/hydrate + older pages (`rooms.rs`) |
| `GET` | `/v1/rooms/persistent/{key}/transcript?after_seq=` | Forward catch-up when the tail is down (`rooms.rs`) |
| `GET` | `/v1/rooms/persistent/{key}` | Roster re-read after a join/leave row (`rooms.rs`) |
| `GET` | `/v1/rooms/persistent/{key}/events?after_seq=` | SSE: `room_message`, `room_access`, `room_read_cursor` (`rooms.rs`) |
| `POST` | `/v1/rooms/persistent/{key}/messages` | Send a message (`rooms.rs`) |
| `POST` | `/v1/rooms/persistent/{key}/outbox/retry` | Retry a failed federated send (`rooms.rs`) |
| `PATCH` | `/v1/rooms/persistent/{key}/read-cursor` | Advance the reader's read cursor (`rooms.rs`) |
| `POST` | `/v1/rooms/persistent/{key}/participants` | Join the open room as a human (`rooms.rs`) |
| `DELETE` | `/v1/rooms/persistent/{key}/participants/{id}` | Leave / remove a participant (`rooms.rs`) |
| `DELETE` | `/v1/rooms/persistent/{key}/members/{member_id}` | Remove a federated member (`rooms.rs`) |
| `POST` | `/v1/rooms/persistent/{key}/invites` | Mint an invite (`room_invite.rs`) |
| `POST` | `/v1/rooms/persistent/invites/redeem` | Redeem an invite (`room_redeem.rs`) |
| `GET`/`POST`/`DELETE` | `/v1/rooms/persistent/{key}/agents[...]` | Agent binding ceremony (`room_agent_authorization.rs`) |
| `GET` | `/v1/agents` | List known agent identities |
