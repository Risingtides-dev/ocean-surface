# Ocean Rooms — Design Direction (2026-09-09)

Direction for the Rooms workspace in the Leptos core (`crates/ocean-surface-ui`,
`styles/*.css`) and the transports that carry it (the surface proxy; the Tauri
shell where a desktop-lane slice is named). It supersedes the rendering bullets
of `OCEAN_ROOMS_PRODUCT.md`; `OCEAN_WEB_SURFACE_DESIGN.md` stays the visual
authority and gains the rulings recorded in §7 and §10.

Baseline for every surface slice is `origin/main` at `1b88f86`, the revision
`~/.config/ocean-surface/deployed-rev` says is live on :8790. The local
checkout on `main` (`ce855dd`) is 91 commits behind that, carries an unpushed
drawer refactor, and holds another session's uncommitted device-profile diff
in `app.rs`, `daemon.rs`, and the proxy; nothing here is built on it.
`origin/codex/rooms-clarity-20260908` (copy rewording of the unbound warning)
is superseded by S7, which deletes the warning.

Synthesis of the ops-console proposal (the winner) with the strongest parts of
the programmatic-native and conversation-first proposals. Where a judge named
a fatal flaw, the section that owned it records the answer as a "Ruling". The
daemon dependency this direction relies on (§8, S0) is already in flight in
the ocean-os worktree `cc/rooms-phase2-close`; the shapes quoted below are the
ones that code emits.

## 1. Problem statement

Observed on :8790 (origin/main `1b88f86`), campaigns room, 2026-09-09.

- The transcript had 11 rows; 9 were System rows: `[room agent bootstrap
  audit]` twice, `Room profile created`, `Room profile updated`, `[room agent
  authority audit]`, `Folder shared`, `[room agent admission audit]`,
  `auto-convene: room-builder (mention)`, `[room agent output audit]`. Each
  renders as a full-height `.rooms-workspace__msg--system` row: a literal "S"
  in a gray circle (`rooms_workspace.rs:3763`), name `system`, italic body
  rendered verbatim through `body_view` (`:3790`). `is_compact_system_row` is
  `kind != Message` and `is_grouped` never groups System rows
  (`room_messages.rs`), so nine audits are nine rows. It reads as a log.
- The agent's real reply (seq 50) is a thread reply on seq 47. The timeline
  iterates `partition_thread_messages(...).roots` only (`:3719-3724`) and
  threads default closed, so the reply is reachable only through the
  hover-revealed "Open thread (1)" action (`:3805-3865`).
- Root rows label the author with the raw `author_id` and a two-letter prefix
  of it as the avatar (`:3763-3778`); thread rows use `roster_display_name`
  (`:1245`). Two naming rules in one timeline; federated rooms show Bedrock
  member ids in the main log.
- One human is three roster rows. `smaths`: the proxy login, where
  `/api/config` publishes `user_id == user_display_name == username` (proxy
  `main.rs:1392-1393`; users.json has no display-name field).
  `surface-operator` / "Operator": `SINGLE_OPERATOR_ROOM_ID` (`rooms.rs:109`),
  adopted by `RoomIdentity::from_proxy_config` when `user_id` is empty
  (`:762-770`) and by every direct host, Tauri and the extension
  (`daemon.rs:2731`, `rooms.rs:1121-1127`). `web-18c11f5d551e63f8`: minted by
  a pre-2026-08-26 build; the minter was removed in `79577b9`/`e27c75b`, the
  roster row was never cleaned. ocean-mcp on main adds a fourth, `$USER`
  (`ocean_mcp.rs:76-83`). Live `GET .../inspect` reports `owner.member_id ==
  "surface-operator"` and both agent owners as `surface-operator`, so John
  signed in as `smaths` cannot authorize anything in the browser
  (`room_agent_authorization.rs:557-563` renders "Only the room owner can
  authorize local agents"); ocean-store refuses a second human owner
  (`LocalRoomOwnerConflict`, `lib.rs:3684-3692`) and main has no transfer
  route. The proxy login principal is not the room member id anywhere.
- Package authoring (`crate::agents::AgentBuilder`: name, description, model,
  tools CSV, `instructions` textarea with a 96px min-height) is mounted inside
  the authorization ceremony under a "Choose a package" `<select>`
  (`room_agent_authorization.rs:1750-1760`), inside a 220/236px rail that also
  stacks Members, Invite, Triggers plus the workspace binding form, Summary,
  Artifacts, Files, Repo, Workspace, and a Response Policy text duplicate of
  Triggers (`rooms_workspace.rs:4280-4948`). Authoring a package and
  authorizing a member are two jobs stacked with no hierarchy. The surface
  can add exactly one agent per Local room because bootstrap is gated on
  `bindings.is_empty()` (`room_agent_authorization.rs:538`); the daemon has no
  such rule (`ocean-store::bootstrap_local_room_agent` passes a matching
  owner and inserts the Nth agent participant). `context-cartographer` sits
  on the roster with no owner and no binding and is unreachable from the UI.
- A yellow note "No workspace folder is bound. Agents in this room cannot
  run" derives solely from `Room.workspace_root` (`room_is_unbound`,
  `rooms.rs:1070`; rendered `rooms_workspace.rs:381-386`) while `inspect`
  reports `room-builder` with `cwd_source: resource_grant`, a granted
  read-only folder, three credential slots and a profile. The surface has zero
  references to `/inspect`, `/profile`, or `/resources`.
- The whole app is a centered `max-width: 1120px` column (`base.css:56-63`,
  `--shell-max` in `tokens.css:222`); in a 1568px window a 240px left rail, a
  236px right rail and an optional 380px thread panel share about 640px of
  transcript. The left rail permanently shows a create form with four trigger
  checkboxes, two of them always disabled ("federated rooms only",
  `create_trigger_row_dead_here`, `:231-246`), a "Workspace folder on the
  daemon host" path input, and the join-by-code input (`:3412-3512`).
- The members chip and drawer exist only at or below 1080px; the inline rail
  is `display:none` between 901 and 1080px; above 1081px the roster is a
  permanent rail. Two products by window width.
- The proxy injects `X-Ocean-Operator` only for the `/agents` mutation shapes
  (`main.rs:2876-2896`), and the persistent-rooms wildcard forwards
  get/post/patch/delete but not PUT (`:1209-1216`), so `PUT .../profile` and
  `POST .../resources*` cannot be driven from the PWA today.
- `OCEAN_WEB_SURFACE_DESIGN.md` says control density is a defect and a row of
  same-weight buttons is failure; the rooms page violates both.
  `rooms-workspace.css` carries four border-plus-shadow ban violations (lines
  1946, 2544, 2576, 2646) and two 999px text-bearing controls (`__jump-new`
  2744, `__mention-kind` 2885).

## 2. Product model

Rooms are where Rising Tides work gets done, for humans and agents together.
Most of the team reads and posts through ocean-mcp from Claude Code or Codex;
they open this surface to read the conversation, see who is working, approve
an agent, share a folder, or unblock a stuck turn. The surface is a
conversation in the middle with an operations console one click away: the
transcript is reserved for people and the agents' real replies, and every fact
about authority (who may act, in which folder, with which credentials, since
which decision) is read from the daemon's own projections and shown in one
quiet place, never inferred client-side.

Vocabulary, mapped to the wire.

- **Room**: `Room {id, name, participants, trigger_policy, workspace_root}`
  plus `RoomAccessProjection` (state local|connecting|live|recovering|
  revoked). A Local room's roster is `participants`; every other state's
  roster and mention ids come only from `access.members`.
- **Member**: a human participant. Its id is the person's proxy login
  principal (§3). The display name is cosmetic and resolved by one function.
- **Agent as member**: an agent is a member only when it has a binding
  (`GET .../agents` / `inspect.agents[]`) with status active, suspended or
  stale. `participants[].kind == agent`, `agent_owners`,
  `access.members[].actor_type == agent` and `public_agent_descriptor` are
  display projections that confer nothing. Revoked bindings are absent from
  every roster and every mention list and appear only in the activity ledger.
  Ruling (Phase 1 §11, "absent, not greyed-and-clickable"): an agent
  participant with no binding is listed nowhere as a member and has no row
  action; the owner reaches it through the authorize sheet's package list,
  where a package that is already on the roster is labelled "on roster, not
  authorized" and bootstrap adopts it.
- **Folder**: a Phase 2 resource grant (`inspect.resources[]`: display_name,
  access_mode, status, authorized agents, generation, expiry, granted_by).
  `Room.workspace_root` is the default folder, the third rung of the daemon's
  cwd rule, never a room-wide truth about whether agents can run.
- **Credential slot**: `inspect.credential_slots[]` with status
  resolved|missing|expired|resolver_not_open. Values are never on the wire.
- **Provenance**: every agent-authored row and every agent member card
  carries "acting for {owner display name}" from `binding.owner_member_id`
  resolved through aliases. The transcript is the audit log (Phase 2 §11.3):
  the surface types it and folds it, it never filters it.
- **Federated rooms**: the daemon is the Bedrock member (`self_member_id`).
  Rising Tides has one daemon per human (users.json routes `ecfromthedc` to
  his node), so two humans share a room through Bedrock federation, which
  needs Postgres. This direction renders federated members by display name
  with a mono node chip and keeps invite/redeem reachable; cross-daemon human
  identity beyond that is Phase 3 and needs its own manifest.

## 3. Identity

### 3.1 Rule

One human = one member id = the proxy login principal, the users.json
username (`smaths`, `ecfromthedc`), on every host. The daemon publishes the
same string as its own identity, so a terminal (ocean-mcp), the desktop app,
the extension and the browser converge on one id without any client inventing
one. Display name is a separate, cosmetic field. The surface never mints an
id, never adopts an OS username, never warm-starts an id from localStorage,
and never acts before an id is resolved.

