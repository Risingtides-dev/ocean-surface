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

The `key` is a free-form identifier (sluggish: lower-kebab). `trigger_policy`
is optional and stored verbatim (see Agent Wake Policy). `workspace_root` is
optional on the wire but load-bearing for the product: when set, a room-bound
agent turn resolves its project and `cwd` from it; when absent, the daemon
leaves the room unbound and **every agent turn in it fails closed with
`workspace_unavailable`**. `CreateRoomBody` carries no participant identity,
so creation does not add the current human to the roster. The surface opens a
successful create, but the creator remains outside the roster until they
explicitly select **Join room**, which invokes the ordinary participant join
below.

Status 2026-09-01: the surface's create form sends `key`, `name` and
`trigger_policy` only (`crates/ocean-surface-ui/src/rooms.rs`, `CreateRoomBody`),
so surface-created rooms cannot wake agents until it sends `workspace_root` and
offers a bind control for existing rooms. That is the rooms definition of done,
line 1.4 (`ocean-os/docs/specs/2026-09-01-ocean-rooms-definition-of-done.md`).

### 2. Joining a Room

A human joins an existing room by key:

```
POST /v1/rooms/persistent/{key}/participants
{ "id": "smaths", "kind": "human", "display_name": "smaths" }
```

The daemon adds the participant to the roster and broadcasts a `room_access`
event to all connected SSE subscribers, which replaces the surface's access
projection behind the room-generation guard.

On one daemon, a human who knows the room key can use this local join path.
Across daemons, the owner mints an invite and the joining daemon redeems it;
the invite/redeem flow establishes the federated membership and access
projection rather than calling this participant route directly.

### 3. Opening a Room

A surface opens a room by hydrating from its snapshot, newest page first:

```
GET /v1/rooms/persistent/{key}/snapshot?before_seq=<u64::MAX>&limit=1000
```

The response carries the `Room` entity, the roster, `agent_owners`, one page of
transcript, the paging cursors (`last_seq`, `next_seq`, `prev_seq`, `has_more`),
`closed`, and a `RoomAccessProjection` whose `state` is one of. Access state is
separate from roster membership; `Room.participants` is authoritative for
whether the current human has explicitly joined:

- `local` — the room lives on this daemon and does not require a federation
  bridge. A newly created room can already be `local` before its creator selects
  **Join room**.
- `connecting` — the room is federated and the bridge has not reached the
  remote stream yet.
- `live` — the federated stream is caught up.
- `recovering` — the bridge is replaying from its durable cursor.
- `revoked` — this principal's membership was removed; nothing here is writable.

Older history is fetched by further backward pages (`before_seq`); `after_seq`
and `before_seq` are never sent together (the daemon answers the pair with a
typed 400, `conflicting_transcript_cursors`). `GET /v1/rooms/persistent/{key}`
still serves the room with its oldest page and 404s on a closed room; the
snapshot serves closed rooms too, which is why hydration goes through it.

After hydrating, the surface subscribes to the room-scoped SSE endpoint. Both
cursor forms are omitted until hydration or a live frame has supplied a real
message sequence; an empty room must not invent sequence zero:

```
GET /v1/rooms/persistent/{key}/events[?after_seq=<last_observed_seq>]
[Last-Event-ID: <last_observed_seq>]
```

Three frames arrive on this stream: `room_message` (the only frame with an
`id:`, which advances the sequence cursor), `room_access` (replaces the access
projection behind the room-generation guard without advancing the sequence),
and `room_read_cursor` (the daemon-owned read cursor, see below). The surface
must **never** consume the global agent-event stream for room data — rooms
have their own event namespace.

The daemon owns the read cursor: the surface advances it with
`PATCH /v1/rooms/persistent/{key}/read-cursor` from scroll intent, and unread
state on the rail derives from it, so a reload preserves what was read.

### 4. Sending Messages

A participant sends a chat message into an open room:

```
POST /v1/rooms/persistent/{key}/messages
{ "author_id": "smaths", "author_kind": "human", "body": "Let's fix the map component @builder", "thread_parent_seq": null }
```

The daemon's request struct is `deny_unknown_fields`: there is no `content`
and no `mention_ids`. Mentions are `@id` tokens in `body`, resolved against the
roster; they drive trigger evaluation. `thread_parent_seq` names the parent
message for a one-level thread reply. Session attribution is derived by the
daemon from the path that produced the row, never from the client.

