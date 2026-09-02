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
{ "key": "my-room", "name": "My Room", "workspace_root": "/path/to/project" }
```

The `key` is a free-form identifier (sluggish: lower-kebab). `workspace_root` is
optional — when set, agent turns in this room resolve their project and `cwd` from
it. The creating human is automatically added as a `RoomParticipant { kind: Human }`.

The daemon responds with a `Room` entity including the full participant roster,
timestamps, and trigger policy.

### 2. Joining a Room

A human joins an existing room by key:

```
POST /v1/rooms/persistent/{key}/participants
{ "id": "smaths", "kind": "human", "display_name": "smaths" }
```

The daemon adds the participant to the roster and broadcasts a `room_access`
event to all connected SSE subscribers, which replaces the surface's access
projection behind the room-generation guard.

Until invite/redeem routes exist, room membership is open — any surface that
knows the room key can join. This is acceptable for G1 (trusted team use).

### 3. Opening a Room

A surface opens a room by key:

```
GET /v1/rooms/persistent/{key}
```

The response includes the full `Room` entity plus a `RoomAccessProjection`:
- `Local` — the room exists on this daemon; the operator is a participant.
- `Remote` — the room key resolves but the daemon cannot serve it locally.
- `None` — loading or no open room (surface state, never a local discriminator).

The transcript hydrates once from the room entity, then the surface subscribes
to the room-scoped SSE endpoint:

```
GET /v1/rooms/persistent/{key}/events
Last-Event-ID: <last_known_sequence>
```

Both `room_message` and `room_access` frames arrive on this stream. Messages
advance the sequence cursor; access projections replace state without advancing
the sequence. The surface must **never** consume the global agent-event stream
for room data — rooms have their own event namespace.

### 4. Sending Messages

A participant sends a chat message into an open room:

```
POST /v1/rooms/persistent/{key}/messages
{ "content": "Let's fix the map component", "mention_ids": ["builder"] }
```

The daemon appends the message to the transcript, advances the sequence, and
broadcasts a `room_message` SSE frame to all subscribers. The surface renders
pending outbox items outside the confirmed transcript until the SSE frame
confirms delivery.

### 5. Leaving / Closing

Closing a room on the surface means unsubscribing from SSE and clearing local
transcript state. The daemon retains the room and its transcript permanently.
Participants can be removed via `DELETE /v1/rooms/persistent/{key}/participants/{id}`.

---

## Agent Participation

### Binding an Agent to a Room

Agents are selected from the daemon-owned `/v1/agents` identity catalog — never
created as free-text. To add an agent to a room:

```
POST /v1/rooms/persistent/{key}/participants
{ "id": "builder", "kind": "agent", "display_name": "Builder" }
```

The daemon validates that `id` resolves to a known agent identity.

### Agent Wake Policy

Each room carries an optional `RoomTriggerPolicy`:

```json
{
  "on_mention": true,
  "on_thread_reply": true,
  "on_component_event": false,
  "on_schedule": null
}
```

When `on_mention` is true, the daemon wakes the agent when its participant id
appears in a message's `mention_ids`. When `on_thread_reply` is true, replies
in a thread the agent participates in also wake it. `on_schedule` accepts an
optional cron expression for periodic wake-ups.

The daemon's room loop handles wake delivery; the surface only renders the
policy state and provides toggles for the human to configure it.

### Agent Turns in Rooms

An agent turn in a room context resolves its `cwd` and `project_id` from the
room's `workspace_root`. The surface posts via the standard turn endpoint:

```
POST /v1/agent/turns
{
  "session_id": "<room-bound session>",
  "prompt": "@builder review the map PR",
  "room_key": "ocean-surface-map-fix",
  "client_type": "surface-web"
}
```

The daemon resolves the owning project from `room_key → workspace_root` and
injects it into the turn context. The agent's response streams back as SSE
events on the room's event endpoint, rendered as a `room_message` with the
agent's participant id.

---

## Surface Contract

### Browsing Rooms

The rooms rail is a flex column listing rooms ONE PAGE at a time:

```
GET /v1/rooms/persistent            -> { ok, rooms, read_states, next_cursor, has_more }
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
- An unread dot, from the list's own `read_states`
- Open-room selection state (`aria-selected`, roving tabindex across the rows)

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

`.rooms-panel__list` keeps `min-height: 0` with vertical overflow — long room
lists scroll instead of pushing status/actions outside the viewport.

### Transcript Rendering

Messages render as a scrolling transcript with:
- Participant display name + avatar
- Timestamp (relative: "2m ago", "yesterday")
- Message body (markdown)
- @-mentions rendered as colored pills
- Agent messages distinguished by a subtle "agent" badge
- Pending outbox items rendered below the confirmed transcript with a spinner;
  failed items expose the daemon retry action