Ruling (answers "the identity ladder still shell-asserts ids"): there is no
placeholder identity and no host-local id file the surface writes. The two
sources are the proxy session (browser) and the daemon's `GET /v1/identity`
(direct hosts, and the proxy's cross-check). An OS username is never a
fallback. `SINGLE_OPERATOR_ROOM_ID`, `RoomIdentity::from_proxy_config`'s
"surface-operator"/"Operator" branch and `RoomIdentity::direct_host` are
deleted; the string `surface-operator` survives only in
`legacy_identity_kind` for display classification.

### 3.2 Sources

`rooms.rs` gains `identity: RwSignal<IdentityState>`:

```
IdentityState = Unresolved
              | Absent { reason: SignIn | ConfigureMember | DaemonMismatch { daemon_member_id } }
              | Resolved { id, display_name, host: ProxyLogin | Desktop | Extension }
```

- Daemon (S0): `GET /v1/identity` (credential-free) answers
  `{ok, member_id: string|null, display_name: string|null, source:
  "member.toml"|"env"|"unset"}` from the same file ocean-mcp already resolves,
  `<OCEAN_CONFIG_DIR or ~/.config/ocean-rs>/member.toml` (`member_id = "..."`,
  optional `display_name`), then `OCEAN_MEMBER_ID`. When neither is set the
  route answers `member_id: null` and never a process user.
- Browser through the proxy: `GET /api/config` publishes `user_id` (the
  session principal), `user_display_name` (a new optional `display_name` per
  users.json entry, fallback username) and `daemon_member_id` (the proxy's own
  call to the resolved daemon's `/v1/identity`, null when the daemon predates
  the route). Session present and `daemon_member_id` null or equal ->
  `Resolved{ProxyLogin}`. Session present and `daemon_member_id` different ->
  `Absent{DaemonMismatch}` and one status line in the composer: "This daemon
  identifies as {daemon} but you are signed in as {user}. Set member_id in
  member.toml on the daemon host." Reads work. Auth-off proxy (no
  users.json): the proxy publishes `user_id = daemon_member_id`; when that is
  null, `Absent{ConfigureMember}` with the member.toml remedy.
- Tauri and the Chrome extension (direct hosts): `GET /v1/identity` straight
  from the daemon (a plain credential-free GET, no `host.rs` seam, CORS
  already trusts both origins). `member_id` null -> `Absent{ConfigureMember}`.
  Ruling (answers "the extension silently goes read-only"): the extension
  keeps posting after S1, as the daemon identity rather than as "Operator".
- Ocean-mcp (S0, in flight): `--member`, then `member.toml`, then
  `OCEAN_MEMBER_ID`; with none, reads work and join/post refuse with the
  member.toml hint. Same file as `/v1/identity`, so a terminal and the
  desktop app on one box are one person.

Every write gate (join, post, summarize, upload, artifact create, authorize,
read-cursor PATCH, invoke) checks `Resolved`. Compatibility shim: `identity_id`,
`identity_name`, `identity_authoritative` survive S1 as Memos derived from
`identity`, so the eight files that read them (`attachments.rs`,
`room_agent_authorization.rs`, `room_artifacts.rs`, `room_markdown.rs`,
`room_repo.rs`, `room_summary.rs`, `room_workspace_panel.rs`,
`rooms_workspace.rs`) compile under `-D warnings` without being edited in S1;
the shim is removed in S8. The self card (§7) shows display name, mono id and
host; the surface has never shown who you are.

### 3.3 Enforcement at the proxy

When auth is on, the proxy refuses any persistent-rooms write whose asserted
human id differs from the session principal with `403 principal_mismatch`.
Checked fields: `author_id` (messages, artifacts), `uploader_id` (attachments
query), `requested_by` (summarize), `invoked_by` (invoke), `actor_id` (close,
workspace lane), and `id` on `POST .../participants` when `kind` is human or
absent. Agent-kind joins and operator-lane bodies are not touched. The daemon
still accepts any string from a non-proxy client; ocean-mcp converges through
member.toml and refuses to act without one. Two humans on one daemon cannot be
told apart by the daemon today; that is recorded in §8 as a gap, not solved
here.

### 3.4 Migration of `surface-operator` and `web-<hex>` rows

The ledger is never rewritten client-side, and the migration runs BEFORE any
surface slice deploys (§9), so there is no window in which nobody can act as
the campaigns owner.

Daemon (S0, in flight in ocean-os `cc/rooms-phase2-close`):
`POST /v1/rooms/persistent/{key}/participants/{participant_id}/retire
{decision_id, successor_id}`, operator lane, replay-safe in the room-wide
decision namespace.

- `{participant_id}` must be `surface-operator` or `web-` plus sixteen
  lowercase hex digits; anything else is `400 participant_not_retirable`. The
  pattern lives in the daemon (`room_retirement::is_retirable_placeholder`),
  which is what keeps the route from becoming an identity-takeover primitive.
  `successor_id` must be a live Human participant (`409 successor_not_human`);
  a placeholder or the same id is `400 invalid_successor_id`.
- One transaction moves the Local room owner role and every
  `room_agent_owners` row from the placeholder to the successor, removes the
  placeholder's roster row, records `from -> to` in
  `room_participant_aliases`, and appends one System row `Participant retired:
  {from} -> {to}` (whitelisted in `room_history_text`). Bindings keep their
  frozen `owner_member_id`; owner and target proofs resolve it through the
  alias chain (`resolve_participant_alias` before comparison), so the
  successor can suspend, resume and re-authorize what the placeholder
  authorized without any decision record being edited.
- Response `{ok, changed, alias: {from, to}, owner_moved, agents_moved,
  owner_member_id, aliases}`. `inspect`, room detail and `snapshot` carry
  `aliases: [{from, to, retired_at}]`.

Ruling (answers "any proxy user can Merge into me"): the surface does not
expose the retire route. It is an operator maintenance action run once per
room from the daemon host (documented in the operator guide) and applied to
campaigns as part of S0's deploy. The surface's only role is display: the
Members tab lists remaining legacy rows under a collapsed "Legacy identities"
disclosure with no action and the sentence "Retire from the daemon host:
participants/{id}/retire". `Rooms::display_name_for(author_id)` resolves
roster -> `access.members` -> binding display names -> `aliases` -> raw id,
so historical rows authored by `surface-operator` render as "smaths" with a
"legacy" marker on hover. Ruling (answers "Remove on the owner row"): no
Remove is offered for a row that holds the owner role or owns a binding.

### 3.5 Agents' member cards

An agent row (Members & Agents tab) and an agent-authored transcript row share
one anatomy: the `icons.rs` Robot glyph in place of an avatar, the binding
`display_name`, the sub-line "acting for {owner display name}", and a status
dot: working (`--accent` pulse from the working signal), idle (none), paused
(`--warn`, suspended), needs re-authorization (`--warn` ring, stale). Mention
chips for agents carry the Robot glyph. Federated agents (from
`access.members` with `public_agent_descriptor`) render the same card with a
mono node chip; they are still not invokable unless a binding exists.

## 4. Transcript vs activity

### 4.1 Two ledgers, one wire

The room keeps exactly one SSE tail and one `transcript` signal. A new pure,
natively tested module `room_timeline.rs` turns that vector into a flat item
list on every change:

```
build_timeline(rows: &[RoomMessage], roster: &RosterView, viewer: &str,
               open_read_seq: Option<u64>, now: DateTime) -> Vec<TimelineItem>

TimelineItem = DayDivider { day }
             | UnreadDivider
             | Message { seq, author: Author, continuation: bool, body,
                         attachment_id, reply_summary: Option<ReplySummary>,
                         inline_replies: Vec<InlineReply> }
             | Activity { first_seq, last_seq, events: Vec<ActivityEvent>,
                          severity: Info | Err }
             | OutboxItem { .. }

Author = { id, display_name, kind: Human | Agent { acting_for: Option<String> } }
InlineReply = { seq, author, body, footprint: Vec<ActivityEvent> }
```

The conversation is the `Message` items; the activity ledger is the
`Activity` items; both come from the same rows, so nothing is lost and
nothing needs a second fetch. `Author` comes from `Rooms::display_name_for`;
root rows, inline replies and thread rows use it, so the raw-id label and the
two-letter avatar disappear everywhere.

### 4.2 Classification: the daemon's closed whitelist

`ActivityEvent::classify(&RoomMessage) -> Option<ActivityEvent>` runs only on
non-Message rows and matches the strings the daemon emits from
`room_history_text` (`persistent_rooms.rs`, the one function every human and
model renderer shares) plus participant kinds. It never parses JSON. The
bracket forms are matched by prefix because the daemon (S0) appends the agent
member id after the fixed label when the audit carries one. Exact table:

| Wire (kind / body) | ActivityEvent | Renders as |
| --- | --- | --- |
| kind `participant_joined` | `Joined{who}` | "{name} joined"; a run folds to "A, B and 2 others joined" |
| kind `participant_left` | `Left{who}` | "{name} left" |
| `[room agent bootstrap audit] {agent}` | `Bootstrapped{agent}` | "{agent} bootstrapped" |
| `[room agent authority audit] {agent}` | `AuthorityChanged{agent}` | "{agent} authority changed" |
| `[room agent admission audit] {agent}` | `Admitted{agent}` | "{agent} admitted" |
| `[room agent admission refused: {code}] {agent}` | `Refused{agent, code}` | "{agent} refused: {code}" in `--err`; severity Err; never folded into a neutral summary |
| `[room agent output audit] {agent}` | `Output{agent}` | "{agent} replied"; closes the agent's working window |
| `auto-convene: {target} ({reason})` | `Convened{target, reason}` | "{target} convened ({reason})"; opens the working window |
| `auto-convene failed for {agent}: turn_failed` | `TurnFailed{agent}` | "{agent} turn failed" in `--err`; severity Err; closes the window |
| `Room profile created` / `Room profile updated` | `ProfileChanged` | "profile created" / "profile updated"; refetches inspect |
| `Folder shared` | `FolderChanged{Shared}` | "folder shared"; refetches inspect |
| `Folder access suspended` / `resumed` / `revoked` | `FolderChanged{verb}` | "folder access {verb}"; refetches inspect |
| `Participant retired: {a} -> {b}` | `IdentityMerged{from, to}` | "{a} retired into {b}"; refetches snapshot aliases |
| `{who} closed the room` / `operator {id} closed the room` | `Closed` | "room closed"; last item; composer replaced by the audit-view notice |
| body starts `workspace ` | `Workspace{line}` | one line in the strip; still handed to `room_workspace_panel::marker_wake` |
| attachment marker (`attachment_id` set) | none | stays a message-like row with inline media (`transcript_media.rs`); never folded |
| any other System row | `Other{body}` | body verbatim in the strip in `--fg-3`; a future un-whitelisted type renders as text, never parsed |

