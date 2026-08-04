# Ocean Rooms — Daily-Driver Implementation Plan

**Branch:** `feat/rooms-slack-workspace` (ocean-surface worktree `rooms-workspace`)
**Status:** draft → executing
**Cap:** 30 minutes to produce this document; per-slice checkpoints at 30-minute cadence.

## Goal

Ship Ocean Rooms as the daily-driver multi-human + multi-agent workspace in the
shared Leptos UI — identical in web/PWA and Tauri desktop. Two humans + two
agents occupy one room, live messages appear on both clients without refresh,
`@mention` wakes the right agent once, reply is correctly attributed, reload
preserves transcript/roster, and the same UI runs web + Tauri.

## Starting state

### Daemon (ocean-os, `main` branch)

Solid. `persistent_rooms.rs` (4,796 lines) has:
- Full CRUD: `POST /v1/rooms/persistent`, `GET` list/detail, `POST/DELETE` participants
- `POST /v1/rooms/persistent/{key}/messages` — append, `@mention` trigger evaluation, real agent turn queuing via `spawn_room_agent_turn`
- `GET /v1/rooms/persistent/{key}/events` — merged SSE tail with `room_message` + `room_access` frames, durable `Last-Event-ID` replay
- `GET /v1/rooms/persistent/{key}/transcript`, `/snapshot`
- Federation: outbox, invite/redeem, register agents, access projection
- Agent loop safety: agent-authored messages never re-trigger

### Surface (ocean-surface, `main` branch)

Solid. `rooms.rs` (2,530 lines) has:
- Wire types: `Room`, `RoomParticipant`, `RoomMessage`, `RoomAccessProjection`, federation types, trigger policy
- REST client: create, list, get, join, leave, post message, outbox retry
- SSE tail with sequence resume, `TailState` (Replaying / Live / Reconnecting)
- `RoomsPanel` — overlay with room list, create form, trigger policy config
- `RoomStage` — active room view: participant roster (local + federated), transcript, mention hints, agent add input, composer
- Access-gated: only `Local` / `Live` access allows writes
- Already wired into `app.rs` via `Rooms::new()` + `RoomStage` / `RoomsPanel`
- CSS in `styles/panels.css` covering rooms-panel, rooms-composer, rooms-chip

### Worktrees

- `rooms-product` (7f499fd, on main): clean, zero diff
- `rooms-workspace` (13343b6, 16 files ahead of main): composer attachments, session-sync quarantine removal, proxy public-login fixes — no rooms UI changes

## What "Slack-quality daily driver" means for G1

| Dimension | Current | Target |
|-----------|---------|--------|
| Channel list | Modal overlay | Persistent sidebar rail |
| Room open | Swaps entire chat surface | Channel rail persists; room content in center |
| Unread state | None | Dot/count per channel, clears on open |
| Message density | One card per message | Consecutive same-author within 5 min → compact |
| System messages | Full-height cards | Compact join/leave/system rows |
| Participant presence | Static roster list | Live dots + agent availability |
| Agent import | None (only join by id) | "Add agent" picker from `GET /v1/agents` |
| Composer | Text input + send | Text + @mention autocomplete + Enter sends |
| Compact layout | Same as desktop | Channel list full-width, tap opens room |
| Keyboard nav | Partial | Full: ↑/↓ channels, Enter opens, Tab to composer, Esc back to list |
| Thread replies | None | Inline thread view off parent message |
| Agent reply attribution | Via message post | Session-attributed (agent identity = session) |

## Daemon gaps (minimum for G1)

### G1-A: Threaded messages

**File:** `crates/ocean-core/src/lib.rs`, `crates/ocean-store/src/lib.rs`, `crates/ocean-daemon/src/persistent_rooms.rs`

Add `parent_seq: Option<u64>` to `RoomMessage` so a reply can link to a parent.
`on_thread_reply` trigger evaluation already exists in the schema but has no wire
path — thread replies make it real.

- `RoomMessage.parent_seq`: `Option<u64>`, default `None`
- `PostMessageRequest.parent_seq`: optional in request body
- `post_message` handler: on `parent_seq`, evaluate `ThreadReply` trigger for all agents on the parent thread
- `GET /v1/rooms/persistent/{key}/transcript`: optional `?parent_seq=N` filter (entire transcript still works; this is additive)

**Owner:** @author (already in ocean-os, knows persistent_rooms)

### G1-B: Session-attributed speak endpoint

**Files:** `crates/ocean-core/src/lib.rs`, `crates/ocean-daemon/src/persistent_rooms.rs`