**Live-follow is intent-aware:** the transcript follows new messages while the
reader is at/near the bottom. Scrolled-up history reading is never yanked; a
zero-height sticky `↓ latest` affordance returns and re-pins. Session switches
always re-pin to the latest message.

### Composer

The message composer is enabled only for `Local` and `Live` access projections.
It supports:
- Plain text with markdown
- @-mention autocomplete from the room's participant roster
- Send on Enter, newline on Shift+Enter
- Pending outbox state with retry on failure

### Federated Rooms (Future)

Federated room outbox items render outside the confirmed transcript. Pending
items are informational; only failed items expose the retry action. Invite and
redeem UI is absent until daemon-owned outbound routes exist. The access
projection returned by a retry applies immediately behind the room-generation
guard.

---

## Onboarding Flow (Human)

### First Join — Web Surface

1. Operator navigates to the Ocean surface PWA (or desktop app).
2. Operator authenticates (username/password → HttpOnly session cookie).
3. Surface opens to the default chat view.
4. Operator opens the Rooms panel (sidebar or command palette).
5. Surface calls `GET /v1/rooms/persistent` — populated room list renders.
6. Operator clicks a room → `GET /v1/rooms/persistent/{key}` hydrates the room
   with access projection and transcript.
7. SSE subscription opens on `GET /v1/rooms/persistent/{key}/events`.
8. Transcript renders; composer enables if access is Local/Live.
9. Operator types a message, hits Enter → message posts, outbox renders pending,
   SSE confirms, outbox resolves.

### First Join — Desktop (Tauri)

Same flow, but the Tauri shell loads the identical `dist/` bundle. Room SSE
streams directly to the daemon without a proxy intermediary.

### Inviting Another Human (Future — G2+)

Invite and redeem routes are daemon future work. For G1, the operator tells the
other human the room key, and they join via the rooms panel's "Join room" input.

---

## Onboarding Flow (Agent)

### Porting an Existing Agent

An agent identity already registered with the daemon (visible in
`GET /v1/agents`) can be added to any room:

1. Human opens the room's participant roster.
2. Human selects "Add participant" → agent picker renders daemon identities.
3. Human selects the agent → `POST /v1/rooms/persistent/{key}/participants`
   with `kind: "agent"`.
4. Daemon validates the agent identity, adds it to the roster, broadcasts
   `room_access`.
5. Agent appears in the roster and mention autocomplete.

### Configuring Agent Wake Behavior

1. Human opens the room settings.
2. Human toggles `on_mention` (wake when @-mentioned) and/or `on_thread_reply`.
3. Surface PATCHes the room's trigger policy.
4. Daemon updates the policy; subsequent mentions in the room will wake the agent.

### Agent's First Turn in a Room

1. Human @-mentions the agent in a message: "@builder review this diff".
2. Daemon resolves: agent is in the room, `on_mention` is true → wake.
3. Daemon creates a room-bound session with `workspace_root` from the room entity.
4. Agent receives the turn context: prompt, room transcript, workspace state.
5. Agent responds; response appears as a room message with the agent's
   participant id and display name.
6. Human sees the agent's response in the transcript, same stream as human
   messages.

### Agent Session Identity

Room-bound agent sessions carry the `room_key` in their turn context. The daemon
persists these sessions under the room's store namespace so transcript history
includes agent turns. The surface does not need to manage agent session lifecycle
beyond posting turns — the daemon owns session creation, wake, and teardown.

---

## What Rooms Are NOT (G1)

- **Not a replacement for the chat/PTY session surface.** Rooms are additive —
  a collaboration layer for teams. The solo agent chat session remains the
  primary coding surface.
- **Not a real-time voice/video space.** LiveKit controls stay outside the room
  lifecycle until explicitly reintroduced behind a reviewed platform contract.
- **Not a federated protocol.** G1 rooms are daemon-local. Federation (remote
  room resolution, cross-daemon messages) is G2+.
- **Not a file-sharing surface.** Room messages carry text; file and image
  attachments are future work.
- **Not a project management tool.** No kanban, no issue tracker, no sprints.
  Rooms carry conversation and agent turns. Workflows that need structured
  tracking belong in the Longhouse or the agent session surface.

---

## Daemon API Summary

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/v1/rooms/persistent` | List rooms (paginated) |
| `POST` | `/v1/rooms/persistent` | Create a room |
| `GET` | `/v1/rooms/persistent/{key}` | Get room + transcript + access projection |
| `GET` | `/v1/rooms/persistent/{key}/events` | SSE stream (room messages + access) |
| `POST` | `/v1/rooms/persistent/{key}/messages` | Send a message |
| `POST` | `/v1/rooms/persistent/{key}/participants` | Add a participant |
| `DELETE` | `/v1/rooms/persistent/{key}/participants/{id}` | Remove a participant |
| `GET` | `/v1/agents` | List known agent identities |