The daemon appends the message to the transcript, advances the sequence, and
broadcasts a `room_message` SSE frame to all subscribers. In a federated room
the post is a 202 outbox row until the ordered remote stream confirms it. The
surface renders pending outbox items outside the confirmed transcript until
the SSE frame confirms delivery; only failed items expose the retry action
(`POST /v1/rooms/persistent/{key}/outbox/retry`).

### 5. Leaving / Closing

Closing a room on the surface means unsubscribing from SSE and clearing local
transcript state. The daemon retains the room and its transcript permanently.
Participants can be removed via `DELETE /v1/rooms/persistent/{key}/participants/{id}`.

---

## Agent Participation

### Binding an Agent to a Room

Agents are selected from the daemon-owned `/v1/agents` identity catalog — never
created as free-text. A roster row alone is not execution authority. The
surface resolves the selected package through
`GET /v1/rooms/persistent/{key}/agents/preview/{package}` and submits the
operator-reviewed activation, context, memory and capability decision through
`POST /v1/rooms/persistent/{key}/agents`.

For the first agent in a Local room, the operator-authenticated
`POST /v1/rooms/persistent/{key}/agents/bootstrap` atomically establishes the
server-derived owner/agent membership needed for that review; it does not
authorize the package by itself. In a federated room,
`POST /v1/rooms/persistent/{key}/members/agents` first registers the
non-authorizing membership projection, after which the same preview and
authorization ceremony applies. Reauthorization uses
`POST /v1/rooms/persistent/{key}/agents/{member}/reauthorize` with a fresh
decision id.

### Agent Wake Policy

Each room carries an optional `RoomTriggerPolicy`, mirrored field for field
between `ocean_core` and the surface:

```json
{
  "on_mention": true,
  "on_thread_reply": true,
  "on_build_failure": false,
  "on_ci_failure": false,
  "on_component_event": false,
  "on_schedule": null
}
```

`on_mention` and `on_thread_reply` are evaluated per non-agent-authored
transcript message; `on_build_failure` and `on_ci_failure` are evaluated per
ingested workspace ledger row (a different lane). `on_component_event` and
`on_schedule` are unwired: the daemon answers a typed 400 (`trigger_unwired`)
for a policy carrying `on_component_event: true` or a set `on_schedule`, by
value, so serializing the defaults is accepted. `on_thread_reply` cannot fire
in a federated room (there is no confirmed federated thread source yet); the
surface must say so rather than offer it there.

`PATCH /v1/rooms/persistent/{key}` replaces the stored policy wholesale, so the
surface always sends the complete object. The daemon's room loop handles wake
delivery; the surface renders the policy state and provides toggles.

### Agent Turns in Rooms

There is no room-aware turn endpoint and no `room_key` on `/v1/agent/turns`.
An agent turn in a room is produced by the daemon: a posted message is
evaluated against the room's trigger policy, and a matching trigger convenes
the bound agent in a room-bound session whose `cwd` and `project_id` resolve
from the room's `workspace_root`. The agent's reply is appended to the
transcript under the agent's participant id and arrives on the room's event
endpoint as an ordinary `room_message`.

A room without `workspace_root` refuses the turn with `workspace_unavailable`;
a room whose history the agent may not read refuses with
`room_history_unavailable`. Today both refusals land only as a generic audit
row; rendering them in the transcript, with a convening indicator while the
turn runs, is the rooms definition of done, line 2.2.

---

## Surface Contract

### Browsing Rooms

The rooms rail lists the rooms the daemon knows about:

```
GET /v1/rooms/persistent?limit=50&cursor=<opaque>
```

The response is `{ ok, rooms, read_states, next_cursor, has_more }`. The
daemon pages (100 by default, 1000 at most). Each room card shows:
- Room name and key
- Participant count (human/agent breakdown)
- Last activity timestamp
- Unread state from the daemon-owned read cursor
- A join/open affordance

The rail scrolls inside its own column with `min-height: 0`; long room lists
never push status or actions outside the viewport. The legacy `.rooms-panel`
markup is gone; the live surface is the rooms workspace in
`crates/ocean-surface-ui/src/rooms_workspace.rs` and its stylesheet.