Agent turn replies need proper attribution. `RoomParticipantKind::Session` plus
a `POST /v1/rooms/persistent/{key}/speak` route where the daemon itself is the
author (the agent turn posts its reply back). The author already scoped this and
reported it compiling in a `feat/ocean-native-crew-chat` worktree.

- `RoomParticipantKind::Session`
- `POST /v1/rooms/persistent/{key}/speak` — daemon-internal, scoped Extension event fanout
- Agent turn reply automatically attributed to the session-bound agent

**Owner:** @author (already has compiling code; commit + push + test)

### G1-C: User-owned agent import surface

**Files:** `crates/ocean-surface-ui/src/rooms.rs`

The daemon already has `POST /v1/rooms/persistent/{key}/members/agents` and
`GET /v1/agents`. The surface needs an agent picker so the room owner can add
their local agent folders to a room. Current "Add agent" input in `RoomStage`
accepts a raw string; wire it to the daemon agent list.

- Fetch `GET /v1/agents` → list of available agent names
- Typeahead/select in the room's "Add agent" section
- POST to `members/agents` on confirm

**Owner:** @builder or @designer (surface-side, no daemon changes)

## Surface slices (ordered, non-overlapping file ownership)

All slices land in `crates/ocean-surface-ui/src/` plus styles.

### Slice 1: Channel Rail (persistent sidebar)

**Files claimed:**
- `crates/ocean-surface-ui/src/channel_rail.rs` (new)
- `styles/panels.css` (add `.channel-rail` section)
- `crates/ocean-surface-ui/src/app.rs` (wire rail into layout, ~20 lines)

**Scope:**
- Persistent left sidebar, always visible when rooms are "open"
- Room list with name, last-message snippet, unread dot, active highlight
- Unread tracking: per-room `last_seen_seq` in a reactive map, dot when `room.updated_at > last_seen_seq` equivalent
- Keyboard: ↑/↓ navigate, Enter open room, Esc close sidebar
- Compact: rail becomes full-width channel list; tap opens room full-width
- Transition: `RoomsPanel` overlay replaced by this rail; `show_rooms` signal toggles rail visibility

**Owner:** @builder
**Checkpoint:** compiling channel rail with room list + keyboard nav; app mounts it

### Slice 2: Room Shell Layout

**Files claimed:**
- `crates/ocean-surface-ui/src/room_stage.rs` (extract from rooms.rs)
- `styles/panels.css` (add `.room-shell`, `.room-content` sections)
- `crates/ocean-surface-ui/src/app.rs` (layout: channel rail | room content)

**Scope:**
- Two-panel layout: channel rail (left, ~260px) + room content (center)
- Room content: header (name, topic, join/leave, back to rooms), roster (collapsible), transcript, composer
- Extract `RoomStage` rendering into a dedicated module (rooms.rs is 2,530 lines)
- Responsive: on compact, rail and room swap via viewport toggle (not both at once)
- Channel switching: clicking a rail item switches `rooms.open_key` without page navigation

**Owner:** @designer
**Checkpoint:** two-panel layout compiling; channel switch works; compact toggle works

### Slice 3: Message Density

**Files claimed:**
- `crates/ocean-surface-ui/src/room_messages.rs` (new, extract transcript rendering)
- `styles/panels.css` (add `.rooms-msg--compact`, `.rooms-msg--grouped`)

**Scope:**
- Consecutive messages from same author within 5 minutes → compact rendering (avatar once, smaller name, tighter spacing)
- System messages (join/leave) → single-line compact rows, not full message cards
- Timestamp headers at conversation gaps (>15 min)
- Day separators
- Scrolling: auto-scroll to bottom on new messages unless user has scrolled up
- "↓ New messages" affordance when scrolled up and new content arrives

**Owner:** @designer
**Checkpoint:** compact message groups rendering; system rows compact; day separators

### Slice 4: Agent Import Picker

**Files claimed:**
- `crates/ocean-surface-ui/src/rooms.rs` (modify `agent_ids_for` + add agent picker)
- `styles/panels.css` (add `.rooms-agent-picker`)

**Scope:**
- Fetch `GET /v1/agents` when "Add agent" is focused
- Typeahead dropdown with agent name + description
- On select: POST to `members/agents` with the agent name
- Roster updates via SSE access projection
- Disable add for non-Local rooms (federation only)

**Owner:** @builder
**Checkpoint:** agent picker fetches list, selects, posts, roster updates

### Slice 5: Composer Polish