Rows written before S0 carry no agent suffix; `room_history_text` computes
the label at read time from the stored JSON, so after S0 deploys every
existing audit row re-renders with the agent id. The un-suffixed forms stay
matched for daemons that predate S0; they yield the same variants with
`agent: None` and render as "agent admitted" and so on.

### 4.3 Compaction

Consecutive activity events with no `Message` between them and within five
minutes fold into one `Activity` item. Day and unread dividers are barriers.
Grouping is anchored from the newest row, so loading older history never
re-partitions items already on screen. Activity never extends an author
continuation run and never breaks one. Attachment markers never fold.

Turn footprint rule (answers "the inline reply renders above the events that
caused it"): when an activity run directly follows a human root and contains
`Convened{target}`, and `target` authored a thread reply to that root, that
run and any `Output{target}` that directly follows the reply become the
`InlineReply.footprint` and render on the reply's meta line ("room-builder ·
convened by mention · replied in 14s") with the events in the reply's
disclosure. No separate strip is drawn for a turn that produced a visible
reply. A `Refused` or `TurnFailed` never becomes a footprint; it stays a
visible strip with severity Err.

Render: one 28px strip, 12px text in `--fg-3`, a 6px gutter dot in place of
an avatar (`--fg-3` idle, `--accent` while a `Convened` in the group has no
matching reply, `--err` when severity is Err), a summary sentence built from
the events ("Operator joined · room-builder bootstrapped · profile created,
updated · authority changed · folder shared"), a mono time at right, and a
chevron that expands the strip in place to one line per event with a mono
timestamp. Nothing is hidden and everything is one click away. The header
overflow's "Show activity" toggle (persisted `ocean.rooms.activity.v1`)
renders every strip expanded for readers who want the raw ledger. Ruling
(answers "a third representation of activity"): there are exactly two, the
strip and the expanded strip; there is no separate ledger section.

Campaigns fixture (seq 38-51, the live sequence): 38 message (the ghost),
39-40 joins, 41-46 audits, 47 message, 48 admission audit, 49 auto-convene,
50 agent thread reply, 51 output audit. Output: `Message 38`,
`Activity[39-46]`, `Message 47` with `InlineReply 50` whose footprint is
{48, 49, 51}. Two messages, one strip, one inline reply. This is the native
test fixture; the expected summary text is asserted with and without the
agent suffix.

### 4.4 Conversation rows

Same author within seven minutes with no barrier is a continuation: avatar
and name suppressed, timestamp revealed on hover with the coarse-pointer
floor. Avatars are 24px discs: one uppercase letter of the display name for
humans in the five existing hashed hues (`avatar_identity_class`, keyed on the
id so a person keeps a hue across renames), the Robot glyph for agents, no
avatar for activity. Agent-authored rows carry the "acting for {owner}"
sub-line. Bodies still render only through `room_markdown::body_view`.

### 4.5 Agent replies are not hidden

Convened replies arrive as thread replies on the triggering seq. Rule: a
thread reply authored by an agent on a root authored by a human renders inline
under the root as an `InlineReply` (full body, Robot avatar, 32px indent with a
hairline guide, a quiet "Reply in thread" link), up to two per root. Ruling
(answers "the reply collapses the moment a human replies in thread"): inline
replies never fold back; human replies and any further agent replies are
represented by the `ReplySummary` row under them: overlapping avatar stack
(max 3), "N replies", "(N new)" in accent where new means seq greater than the
open-time read snapshot (there is no per-thread cursor on the wire), "last
reply 5m", and an always-visible "Open thread". The thread panel keeps one
level; the Inline/Panel toggle moves onto the thread header and persists in
`ocean.rooms.thread-view.v1`; the Escape ladder is unchanged. ocean-mcp's
`ocean_room_read` still hands these rows over as thread replies; the rule is
written into `OCEAN_ROOMS_PRODUCT.md` and the tool description (S8).

### 4.6 Unread

The unread divider is computed from the read-cursor snapshot taken at room
open (`applied_open_read_seq`, `rooms.rs:3395`), keyed by seq, placed before
the first root whose seq exceeds it and whose author is not the viewer.
Own-authored rows never count as unread. Ruling: agent replies count as unread
like any other row. The cursor stays monotonic and only auto-advances when
the transcript is measured near the bottom; "Mark unread" from a message is a
session-local overlay; "Mark as read" from the sidebar overflow or header is an
explicit user action and PATCHes `latest_seq`. Live-follow only while pinned
near bottom; a centred "N new messages" chip (`--radius-sm`) re-pins.

### 4.7 Working signal

`room_activity.rs` (pure, native-tested):

```
working_agents(transcript, bindings, invokes: &[Invoke202], now)
  -> Vec<WorkingAgent { agent_member_id, since, trigger_seq, last_headline }>
```

A window opens on `Convened{target}` or on the surface's own `POST
.../agents/{m}/invoke` 202 (`request_id`, `session_id`, `message_seq`) and
closes on `Output{target}`, `Refused{target}`, `TurnFailed{target}`, a thread
reply by `target` on `trigger_seq`, or a ten-minute cutoff. With the S0
attributed lines every close is keyed by agent, so two agents convened in one
burst do not confuse each other. Against a daemon that predates S0, an
unattributed `Output` closes the oldest open window; the doc says so and the
UI never claims more. `inspect.agents[].session_exists` refines the display
once loaded. One `Memo` feeds the header attention chip, the composer activity
bar and the Agents rows, so the surfaces cannot disagree. Ruling (answers "a
sidebar working dot for other rooms"): the room list carries no turn state
and only the open room is tailed, so there is no sidebar working dot; the
list-level summary is requested in §8.

## 5. Room details drawer

Ruling (answers "the console is the old kitchen-sink rail with disclosures"):
there is no permanent right rail. The console is one slide-over drawer,
`room_details.rs`, 380px, full-screen at or below 650px, `--bg-raised` +
`--shadow-lg`, no border, `--scrim` backdrop, 280ms slide with the
reduced-motion kill after the state selectors, closed by default, pinnable
inline at or above 1440px only as an opt-in (`ocean.rooms.details-pinned.v1`,
default off). It is opened by the header members pill, the attention chip,
the overflow row "Room details", and deep links `ocean://room/{key}#agents`.
`details_open: RwSignal<Option<DetailsTab>>` persists at
`ocean.rooms.details.v1`. Four plain-text tabs: Members & Agents, Folders,
Profile, Settings. Each tab has at most one primary; every other action is a
ghost or lives in a row overflow. Summary, Notes & tasks (artifacts) and Files
are not tabs: they open from the header overflow as their existing standalone
panels, each with its single primary (Summarize now, New note, Upload). The
Bedrock Repo and Workspace consoles open from one overflow row "Workspace
(federated)" that exists only when `inspect.federated`. Nothing is mounted
twice.

Hydration: one `GET .../inspect` per room open through `room_inspect.rs`
(`RoomInspect` types with string generations, `load_inspect(key)` guarded by
`Rooms::room_is_current(generation, key)` and a latest-wins ticket, into
`scope.inspect: RwSignal<Option<RoomInspect>>`). It refetches when the tail
delivers `Bootstrapped`, `AuthorityChanged`, `ProfileChanged`,
`FolderChanged`, `IdentityMerged` or `Workspace`, and after every operator
mutation. The header summary, the attention chip, every tab and the composer
activity bar read that one signal.

### 5.1 Members & Agents

People: avatar, display name, role ("owner" from `inspect.owner`, member
otherwise), presence dot only when `derived_presence` exists (federated; Local
rooms never show one), "you" chip on `self_member_id` or the resolved
identity, mono node chip on federated rows, hover/touch Remove with a
two-step confirm (federated rows via `DELETE .../members/{id}`, Local rows via
`DELETE .../participants/{id}`, never on a row holding the owner role or a
binding). "Legacy identities" is a collapsed disclosure of Human rows matching
`surface-operator` or `^web-[0-9a-f]{16}$`, read-only, with the retire remedy
sentence (§3.4). Invite is not here; it is a Settings action.

Agents: one row per binding from `inspect.agents[]` (fallback `GET
.../agents`), grouped Active / Paused / Needs re-authorization. Row: Robot
glyph, display name, "acting for {owner}", activation label ("only when
invoked" / "when @mentioned" / "mentions, tasks and thread replies"), the
execution line from `inspect.agents[].execution` ("runs in Campaigns (shared
folder, read)" for `resource_grant` with the resource resolved by id, "runs in
the default folder" for `room_workspace_root`, "cannot run — no folder" in
`--warn` for `unbound` with a link to Folders), a session dot from
`session_exists`, and the status dot (§3.5). Row overflow: Suspend, Resume,
Re-authorize (stale only), Revoke, "Invoke on my last message" (only when the
viewer authored the newest root; `POST .../agents/{m}/invoke {invoked_by,
message_seq}`; the 202 opens a working window), "Preview folder as this agent"
(jumps to Folders). Non-owners see the tab read-only with one sentence "Only
the room owner ({display name}) can authorize agents". "+ Authorize agent" is
the tab's single primary and renders only when `inspect.owner.eligible` (or
`bindings.owner_eligible`, or no owner yet on a Local room) and
`host::room_authority_mutations_supported()`; otherwise it is absent, not
disabled.

Phase 1 authorize flow, step by step. `room_agent_sheet.rs`,
`AuthorizeStep = Pick | Preview | Policies | Review | Submitting |
Done{binding} | Failed{code, message}`; all state lives in
`RoomAgentAuthorizationState` constructed once at workspace scope; every
mutation exits through the single seam `rooms::send_operator_mutation(route,
body)` with an `OperatorRoute`.

1. Pick — `GET /v1/agents` minus packages already bound in this room. Plain
   rows: name left, description and model in `--fg-3` right, "N skills" mono.
   A package already on the roster without a binding is labelled "on roster,
   not authorized". A ghost "Author a new package" link navigates to the
   Agents view (§6); it never embeds the builder.
2. Preview — `GET .../agents/preview/{package}`: display name, member id
   (`== package id` after bootstrap, mono), `definition_digest` (mono,
   truncated, copy), revision, requested capabilities as a plain list,
   unavailable capabilities each with its reason in a disclosure, and one
   sentence: "Grants can only narrow what the package requests. In this phase
   no capability is grantable; the agent is conversational and reads only the
   folders you share under Folders." Because `grantable_capabilities` is
   always `[]`, no checklist is rendered.
3. Policies — three radio groups with the daemon defaults selected and
   nothing else pre-ticked: activation explicit_only / mention /
   task_and_thread, context invocation_only / room_recent / room_history,
   memory none / room, one line of consequence under each. Folders is a
   read-only list of existing grants naming this agent (empty for a new
   agent) with "Share or edit folders after authorizing"; when the resource
   update route exists (§8 gap 6) this step gains checkboxes.
4. Review — a `<dl>` of exactly the body that will be sent (`agent_member_id`,
   `agent_package_id`, `owner_member_id` = the resolved identity, the three
   policies, `room_capability_grants: []`). `decision_id` is minted here with
   `crypto.randomUUID` and held so a retry replays safely. Primary
   "Authorize", ghost "Back", one status line above the buttons.
5. Submit — Local room: ALWAYS `POST .../agents/bootstrap {owner_member_id,
   agent_package_id}` first (201 created or 200 exists; verified against
   `ocean-store::bootstrap_local_room_agent`: a matching owner passes, an
   unowned existing agent participant such as `context-cartographer` is
   adopted, an Nth agent participant is inserted), then `POST .../agents
   {...}`. The surface's `bindings.is_empty()` gate is deleted; this is what
   lets a Local room hold more than one agent. Federated room: bootstrap is
   skipped; preview must already carry `agent_member_id` and
   `owner_member_id`, else the sheet stops with "This agent is not yet a
   member of the federated room".
6. Refusals map to one status line each with the input preserved: 403
   `room_owner_required` -> "Only {owner} can authorize"; 409
   `agent_binding_exists` -> the sheet closes on the existing row; 409
   `decision_replay_mismatch` -> re-mint and retry once; 503 -> "Operator
   lane unavailable on this daemon: operator.key is not readable"; 403
   `foreign_origin_rejected` -> "This host is not in
   OCEAN_OPERATOR_ALLOWED_ORIGINS". Done shows the new row; the ledger shows
   the bootstrap and authority strips; `inspect` refetches.

Lifecycle: Suspend, Resume and Revoke each open a two-line confirm that mints
a fresh `decision_id`. Revoke is a two-step danger action labelled "Revoke —
this cannot be undone; re-adding creates a new member id", and the row
disappears. Re-authorize (stale only) opens a two-panel sheet showing the
binding's digest and revision against the preview's, with the policy radios;
confirm posts `.../agents/{m}/reauthorize` with a fresh `decision_id`. The
daemon has no finer diff and the sheet says so. Never a silent re-pin.

### 5.2 Folders

Rows from `inspect.resources`: display_name, access_mode chip (list / read /
write / execute), status (available `--ok` dot; suspended `--warn` "paused";
revoked rows are hidden and remain in the ledger), "N agents" resolved to
names in a title tooltip, `expires_at` relative in mono, granted_by display
name, generation in mono on hover. Row overflow: Suspend, Resume, Revoke
(two-step), "Preview as {agent}". Preview POSTs `.../resources/{id}/list
{agent_member_id, path}` and renders entries as plain rows (name, kind, size
mono); a file POSTs `/read` and shows the chunk in a read-only mono well with
`next_offset` paging and the footer `via: operator_preview ·
binding_generation {g}`. Every refusal maps to one status line
(`agent_not_authorized_for_resource` -> "{agent} is not authorized for this
folder", `stale_generation` -> "binding changed, reopen",
`path_escapes_root`, `binary_not_supported`, `file_too_large`, ...).

Phase 2 grant flow, "+ Share folder" (the tab's single primary; owner and
host-mutation-capable only):

1. Display name.
2. Folder. Desktop (once the desktop-lane slice D lands): the native chooser
   through a `host::pick_folder()` seam. Browser: a text input labelled
   "Absolute folder on the daemon host" with the daemon's rules quoted
   (absolute, existing, not `/` or the home folder); the value is sent once
   and never shown again because no route returns `local_root`. Ruling: this
   is not the browser asserting authority; the daemon canonicalises,
   validates and audits the root, and the operator lane gates the write.
3. Access mode radios, default read.
4. Authorized agents: checklist of active bindings, default none.
5. Optional expiry.
6. Review, then Submit: mint `decision_id`, `POST .../resources`; 409
   `root_already_granted` names the existing row; 400 `dangerous_root`,
   `local_root_not_found`, `local_root_not_directory` show the daemon's text
   inline with the form preserved.

The "Default folder" row shows `Room.workspace_root` in mono (or "none") with
the cwd rule in one sentence ("used only when an agent has no shared folder"),
a ghost Edit (`PATCH {workspace_root}`, blank unbinds) and "Promote to shared
folder", which pre-fills the grant form with that path and read mode;
migration never happens implicitly. The per-agent "cannot run" line lives on
the Agents rows and in the header summary; there is no room-wide warning.

### 5.3 Profile

Execution summary at the top: one line per bound agent from
`inspect.agents[].execution`; the only warn state is `cwd_source == unbound`,
each naming its remedies ("Share a folder" / "Set default folder"). Repos
rows: alias, remote (mono, never a path), default branch, linked folder by
resource display name. Tools rows: kind chip (mcp / plugin / builtin), name,
allowed list; `installed` renders as "not verified" because the daemon always
answers `unknown`. Credential slots: name in mono, purpose, "required" chip,
resolvers, status dot resolved (`--ok`) / missing (`--err`, "no resolver
satisfied") / expired (`--warn`) / resolver_not_open (`--fg-3`, "resolver not
available on this node"). Agent defaults: one select per bound agent listing
live grants or "room default".

"Edit" opens the profile sheet with tabs Slots / Repos / Tools / Defaults:
slot name validated `[A-Z][A-Z0-9_]*` client-side, resolvers as typed chips
(`env:` / `oauth:` / `keychain:`, with the sentence "keychain resolvers are
not open yet; a required slot with only a keychain resolver will block
admission" at edit time), repos and tools as plain rows, defaults as selects.
Save mints `decision_id` and `PUT .../profile` with the whole profile through
the seam; 201/200 are success, `changed: false` shows "no changes", and the
typed 400 family (`invalid_repo_remote`, `duplicate_repo_alias`,
`invalid_tool_kind`, `duplicate_tool`, `invalid_credential_slot_name`,
`invalid_resolver`, `duplicate_credential_slot`, `resource_not_found`,
`profile_too_large`) renders as a field-level line. Values are never shown or
requested; `secrets/set` on federated rooms stays inside the Workspace panel
and echoes names only.

### 5.4 Settings

Room name (`PATCH`). Trigger policy as two switches, on_mention and
on_thread_reply; build/CI rows are absent on Local rooms and one "federated
rooms only" sentence on federated rooms, never disabled checkboxes; writes
send the full object with every bool present, `on_component_event: false` and
no `on_schedule`. The Response Policy duplicate is deleted. Default folder
(the same control as §5.2, mounted once). Invite (mounts `RoomInvite`; the
irreversible-federation sentence is one status line above its single primary;
the code is shown once and never persisted or logged). Signed in as {display}
· {id} · {host}. Leave. Close room (owner; two-step danger).

### 5.5 Attention chip

`needs_review(inspect, outbox)` counts stale bindings + agents with
`cwd_source == unbound` + required slots not resolved + failed outbox items +
legacy identities present. The header's conditional chip reads "Review N" and
opens the matching tab; while an agent is working it reads "{agent} · 14s"
instead and opens Members & Agents on that row. One slot, one chip, absent
when there is nothing to say. Ruling (answers "needs-review needs N+1
inspect"): the chip is computed for the open room only; the sidebar has no
needs-review section until the list carries a summary (§8).

## 6. Agent authoring leaves the room

`AgentBuilder` (`agents.rs`) is unmounted from every rooms file. It lives on
the Agents view, `agents_workspace.rs`, a sibling of the room stage selected
by `RoomsView::Agents` and reached from the sidebar's pinned "Agents" row, the
palette command `/agents`, and the authorize sheet's "Author a new package"
link. It is a catalog in the plain-row register: rows from `GET /v1/agents`
(name, description and model in `--fg-3`, "N skills" mono, "bound in
#campaigns" from the OPEN room's bindings only, never N+1); one primary "+ New
package"; row overflow Edit, Delete (two-step). The builder panel is
full-height with a real instructions textarea (min-height 240px) and keeps its
write layer and rules unchanged: `/v1/models` picker via `Rooms::models`, tools
free text, `blocks_save` on `subprocess_capability`, read-only name while
editing, empty description/model -> None. Saving never joins or authorizes
anything; after a save the view offers "Authorize in #{open room}", which
opens the S6 sheet at Preview for that package. Authoring and authorizing stay
two jobs with a hierarchy.

## 7. Layout, tokens, density, keyboard, compact

Full-bleed. `base.css` adds `.ocean-surface:has(> .rooms-workspace) {
max-width: none; }`. Verified on origin/main: `<RoomsWorkspace>` mounts inside
a wrapperless `<Show>` as a direct child of `<main class="ocean-surface">`
(`app.rs:2836`), so the selector holds without an `app.rs` hunk; the WASM
bundle's browser floor (Chrome 105 / Safari 15.4) covers Tauri's WKWebView
and the extension. Ruling (answers "repeals §4 without a ruling"): Rooms is
the default-on primary workspace, so the whole shell, header included, is
full-bleed whenever Rooms is shown; the 1120px cap applies to Direct-messages
mode and every non-rooms surface. `OCEAN_WEB_SURFACE_DESIGN.md` §4 records
that sentence in S8. Fallback for an older engine is the cap, not breakage.

Grid: sidebar 248px | stage (flex 1) | thread panel 360px inline at or above
1280px, focus overlay below | details drawer as a slide-over at every width
(pin opt-in at or above 1440px). Breakpoints consolidate to 1440 / 1280 / 960
/ 650; the 901-1080 rail gap disappears because the roster is a drawer
everywhere and the members pill is always in the header. Below 650: one pane
at a time (rooms list page, room page, drawers as full-screen sheets), back
chevron in the header, `env(safe-area-inset-*)` on header and dock, 16px
inputs. All rules at or below 650 live in `styles/compact.css`, landed in the
same slice as the markup they cover. The 380px extension side panel is the
standing narrow proof: no horizontal scroll, the header status line collapses,
activity strips wrap to two lines.

Stylesheets. Ruling (answers "seven new stylesheets"): no new rooms
stylesheet family. `rooms-workspace.css` is rewritten in place, region by
region, collapsing its restated panel recipes onto the §3 vocabulary; the
Agents view's `agents-workspace__*` rules live in the same file because the
view is mounted from the rooms sidebar. The three-place enumeration
(`index.html`, `extension/sidepanel.html`, `scripts/build-extension.sh`) is
not touched; cascade order is unchanged; `rooms-interaction.css` stays the
designer-owned additive lane and is not edited by any slice.

Tokens and type: everything from `tokens.css` and the design doc. Poppins;
room title 18, body 14/1.55 on a 72ch measure (a `max-width` on
`__msg-body`, not padding), secondary 13, metadata and strips 12 in `--fg-3`,
section labels 11/600/0.08em uppercase; `--mono` only for ids, digests,
paths, seqs, timestamps, counts. `--accent` appears exactly on Send,
Authorize, Share folder, the working/live dot, the unread divider, active
room selection and focus rings; approve/join/authorize are primary,
revoke/remove/close are danger, everything else ghost or secondary; `--ok`
connected/resolved/active, `--warn` suspended/stale/reconnecting/expired,
`--err` revoked/failed/missing-required. Radii: 4 on chips, 6 on
inputs/buttons/rows/menus, 10 on the composer dock and sheets; pill only on
avatars, presence dots and the working dot; `__jump-new` and `__mention-kind`
move to `--radius-sm`. Drawers and sheets take the elevation recipe without a
border (fixing 1946/2544/2576/2646). Motion 160/240ms `--ease`; the working
dot breathes with `--glow-thinking`; every animation gets its reduced-motion
kill after the state selectors. Icons only from `icons.rs` (Groups, Cog,
Paperclip, Send, Stop, Robot, Person, Folder, ChevronDown, Close, Refresh);
no emoji.

Density: rows are the plain-row register from `.ocean-more__menu`: title
left, mono metadata right in `--fg-3`, hairline `--border-subtle`
separators, `--bg-hover`, one affordance per row, destructive actions
hover-revealed with a 0.35 opacity floor and a full tap path on coarse
pointers, hit targets at least 20px. No icon+title+subtitle lockups, no
same-weight button rows, one primary per tab or sheet, one overflow per
surface. Sheets (authorize, share folder, profile, preview) are `--bg-raised`
+ `--shadow-lg` slide-overs with the stepper as plain text "2 of 5", one
primary bottom right, Cancel ghost at left, one status line above the buttons.

Sidebar (`room_sidebar.rs`). Top: a filter input ("Jump to room", 16px on
iOS). Two plain primary rows: Rooms (default) and Agents (§6). Room rows keep
the `__room` listbox, roving tabindex and `aria-selected` contract that
`rooms-interaction.css` depends on: `#` glyph (a node glyph when
`inspect.federated` is known), name at 600 weight plus a 6px dot when
`latest_seq > read_seq`, an `@N` chip only when `attention.mention_count >
0`; "Load more rooms". Right-click / long-press overflow: Mark as read, Copy
room key, Leave (hover reveal with the coarse-pointer floor). Below the list
one ghost row "+ New room" opens a `<dialog>` with two tabs: Create (name only;
policy `{on_mention: true, on_thread_reply: true, on_component_event: false,
on_build_failure: false, on_ci_failure: false}`, no `on_schedule`, no trigger
checkboxes, no folder field) and Join by code (mounts `RoomRedeem`). Bottom: the
self card (`__self`): one-letter avatar, display name, mono member id, host
chip ("browser" / "desktop" / "extension"); `Unresolved` renders "Resolving
identity…", `Absent` renders the remedy sentence. The permanent create form,
its trigger checkboxes, the workspace-path input and the always-visible redeem
input are gone; `create_trigger_row_dead_here` is deleted.

Header (`room_header.rs`), 56px raised bar. Row: `# campaigns`, then a mono
summary in `--fg-3` from `scope.inspect` ("3 people · 2 agents · 1 folder ·
3 credentials ok", turning "1 credential missing" in `--warn` or "1 agent
cannot run" when applicable; nothing until inspect loads). Right cluster is
exactly three controls: the members pill (Groups glyph + people and agents
count; toggles the drawer at Members & Agents at every width), the
conditional attention chip (§5.5), and one `...` overflow built from
`.ocean-more__menu`: Room details, Summary, Notes & tasks, Files, Workspace
(federated only), Show activity (checked state), Copy room key, Mark as read,
Leave room, and for owners a hairline-separated Close room. The access banner
(Connecting / Recovering / Revoked) stays as one `.ocean-status` line under the
header.

Composer (`room_composer.rs`): one dock card owning `:focus-within`; a
chromeless autosizing textarea (Enter sends, Shift+Enter newlines, Up-arrow in
an empty composer recalls the viewer's last message as a draft, 16px under
`pointer: coarse`); a second row with `@` and paperclip ghosts at left
(paperclip uploads through the existing attachments client, raw bytes with
query metadata, and the marker appears in the timeline) and Send/Stop in one
fixed slot at right. Drafts persist per (room, thread) in the composer epoch
and clear only after the SSE echo, as today. A reply banner "Replying in
thread to {display name} — preview ×" when a thread is targeted. The mention
popup keeps its selectors and gains a trust sub-line ("agent · acting for
smaths · mention", "you", "not authorized" greyed and unselectable); the popup
and the Enter/Tab accept rule read one `mentionable` Memo built from active
bindings plus humans, so they cannot disagree. Under the dock: the activity
bar "room-builder · working · 14s" from the working signal, opening the Agents
row on click. Non-members see one "Join room" primary; `Absent` identity sees
its status line; closed rooms see the audit-view notice.

Keyboard: roving-tabindex room listbox; Escape ladder sheet -> details
drawer -> thread -> sidebar drawer (compact) -> app reveals in the existing
order; `focus-visible` rings everywhere. Live-state map: working = accent
pulse, connected = `--ok`, reconnecting = `--warn`, revoked/failed = `--err`;
failures are one `.ocean-status` line where the action was taken, details in
the activity strip.

## 8. Daemon contract used

All routes are under `/v1/rooms/persistent/{key}` unless noted. Lane: none |
actor (roster id in body) | operator (`X-Ocean-Operator`, injected by the
proxy or the Tauri native request; browser code never holds the key).

| Method and path | Lane | Used by |
| --- | --- | --- |
| GET `/v1/identity` (S0) | none | identity on direct hosts; the proxy's cross-check |
| GET `/api/config` (proxy) | session | `user_id`, `user_display_name`, `daemon_member_id` |
| GET `/v1/rooms/persistent?limit&cursor` | none | sidebar list, `read_states`, `attention` |
| POST `/v1/rooms/persistent` | none | New room dialog |
| PATCH `{key}` | none | name, trigger switches, default folder |
| POST `{key}/close` | actor | Settings, Close room |
| GET `{key}/snapshot?before_seq` / `?after_seq` (+ `aliases`, S0) | none | room open at the tail, load older via `prev_seq` |
| GET `{key}/events` (SSE `room_access` / `room_read_cursor` / `room_message`) | none | the one room tail |
| GET / PATCH `{key}/read-cursor` | none | unread divider, mark as read |
| POST `{key}/messages` | actor | composer, thread reply |
| POST / DELETE `{key}/participants[/{id}]` | none | join, leave, remove non-owner rows |
| DELETE `{key}/members/{member_id}` | none | remove a federated member |
| POST `{key}/outbox/retry` | none | failed outbox item |
| GET `{key}/inspect` (+ `aliases`, S0) | none | `scope.inspect`: header summary, attention chip, every drawer tab, activity bar |
| GET `/v1/agents` | none | Pick step, Agents view |
| GET `{key}/agents`, GET `{key}/agents/preview/{package}` | none | Agents rows fallback, Preview step, re-authorize comparison |
| POST `{key}/agents/bootstrap` | operator | Submit step, always first on Local rooms |
| POST `{key}/agents` | operator | Submit step |
| POST `{key}/agents/{m}/reauthorize`, `/suspend`, `/resume`; DELETE `{key}/agents/{m}` (body) | operator | Agents row overflow |
| POST `{key}/agents/{m}/invoke` | actor | "Invoke on my last message"; the 202 opens a working window |
| GET / PUT `{key}/profile` | none / operator | Profile tab |
| GET `{key}/resources`, GET `{key}/resources/{id}` | none | Folders tab |
| POST `{key}/resources` | operator | Share folder |
| POST `{key}/resources/{id}/suspend`, `/resume`; DELETE `{key}/resources/{id}` (body) | operator | Folders row overflow |
| POST `{key}/resources/{id}/list`, `/read` | operator | Preview as agent |
| POST `{key}/participants/{id}/retire` (S0) | operator | not called by the surface (§3.4); operator maintenance from the daemon host |
| POST `{key}/artifacts`, `/artifacts/{id}/amend`, GET `/artifacts[/room-summary]` | actor / none | Notes & tasks and Summary panels (unchanged) |
| POST `{key}/summarize` | actor | Summary panel (unchanged) |
| POST / GET / DELETE `{key}/attachments[/{id}]` | actor / none | paperclip, Files panel |
| POST `{key}/invites`, POST `/v1/rooms/persistent/invites/redeem` | none | Invite, Join by code |
| `{key}/workspace*` lanes | actor | Workspace (federated) panel (unchanged) |

Transport work the surface owns (S1): the proxy wildcard accepts PUT;
`room_agent_authority_mutation` becomes `room_operator_mutation` and injects
the key for PUT profile, POST resources, POST resources/{id}/(suspend|resume|
list|read) and DELETE resources/{id}, forwarding DELETE bodies, in addition
to the existing `/agents` shapes; `GET /v1/identity` is forwarded
credential-free; users.json gains `display_name`; `/api/config` gains
`daemon_member_id`; principal enforcement (§3.3). A 503 from any operator
route renders "Operator lane unavailable on this daemon: operator.key is not
readable"; a 403 `foreign_origin_rejected` renders the origins hint. The Tauri
`daemon_operator_request` allowlist (`crates/ocean-tauri/src/lib.rs`, pinned
to six routes by its own test) and a `host::pick_folder()` seam belong to the
desktop lane (slice D); until D lands, Phase 2 writes are absent on the
desktop app (a missing capability renders as absence) and Phase 1 writes keep
working there.

Gaps to request from ocean-os. S0 carries the first four (retirement,
attributed audit lines, ocean-mcp identity are already in flight; the identity
route is the addition this direction needs); the rest are follow-ups with the
waiting behaviour stated.

1. `GET /v1/identity` from `member.toml` / `OCEAN_MEMBER_ID`, null when
   unset, never a process user. Waiting behaviour: direct hosts are
   `Unresolved` and read-only; the proxy publishes `daemon_member_id: null`
   and trusts the session principal alone.
2. `POST .../participants/{id}/retire` with the placeholder pattern enforced
   server-side, `room_participant_aliases`, alias-aware owner and target
   proofs, `aliases` on inspect, detail and snapshot, whitelist row
   `Participant retired: a -> b`. In flight. Waiting behaviour: campaigns
   authority stays with `surface-operator`; no surface slice deploys.
3. Audit lines that carry the agent and the outcome: `[room agent admission
   audit] {agent}`, `[room agent admission refused: {code}] {agent}`,
   `[room agent output audit] {agent}`, bootstrap and authority likewise. In
   flight. Waiting behaviour: strips read "agent admitted"; an unattributed
   output closes the oldest working window.
4. ocean-mcp member resolution `--member` -> `member.toml` -> `OCEAN_MEMBER_ID`
   -> refuse join/post with a hint. In flight. Waiting behaviour: terminal
   users appear as `$USER`; tell the team to set the file.
5. Room list summary fields (`working: bool`, `needs_review: u32`, or an
   equivalent per-room card) so the sidebar can badge rooms that are not open.
   Waiting behaviour: attention chip for the open room only; no sidebar
   working dot.
6. `PATCH .../resources/{id} {authorized_agent_member_ids}` so an agent can be
   added to a grant without revoke + re-grant. Waiting behaviour: the Folders
   row explains revoke + re-grant; the authorize sheet's Folders step is
   read-only.
7. Optional `posted_via` (client type) on messages so a Claude Code / Codex
   post by `smaths` can show "via Claude Code". Waiting behaviour: no
   attribution beyond the member id.
8. Daemon-side principal check for local rooms (author_id must be the
   daemon's identity or a roster human it vouches for) so two humans on one
   daemon are distinguishable. Waiting behaviour: the proxy enforces (§3.3),
   the daemon trusts asserted ids from non-proxy clients.
9. Operator guide: `<config_dir>/operator.key` and
   `OCEAN_OPERATOR_ALLOWED_ORIGINS` (in flight); note that `tauri://localhost`
   and the extension origin pass CORS and then fail the operator origin check
   by default.

## 9. Slice plan

Serial merge order. Each slice is one PR branched from `origin/main` in a
detached worktree; the next slice branches from the previous merge. Files
listed are the only files the PR may touch, plus `Cargo.lock`, `events.md`
(merge=union) and the nearest `AGENTS.md`. Exclusive ownership is per open
PR: because the order is serial, `rooms_workspace.rs`, `rooms-workspace.css`,
`compact.css` and the scanner tests are each edited by exactly one open PR at
a time. Only S0 (ocean-os) and D (desktop lane) run in parallel with the
surface slices. Every merge to `origin/main` auto-deploys to :8790, so each
slice is written so the live surface after it loses no capability the
previous revision had; the "live after merge" line states that. Frozen gates
for every surface slice: `cargo fmt --check`; `cargo clippy -p
ocean-surface-ui --target wasm32-unknown-unknown -- -D warnings`; `cargo check
-p ocean-surface-ui --target wasm32-unknown-unknown`; `cargo check -p
ocean-surface-proxy`; `cargo test -p ocean-surface-ui --target
wasm32-unknown-unknown --no-run`; `cargo test -p ocean-surface-ui`; `cargo test
-p ocean-surface-proxy`; plus the live check on :8790 named in the acceptance
(a slice is done when it works in John's hands, not when tests pass).

### S0 daemon: identity route, participant retirement, attributed audit lines, ocean-mcp identity

Goal: land the ocean-os changes this direction depends on and run the
campaigns migration before any surface slice deploys. Retirement, aliases,
attributed placeholders and the ocean-mcp resolver are already in flight in
the `cc/rooms-phase2-close` worktree; `GET /v1/identity` is the addition.

Files (ocean-os): `crates/ocean-daemon/src/main.rs`,
`crates/ocean-daemon/src/persistent_rooms.rs`,
`crates/ocean-daemon/src/room_inspect.rs`,
`crates/ocean-daemon/src/room_agent_authority.rs`,
`crates/ocean-daemon/src/room_retirement.rs`,
`crates/ocean-daemon/src/identity.rs` (new), `crates/ocean-store/src/lib.rs`,
`crates/ocean-store/src/room_retirement.rs`,
`crates/ocean-mcp/src/bin/ocean_mcp.rs`,
`docs/OCEAN_RUNTIME_OPERATOR_GUIDE.md`,
`docs/specs/2026-09-09-ocean-rooms-participant-retirement.md`, the
router_contract test.

Depends on: nothing. Needs its own accepted manifest (the retirement spec);
this document requests it, it does not stand in for it.

Acceptance:
- `GET /v1/identity` answers `{ok, member_id, display_name, source}` from
  member.toml then `OCEAN_MEMBER_ID`; unset answers `member_id: null`; a
  native test proves the process user is never returned.
- Retire route per §3.4: placeholder pattern enforced (400
  `participant_not_retirable`), successor must be a live Human (409
  `successor_not_human`), replay-safe; owner role and `room_agent_owners`
  move, the roster row is removed, the alias is recorded, the System row is
  appended; `binding.owner_member_id` is unchanged and owner/target proofs
  resolve through the alias (store test: after retiring `surface-operator`
  into `smaths`, `smaths` suspends and re-authorizes `room-builder`).
- `inspect`, detail and `snapshot` carry `aliases`; `room_history_text` emits
  every string in §4.2 and the whitelist test covers each.
- ocean-mcp resolver per §3.2; reads work without a member; join/post refuse
  with the member.toml hint.
- Operator guide documents `operator.key`, `OCEAN_OPERATOR_ALLOWED_ORIGINS`,
  the retire route and `/v1/identity`; router_contract green; `cargo test`
  green in the workspace.
- Deployed from main; `member.toml` on John's daemon says `smaths` and on
  Eric's says `ecfromthedc`; on campaigns `surface-operator` and
  `web-18c11f5d551e63f8` are retired into `smaths` (decision ids in
  events.md); `GET .../inspect` shows `owner.member_id == "smaths"` and both
  agents owned by `smaths`.

Live after merge: unchanged surface; campaigns rows already re-render with
agent-attributed audit text.

### S1 identity and transport

Goal: one human = one member id on every host; delete the placeholder and
every minting path; add `display_name_for`, the `OperatorRoute` seam,
principal enforcement, and every proxy transport shape later slices need.

Files: `crates/ocean-surface-ui/src/rooms.rs`,
`crates/ocean-surface-ui/src/daemon.rs`,
`crates/ocean-surface-proxy/src/main.rs`,
`crates/ocean-surface-proxy/tests/rooms_operator_lanes.rs` (new),
`crates/ocean-surface-ui/tests/room_identity_resolution.rs` (new),
`crates/ocean-surface-ui/tests/room_mention_notification.rs`,
`crates/ocean-surface-ui/tests/room_hydration_resume.rs`.

Depends on: S0 deployed and the campaigns migration done.

Acceptance:
- `grep -rn "SINGLE_OPERATOR_ROOM_ID\|surface-operator\|\"Operator\"\|direct_host()" crates/ocean-surface-ui/src` returns only `legacy_identity_kind`; no `web-` minting path exists; native tests: empty `user_id` with auth on -> `Absent{SignIn}`; auth-off publishes the daemon id; direct host with `member_id: null` -> `Absent{ConfigureMember}`; mismatch -> `Absent{DaemonMismatch}`; no JoinBody is ever built from a non-`Resolved` state; the string `surface-operator` and an OS username are never produced.
- `IdentityState` exists; `identity_id / identity_name / identity_authoritative` are Memos derived from it (shim, removed in S8); every write gate reads `Resolved`.
- `Rooms::display_name_for(id)` resolves roster -> `access.members` -> binding display names -> `aliases` -> raw id; `legacy_identity_kind` classifies `surface-operator` and `web-[0-9a-f]{16}`; snapshot/inspect decode `aliases` and tolerate its absence (native tests).
- `rooms::OperatorRoute` enumerates Authorize, Bootstrap, Reauthorize, Suspend, Resume, Revoke, ProfilePut, ResourceGrant, ResourceSuspend, ResourceResume, ResourceRevoke, ResourcePreviewList, ResourcePreviewRead; `send_operator_mutation(route, body)` is the only builder of those requests, choosing the Tauri native transport or the proxy fetch; a native test asserts every route's method and path.
- Proxy: wildcard accepts PUT; `room_operator_mutation` injects the key for all thirteen shapes and forwards DELETE bodies; `GET /v1/identity` forwarded; `/api/config` publishes `user_display_name` from users.json and `daemon_member_id`; principal enforcement returns 403 `principal_mismatch` for the §3.3 fields and leaves agent joins and operator bodies alone; proxy tests cover each shape, the mismatch cases, and that GET inspect/profile/resources/preview/identity stay credential-free; unregistered verbs still answer the empty-body refusal.
- Live on :8790: signed in as `smaths`, a post lands with `author_id smaths`; a forged `author_id` is refused; the Tauri app opened against the same daemon posts as `smaths` and the campaigns roster gains no new human row (compare `GET /v1/rooms/persistent/campaigns` before and after); logged out shows "Sign in to post".

Live after merge: identical markup; Tauri and the extension post as the
daemon identity instead of "Operator".

### S2 seams: workspace decomposition (pure move)

Goal: zero behaviour change. Split `rooms_workspace.rs` into region modules
behind one `RoomsScope` (Copy struct of every workspace-scope signal,
constructed once), re-point every source-derived test through a
concatenating `view_source`, keep the rendered DOM identical, so every later
slice owns distinct files.

Files: `crates/ocean-surface-ui/src/rooms_workspace.rs` (reduced to the
composition root and `RoomsScope`), `crates/ocean-surface-ui/src/room_sidebar.rs`,
`room_header.rs`, `room_stage.rs` (timeline + thread markup verbatim),
`room_composer.rs`, `room_rail.rs` (the existing right rail verbatim) (all
new), `crates/ocean-surface-ui/src/main.rs`,
`crates/ocean-surface-ui/tests/common/mod.rs`, and every scanner test that
reads `rooms_workspace.rs` on origin/main: `agent_ownership_rail.rs`,
`ci_failure_trigger_control.rs`, `closed_room_audit_view.rs`,
`dead_selector_removal.rs`, `mobile_composer_regressions.rs`,
`room_agent_authorization_regressions.rs`, `room_list_paging_affordance.rs`,
`room_load_older_affordance.rs`, `room_workspace_binding.rs`,
`unheld_room_controls.rs`; `AGENTS.md` (module ownership map).

Depends on: S1.

Acceptance:
- `rooms_workspace.rs` holds `RoomsWorkspace`, builds `RoomsScope` once and mounts `room_sidebar::view`, `room_header::view`, `room_stage::view`, `room_composer::view`, `room_rail::view`; `RoomsView { Room, Agents }` exists with `Agents` rendering nothing yet; no signal is constructed inside an access closure. Ruling (answers "reserved fields vs `-D warnings`"): `RoomsScope` carries only fields with a reader in this PR — `inspect: RwSignal<Option<RoomInspect>>` (read by the header summary, which renders nothing while `None`), `working: RwSignal<Vec<WorkingAgent>>` (read by the composer bar, empty), `view`, `details_open` — and `room_inspect.rs` ships here with the types and `load_inspect` so `inspect` is genuinely populated; nothing is reserved unread.
- `tests/common::view_source("rooms_workspace.rs")` returns the concatenation of the composition root and the five region files; every scanner test passes with its assertion content unmodified; the Escape ladder `on:keydown` stays on the `.rooms-workspace` root.
- The rendered markup of the campaigns room is identical before and after (saved DOM diff attached to the PR); stylesheets untouched; gates green.

Live after merge: identical.

### S3 timeline: the conversation model

Goal: kill the log-not-a-conversation jank with the pure timeline model,
whitelist classification, five-minute compaction, turn footprints, inline
agent replies, display names and correct avatars, the unread divider, and the
working signal.

Files: `crates/ocean-surface-ui/src/room_timeline.rs` (new),
`crates/ocean-surface-ui/src/room_activity.rs` (new),
`crates/ocean-surface-ui/src/room_messages.rs`,
`crates/ocean-surface-ui/src/room_stage.rs`,
`crates/ocean-surface-ui/src/main.rs`, `styles/rooms-workspace.css` (timeline
and thread rules), `crates/ocean-surface-ui/tests/room_timeline_model.rs`
(new), `crates/ocean-surface-ui/tests/closed_room_audit_view.rs`,
`crates/ocean-surface-ui/tests/room_load_older_affordance.rs`,
`crates/ocean-surface-ui/tests/open_transcript_layout.rs`.

Depends on: S2.

Acceptance:
- `build_timeline`, `ActivityEvent::classify`, `compact`, `inline_thread_replies` and `working_agents` are pure; the campaigns fixture (seq 38-51) yields `Message 38`, `Activity[39-46]`, `Message 47` with one `InlineReply 50` whose footprint is {48, 49, 51}; `classify` matches only §4.2 strings (with and without the agent suffix) plus participant kinds and never parses JSON; a run split by a day divider yields two strips; prepending older rows leaves existing boundaries unchanged; attachment markers are never folded; `Refused` and `TurnFailed` never become footprints.
- `.rooms-workspace__activity` strips render at 28px with the gutter dot, one-line summary and in-place expansion listing each event with a mono timestamp; the literal "S" avatar and `__msg--system` full rows are gone for whitelisted rows; `Other` bodies render verbatim inside a strip.
- Root rows, inline replies and thread rows all use `display_name_for`; humans get a one-letter avatar in the hashed hue, agents the Robot glyph and the "acting for {owner}" sub-line.
- Inline reply under the human root with the footprint meta line; `ReplySummary` with avatar stack, N replies, (N new) from the open-time snapshot, last reply, always-visible Open thread; the Inline/Panel toggle is on the thread header; the Escape ladder is intact.
- `working_agents` covers the live campaigns sequence, an explicit invoke 202, a refusal, a turn failure and the ten-minute cutoff; `scope.working` is written from it; "Show activity" persists at `ocean.rooms.activity.v1`.
- Unread divider from the open-time snapshot; own rows never unread; agent rows count.
- Live on :8790: campaigns opens with room-builder's reply visible without a click, one activity strip that expands in place, names reading smaths and room-builder, no `[room agent ... audit]` text and no gray "S" anywhere in the stage.

Live after merge: strictly better transcript; rails unchanged.

### S4 shell: full-bleed, sidebar, header, composer, compact

Goal: the full-bleed rule, the plain-row sidebar with the New room dialog
and self card, the three-control header with overflow, the two-row composer
dock, the ban fixes, and the compact rules. The right rail stays, reachable
through the members pill as a drawer at every width, until S6 replaces it.

Files: `crates/ocean-surface-ui/src/rooms_workspace.rs` (scope fields for
the dialog and drawer state), `crates/ocean-surface-ui/src/room_sidebar.rs`,
`room_header.rs`, `room_composer.rs`, `room_rail.rs` (drawer wrapper only),
`crates/ocean-surface-ui/src/room_redeem.rs`,
`crates/ocean-surface-ui/src/room_markdown.rs` (mention chip glyph),
`crates/ocean-surface-ui/src/attachments.rs` (expose
`upload_from_composer`), `styles/base.css`, `styles/compact.css`,
`styles/rooms-workspace.css` (layout, sidebar, header, composer rules),
`crates/ocean-surface-ui/tests/mobile_composer_regressions.rs`,
`crates/ocean-surface-ui/tests/mobile_component_reflow.rs`,
`crates/ocean-surface-ui/tests/room_list_paging_affordance.rs`,
`crates/ocean-surface-ui/tests/dead_trigger_row_affordance.rs`,
`crates/ocean-surface-ui/tests/ci_failure_trigger_control.rs`,
`crates/ocean-surface-ui/tests/room_shell_layout.rs` (new),
`crates/ocean-surface-ui/tests/room_composer_dock.rs` (new).

Depends on: S3.

Acceptance:
- `.ocean-surface:has(> .rooms-workspace) { max-width: none }` in `base.css`; at 1568px the transcript column is at least 900px with the rail closed; the body measure is a 72ch `max-width` on `__msg-body`.
- Sidebar per §7: no create form, trigger checkboxes, workspace-path input or always-visible redeem input; `+ New room` dialog with Create (name only, normalized policy) and Join by code (mounts `RoomRedeem`); Rooms and Agents primary rows (Agents renders the empty `RoomsView::Agents` until S5); rows bold-plus-dot on unread, `@N` chip only on mentions; self card from `IdentityState`; overflow with Mark as read, Copy room key, Leave; `create_trigger_row_dead_here` and its tests retired.
- Header per §7: name + inspect summary; exactly members pill, attention chip (outbox and inspect counts), one overflow; the members pill opens the rail as a drawer at every width; the `__members-chip` display:none rules and the 901-1080 gap are removed.
- Composer per §7: textarea autosize, Enter / Shift+Enter, 16px under `pointer: coarse`, `@` and paperclip ghosts (upload lands and the marker appears), Send in a fixed slot; mention popup keeps its selectors and gains the trust sub-line; popup and accept rule read one `mentionable` Memo; rendered agent mentions carry the Robot glyph; non-members see Join room; activity bar and reply banner render.
- `compact.css` holds every rooms rule at or below 650 (single pane, back chevron, full-screen drawers, safe-area); no horizontal scroll at 380px in the side panel; a test derives every `opacity: 0` reveal in `rooms-workspace.css` and asserts a `(hover: none)` floor.
- Ban fixes: `__jump-new` and `__mention-kind` on `--radius-sm`; drawers use elevation without a border; reduced-motion kills after state selectors; no colour literals in `compact.css`.
- Selector-derived tests updated; verified on :8790 at 1568, 1024, 650 and in the side panel.

Live after merge: full-bleed rooms, new sidebar/header/composer; every rail
section still reachable through the members pill.

### S5 Agents view: package authoring leaves the room

Goal: the Agents view with the catalog and the builder, reached from the
sidebar row, `/agents` and the future sheet link, so S6 can unmount the
builder from the room with no gap.

Files: `crates/ocean-surface-ui/src/agents_workspace.rs` (new),
`crates/ocean-surface-ui/src/agents.rs`,
`crates/ocean-surface-ui/src/palette.rs`,
`crates/ocean-surface-ui/src/rooms_workspace.rs` (mount for
`RoomsView::Agents`), `crates/ocean-surface-ui/src/main.rs`,
`styles/rooms-workspace.css` (`agents-workspace__*` rules),
`styles/compact.css`, `crates/ocean-surface-ui/tests/agents_workspace_view.rs`
(new).

Depends on: S4.

Acceptance:
- The sidebar Agents row switches `RoomsView` to Agents: catalog rows from `GET /v1/agents` (name, description, model, skills count, "bound in #{open room}" from the open room's bindings only); one primary "+ New package"; row overflow Edit / Delete (two-step).
- The builder keeps its markup and rules (models picker via `Rooms::models`, tools free text, `blocks_save` on `subprocess_capability`, read-only name on edit, empty description/model -> None); instructions textarea at least 240px inside a scrolling panel; Save/Delete go through the existing POST/PUT/DELETE `/v1/agents` paths.
- `/agents` palette command opens the view; closing re-fetches `/v1/agents`; the compact rules for the view are in `compact.css`.
- The room still mounts the old authorization panel with its nested builder until S6; a source test asserts `AgentBuilder` is mounted in `agents_workspace.rs`; gates green; verified on :8790 by creating a package.

Live after merge: authoring available in two places for one wave (room and
Agents view); nothing lost.

### S6 room details drawer: Members & Agents, authorize sheet, Settings

Goal: replace the nine-section rail with the details drawer (Members &
Agents with the six-step authorize flow, Settings), move Summary / Notes &
tasks / Files / Workspace to the header overflow as standalone panels, and
unmount the builder from the room.

Files: `crates/ocean-surface-ui/src/room_details.rs` (new),
`crates/ocean-surface-ui/src/room_members.rs` (new),
`crates/ocean-surface-ui/src/room_agent_sheet.rs` (new),
`crates/ocean-surface-ui/src/room_settings.rs` (new),
`crates/ocean-surface-ui/src/room_inspect.rs` (`needs_review`),
`crates/ocean-surface-ui/src/room_agent_authorization.rs` (reduced to the
binding client and row components; select and builder removed),
`crates/ocean-surface-ui/src/room_invite.rs`,
`crates/ocean-surface-ui/src/room_rail.rs` (deleted),
`crates/ocean-surface-ui/src/rooms_workspace.rs` (drawer and overflow panel
mounts), `crates/ocean-surface-ui/src/room_header.rs` (overflow rows),
`crates/ocean-surface-ui/src/agents_workspace.rs` ("Authorize in #room"
hand-off), `crates/ocean-surface-ui/src/main.rs`, `styles/rooms-workspace.css`
(rail rules deleted, drawer and sheet rules added), `styles/compact.css`,
`crates/ocean-surface-ui/tests/agent_ownership_rail.rs`,
`crates/ocean-surface-ui/tests/room_agent_authorization_regressions.rs`,
`crates/ocean-surface-ui/tests/unheld_room_controls.rs`,
`crates/ocean-surface-ui/tests/room_agent_sheet_flow.rs` (new),
`crates/ocean-surface-ui/tests/room_details_drawer.rs` (new).

Depends on: S5.

Acceptance:
- `RoomInspect` decodes the full inspect shape including `aliases` and string generations (never `Number()`); `load_inspect` runs on room open, drawer open, the six activity kinds and after every operator mutation with latest-wins tickets and generation checks; `needs_review` counts stale + unbound agents + unresolved required slots + failed outbox + legacy identities; native tests cover projection and count.
- Drawer: four plain-text tabs; Members & Agents per §5.1 (People rows with the owner and binding-holder Remove rule, the read-only Legacy identities disclosure, Agents rows only for bindings grouped Active / Paused / Needs re-authorization with the execution line and session dot, overflow Suspend / Resume / Re-authorize / Revoke / Invoke on my last message each minting a `decision_id` where required and exiting through `send_operator_mutation`; revoked absent; no clickable "not authorized" row; non-owners see no primary and the owner sentence).
- Authorize sheet implements the six steps of §5.1: Pick (minus bound, "on roster, not authorized" label, Author link to the Agents view), Preview (no empty checklist), Policies (defaults, nothing pre-ticked, read-only Folders list), Review (exact body, `decision_id` minted here), Submit (bootstrap first on Local rooms, always), refusal mapping; native tests cover the step machine, the bootstrap-then-authorize order and the error table.
- Re-authorize sheet shows previous against current digest and revision and never re-pins silently.
- Settings per §5.4: two switches sending the full object; Invite; Signed in as; Leave; Close two-step; the Response Policy duplicate is gone.
- Summary, Notes & tasks, Files open from the header overflow with one primary each and CAS 409 handling intact; the Workspace (federated) row appears only when `inspect.federated`; the rail markup (`__right`, `__right-list`, the sliver-plus-open-button pattern) is gone; no rooms file references `AgentBuilder`; the "Choose a package" `<select>` is gone.
- Drawer and sheet have their rules at or below 650 in `compact.css`; a 380px side-panel screenshot is attached.
- Live on :8790 as `smaths` (owner after S0): a second package is authorized in campaigns and `context-cartographer` moves to Active; gates green.

Live after merge: the rail is gone; every former section has a home in the
same revision (drawer tabs or overflow panels); the workspace-binding section
is mounted verbatim under Settings until S7.

### S7 room details drawer: Folders and Profile

Goal: the first surface client for `/resources` and `/profile`; delete the
false unbound warning; ship Folders (grant, suspend, resume, revoke, operator
preview, promote default folder) and Profile (execution summary, repos,
tools, slots, agent defaults, profile sheet).

Files: `crates/ocean-surface-ui/src/room_folders.rs` (new),
`crates/ocean-surface-ui/src/room_profile.rs` (new),
`crates/ocean-surface-ui/src/room_inspect.rs`,
`crates/ocean-surface-ui/src/room_details.rs` (tab mounts),
`crates/ocean-surface-ui/src/room_settings.rs` (default-folder control moves
to the shared row), `crates/ocean-surface-ui/src/main.rs`,
`styles/rooms-workspace.css` (folders and profile rules; `__workspace-*`
rules deleted), `styles/compact.css`,
`crates/ocean-surface-ui/tests/room_workspace_binding.rs`,
`crates/ocean-surface-ui/tests/room_folders_tab.rs` (new),
`crates/ocean-surface-ui/tests/room_profile_tab.rs` (new).

Depends on: S6.

Acceptance:
- Folders rows per §5.2 from `inspect.resources`; overflow Suspend / Resume / Revoke / Preview as agent (list and read with paging and the `via` footer); every documented refusal maps to one status line; `local_root` is never rendered from server state.
- Share folder per §5.2: the browser path input with the daemon's rules quoted; on the desktop app the native chooser when `host::pick_folder()` exists (slice D) and the text input otherwise; success adds a row and refreshes inspect; 409 `root_already_granted` names the existing row.
- Profile per §5.3: execution summary; slots with the four statuses and the keychain warning; tools "not verified"; repos; agent defaults; the profile sheet PUTs the whole profile with a `decision_id`, handles 201/200/`changed: false` and the 400 table field-by-field.
- No `__workspace-unbound`, `room_is_unbound` render, or "Agents in this room cannot run" text anywhere; the only cannot-run copy names the agent from `inspect.agents[].execution.cwd_source == unbound`; `room_workspace_binding.rs` is rewritten to assert that.
- Live on :8790 with campaigns: room-builder shown running in the granted folder, three credential slots with statuses, a preview list of the granted folder; gates green.

Live after merge: Phase 2 is visible and operable from the browser.

### S8 docs and cleanup

Goal: remove the identity shim, align every written contract with what
shipped, and record the rulings.

Files: `crates/ocean-surface-ui/src/rooms.rs` (shim removal),
`crates/ocean-surface-ui/src/{attachments,room_agent_authorization,room_artifacts,room_markdown,room_repo,room_summary,room_workspace_panel}.rs`
(readers move to `identity`), `docs/OCEAN_ROOMS_PRODUCT.md`,
`docs/OCEAN_WEB_SURFACE_DESIGN.md` (§4 full-bleed ruling; §6 live-dot
self-question resolved to the Material map), `AGENTS.md` (Rooms Contract:
identity rule, whitelist rule, module ownership map, inspect refresh
triggers), `events.md`; in ocean-os, the `ocean_room_read` tool description
(inline-reply rule) and `AGENTS.md` (whitelist rule).

Depends on: S7.

Acceptance:
- `identity_id`, `identity_name`, `identity_authoritative` no longer exist; every reader uses `IdentityState`; gates green.
- `OCEAN_ROOMS_PRODUCT.md` describes the timeline model, the identity rule (users.json username = member id = member.toml = `/v1/identity`), the drawer tabs, the authorize order (bootstrap then authorize), Folders and Profile, and drops the participant-picker and "invite/redeem future" sections; the `AGENTS.md` line "extension/Tauri hosts use the stable surface-operator identity" is replaced.
- Both repos' `AGENTS.md` carry the rule: a new daemon audit type is added to `room_history_text` daemon-side and to `ActivityEvent::classify` surface-side in the same change window, with a shared fixture listing the strings.
- events.md entries with the worktree tag for every slice.

### D desktop lane (parallel, owned by the desktop session)

Goal: Phase 2 writes and the native folder chooser on the desktop app.

Files: `crates/ocean-tauri/src/lib.rs`,
`crates/ocean-surface-ui/src/host.rs`, `crates/ocean-tauri/tests/operator_allowlist.rs`.

Depends on: S1 (for the `OperatorRoute` shapes). Not on the critical path.

Acceptance:
- `daemon_operator_request` accepts PUT `.../profile`, POST `.../resources`, POST `.../resources/{id}/(suspend|resume|list|read)` and DELETE `.../resources/{id}` with a JSON body, and still rejects everything else; the allowlist test enumerates the new set and the "last handler" assertion in `room_agent_authorization_regressions.rs` still holds.
- `host::pick_folder() -> Option<String>` opens the native chooser on Tauri and is `None` elsewhere; `room_folders.rs` consumes it through the seam without a host.rs edit in any surface slice.
- Authorizing an agent and sharing a folder from the desktop app against the local daemon succeed; if `OCEAN_OPERATOR_ALLOWED_ORIGINS` is required for `tauri://localhost`, the setup hint from §8 is shown rather than a silent failure.

## 10. What this does NOT change

- Wire types, seq encoding, the one-room-tail rule, generation guards, the 8s
  silent list refresh, `ocean.rooms.view.v1` and `ocean.rooms.thread-view.v1`
  persistence, `ocean://room/{key}` deep links.
- `room_markdown::body_view` and the http(s)-only link allowlist; attachment
  rendering rules; invite codes as bearer grants shown once.
- The daemon trust model: the browser never sees the operator key; operator
  routes fail closed; the extension has no authority transport.
- Phase 1 and Phase 2 daemon semantics: bootstrap, authorize, narrowing-only
  grants, decision ids, the cwd rule, `grantable_capabilities == []`,
  `installed: "unknown"`, keychain resolvers reporting resolver_not_open,
  admission refusals landing as transcript rows. The surface explains these;
  it does not work around them.
- Phase 3: node identity, remote resources, federated credential-slot
  projection, an audit projection route, per-user operator scoping, a
  daemon-side principal check, and cross-daemon human identity. Each needs its
  own manifest; §8 only requests them.
- The retire route stays an operator maintenance action; the surface never
  calls it.
- The existing panels for Summary, Notes & tasks, Files, Repo and Workspace:
  they move to the header overflow unchanged; their internals are not
  redesigned here.
- Class names: every `rooms-workspace__*`, `rooms-panel__*`, `room-stage__*`,
  `rooms-md__*` and `is-*` selector that exists today stays; new elements use
  new names in the same families; `rooms-interaction.css` is untouched. No
  new stylesheet, no new class family.
- The GPUI crate, the TUI, Council, Island, Call, Canvas, Floor.
- LiveKit, voice, kanban or any project-management surface; Rooms remain
  text-first collaboration with agents as members.