Status 2026-09-01: the surface does not decode `next_cursor` or `has_more`, so
the hundred-and-first room is invisible, and it refreshes the full list every
eight seconds while mounted. Both are the rooms definition of done, line 1.6.

### Transcript Rendering

Messages render as a scrolling transcript with:
- Participant display name + avatar
- Timestamp, in the reader's local time, relative for the recent past ("2m
  ago", "yesterday") and absolute beyond it; day separators follow the local
  calendar. Status 2026-09-01: times and day separators are UTC rendered as if
  local (definition of done, line 1.9).
- Message body (markdown-lite with a scheme allowlist, no `innerHTML`)
- @-mentions resolved against the roster and rendered as pills
- Agent messages distinguished by a subtle "agent" badge
- One-level threads: replies carry `thread_parent_seq` and open in a thread
  column; a reply whose root fell outside the hydrated window must still be
  reachable (definition of done, line 1.5)
- Pending outbox items rendered below the confirmed transcript with a spinner;
  failed items expose the daemon retry action

**Live-follow is intent-aware:** the transcript follows new messages while the
reader is at/near the bottom. Scrolled-up history reading is never yanked; a
zero-height sticky `↓ latest` affordance returns and re-pins. Session switches
always re-pin to the latest message. Hydration opens at the newest page and
backfills a bounded number of older pages; the member can ask for older
history and can see whether more exists (definition of done, line 1.5).

### Composer

The message composer is enabled only for `Local` and `Live` access projections.
It supports:
- Plain text with markdown
- @-mention autocomplete from the room's participant roster
- Send on Enter, newline on Shift+Enter
- Pending outbox state with retry on failure

### Federated Rooms

Shipped when the daemon has federation configured. With that configuration, a
successful owner invite mint (`POST /v1/rooms/persistent/{key}/invites`) is the
irreversible transition: it registers the Local room with the Bedrock
federation control plane, installs its credential, and leaves the room
federated even if no peer ever redeems the invite. Redemption
(`POST /v1/rooms/persistent/invites/redeem`) attaches the other daemon; it is
not the point at which the source room becomes federated. The UI must therefore
show the arming warning before mint. Without federation configuration,
invite minting answers `503 federation_unavailable` and the room remains Local;
the failed request does not convert it. The bridge keeps a durable cursor on
Bedrock's room stream; the access projection moves through `connecting`, `live`
and `recovering` as it does. Federated outbox items render outside the confirmed
transcript; pending items are informational and only failed items expose retry.
Summaries, artifacts and attachments write to this daemon's store only and stay
writable through `connecting` and `recovering`; the composer, invites and repo
commands need `local` or `live`.

---

## Onboarding Flow (Human)

### First Join — Web Surface

1. Operator navigates to the Ocean surface PWA (or desktop app).
2. Operator authenticates (username/password → HttpOnly session cookie).
3. Surface opens to the default chat view.
4. Operator opens the Rooms panel (sidebar or command palette).
5. Surface calls `GET /v1/rooms/persistent` — populated room list renders.
6. Operator clicks a room →
   `GET /v1/rooms/persistent/{key}/snapshot?before_seq=<u64::MAX>&limit=1000`
   hydrates the newest transcript page and access projection.
7. SSE subscription opens on `GET /v1/rooms/persistent/{key}/events`.
8. Transcript renders; composer enables if access is Local/Live.
9. Operator types a message, hits Enter → message posts, outbox renders pending,
   SSE confirms, outbox resolves.

### First Join — Desktop (Tauri)

Same flow, but the Tauri shell loads the identical `dist/` bundle. Room SSE
streams directly to the daemon without a proxy intermediary.

### Inviting Another Human