**Files claimed:**
- `crates/ocean-surface-ui/src/rooms.rs` (composer section)
- `styles/composer.css` (add `.rooms-composer__mention`)

**Scope:**
- @mention autocomplete: type `@` in composer → dropdown of room participants
- Enter sends message (not newline) unless Shift+Enter
- Send button disabled while in-flight
- Pending outbox items render below composer with retry affordance (already exists, verify)

**Owner:** @builder
**Checkpoint:** @mention autocomplete works; Enter sends; shift-Enter newline

### Slice 6: Daemon Threads + Surface Thread Pane

**Files claimed (daemon):**
- `crates/ocean-core/src/lib.rs` (`RoomMessage.parent_seq`)
- `crates/ocean-store/src/lib.rs` (migration, query by parent_seq)
- `crates/ocean-daemon/src/persistent_rooms.rs` (post_message parent_seq, thread_reply triggers)

**Files claimed (surface):**
- `crates/ocean-surface-ui/src/room_thread.rs` (new)
- `styles/panels.css` (add `.room-thread`)

**Scope:**
- Thread parent_seq field + migration
- Thread trigger: posting with parent_seq fires `on_thread_reply` for agents in the parent's thread
- `GET /v1/rooms/persistent/{key}/transcript?parent_seq=N` returns thread messages
- Surface: clicking "Reply in thread" on a message opens a thread pane (right side, ~360px)
- Thread pane: parent message, replies, thread composer
- Thread indicator on parent message: "N replies" count

**Owner:** @author (daemon), @builder (surface thread pane)
**Checkpoint:** thread parent_seq compiles + migration passes; surface thread pane opens with replies

### Slice 7: Presence Indicators

**Files claimed:**
- `crates/ocean-surface-ui/src/room_presence.rs` (new)
- `styles/panels.css` (add `.rooms-chip__presence` enhancements)

**Scope:**
- Lightweight polling: every 30s, surface POSTs a heartbeat for its identity
- Daemon tracks last-heard per participant in memory (no schema change)
- Roster renders presence dot: green (≤60s), yellow (≤5min), grey (>5min)
- SSE `room_access` frames carry presence map when it changes
- Compact enough for G1; no persistent presence store

**Owner:** @designer
**Checkpoint:** presence dots render on roster; update within 60s

### Slice 8: Integration + Daily-Driver Acceptance

**Files claimed:** none new; integration touches across all slices.

**Scope:**
- Run web build (`./run-surface.sh`) and Tauri build (`./run-tauri.sh`)
- Create test room, join two browser sessions + one Tauri session
- Post messages, verify live SSE delivery to all clients
- @mention an agent, verify turn fires, reply appears, attribution correct
- Reload browser, verify transcript/roster restored
- Compact layout: phone viewport, verify channel list → room navigation
- Keyboard: full Tab/Enter/Esc flow
- Fixed defects get immediate review + recheck

**Owner:** @researcher (execution), @reviewer (gate), @planner (integration)

## File ownership matrix

| File | Owner | Slice |
|------|-------|-------|
| `channel_rail.rs` (new) | @builder | 1 |
| `room_stage.rs` (extract) | @designer | 2 |
| `room_messages.rs` (new) | @designer | 3 |
| `room_thread.rs` (new) | @builder | 6 |
| `room_presence.rs` (new) | @designer | 7 |
| `rooms.rs` (modify) | shared — builder takes agent picker, designer takes composer | 4, 5 |
| `app.rs` (modify) | @builder (layout wire-in) | 1, 2 |
| `styles/panels.css` (modify) | @designer | 1-7 |
| `styles/composer.css` (modify) | @builder | 5 |
| `ocean-core/lib.rs` | @author | 6 |
| `ocean-store/lib.rs` | @author | 6 |
| `persistent_rooms.rs` | @author | 6 |

## Cadence

- **30-minute checkpoints:** each slice commit compiles and doesn't break existing rooms panel
- **Review gate:** @reviewer inspects each checkpoint before the next slice starts
- **Silent lane reassignment:** any unclaimed slice after 1 hour goes to the next available builder
- **Acceptance:** @researcher runs the full two-human + two-agent smoke after slice 8 integration

## Definition of shipped

1. Two browsers + one Tauri see the same room's live messages
2. `@agent-name` in the composer queues a real agent turn, reply appears attributed
3. Agent replies do not re-trigger other agents
4. Reload preserves transcript + roster + unread state
5. Channel rail shows unread dots, clearing on room open
6. Compact layout works on phone viewport
7. Same UI bundle runs in web/PWA and Tauri desktop without platform-specific code paths