Two paths exist. Inside one daemon, the operator tells the other human the
room key and they join via the rail's "Join room" input (`POST
/v1/rooms/persistent/{key}/participants` with `{ id, display_name, kind }`).
Across daemons, when federation is configured, the owner mints an invite. That
successful mint permanently federates the source room. The owner shares
`onboard_url` when the response provides one, or the raw redeemable invite code
when it does not; the other daemon redeems that handoff to attach itself.
Without that configuration the mint is a `503 federation_unavailable` and no
cross-daemon invitation exists. Bedrock-side operator rooms (public discovery
and operator-targeted invites) are landing in ocean-bedrock (#117) and have no
surface yet.

---

## Onboarding Flow (Agent)

### Porting an Existing Agent

This mutating flow is available only through the authenticated browser PWA
proxy. Tauri and the extension render the authorization state read-only because
`authority_mutations_supported_on_this_host` is false on those hosts; they do
not receive or relay the authority credential. In the browser PWA, an agent
package already registered with the daemon (visible in `GET /v1/agents`) can be
authorized for a room:

1. Human opens the room-agent authorization panel and selects the package.
2. In a Local room with no binding, the surface calls
   `POST /v1/rooms/persistent/{key}/agents/bootstrap`; in a federated room it
   calls `POST /v1/rooms/persistent/{key}/members/agents` to establish only
   the non-authorizing roster membership.
3. The surface loads
   `GET /v1/rooms/persistent/{key}/agents/preview/{package}` and displays the
   daemon-derived package digest, owner eligibility, requested/grantable
   capabilities, activation, context and memory choices.
4. Human confirms one exact decision; the surface posts it to
   `POST /v1/rooms/persistent/{key}/agents` with the daemon-derived owner and
   member ids plus a decision id.
5. Only the returned Active binding gives mentions execution authority. The
   agent then appears in the roster and mention autocomplete; suspend, resume,
   reauthorize and revoke operate on that durable binding.

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
includes agent turns. The surface does not post room turns or manage their
session lifecycle: trigger evaluation, room-bound turn creation, wake and
teardown are daemon-owned.

---

## What Rooms Are NOT

- **Not a replacement for the chat/PTY session surface.** Rooms are additive —
  a collaboration layer for teams. The solo agent chat session remains the
  primary coding surface.
- **Not a real-time voice/video space.** LiveKit controls stay outside the room
  lifecycle until explicitly reintroduced behind a reviewed platform contract.
- **Not daemon-local only.** Federation shipped (invites, redeem, the Bedrock
  bridge); the G1 statement that rooms are daemon-local is history.
- **Not a general file-sharing surface.** Room attachments exist (8 MiB, an
  image allowlist for inline display, everything served as a download) but
  rooms carry no sync, no folders and no external sharing.
- **Not a project management tool.** Room artifacts (tasks, decisions,
  knowledge, the room summary) are lightweight and CAS-versioned; there is no
  kanban, no issue tracker, no sprints. Workflows that need structured tracking
  belong in the Longhouse or the agent session surface.

---

## Daemon API Summary

The daemon serves forty room routes; `ocean-os/crates/ocean-daemon/src/main.rs`
(`room_routes()`) and `docs/OCEAN_RUNTIME_OPERATOR_GUIDE.md` are authoritative.
The families the surface calls:

| Family | Routes |
|--------|--------|
| Lifecycle | `GET/POST /v1/rooms/persistent`, `GET/PATCH /v1/rooms/persistent/{key}` |
| Hydration and history | `GET …/{key}/snapshot` (`after_seq` or `before_seq`, `limit`), `GET …/{key}/transcript` |
| Live | `GET …/{key}/events` (`after_seq`, `Last-Event-ID`), `PATCH …/{key}/read-cursor` |
| Messages | `POST …/{key}/messages`, `POST …/{key}/outbox/retry` |
| Roster | `POST …/{key}/participants`, `DELETE …/{key}/participants/{id}` |
| Federation | `POST …/{key}/invites`, `POST /v1/rooms/persistent/invites/redeem`, `POST …/{key}/members/agents`, `DELETE …/{key}/members/{id}` |
| Room agents | `GET/POST …/{key}/agents`, `…/agents/bootstrap`, `…/agents/preview/{pkg}`, `…/agents/{id}` (`reauthorize`, `suspend`, `resume`, `invoke`) |
| Artifacts and summary | `GET/POST …/{key}/artifacts[/{id}[/amend]]`, `POST …/{key}/summarize` |
| Attachments | `GET/POST/DELETE …/{key}/attachments[/{id}]` |
| Workspace and repo | `GET …/{key}/workspace`, `GET/POST …/{key}/workspace/{leaf}` (an allowlisted proxy to the room's Bedrock container: execs, files, ports, secrets, repo bind/clone/build/ci) |
| Identity catalog | `GET/POST /v1/agents`, `GET/PUT/DELETE /v1/agents/{name}` |
