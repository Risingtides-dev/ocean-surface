//! Persistent Rooms panel — list, create, join/leave, transcript + composer.
//!
//! The web counterpart to the daemon's persistent-rooms surface (OCEAN-65,
//! routes under `/v1/rooms/persistent/*`). A room is a durable, named
//! collaboration space with a participant roster and an append-only transcript.
//! This module owns:
//!
//!   GET    /v1/rooms/persistent                       → list rooms
//!   POST   /v1/rooms/persistent                       → create a room
//!   GET    /v1/rooms/persistent/{key}                 → room record (roster refresh)
//!   GET    /v1/rooms/persistent/{key}/snapshot        → hydrate: room + one
//!                                                       transcript page + cursor
//!   PATCH  /v1/rooms/persistent/{key}                 → update trigger policy
//!   POST   /v1/rooms/persistent/{key}/participants    → join
//!   DELETE /v1/rooms/persistent/{key}/participants/{id}→ leave
//!   POST   /v1/rooms/persistent/{key}/messages        → post a message
//!   GET    /v1/rooms/persistent/{key}/events           → live SSE tail (TASK-10)
//!
//! Live updates: the daemon's room-scoped SSE (TASK-10, `GET
//! /v1/rooms/persistent/{key}/events`) streams every transcript row as a
//! `room_message` frame with `id:=seq`. The surface hydrates once through
//! `/snapshot` — the read that answers a cursor beside its page — then tails
//! live with sequence resume (`?after_seq=` only when a hydrated sequence
//! exists on each newly constructed browser connection) — no poll, no
//! global-stream workaround (TASK-11). Hydration has never been the whole
//! transcript for a long room: the daemon caps a page at 1000 rows and serves
//! the OLDEST of them, and it is the tail's durable replay from that page's
//! last sequence that delivers everything after it.
//!
//! The whole module is self-contained — it carries its own request layer rather
//! than threading rooms state through the `Daemon` handle — so it never touches
//! the live agent loop / session SSE code.

use std::collections::HashMap;

use futures_util::future::Either;
use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

use crate::rooms_workspace::composer_writes_allowed;

/// SSE tail connection state for the live indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TailState {
    /// Initial catch-up replay in progress.
    Replaying,
    /// Live stream connected, receiving frames in real time.
    Live,
    /// Connection dropped, attempting reconnect.
    Reconnecting,
}

/// How MANY rows hydration asks for through `/snapshot`; the cursor below is
/// which END they come from, and the first paint needs both. The route's own
/// default is 200 while the store's ceiling is 1000, so naming the ceiling keeps
/// the first paint exactly the size it was on the unpaged read — moving onto the
/// cursor-bearing route adds the cursor without shrinking what an operator sees.
///
/// Doubles as the window [`hydration_backfill_start`] measures a page against:
/// a page the daemon filled to this number is the only one that can have rows
/// behind it.
const HYDRATION_TRANSCRIPT_LIMIT: usize = 1000;

/// The `before_seq` that opens a room at its NEWEST page. `/snapshot` pages
/// backward exactly when this parameter is present, and a backward page is the
/// newest `limit` rows strictly older than the cursor — so a cursor past every
/// seq the room could ever hold is the literal way to name the tail, not a
/// sentinel the daemon special-cases (ocean-os#436, and the ecosystem contract's
/// "Transcript window" says so in as many words). `u64::MAX` is the only value
/// no stored row can reach; the store pins it green in
/// `transcript_tail_page_cursor_above_i64_max_is_the_newest_page`, which exists
/// because an unchecked cast to SQLite's i64 once wrapped a cursor negative and
/// read the wrong end of the log.
const HYDRATION_TAIL_CURSOR: u64 = u64::MAX;

/// Pages one catch-up read of `/transcript` will walk before it stops asking,
/// and the same bound on the backward hydration walk. The route serves 200 rows
/// a page, so five is the same 1000 rows a fresh open paints
/// ([`HYDRATION_TRANSCRIPT_LIMIT`]). Past that a read that runs on every join,
/// leave, removal and send would be pulling a long-lived room's whole log
/// through four unrelated mutations, which is the full-table read the route's
/// paging exists to avoid. The live tail owns everything beyond the cap.
const MAX_TRANSCRIPT_CATCHUP_PAGES: usize = 5;

/// Rows per page of the backward hydration walk. The forward catch-up gets 200
/// from `/transcript`'s own default without asking; this names the same number
/// so the two walks are the same shape and the one page cap above bounds both to
/// the same 1000 rows. Spelled out rather than left to the route default because
/// the backward read must pass `limit` beside `before_seq` anyway.
const BACKFILL_TRANSCRIPT_PAGE_LIMIT: usize = 200;

/// Stable identity used by explicit single-operator and direct-host surfaces.
/// Browser deployments with a signed-in user never use this value: they stay
/// unresolved until `/api/config` publishes the current login.
const SINGLE_OPERATOR_ROOM_ID: &str = "surface-operator";

// ---- Wire types (mirror ocean-core Room / RoomMessage / RoomParticipant) ----

/// What kind of actor a participant / message author is. Mirrors
/// `ocean_core::RoomParticipantKind` (snake_case on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomParticipantKind {
    Human,
    Agent,
    Bot,
    Tool,
    System,
}

impl RoomParticipantKind {
    /// The author/roster chip mark — hand-drawn SVGs from `icons.rs`, same
    /// stroke family as the rest of the surface (emoji glyphs are forbidden
    /// in product UI; the 07-08 purge missed this path — QA-006).
    #[allow(dead_code)]
    fn icon(self) -> AnyView {
        match self {
            RoomParticipantKind::Human => view! { <crate::icons::Person /> }.into_any(),
            RoomParticipantKind::Agent => view! { <crate::icons::Robot /> }.into_any(),
            RoomParticipantKind::Bot => view! { <crate::icons::Cog /> }.into_any(),
            RoomParticipantKind::Tool => view! { <crate::icons::Wrench /> }.into_any(),
            RoomParticipantKind::System => view! { <crate::icons::Spark /> }.into_any(),
        }
    }

    /// A lowercase word for the kind — shown next to the icon so the roster makes
    /// it explicit who's an agent (i.e. auto-convene-able) vs. a human.
    #[allow(dead_code)]
    fn label(self) -> &'static str {
        match self {
            RoomParticipantKind::Human => "human",
            RoomParticipantKind::Agent => "agent",
            RoomParticipantKind::Bot => "bot",
            RoomParticipantKind::Tool => "tool",
            RoomParticipantKind::System => "system",
        }
    }
}

/// One participant in a room's roster. Mirrors `ocean_core::RoomParticipant`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoomParticipant {
    pub id: String,
    pub kind: RoomParticipantKind,
    pub display_name: String,
}

/// What kind of transcript entry a message is. Mirrors
/// `ocean_core::RoomMessageKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomMessageKind {
    Message,
    ParticipantJoined,
    ParticipantLeft,
    System,
}

/// One transcript entry. Mirrors `ocean_core::RoomMessage`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RoomMessage {
    pub seq: u64,
    pub author_id: String,
    pub author_kind: RoomParticipantKind,
    pub kind: RoomMessageKind,
    pub body: String,
    #[serde(default)]
    pub created_at: String,
    /// Confirmed-federation metadata. `None` for local-only rooms and G1
    /// messages. Present only after Bedrock confirms.
    #[serde(default)]
    pub federated: Option<FederatedMessageMeta>,
    /// Root message sequence for a one-level thread reply. `None` for roots.
    #[serde(default)]
    pub thread_parent_seq: Option<u64>,
    /// Attachment described by an upload/removal marker row. `None` on every
    /// other row, and always absent from daemons predating the field.
    #[serde(default)]
    pub attachment_id: Option<String>,
}

// ---- Federated wire types (exact mirror of ocean-core e2796999) -------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedMessageMeta {
    pub ledger_event_id: String,
    pub global_sequence: u64,
    pub source_id: String,
    pub source_sequence: u64,
    pub client_event_id: String,
    pub origin_principal_id: String,
    pub origin_member_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederatedRoomMemberProjection {
    pub member_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_member_id: Option<String>,
    pub actor_type: FederatedActorType,
    pub role_in_room: FederatedRoomRole,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_agent_descriptor: Option<PublicAgentDescriptor>,
    pub joined_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_presence: Option<MemberPresence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_binding_available: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedActorType {
    User,
    Agent,
}

impl FederatedActorType {
    #[allow(dead_code)]
    fn icon(self) -> AnyView {
        match self {
            Self::User => view! { <crate::icons::Person /> }.into_any(),
            Self::Agent => view! { <crate::icons::Robot /> }.into_any(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedRoomRole {
    Owner,
    Member,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberPresence {
    Live,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicAgentDescriptor {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_alias: Option<String>,
    #[serde(default)]
    pub skills_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomReadCursorProjection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirrored_upstream_read_seq: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoomReadSummary {
    pub latest_seq: Option<u64>,
    pub read_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomOutboxItem {
    pub client_event_id: String,
    pub source_id: String,
    pub source_sequence: u64,
    pub author_member_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mention_member_ids: Vec<String>,
    pub state: OutboxItemState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxItemState {
    Pending,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomAccessProjection {
    pub state: RoomAccessState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_confirmed_global_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<FederatedRoomMemberProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbox: Vec<RoomOutboxItem>,
}

/// Which WORKER owns which Agent participant in one room, from the daemon's
/// `agent_owners` array (ocean-os#437). Both ids are LOCAL participant ids: the
/// store joins the ownership row to `participants` on `agent_id` and answers
/// `owner_present` as "is `owner_id` still on that same roster", ordered by
/// roster position. That namespace is the reason this projection lands on the
/// Local roster branch and not the federated one — a federated row's
/// `member_id` is a bedrock-minted binding id from a different table.
///
/// `owner_present` is a field rather than a filter because the binding OUTLIVES
/// the worker: anyone may remove a participant, so an owner can leave while the
/// ownership really did happen and the agent really is unclaimed now. Dropping
/// the row would deny the first; reporting the row alone would assert a live
/// claim the room cannot prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomAgentOwner {
    /// The owned Agent participant's id, as it appears in `Room::participants`.
    pub agent_id: String,
    /// The owning Human participant's id, in that same roster.
    pub owner_id: String,
    /// Whether that Human is still on the roster. `#[serde(default)]` is this
    /// module's idiom for every field of an additive projection; the daemon has
    /// answered it since the projection existed.
    #[serde(default)]
    pub owner_present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomAccessState {
    Local,
    Connecting,
    Live,
    Recovering,
    Revoked,
}

/// How a room's agents are auto-woken. Mirrors `ocean_core::RoomTriggerPolicy`
/// field for field. All flags default off. Four triggers are live: the daemon
/// evaluates `on_mention` and `on_thread_reply` per non-agent-authored
/// transcript message (OCEAN-65 / OCEAN-111), and `on_build_failure` /
/// `on_ci_failure` per ingested workspace ledger row — a different lane, not a
/// message evaluation. `on_component_event` and `on_schedule` are unwired:
/// nothing ever fires them, and the daemon's write routes answer a typed 400
/// (`trigger_unwired`) for a policy carrying `on_component_event: true` or a
/// set `on_schedule`. Refusal is by VALUE, not presence, so serializing the
/// defaults is accepted; both fields stay `Deserialize` because stored dead
/// values remain readable.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoomTriggerPolicy {
    /// Wake an agent when it is @-mentioned in the transcript (the common case).
    #[serde(default)]
    pub on_mention: bool,
    /// Wake an agent when someone replies in a thread it participates in.
    #[serde(default)]
    pub on_thread_reply: bool,
    /// Unwired: never fires, and the daemon refuses any write where this is
    /// `true` (`trigger_unwired`). Kept so stored `true` values still decode.
    #[serde(default)]
    pub on_component_event: bool,
    /// Wake the room's agents when a workspace build fails. Off by default,
    /// so every policy stored before this field existed keeps its behavior.
    #[serde(default)]
    pub on_build_failure: bool,
    /// Wake the room's agents when a workspace CI check comes back red. The
    /// daemon half is live — `ocean_core` carries the flag and a `CiFailure`
    /// event, the store round-trips the key, and a red
    /// `room.workspace.ci_checked` row convenes the roster's agents — and this
    /// surface now carries a control for it too (see `TriggerToggle` in
    /// `rooms_workspace.rs`). Mirroring it still matters for the rooms whose
    /// flag the daemon set before that control existed: this surface PATCHes
    /// the policy WHOLESALE, so such a room would lose the flag to the next
    /// flip of any other row if this struct did not know the key. A daemon
    /// predating the field drops it harmlessly — the room write routes deny no
    /// unknown field.
    #[serde(default)]
    pub on_ci_failure: bool,
    /// Unwired: no schedule ever fires, and the daemon refuses any write
    /// where this is set (`trigger_unwired`). Stored crons stay readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_schedule: Option<String>,
}

/// A persistent room. Mirrors `ocean_core::Room` (we read only the fields the
/// panel renders).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Room {
    /// The room key. `ocean_core::RoomKey` serializes as a bare string
    /// (`pub struct RoomKey(pub String)`), so this deserializes directly.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub participants: Vec<RoomParticipant>,
    #[serde(default)]
    pub created_at: String,
    /// Last change to roster/metadata/transcript — shown as "last activity".
    #[serde(default)]
    pub updated_at: String,
    /// Optional auto-convene trigger policy. `None` = no automatic triggers.
    #[serde(default)]
    pub trigger_policy: Option<RoomTriggerPolicy>,
    /// The workspace folder on the DAEMON's host this room is bound to, if any.
    ///
    /// Nothing to do with the SESSION workspace root every other module in this
    /// crate means by that name — this one is the room's own, and it is what a
    /// room-bound agent turn resolves its project and `cwd` from. `None` is an
    /// unbound room, where every agent turn fails closed on the daemon with
    /// `workspace_unavailable`, so the mention that was supposed to wake an
    /// agent does nothing at all. `#[serde(default)]` because a daemon
    /// predating the field simply omits it, and an omitted binding reads the
    /// same as no binding.
    #[serde(default)]
    pub workspace_root: Option<String>,
}

// ---- Response envelopes (the daemon's `json!({ "ok": .., .. })` shapes) ------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RoomsListResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    rooms: Vec<Room>,
    #[serde(default)]
    read_states: Vec<RoomReadStateWire>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RoomReadStateWire {
    room_id: String,
    #[serde(default)]
    latest_seq: Option<String>,
    #[serde(default)]
    read_seq: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct ReadCursorPatchBody {
    read_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ReadCursorPatchEnvelope {
    #[serde(default)]
    ok: bool,
    cursor: RoomReadCursorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct RoomReadCursorBody {
    room_id: String,
    #[serde(default)]
    read_seq: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadCursorProjectionTarget {
    Local,
    MirroredUpstream,
}

/// `GET /v1/rooms/persistent/{key}/snapshot` — the hydration envelope. The
/// unpaged room GET answers a transcript with nothing beside it, so a room past
/// the store's 1000-row cap hands back its oldest rows and no field says so.
/// This route answers the same room, transcript and access plus the page's own
/// cursor, in whichever direction the read asked for.
///
/// Hydration always asks BACKWARD ([`HYDRATION_TAIL_CURSOR`]), so `prev_seq` and
/// `has_more` are the cursor pair this envelope decodes and
/// [`Rooms::backfill_open_transcript`] is what reads them. `next_seq` stays
/// undecoded, and now for a stronger reason than the one that used to stand
/// here: a backward page's `next_seq` is `null` on the wire by construction, so
/// decoding it would add a field that is not merely unread but never populated —
/// exactly the dead code the `-D warnings` release lane rejects. A forward
/// `/snapshot` read would populate it, and this crate makes none.
#[derive(Debug, Clone, Deserialize)]
struct RoomSnapshotResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    room: Option<Room>,
    #[serde(default)]
    transcript: Vec<RoomMessage>,
    /// Required on every successful room open, including local rooms.
    access: RoomAccessProjection,
    /// Highest `seq` on this page, in the daemon's own words. The
    /// `#[serde(default)]` is this module's idiom for every additive field
    /// rather than a compatibility window: `/snapshot` has carried `last_seq`
    /// since the commit that introduced the route, so no shipped daemon omits
    /// it. The fallback below is a decode safety net, not a version bridge.
    ///
    /// Unchanged by the move to a backward read: the daemon derives it from the
    /// page's last row either way, and on a tail-anchored page that row is the
    /// newest in the room — which is precisely the `after_seq` the live tail
    /// wants. Anchoring hydration at the other end made this field MORE
    /// truthful, not less.
    #[serde(default)]
    last_seq: Option<u64>,
    /// OLDEST row on this page, replayed as the next `before_seq` to walk
    /// further back. `Some` only on a backward page — a forward one carries
    /// `next_seq` instead and leaves this null — and `None` once a page reaches
    /// the start of the transcript. Read by
    /// [`Rooms::backfill_open_transcript`] through
    /// [`transcript_backfill_cursor`], and by
    /// [`Rooms::load_older_transcript_page`] through
    /// [`transcript_older_cursor`] — the same cursor, once the walk's page cap
    /// has handed the decision to the operator.
    #[serde(default)]
    prev_seq: Option<u64>,
    /// Whether more rows exist in the direction THIS page paged — older rows,
    /// for the backward read hydration makes. It is not "the room has more
    /// messages": on the tail page of a 5000-row room it is true and every row
    /// it refers to is behind what was just painted.
    #[serde(default)]
    has_more: bool,
    /// Whether the soft-closed AUDIT view answered this read rather than the
    /// live one. `/snapshot` has always fallen through to it so a finished
    /// call stays replayable, and until ocean-os#434 the body never said which
    /// arm produced the record — leaving a frozen room hydrating
    /// indistinguishably from a live one, above a tail `/events` 404s forever
    /// and a composer whose every send 404s too. Nothing else in this envelope
    /// can stand in for it, `access` least of all: closing a room is a soft
    /// stamp on the room row that leaves its access row untouched, so a closed
    /// room answers 200 still projecting whatever its rail projected while it
    /// was live — `Local` for a room that never had an access row, an
    /// unchanged `Live` or `Revoked` (members and outbox included) for a
    /// federated one.
    ///
    /// `#[serde(default)]` because the contract rules the field additive and a
    /// daemon that predates it says nothing, which must read as OPEN — the
    /// other direction would shut the composer on every room a pre-field
    /// daemon serves.
    #[serde(default)]
    closed: bool,
    /// Which worker owns which Agent participant in this room — see
    /// [`RoomAgentOwner`]. The contract puts it on BOTH room detail and this
    /// snapshot for the same reason `closed` rides here: hydration goes through
    /// `/snapshot`, so a field only room detail serves is a field no client can
    /// reach. This crate opens no other room read, which is why `/snapshot` is
    /// the only envelope in this file that decodes it.
    ///
    /// `#[serde(default)]` because the contract rules the field additive: a
    /// daemon predating ocean-os#437 says nothing and an empty list is exactly
    /// what "no local ownership recorded" means, which is also what every room
    /// that predates the feature reports from a current daemon.
    ///
    /// A soft-closed room reports it unchanged — closing retains the roster and
    /// the ownership rows — so the audit view says who owned what and whether
    /// they were still present when it froze.
    #[serde(default)]
    agent_owners: Vec<RoomAgentOwner>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RoomErrorResponse {
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RoomMutateResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    room: Option<Room>,
    #[serde(default)]
    error: Option<String>,
}

/// `GET /v1/rooms/persistent/{key}/transcript` — ONE bounded page, never the
/// log. The route's default is 200 rows, so the cursor beside the page is the
/// whole difference between catching up and keeping the first 200 rows of a
/// burst forever: `next_seq` is the daemon's own `after_seq` for the page after
/// this one and `has_more` says such a page exists, and
/// [`Rooms::refresh_open_transcript`] walks both.
///
/// [`RoomSnapshotResponse`] decodes its own `has_more` beside a BACKWARD
/// cursor; the two fields are named the same and mean opposite directions, which
/// is why neither envelope borrows the other's. `next_seq` is forward-only and
/// lives only here — a `/snapshot` read this crate makes never populates it.
#[derive(Debug, Clone, Deserialize)]
struct TranscriptResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    transcript: Vec<RoomMessage>,
    /// `Some` only on a `has_more` page — the store leaves it null once a page
    /// reaches the end of the log. `#[serde(default)]` is this module's idiom
    /// for every additive field rather than a compatibility window: the route
    /// has carried both since OCEAN-249.
    #[serde(default)]
    next_seq: Option<u64>,
    #[serde(default)]
    has_more: bool,
}

// ---- Request bodies (match the daemon's serde::Deserialize structs) ----------

#[derive(Debug, Clone, Serialize)]
struct CreateRoomBody<'a> {
    key: &'a str,
    name: &'a str,
    /// Optional trigger policy. Skipped when `None` so the daemon's `#[serde(default)]`
    /// (no triggers) applies; otherwise the daemon stores it verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    trigger_policy: Option<RoomTriggerPolicy>,
    /// The workspace folder on the daemon's host to bind the new room to.
    /// Skipped when `None` — an omitted binding is what the daemon reads as
    /// "unbound", and sending an explicit `null` would mean the same thing
    /// while looking like a value the operator chose.
    ///
    /// Until this field existed the surface sent `key`, `name` and
    /// `trigger_policy` only, so EVERY room this form made was unbound and
    /// every agent mention in it did nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_root: Option<&'a str>,
}

/// `PATCH /v1/rooms/persistent/{key}` carrying the workspace binding alone.
///
/// Deliberately NOT `skip_serializing_if`: an explicit `null` is how the
/// daemon is told to UNBIND, and an omitted field means "leave it alone", so
/// skipping `None` here would make the unbind control a no-op that reported
/// success. The mirror of [`RoomPolicyPatchBody`], which sends the policy
/// alone for the same reason — one field per body, so neither control can
/// clobber the other's value.
#[derive(Debug, Clone, Serialize)]
struct RoomWorkspacePatchBody<'a> {
    workspace_root: Option<&'a str>,
}

#[derive(Debug, Clone, Serialize)]
struct RoomPolicyPatchBody<'a> {
    /// Always the COMPLETE policy. The daemon's PATCH replaces the stored
    /// policy wholesale (absent = unchanged, null = clear), so a partial
    /// object here would silently zero every flag it omitted.
    trigger_policy: &'a RoomTriggerPolicy,
}

#[derive(Debug, Clone, Serialize)]
struct JoinBody<'a> {
    id: &'a str,
    display_name: &'a str,
    kind: RoomParticipantKind,
}

#[derive(Debug, Clone, Serialize)]
struct PostMessageBody<'a> {
    author_id: &'a str,
    author_kind: RoomParticipantKind,
    body: &'a str,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread_parent_seq: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
struct RetryOutboxBody<'a> {
    client_event_id: &'a str,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct RetryOutboxSuccess {
    ok: bool,
    access: RoomAccessProjection,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
struct RetryOutboxErrorResponse {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Identity of this surface as a room participant. Browser hosts receive it
/// from the current authenticated proxy session; direct hosts use the stable
/// single-operator identity.
#[derive(Debug, Clone)]
pub struct RoomIdentity {
    pub id: String,
    pub display_name: String,
}

impl RoomIdentity {
    fn unresolved() -> Self {
        Self {
            id: String::new(),
            display_name: String::new(),
        }
    }

    pub(crate) fn from_proxy_config(id: &str, display_name: &str) -> Self {
        let id = id.trim();
        if id.is_empty() {
            return Self {
                id: SINGLE_OPERATOR_ROOM_ID.to_string(),
                display_name: "Operator".to_string(),
            };
        }
        let display_name = display_name.trim();
        Self {
            id: id.to_string(),
            display_name: if display_name.is_empty() {
                id.to_string()
            } else {
                display_name.to_string()
            },
        }
    }

    pub(crate) fn direct_host() -> Self {
        Self::from_proxy_config("", "")
    }
}

/// Outcome of a typed create-room operation. Each outcome carries the
/// request it resolves, so surfaces can gate only the matching attempt
/// and leave concurrent submits untouched.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateOutcome {
    /// Room created successfully (key).
    Success { key: String },
    /// Daemon rejected as a duplicate.
    Duplicate,
    /// Daemon rejected for another reason, or client-side reject
    /// (empty name, encode error, network failure).
    Failed { error: String },
}

/// Resolved action for a create-room dispatch — what the surface
/// Effect should do after inspecting the op-id slot.
#[derive(Debug, PartialEq)]
pub enum CreateResolution {
    /// Creation succeeded — clear the draft.
    Success,
    /// Creation failed or was rejected — keep the draft for retry.
    KeepDraft,
    /// No outcome yet (in flight) or op_id belongs to another attempt.
    Pending,
}

/// CAS admission guard for the create-room result channel.
/// Simple op-id comparison — only the dispatch that currently owns
/// the slot may publish status and select its room.
pub struct CasAdmission;

impl CasAdmission {
    /// True when `slot_op` matches `my_op` — the completion is current.
    pub fn admit(slot_op: u64, my_op: u64) -> bool {
        slot_op == my_op
    }
}

/// Reactive handle for the rooms panel. Holds the room list, the open room +
/// its transcript, status text, and the SSE-tail generation counter. Cloned
/// freely (all fields are `Copy` signal handles), like [`crate::daemon::Daemon`].
#[derive(Clone, Copy)]
pub struct Rooms {
    /// Daemon base URL signal, shared with `Daemon::url` so requests follow the
    /// origin learned at bootstrap (phone-via-tunnel resolves it asynchronously,
    /// so we must read it live at request time, not snapshot it at construction).
    pub url: RwSignal<String>,
    /// The daemon's model catalogue signal, shared with `Daemon::models` (`GET
    /// /v1/models`, populated once at bootstrap). Rooms itself never reads it;
    /// it is carried so the members rail can hand it to the agent builder,
    /// whose model picker must offer the daemon's own list rather than a
    /// hardcoded one. Sharing the handle means zero extra requests.
    pub models: RwSignal<Vec<crate::daemon::ModelInfo>>,
    /// All persistent rooms (from `GET /v1/rooms/persistent`).
    pub list: RwSignal<Vec<Room>>,
    /// Whether the first `fetch_rooms` has resolved (success or failure). Starts
    /// false so the panel shows a loading placeholder instead of falsely
    /// asserting "No rooms yet" during the initial in-flight fetch.
    pub rooms_loaded: RwSignal<bool>,
    /// Whether the latest room-list request is still in flight.
    pub rooms_loading: RwSignal<bool>,
    /// Error from the latest room-list request, if that request failed.
    pub rooms_error: RwSignal<Option<String>>,
    /// Monotonic ticket ensuring only the latest overlapping list request may
    /// publish list/loading/error state.
    list_request_ticket: RwSignal<u64>,
    /// The currently selected room key, if any.
    pub open_key: RwSignal<Option<String>>,
    /// The open room's full record (roster + metadata).
    pub open_room: RwSignal<Option<Room>>,
    /// The open room's transcript, ascending by `seq`.
    pub transcript: RwSignal<Vec<RoomMessage>>,
    /// The open room's resume point: the highest `seq` this client has ingested,
    /// and the module's ONE answer to where a catch-up read starts. Hydration
    /// seeds it from `/snapshot`'s own cursor through [`Rooms::start_live_tail`],
    /// and both the tail and [`Rooms::refresh_open_transcript`] advance it as
    /// they ingest — so nothing re-derives a resume from the painted rows, which
    /// is the rule `start_live_tail` states and the catch-up read used to break.
    /// Monotonic: see [`advanced_resume_seq`] for why a lagging ingest may never
    /// lower it.
    resume_seq: RwSignal<Option<u64>>,
    /// Where an on-demand older read resumes, and the whole reason the workspace
    /// can offer one. `Some` means the daemon said older rows exist and nothing
    /// on screen reaches them; `None` means a page provably reached the start of
    /// the log, or no room is open. Written wherever
    /// [`Rooms::backfill_open_transcript`] stops — a walk that hits its page cap
    /// used to drop the page's `prev_seq` and `has_more` at that instant, which
    /// is what left a long room's oldest painted row reading as the first
    /// message in it. Read through [`Rooms::older_transcript_available`].
    older_cursor: RwSignal<Option<u64>>,
    /// Whether a backward read has ANSWERED for this room — the second half of
    /// the affordance's state, and the whole reason it can say something rather
    /// than disappear.
    ///
    /// `older_cursor` alone cannot: `None` is FOUR situations at once — no room
    /// open, hydration still walking, a page that provably reached the start of
    /// the log, and a walk that never ran. The first two must render nothing and
    /// the third must say the beginning has been reached, and telling them apart
    /// is exactly what "the oldest row on screen is the first message in this
    /// room" needs before it may be claimed. Set wherever a backward page has
    /// been read to completion, or where the first paint provably held the whole
    /// log; cleared with the cursor on the way out of a room. See
    /// [`older_history_state`], which is where the two are combined.
    older_settled: RwSignal<bool>,
    /// Whether an on-demand older page is in flight, so a second press cannot
    /// fire a second request against a cursor the first has not yet moved —
    /// which would prepend the same page twice were `prepend_transcript_page`
    /// not strict about it, and spends a request regardless.
    older_in_flight: RwSignal<bool>,
    /// Free-form status line (errors, in-flight notices).
    pub status: RwSignal<String>,
    /// Monotonic generation: bumped when the open room changes so a stale
    /// poll/SSE loop retires instead of writing into the wrong room.
    generation: RwSignal<u64>,
    /// Current participant id, used for join/leave/post. Browser-hosted Rooms
    /// keep this empty until the signed-in identity resolves from `/api/config`.
    pub identity_id: RwSignal<String>,
    /// This browser's display name.
    pub identity_name: RwSignal<String>,
    /// Whether `identity_id` came from the DAEMON rather than from a
    /// current authenticated config response. False until `/api/config`
    /// answers on browser hosts. See
    /// [`Rooms::identity_resolved`] for why the distinction is the whole
    /// difference between a gate and a formality.
    pub identity_authoritative: RwSignal<bool>,
    /// Tail state for the live connection indicator. Starts as Replaying during
    /// initial catch-up, switches to Live once connected, and to Reconnecting on
    /// drop/retry. The view reads this to render the status bar indicator.
    tail_state: RwSignal<TailState>,
    /// Available agent names fetched from GET /v1/agents (TASK-9/TASK-11).
    pub available_agents: RwSignal<Vec<String>>,
    /// Whether the first `fetch_agents` has resolved. Starts false so the
    /// add-agent picker shows nothing rather than a premature "No agents" while
    /// the initial `/v1/agents` fetch is still in flight (same flash class as
    /// `rooms_loaded`).
    pub agents_loaded: RwSignal<bool>,
    /// Required access projection for the open room. `None` means loading or
    /// no room is open; local rooms carry `Some(state = Local)`.
    pub access: RwSignal<Option<RoomAccessProjection>>,
    /// Whether the open room is the daemon's frozen soft-closed audit view
    /// rather than a live room, from `/snapshot`'s `closed`. A separate axis
    /// from `access` and not derivable from it — closing stamps `closed_at` on
    /// the room row and leaves the access row alone, so a frozen room goes on
    /// projecting exactly what it projected while it was live — which is why
    /// it is a signal beside `access` rather than another access state. False
    /// while loading, for every open room, and against a daemon that predates
    /// the field. `open_room` publishes it and starts no tail on `true`; the
    /// composer's write gate
    /// ([`crate::rooms_workspace::composer_writes_allowed`]) reads it to hold
    /// every send, and [`Rooms::retry_outbox`] refuses on it too.
    pub closed: RwSignal<bool>,
    /// Which worker owns which Agent participant in the open room, in roster
    /// order, from `/snapshot`'s `agent_owners`. Empty means loading, no open
    /// room, no owned agents, or a daemon predating the field — all four read
    /// the same way in the rail, which renders every agent it cannot match as
    /// unclaimed. Published by [`Rooms::open_room`] unconditionally on
    /// `closed`, because a frozen room is the one whose ownership a reader can
    /// no longer ask anyone about.
    pub agent_owners: RwSignal<Vec<RoomAgentOwner>>,
    /// Per-room durable unread summary from the daemon room list.
    pub read_summaries: RwSignal<HashMap<String, RoomReadSummary>>,
    /// Durable read cursor for the currently open room.
    pub open_read_cursor: RwSignal<Option<RoomReadCursorProjection>>,
    /// Monotonic in-flight guard for PATCH /read-cursor on the open room.
    read_cursor_in_flight: RwSignal<Option<u64>>,
    /// Last read cursor value attempted for the open room; dedupes monotonic re-sends.
    last_sent_read_cursor: RwSignal<Option<u64>>,
    /// Surfaces snapshot the op_id before dispatching and gate only on the
    /// outcome carrying a matching id — concurrent submits never cross-resolve.
    pub create_op: RwSignal<(u64, Option<CreateOutcome>)>,
    /// Whether a trigger-policy PATCH on the open room is in flight. The
    /// workspace disables its toggles on this, so two flips can never
    /// interleave and resolve out of order.
    pub policy_update_in_flight: RwSignal<bool>,
    /// Error from the last trigger-policy PATCH, shown inline by the toggles
    /// section. A refused flip does not snap the box back — `open_room` is
    /// untouched, so nothing re-renders — which is why the error must be
    /// visible: the box alone would overstate what was stored.
    pub policy_update_error: RwSignal<Option<String>>,
    /// Whether a workspace-binding PATCH on the open room is in flight. Its own
    /// flag rather than a share of `policy_update_in_flight`: the two send
    /// disjoint bodies to the same route, so holding one control while the
    /// other is mid-flight would be a hold with nothing behind it.
    pub workspace_update_in_flight: RwSignal<bool>,
    /// Typed outcome of the last workspace-binding PATCH, shown inline beside
    /// the control. `None` is "nothing to report" — a success clears it,
    /// because the section re-renders from the record the daemon returned and
    /// the binding is then visible on its own.
    pub workspace_update_status: RwSignal<Option<WorkspaceBindStatus>>,
}

/// What the surface can say about a workspace-binding PATCH, decided from the
/// daemon's reply rather than from the path the operator typed.
///
/// The surface never pre-validates the path: the folder has to exist on the
/// machine running the DAEMON, which a browser cannot see, so any local check
/// would be guessing about a filesystem it has no access to. The daemon
/// canonicalizes and answers, and this is the reading of that answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceBindStatus {
    /// The daemon's `400 invalid_workspace_root`: not an absolute path, or not
    /// an existing directory on the daemon's host.
    InvalidPath,
    /// Anything else that went wrong — transport, decode, an unrecognised
    /// daemon error — carried verbatim so a new daemon refusal is readable
    /// here before this surface learns to name it.
    Failed(String),
}

impl WorkspaceBindStatus {
    /// The sentence shown beside the control.
    ///
    /// Deliberately NOT the `workspace_unavailable` wording `room_repo.rs`
    /// uses: that is the COMPUTE lane saying Bedrock is unreachable, a
    /// different condition with a different fix. This one is about a path on
    /// the daemon's own host.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidPath => "that folder is not an absolute path, or does not exist on the machine running the daemon".to_string(),
            Self::Failed(error) => format!("workspace update failed: {error}"),
        }
    }

    /// Read a failed PATCH's daemon `error` string into a typed status. The
    /// daemon's refusal body is the frozen `{"ok": false, "error":
    /// "invalid_workspace_root"}`, so the exact code is what this matches —
    /// never a substring of prose, which would retag an unrelated message that
    /// happened to quote it.
    pub fn from_daemon_error(error: &str) -> Self {
        if error.trim() == "invalid_workspace_root" {
            Self::InvalidPath
        } else {
            Self::Failed(error.to_string())
        }
    }
}

/// Is this room unbound — no workspace folder on the daemon's host, so every
/// agent turn in it fails closed before it starts?
///
/// A pure predicate over the decoded record, so the notice and the control's
/// wording cannot drift apart, and so the rule is testable without a browser.
pub fn room_is_unbound(room: &Room) -> bool {
    room.workspace_root
        .as_deref()
        .is_none_or(|root| root.trim().is_empty())
}

/// Should the workspace field re-seed itself from the room's stored binding?
///
/// Only when the open room's IDENTITY changed. The seeding effect has to read
/// `open_room` to find the stored value, which makes it re-run on every write
/// to that signal — including a trigger-policy PATCH completing two inches
/// away, or any hydration refresh. Re-seeding on each of those overwrites a
/// path the operator is halfway through typing, so the text vanishes before
/// Bind can be pressed. Keyed on identity, an unrelated update leaves the draft
/// alone and only a room switch replaces it.
pub fn workspace_draft_should_reseed(seeded_for: Option<&str>, open_room_id: Option<&str>) -> bool {
    seeded_for != open_room_id
}

/// The value a create form's workspace field should send: the trimmed path, or
/// `None` when the operator left it empty. Empty means "leave the room
/// unbound", which is exactly what the field being absent from the body says.
pub fn create_workspace_root(draft: &str) -> Option<String> {
    let trimmed = draft.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomsFetchMode {
    Interactive,
    Silent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoomsListSuccess {
    rooms: Vec<Room>,
    read_summaries: HashMap<String, RoomReadSummary>,
}

impl Rooms {
    /// Construct a rooms handle that shares the live `Daemon::url` signal, so it
    /// always targets the origin resolved by bootstrap. Room collaboration is
    /// daemon-native text; LiveKit state is intentionally outside this type.
    pub fn new(daemon: &crate::daemon::Daemon) -> Self {
        // A browser login is authoritative, so never act under the identity a
        // previous tenant left in localStorage while `/api/config` is in flight.
        // Direct extension/Tauri hosts have no proxy login and use their stable
        // local-operator identity immediately.
        let direct_host =
            crate::daemon::running_as_extension() || crate::daemon::running_as_tauri();
        let identity = if direct_host {
            RoomIdentity::direct_host()
        } else {
            RoomIdentity::unresolved()
        };
        let daemon_adopted_id = daemon.adopted_user_id;
        let daemon_adopted_name = daemon.adopted_display_name;
        let rooms = Self {
            url: daemon.url,
            models: daemon.models,
            list: RwSignal::new(Vec::new()),
            rooms_loaded: RwSignal::new(false),
            rooms_loading: RwSignal::new(false),
            rooms_error: RwSignal::new(None),
            list_request_ticket: RwSignal::new(0),
            open_key: RwSignal::new(None),
            open_room: RwSignal::new(None),
            transcript: RwSignal::new(Vec::new()),
            resume_seq: RwSignal::new(None),
            older_cursor: RwSignal::new(None),
            older_settled: RwSignal::new(false),
            older_in_flight: RwSignal::new(false),
            status: RwSignal::new(String::new()),
            generation: RwSignal::new(0),
            identity_id: RwSignal::new(identity.id),
            identity_name: RwSignal::new(identity.display_name),
            identity_authoritative: RwSignal::new(direct_host),
            tail_state: RwSignal::new(TailState::Replaying),
            available_agents: RwSignal::new(Vec::new()),
            agents_loaded: RwSignal::new(false),
            access: RwSignal::new(None),
            closed: RwSignal::new(false),
            agent_owners: RwSignal::new(Vec::new()),
            read_summaries: RwSignal::new(HashMap::new()),
            open_read_cursor: RwSignal::new(None),
            read_cursor_in_flight: RwSignal::new(None),
            last_sent_read_cursor: RwSignal::new(None),
            create_op: RwSignal::new((0, None)),
            policy_update_in_flight: RwSignal::new(false),
            policy_update_error: RwSignal::new(None),
            workspace_update_in_flight: RwSignal::new(false),
            workspace_update_status: RwSignal::new(None),
        };

        // Identity is RESOLVED, not snapshotted. `Rooms::new` runs synchronously
        // in the App body while bootstrap's /api/config fetch is still in
        // flight, so the value read above is whatever the last session left
        // behind. This effect rewrites it the moment bootstrap answers, which is
        // what stops the first session after a login from acting as the previous
        // identity.
        let handle = rooms;
        Effect::new(move |_| {
            let id = daemon_adopted_id.get();
            if id.is_empty() {
                return;
            }
            let name = daemon_adopted_name.get();
            let display = if name.is_empty() { id.clone() } else { name };
            handle.identity_id.set(id);
            handle.identity_name.set(display);
            // Only now may this surface act. Set last, after both strings are
            // in place, so nothing can observe an authoritative flag over a
            // half-written identity.
            handle.identity_authoritative.set(true);
        });

        rooms
    }

    /// Whether bootstrap has resolved who we are. Join and post refuse while
    /// this is false: acting under an unresolved identity is exactly how ghost
    /// members were created.
    ///
    /// The test is "the daemon answered", NOT merely "we have a non-empty id".
    /// Direct hosts are authoritative at construction; browser hosts stay
    /// unresolved until the current authenticated `/api/config` response.
    pub fn identity_resolved(&self) -> bool {
        identity_may_act(
            self.identity_authoritative.get_untracked(),
            &self.identity_id.get_untracked(),
        )
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }
    /// Reactive projection used by the workspace to suppress its empty state
    /// until snapshot replay has reached the live room tail.
    pub fn transcript_tail_is_live(&self) -> bool {
        self.tail_state.get() == TailState::Live
    }

    /// Current-room predicate over the live `generation` and `open_key`
    /// signals. Async tail work checks it before any non-frame state write;
    /// decoded frames pass through `accept_room_tail_frame` below.
    ///
    /// `pub(crate)` so sibling modules (`rooms_workspace.rs`) holding a
    /// previously-captured `(generation, key)` pair — e.g. a pending
    /// read-advance request built while a room was open — can re-validate it
    /// before dispatching a mutating request. A same-key close/reopen bumps
    /// `generation`, so a stale pair is rejected even though the key still
    /// matches the newly-reopened room.
    pub(crate) fn room_is_current(&self, generation_id: u64, key: &str) -> bool {
        room_request_is_current(
            generation_id,
            self.generation.get_untracked(),
            key,
            self.open_key.get_untracked().as_deref(),
        )
    }

    /// `pub(crate)` snapshot of the live room-identity generation counter —
    /// bumped by every `open_room`/`close_room`. Exposed so a caller building
    /// state that outlives one render (e.g. `ReadAdvanceRequest`) can stamp it
    /// with "as of which room admission" it was computed, then re-validate
    /// via `room_is_current` before acting on it later.
    pub(crate) fn generation_snapshot(&self) -> u64 {
        self.generation.get_untracked()
    }

    /// Reactive generation read for UI lifecycle Effects whose state belongs
    /// to one exact open-room admission. Request code should continue to use
    /// [`Rooms::generation_snapshot`] plus [`Rooms::room_is_current`].
    pub(crate) fn generation_snapshot_reactive(&self) -> u64 {
        self.generation.get()
    }

    /// Synchronously clear the open-room signals and pin `tail_state` to
    /// `Replaying` so no prior room state leaks into the next open. Shared
    /// by `open_room` (pre-hydrate) and `close_room`.
    fn reset_room_state(&self) {
        self.open_room.set(None);
        self.transcript.set(Vec::new());
        self.resume_seq.set(None);
        // Both halves of the older-history state, cleared for the same reason
        // the transcript is: a cursor is one room's position in one room's log,
        // and an in-flight press outlives the room it was made in — its
        // completion re-checks `room_is_current` and writes nothing, so nothing
        // else will ever lower the flag.
        self.older_cursor.set(None);
        // Cleared with the cursor, and for the stronger reason: left standing,
        // the next room opened would claim its first painted row is the start
        // of the log for as long as its own hydration takes to answer.
        self.older_settled.set(false);
        self.older_in_flight.set(false);
        self.access.set(None);
        self.closed.set(false);
        // Ownership is one room's roster fact. Left standing, the previous
        // room's rows would badge same-named agents in the next room opened,
        // and the rail has no other way to tell they are stale.
        self.agent_owners.set(Vec::new());
        self.open_read_cursor.set(None);
        self.read_cursor_in_flight.set(None);
        self.last_sent_read_cursor.set(None);
        self.tail_state.set(TailState::Replaying);
        // In-flight stays as-is — the completion clears it itself — but a
        // previous room's PATCH failure must not read as this room's.
        self.policy_update_error.set(None);
        // Same rule for the binding control: a rejected path belongs to the
        // room it was typed into, and nowhere else.
        self.workspace_update_status.set(None);
    }

    /// Whether the current identity is joined according to the room's explicit
    /// access authority. Local rooms use the daemon-native roster; every
    /// non-local state uses only the safe access-member projection.
    pub fn joined_open(&self) -> bool {
        joined_open_for(
            self.access.get().as_ref(),
            self.open_room.get().as_ref(),
            &self.identity_id.get(),
        )
    }

    /// Fetch the room list (`GET /v1/rooms/persistent`). Overlapping requests
    /// are latest-wins: an older completion cannot publish any list lifecycle
    /// state after a newer request has started.
    pub fn fetch_rooms(&self) {
        self.fetch_rooms_with_mode(RoomsFetchMode::Interactive);
    }

    pub fn fetch_rooms_silent(&self) {
        self.fetch_rooms_with_mode(RoomsFetchMode::Silent);
    }

    fn fetch_rooms_with_mode(&self, mode: RoomsFetchMode) {
        if should_skip_rooms_fetch(mode, self.rooms_loading.get_untracked()) {
            return;
        }
        let base = self.base();
        let me = *self;
        let ticket = self.list_request_ticket.get_untracked().wrapping_add(1);
        self.list_request_ticket.set(ticket);
        if matches!(mode, RoomsFetchMode::Interactive) {
            self.rooms_loading.set(true);
            self.rooms_error.set(None);
        }
        spawn_local(async move {
            let get_url = format!("{base}/v1/rooms/persistent");
            let result = match Request::get(&get_url).send().await {
                Ok(resp) => match resp.json::<RoomsListResponse>().await {
                    Ok(r) if r.ok => match read_summaries_from_wire(&r.read_states) {
                        Ok(read_summaries) => Ok(RoomsListSuccess {
                            rooms: r.rooms,
                            read_summaries,
                        }),
                        Err(error) => Err(format!("rooms decode error: {error}")),
                    },
                    Ok(r) => Err(format!(
                        "rooms list failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => Err(format!("rooms decode error: {err}")),
                },
                Err(err) => Err(format!("rooms fetch error: {err}")),
            };
            let is_current =
                list_request_is_current(ticket, me.list_request_ticket.get_untracked());
            finish_rooms_fetch(&me.rooms_loaded, &me.rooms_loading, mode, is_current);
            if !is_current {
                return;
            }
            match result {
                Ok(success) => {
                    me.list.set(success.rooms.clone());
                    me.read_summaries.update(|current| {
                        *current = merge_room_read_summaries(
                            current,
                            &success.rooms,
                            &success.read_summaries,
                        );
                    });
                    me.rooms_error.set(None);
                }
                Err(error) => {
                    if matches!(mode, RoomsFetchMode::Interactive) {
                        me.status.set(error.clone());
                        me.rooms_error.set(Some(error));
                    }
                }
            }
        });
    }

    /// Fetch available agent names from GET /v1/agents (TASK-9/TASK-11).
    /// The daemon returns `{ "ok": true, "agents": [{"name":"flux", ...}, ...] }`.
    /// We extract just the names for the room agent picker.
    pub fn fetch_agents(&self) {
        let base = self.base();
        let agents_sig = self.available_agents;
        let loaded = self.agents_loaded;
        spawn_local(async move {
            let url = format!("{base}/v1/agents");
            match Request::get(&url).send().await {
                Ok(resp) => {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        if let Some(agents) = json.get("agents").and_then(|a| a.as_array()) {
                            let ids: Vec<String> = agents
                                .iter()
                                .filter_map(|a| {
                                    a.get("name")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string())
                                })
                                .collect();
                            agents_sig.set(ids);
                        } else {
                            agents_sig.set(Vec::new());
                        }
                    }
                }
                Err(_) => {
                    agents_sig.set(Vec::new());
                }
            }
            // Resolved (success, empty, or error) — the picker may now show
            // "No agents" honestly instead of during the in-flight window.
            loaded.set(true);
        });
    }

    /// Create a room (`POST /v1/rooms/persistent`) with an optional auto-convene
    /// `trigger_policy`, then select it. The daemon keys rooms by `key`; we
    /// derive a url-safe key from the name but keep the human name intact.
    /// Atomically dispatch a create-room request, returning the op_id the
    /// caller should snapshot. When the request resolves, `create_op` carries
    /// a typed outcome tagged with that id — surfaces gate only on their own
    /// id and leave concurrent submits untouched.
    ///
    /// Every side effect is gated on CAS admission: a stale completion
    /// superseded by a later dispatch must never select the wrong room or
    /// overwrite the current attempt's status. Stale successes still refresh
    /// the room list so a server-created room is discoverable.
    ///
    /// Callers should gate dispatch on `pending_create` to prevent concurrent
    /// attempts — the closure in `rooms_workspace.rs` does this.
    ///
    /// `workspace_root` is the folder on the DAEMON's host the new room binds
    /// to. `None` (an empty field) leaves the room unbound, which is what every
    /// room this form made used to be — and an unbound room's agent turns all
    /// fail closed with `workspace_unavailable`, so a mention wakes nothing.
    pub fn create_room(
        &self,
        name: String,
        policy: Option<RoomTriggerPolicy>,
        workspace_root: Option<String>,
    ) -> u64 {
        let name = name.trim().to_string();
        if name.is_empty() {
            return 0;
        }
        let key = slugify(&name);
        if key.is_empty() {
            self.status
                .set("room name needs at least one letter/number".into());
            return 0;
        }
        let base = self.base();
        let me = *self;
        let status = self.status;
        let signal = self.create_op;
        let op_id = {
            let (n, _) = signal.get_untracked();
            n.wrapping_add(1)
        };
        // Set "in flight" immediately so the caller doesn't immediately
        // observe a stale-success resolution from a prior request.
        signal.set((op_id, None));
        spawn_local(async move {
            let body = CreateRoomBody {
                key: &key,
                name: &name,
                trigger_policy: policy,
                workspace_root: workspace_root.as_deref(),
            };
            let post_url = format!("{base}/v1/rooms/persistent");
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            let res = match res {
                Ok(req) => req.send().await,
                Err(err) => {
                    // Encode error: CAS-gate the status + outcome together.
                    // Stale encode errors are fully suppressed — they carry
                    // no server-side state.
                    signal.update(|(cur, out)| {
                        if Self::cas_admit_create(*cur, op_id) {
                            status.set(format!("create encode error: {err}"));
                            *out = Some(CreateOutcome::Failed {
                                error: format!("encode: {err}"),
                            });
                        }
                    });
                    return;
                }
            };
            let outcome = match res {
                Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                    Ok(r) if r.ok => CreateOutcome::Success { key: key.clone() },
                    Ok(r) => {
                        let msg = r.error.unwrap_or_else(|| "unknown error".into());
                        if msg.to_lowercase().contains("duplicate") {
                            CreateOutcome::Duplicate
                        } else {
                            CreateOutcome::Failed { error: msg }
                        }
                    }
                    Err(err) => CreateOutcome::Failed {
                        error: format!("decode: {err}"),
                    },
                },
                Err(err) => CreateOutcome::Failed {
                    error: format!("post: {err}"),
                },
            };
            // CAS-gated publish. Admitted: full status + select + list refresh.
            // Stale success: list refresh only (server-created room must be
            // discoverable). Stale failure/duplicate: fully suppressed.
            signal.update(|(cur, out)| {
                if Self::cas_admit_create(*cur, op_id) {
                    *out = Some(outcome.clone());
                    match &outcome {
                        CreateOutcome::Success { key } => {
                            status.set(format!("room '{name}' created"));
                            me.fetch_rooms();
                            me.open_room(key.clone());
                        }
                        CreateOutcome::Duplicate => {
                            status.set(format!("room '{name}' already exists"));
                        }
                        CreateOutcome::Failed { error } => {
                            status.set(format!("create failed: {error}"));
                        }
                    }
                } else if matches!(&outcome, CreateOutcome::Success { .. }) {
                    // Stale success: room exists server-side — refresh the
                    // list so it's discoverable, but never select or report.
                    me.fetch_rooms();
                }
                // Stale failure/duplicate: no side effects.
            });
        });
        op_id
    }

    /// Whether a create-room completion is still current (not superseded
    /// by a later dispatch). Admitted = the slot op matches ours; stale
    /// = the slot was claimed by a newer dispatch.
    pub fn cas_admit_create(slot_op: u64, my_op: u64) -> bool {
        CasAdmission::admit(slot_op, my_op)
    }

    /// Resolve a pending create-room dispatch from the op-id slot.
    /// Called by the surface Effect — only the matching op_id's outcome
    /// determines whether to clear the draft.
    pub fn resolve_create_op(
        current_op: u64,
        my_op: u64,
        outcome: Option<&CreateOutcome>,
    ) -> CreateResolution {
        if current_op != my_op {
            return CreateResolution::Pending;
        }
        match outcome {
            Some(CreateOutcome::Success { .. }) => CreateResolution::Success,
            Some(CreateOutcome::Duplicate) | Some(CreateOutcome::Failed { .. }) => {
                CreateResolution::KeepDraft
            }
            None => CreateResolution::Pending,
        }
    }

    /// Open a room: load its record + the first transcript page, bump the
    /// generation, and start the room-scoped SSE live tail (TASK-10/TASK-11).
    /// Hydration reads `/snapshot`, the route that answers a cursor, so the tail
    /// resumes from the sequence the daemon says it served rather than one
    /// re-derived from the rows on screen. This never was a "full transcript",
    /// and which END of the log it is one page OF is the whole question: the
    /// read now asks backward ([`room_snapshot_url`]), so a room past 1000 rows
    /// opens on its NEWEST page and [`Rooms::backfill_open_transcript`] walks
    /// older from there. Paging forward from the start instead meant opening on
    /// message #1 of a 12 000-row room and reaching the rows the operator came
    /// for only once the SSE tail's replay had dragged the eleven thousand
    /// between them through the stream — and in a soft-closed room, which opens
    /// no tail, not reaching them at all.
    ///
    /// `/snapshot` also falls through to the soft-closed audit view, so a room
    /// that used to fail to open now hydrates. The tail underneath it cannot:
    /// `/events` and `POST /messages` both 404 a closed room. The body now says
    /// which view answered ([`RoomSnapshotResponse::closed`]), and this is the
    /// one place that acts on it — a closed room opens NO `EventSource` at all.
    /// The gate has to live at this call site because
    /// [`Rooms::start_live_tail`] is an unconditional reconnect loop with no
    /// stop condition in it; a tail started against a corpse retries until the
    /// generation bumps. Publishing [`Rooms::closed`] beside the access
    /// projection is the other half: it holds the composer shut and puts the
    /// reason on screen, so the audit view reads as frozen rather than as a
    /// live room that silently refuses every write.
    pub fn open_room(&self, key: String) {
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked().wrapping_add(1);
        self.generation.set(generation_id);
        self.open_key.set(Some(key.clone()));
        self.reset_room_state();
        self.status.set("loading room…".into());

        spawn_local(async move {
            let get_url = room_snapshot_url(&base, &key);
            let result = match Request::get(&get_url).send().await {
                Ok(resp) if resp.ok() => match resp.json::<RoomSnapshotResponse>().await {
                    Ok(r) if r.ok => Ok((
                        r.room,
                        r.transcript,
                        r.access,
                        r.last_seq,
                        r.closed,
                        r.agent_owners,
                    )),
                    Ok(r) => Err(format!(
                        "room load failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => Err(format!("room decode error: {err}")),
                },
                Ok(resp) => {
                    let http_status = resp.status();
                    match resp.json::<RoomErrorResponse>().await {
                        Ok(r) => Err(format!(
                            "room load failed: {}",
                            r.error.unwrap_or_else(|| format!("HTTP {http_status}"))
                        )),
                        Err(err) => Err(format!("room load failed: HTTP {http_status} ({err})")),
                    }
                }
                Err(err) => Err(format!("room fetch error: {err}")),
            };
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok((room, transcript, access, last_seq, closed, agent_owners)) => {
                    let resume_seq = snapshot_resume_seq(last_seq, &transcript);
                    // Where the backward walk starts, read off the page's own
                    // size rather than its `has_more`. A backward page is the
                    // LAST `limit` rows that qualify, so a short one provably
                    // reached the start of the log and a full one is the only
                    // shape that can have rows behind it — which keeps the
                    // decode arm above answering exactly what it always did.
                    // The one case this costs is a room whose length is an
                    // exact multiple of the window: it spends a single request
                    // to be told there is nothing older.
                    let backfill_from =
                        hydration_backfill_start(&transcript, HYDRATION_TRANSCRIPT_LIMIT);
                    me.open_room.set(room);
                    me.transcript.set(transcript.clone());
                    me.access.set(Some(access.clone()));
                    me.closed.set(closed);
                    // Before the `closed` branch below and outside it: the
                    // snapshot IS the audit view, so a frozen room carries this
                    // exactly as a live one does, and it is the frozen room
                    // whose reader has nobody left to ask.
                    me.agent_owners.set(agent_owners);
                    update_open_summary_from_open_room(
                        &me.read_summaries,
                        me.open_key.get_untracked().as_deref(),
                        &transcript,
                        Some(&access),
                        me.open_read_cursor.get_untracked().as_ref(),
                    );
                    me.status.set(String::new());
                    me.fetch_agents();
                    // Unconditional on closedness, unlike the tail below: a
                    // soft-closed room is the one that needs this most, being
                    // the one with no tail to bring it anything else.
                    match backfill_from {
                        Some(before_seq) => {
                            me.backfill_open_transcript(&key, generation_id, before_seq)
                        }
                        // A first page shorter than the window provably reached
                        // the start of the log — `hydration_backfill_start` says
                        // so at length — so nothing is walking and nothing ever
                        // will. That is a SETTLED answer, and the only one that
                        // arrives without a second request: without it, every
                        // short room would render the affordance's "no room
                        // open / still loading" state forever, which is exactly
                        // the silence this slice exists to remove.
                        None => me.older_settled.set(true),
                    }
                    // Not started rather than started-and-stopped: `/events`
                    // 404s a closed room and the loop below treats every
                    // failure as a reason to retry, so the only connection
                    // that never reconnects is the one never opened.
                    if !closed {
                        me.start_live_tail(key, generation_id, resume_seq);
                    }
                }
                Err(error) => me.status.set(error),
            }
        });
    }
    /// Close the open room and stop its live loops.
    pub fn close_room(&self) {
        self.generation.update(|g| *g = g.wrapping_add(1));
        self.open_key.set(None);
        self.reset_room_state();
    }

    /// Apply ONE field of a PATCH response onto the open room and its list row,
    /// leaving every other field as it stands.
    ///
    /// Both room PATCHes answer with the WHOLE `Room`, and the two run
    /// concurrently on purpose — separate in-flight flags, because they write
    /// disjoint fields and holding one control while the other is mid-flight
    /// would be a hold with nothing behind it. Disjoint on the WIRE is not
    /// disjoint in the projection, though: the daemon applies the two writes in
    /// ITS order and the replies race back in THEIRS, so a reply carrying the
    /// other field's pre-change value can land last. Replacing the record
    /// wholesale then reverts a field that is durably stored — and the trigger
    /// toggle builds its next write from the record it can see, so a stale
    /// projection becomes a stale WRITE that un-does a persisted flag.
    ///
    /// Each response therefore merges only the field it owns. The room's other
    /// state (roster, timestamps) keeps arriving through hydration and the SSE
    /// tail, which is where it came from before either control existed.
    fn merge_room_field(&self, answered: &Room, apply: impl Fn(&mut Room, &Room)) {
        self.list.update(|rooms| {
            if let Some(entry) = rooms.iter_mut().find(|r| r.id == answered.id) {
                apply(entry, answered);
            }
        });
        self.open_room.update(|current| {
            if let Some(current) = current.as_mut() {
                // The generation guard already proved the room is current; this
                // id check is what stops a merge landing on a different record
                // if that ever stops being true.
                if current.id == answered.id {
                    apply(current, answered);
                }
            }
        });
    }

    /// Replace the open room's trigger policy (`PATCH /v1/rooms/persistent/{key}`).
    /// Callers flip one flag on a copy of the room's CURRENT policy and pass
    /// the whole thing — the daemon replaces rather than merges, so a delta
    /// would clear every flag it omitted. Success re-renders from the record
    /// the daemon returned, so a shown checkmark is always durable state.
    /// Generation-gated like the read-cursor PATCH: a response that lands
    /// after the operator switched rooms writes nothing.
    pub fn update_open_room_policy(&self, policy: RoomTriggerPolicy) {
        if self.policy_update_in_flight.get_untracked() {
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        self.policy_update_in_flight.set(true);
        self.policy_update_error.set(None);
        spawn_local(async move {
            let patch_url = format!("{base}/v1/rooms/persistent/{}", encode(&key));
            let body = RoomPolicyPatchBody {
                trigger_policy: &policy,
            };
            let result = match Request::patch(&patch_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(req) => match req.send().await {
                    Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                        Ok(r) if r.ok => Ok(r.room),
                        Ok(r) => Err(r.error.unwrap_or_else(|| "unknown error".into())),
                        Err(err) => Err(format!("decode: {err}")),
                    },
                    Err(err) => Err(format!("patch: {err}")),
                },
                Err(err) => Err(format!("encode: {err}")),
            };
            me.policy_update_in_flight.set(false);
            if !me.room_is_current(generation_id, &key) {
                // The operator moved on. On success the daemon already holds
                // the change and the next open re-reads it; on failure the
                // error belongs to a room that is no longer on screen.
                return;
            }
            match result {
                Ok(room) => {
                    if let Some(room) = room {
                        // Only the policy — see `merge_room_field`. Replacing
                        // the record wholesale here would revert a workspace
                        // binding the other PATCH had already stored.
                        me.merge_room_field(&room, |dst, src| {
                            dst.trigger_policy = src.trigger_policy.clone();
                        });
                    }
                }
                Err(error) => me.policy_update_error.set(Some(error)),
            }
        });
    }

    /// Bind or unbind the open room's workspace folder
    /// (`PATCH /v1/rooms/persistent/{key}` carrying `workspace_root` alone).
    ///
    /// `Some(path)` binds — the path must be absolute and must exist on the
    /// machine running the DAEMON, which is the only host that can see it, so
    /// the daemon canonicalizes and refuses; this surface never pre-validates.
    /// `None` sends an explicit `null` and unbinds, putting the room back to
    /// the state where its agent turns fail closed.
    ///
    /// The body carries `workspace_root` alone, so this can never disturb the
    /// stored trigger policy — the daemon leaves an ABSENT field unchanged,
    /// which is the same reason `update_open_room_policy` may send its field
    /// alone without clearing the binding.
    ///
    /// Generation-gated exactly like the policy PATCH: a reply that lands after
    /// the operator switched rooms writes nothing.
    ///
    /// Requires an `ocean-os` daemon carrying `workspace_root` on
    /// `RoomUpdateRequest`. An older daemon rejects the unknown field
    /// (`deny_unknown_fields`) with a typed 400, which surfaces here as a
    /// [`WorkspaceBindStatus::Failed`] naming what it said rather than a
    /// silent no-op.
    pub fn set_open_room_workspace(&self, workspace_root: Option<String>) {
        if self.workspace_update_in_flight.get_untracked() {
            return;
        }
        // A soft-closed room is a frozen audit view: the daemon's `update`
        // writes an OPEN room only, so every bind against a closed one is a
        // guaranteed 404 dressed up as a failed write. The controls are hidden
        // in that state; this is the second lock, so a caller that reaches the
        // method another way cannot spend a round trip to be told no.
        if self.closed.get_untracked() {
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        self.workspace_update_in_flight.set(true);
        self.workspace_update_status.set(None);
        spawn_local(async move {
            let patch_url = format!("{base}/v1/rooms/persistent/{}", encode(&key));
            let body = RoomWorkspacePatchBody {
                workspace_root: workspace_root.as_deref(),
            };
            let result = match Request::patch(&patch_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(req) => match req.send().await {
                    Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                        Ok(r) if r.ok => Ok(r.room),
                        Ok(r) => Err(WorkspaceBindStatus::from_daemon_error(
                            r.error.as_deref().unwrap_or("unknown error"),
                        )),
                        Err(err) => Err(WorkspaceBindStatus::Failed(format!("decode: {err}"))),
                    },
                    Err(err) => Err(WorkspaceBindStatus::Failed(format!("patch: {err}"))),
                },
                Err(err) => Err(WorkspaceBindStatus::Failed(format!("encode: {err}"))),
            };
            me.workspace_update_in_flight.set(false);
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(room) => {
                    if let Some(room) = room {
                        // Only the binding — see `merge_room_field`. Replacing
                        // the record wholesale here would revert a trigger flag
                        // the other PATCH had already stored.
                        me.merge_room_field(&room, |dst, src| {
                            dst.workspace_root = src.workspace_root.clone();
                        });
                    }
                }
                Err(status) => me.workspace_update_status.set(Some(status)),
            }
        });
    }

    /// Join the open room as the current identity
    /// (`POST .../participants`).
    pub fn join_open(&self) {
        // Refuse to join under an unresolved identity. This is the gate that
        // stops ghost members: before it, a page-load whose bootstrap had not
        // answered joined as a minted `web-<random>` and left a dead member in
        // the roster on every visit.
        if !self.identity_resolved() {
            self.status.set("signing you in…".to_string());
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        let id = self.identity_id.get_untracked();
        let name = self.identity_name.get_untracked();
        spawn_local(async move {
            let body = JoinBody {
                id: &id,
                display_name: &name,
                kind: RoomParticipantKind::Human,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/participants", encode(&key));
            let result = match Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(req) => match req.send().await {
                    Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                        Ok(r) if r.ok => Ok(r.room),
                        Ok(r) => Err(format!(
                            "join failed: {}",
                            r.error.unwrap_or_else(|| "unknown error".into())
                        )),
                        Err(err) => Err(format!("join decode error: {err}")),
                    },
                    Err(err) => Err(format!("join post error: {err}")),
                },
                Err(err) => Err(format!("join encode error: {err}")),
            };
            if result.is_ok() {
                me.fetch_rooms();
            }
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(room) => {
                    me.open_room.set(room);
                    me.status.set("joined".into());
                    me.refresh_open_transcript(&key, generation_id, me.resume_seq.get_untracked());
                }
                Err(error) => me.status.set(error),
            }
        });
    }

    /// Leave the open room (`DELETE .../participants/{id}`).
    pub fn leave_open(&self) {
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        let id = self.identity_id.get_untracked();
        spawn_local(async move {
            let del_url = format!(
                "{base}/v1/rooms/persistent/{}/participants/{}",
                encode(&key),
                encode(&id)
            );
            let result = match Request::delete(&del_url).send().await {
                Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                    Ok(r) if r.ok => Ok(r.room),
                    Ok(r) => Err(format!(
                        "leave failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => Err(format!("leave decode error: {err}")),
                },
                Err(err) => Err(format!("leave error: {err}")),
            };
            if result.is_ok() {
                me.fetch_rooms();
            }
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(room) => {
                    me.open_room.set(room);
                    me.status.set("left".into());
                    me.refresh_open_transcript(&key, generation_id, me.resume_seq.get_untracked());
                }
                Err(error) => me.status.set(error),
            }
        });
    }

    /// Remove any participant from the open room
    /// (`DELETE .../participants/{participant_id}`) — [`Self::leave_open`]
    /// aimed at another roster row: same wire call and response handling, but
    /// the status names who went, because "left" on removing someone else
    /// would read as the remover having left.
    pub fn remove_participant(&self, participant_id: String) {
        if participant_id.is_empty() {
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        spawn_local(async move {
            let del_url = format!(
                "{base}/v1/rooms/persistent/{}/participants/{}",
                encode(&key),
                encode(&participant_id)
            );
            let result = match Request::delete(&del_url).send().await {
                Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                    Ok(r) if r.ok => Ok(r.room),
                    Ok(r) => Err(format!(
                        "remove failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => Err(format!("remove decode error: {err}")),
                },
                Err(err) => Err(format!("remove error: {err}")),
            };
            if result.is_ok() {
                me.fetch_rooms();
            }
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(room) => {
                    me.open_room.set(room);
                    me.status.set(format!("removed '{participant_id}'"));
                    me.refresh_open_transcript(&key, generation_id, me.resume_seq.get_untracked());
                }
                Err(error) => me.status.set(error),
            }
        });
    }

    /// Remove a member from the open federated room
    /// (`DELETE .../members/{member_id}`). Unlike the participants DELETE,
    /// a 200 carries the refreshed [`RoomAccessProjection`] itself — the
    /// daemon re-reads bedrock's roster before answering, so the member is
    /// already gone from it — and failures carry `{"ok":false,"error":code}`
    /// (see [`decode_remove_member_response`]). Authorization is bedrock's
    /// owner-or-self policy answered per attempt: the projection has no
    /// "this is you" flag, so every row offers the control and a refusal is
    /// a status line, never a revocation.
    pub fn remove_member(&self, member_id: String, display_name: String) {
        if member_id.is_empty() {
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        spawn_local(async move {
            let del_url = format!(
                "{base}/v1/rooms/persistent/{}/members/{}",
                encode(&key),
                encode(&member_id)
            );
            let result = match Request::delete(&del_url).send().await {
                Ok(resp) => {
                    let http_ok = resp.ok();
                    let http_status = resp.status();
                    match resp.text().await {
                        Ok(body) => decode_remove_member_response(http_ok, http_status, &body),
                        Err(err) => Err(format!("remove decode error: {err}")),
                    }
                }
                Err(err) => Err(format!("remove error: {err}")),
            };
            if result.is_ok() {
                me.fetch_rooms();
            }
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(access) => {
                    apply_access_projection(&me.access, access);
                    me.status.set(format!("removed '{display_name}'"));
                }
                Err(error) => me.status.set(error),
            }
        });
    }

    /// Post a message to the open room (`POST .../messages`). `@id` mentions in
    /// the body drive the daemon's trigger-policy auto-convene.
    pub fn post_message(&self, body: String, thread_parent_seq: Option<u64>) {
        if !composer_writes_allowed(
            self.access.get_untracked().as_ref(),
            self.closed.get_untracked(),
        ) {
            return;
        }
        // A message authored under an unresolved identity is either refused by
        // the daemon (author_not_in_roster) or lands attributed to nobody.
        if !self.identity_resolved() {
            self.status.set("signing you in…".to_string());
            return;
        }
        let body = body.trim().to_string();
        if body.is_empty() {
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        let id = self.identity_id.get_untracked();
        spawn_local(async move {
            let payload = PostMessageBody {
                author_id: &id,
                author_kind: RoomParticipantKind::Human,
                body: &body,
                thread_parent_seq,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/messages", encode(&key));
            let result = match Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&payload)
            {
                Ok(req) => match req.send().await {
                    Ok(resp) if resp.ok() => Ok(()),
                    Ok(resp) => Err(format!(
                        "message failed: {}",
                        resp.text().await.unwrap_or_default()
                    )),
                    Err(err) => Err(format!("message post error: {err}")),
                },
                Err(err) => Err(format!("message encode error: {err}")),
            };
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(()) => {
                    me.refresh_open_transcript(&key, generation_id, me.resume_seq.get_untracked())
                }
                Err(error) => me.status.set(error),
            }
        });
    }

    /// Retry a failed outbox item (`POST …/outbox/retry`).
    ///
    /// Refuses a closed room, and unlike the composer's refusal this one is
    /// not belt-and-braces over a route that would have said no anyway: the
    /// daemon's `retry_failed_outbox` checks that the room EXISTS, not that it
    /// is open, so a press against the frozen audit view answers 202 and
    /// requeues a federated send out of a transcript the pane calls finished.
    /// The button is not painted there either; this holds if it is reached
    /// some other way.
    #[allow(dead_code)]
    pub fn retry_outbox(&self, client_event_id: String) {
        if self.closed.get_untracked() {
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        spawn_local(async move {
            let payload = RetryOutboxBody {
                client_event_id: &client_event_id,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/outbox/retry", encode(&key));
            let result = match Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&payload)
            {
                Ok(req) => match req.send().await {
                    Ok(resp) if resp.status() == 202 => {
                        match resp.json::<RetryOutboxSuccess>().await {
                            Ok(r) if r.ok => Ok(r.access),
                            Ok(_) => Err("retry response invalid".into()),
                            Err(err) => Err(format!("retry decode error: {err}")),
                        }
                    }
                    Ok(resp) => {
                        let http_status = resp.status();
                        match resp.json::<RetryOutboxErrorResponse>().await {
                            Ok(r) => {
                                let detail = match (r.code, r.error) {
                                    (Some(code), Some(error)) => format!("{code}: {error}"),
                                    (Some(code), None) => code,
                                    (None, Some(error)) => error,
                                    (None, None) => format!("HTTP {http_status}"),
                                };
                                Err(format!("retry failed: {detail}"))
                            }
                            Err(err) => Err(format!("retry failed: HTTP {http_status} ({err})")),
                        }
                    }
                    Err(err) => Err(format!("retry post error: {err}")),
                },
                Err(err) => Err(format!("retry encode error: {err}")),
            };
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(access) => {
                    apply_access_projection(&me.access, access);
                    me.status.set("retry queued".into());
                }
                Err(error) => me.status.set(error),
            }
        });
    }

    /// Re-read the open room's transcript from `after_seq` and append what is
    /// new — the fallback for the day the tail is down. On a live connection the
    /// tail already carries the rows the four calling mutations write.
    ///
    /// `after_seq` is the CALLER's, the same rule [`Self::start_live_tail`]
    /// states. It used to be re-derived here from the rows on screen, which left
    /// the module holding two contradictory answers to where a resume comes
    /// from; [`Rooms::resume_seq`] is now the only one.
    ///
    /// And it PAGES. `/transcript` answers at most 200 rows, so the single
    /// request this made kept the first page of anything larger and never asked
    /// again — the daemon had been naming the rest in `next_seq`/`has_more`
    /// since OCEAN-249 and nothing here decoded them. The walk stops at
    /// [`MAX_TRANSCRIPT_CATCHUP_PAGES`]; hitting that cap is not a gap, because
    /// the live tail is a separate connection holding its own position and keeps
    /// delivering — it is this fallback declining to become a full-log read.
    fn refresh_open_transcript(&self, key: &str, generation_id: u64, after_seq: Option<u64>) {
        let base = self.base();
        let me = *self;
        let key = key.to_string();
        spawn_local(async move {
            let endpoint = format!("{base}/v1/rooms/persistent/{}/transcript", encode(&key));
            let mut cursor = after_seq;
            let mut pages_read = 0usize;
            loop {
                // Re-checked before EVERY page, not once at entry: each page is
                // an await, and a room switched during one must not have the
                // response that lands after it appended under the new room.
                if !me.room_is_current(generation_id, &key) {
                    return;
                }
                let Ok(response) = Request::get(&url_with_after_seq(&endpoint, cursor))
                    .send()
                    .await
                else {
                    return;
                };
                let Ok(page) = response.json::<TranscriptResponse>().await else {
                    return;
                };
                if !page.ok || !me.room_is_current(generation_id, &key) {
                    return;
                }
                let covered = last_transcript_seq(&page.transcript);
                if let Some(highest) = covered {
                    me.transcript
                        .update(|transcript| append_transcript_page(transcript, page.transcript));
                    me.resume_seq
                        .update(|seq| *seq = advanced_resume_seq(*seq, highest));
                }
                pages_read += 1;
                let Some(next) =
                    transcript_catchup_cursor(pages_read, page.has_more, page.next_seq, covered)
                else {
                    return;
                };
                cursor = Some(next);
            }
        });
    }

    /// Walk BACKWARD from the hydration page, prepending older rows.
    ///
    /// Anchoring the first paint at the tail is what makes this necessary.
    /// Before it, a 1500-row room painted all of it — the oldest 1000 from the
    /// head plus a 500-row forward catch-up — and the cost was opening on
    /// message #1. Asking for the newest page instead fixes what the operator
    /// sees and, on its own, would strand everything before it: `/transcript` is
    /// forward-only by contract, so nothing else in this module can reach a row
    /// older than the first one painted.
    ///
    /// So this mirrors [`Rooms::refresh_open_transcript`] in the other
    /// direction, on the same page cap and the same page size: 1000 + 5×200, the
    /// same row budget as before, anchored at the end an operator opens a room
    /// to read. It runs once per open rather than on every mutation, which is
    /// why it can afford to run at all.
    ///
    /// Beyond that budget a long room's older history is not on screen, where
    /// before it arrived eventually — a deliberate trade that cost two things.
    ///
    /// The first is closed. Rows older than the window used to be absent with
    /// nothing on screen saying so, because the last page's `has_more` and
    /// `prev_seq` were dropped at the instant the walk returned, leaving the
    /// oldest row painted reading as the first message in the room. They are
    /// parked in [`Rooms::older_cursor`] now, and
    /// [`Rooms::load_older_transcript_page`] replays one page from there per
    /// press. History past the budget is a press away rather than gone, and a
    /// room whose walk provably reached the start of the log parks `None` —
    /// which [`older_history_state`] now reads as `ReachedBeginning` rather than
    /// as nothing, so the top of the transcript says which of the two edges it
    /// is instead of going quiet either way.
    ///
    /// The second is closed too, and it was the harder one: a row INSIDE the
    /// window could render nowhere at all. `rooms_workspace` built the main list
    /// from `partition_thread_messages(&transcript, 0)`, whose `roots` keep only
    /// rows carrying no `thread_parent_seq`; a reply whose ROOT fell outside the
    /// window was dropped from that list, and `thread_root_for` could not find
    /// the missing root either, so no thread pane opened on it. A reply at seq
    /// 2500 to a root at seq 800 was invisible with nothing implying it existed.
    /// The unbounded catch-up this replaced could not leave that standing — the
    /// root arrived eventually — so it arrived with the tail anchor.
    ///
    /// Pressing "load older" was never the answer to it: the walk goes back a
    /// page at a time and cannot jump to one named root. The answer is
    /// `main_transcript_rows`, which keeps such a reply in the MAIN list at its
    /// own position in time, under a note saying its root is not loaded. The
    /// press then stops being the fix and becomes what it always was — the way
    /// to bring the root in, at which point the reply rejoins its thread and
    /// leaves that list on its own.
    ///
    /// So a very long LIVE room trades unbounded eventual history for a correct
    /// first paint plus a way back, and a very long SOFT-CLOSED room comes out
    /// strictly ahead: it opens no tail at all, so its newest rows were never
    /// merely late, they were unreachable.
    ///
    /// Never touches [`Rooms::resume_seq`]. That is the FORWARD position the
    /// live tail resumes from, older rows say nothing about it, and moving it
    /// backward would make the tail re-read what is already painted.
    fn backfill_open_transcript(&self, key: &str, generation_id: u64, before_seq: u64) {
        let base = self.base();
        let me = *self;
        let key = key.to_string();
        spawn_local(async move {
            let mut cursor = before_seq;
            let mut pages_read = 0usize;
            loop {
                // Re-checked before EVERY page for the same reason the forward
                // walk re-checks: each page is an await, and a room switched
                // during one must not have the response prepended under the
                // room the operator switched to.
                if !me.room_is_current(generation_id, &key) {
                    return;
                }
                let url =
                    room_snapshot_tail_url(&base, &key, cursor, BACKFILL_TRANSCRIPT_PAGE_LIMIT);
                // A request that never answered leaves the page it was reading
                // exactly where it was, so the cursor is parked rather than
                // dropped: a dropped one ends the room's history at whatever a
                // flaky network happened to deliver, and says so to nobody.
                let Ok(response) = Request::get(&url).send().await else {
                    me.park_older_cursor(generation_id, &key, Some(cursor));
                    return;
                };
                let Ok(page) = response.json::<RoomSnapshotResponse>().await else {
                    me.park_older_cursor(generation_id, &key, Some(cursor));
                    return;
                };
                if !me.room_is_current(generation_id, &key) {
                    return;
                }
                if !page.ok {
                    me.older_cursor.set(Some(cursor));
                    return;
                }
                let reached_back_to = first_transcript_seq(&page.transcript);
                me.transcript
                    .update(|transcript| prepend_transcript_page(transcript, page.transcript));
                pages_read += 1;
                let Some(next) = transcript_backfill_cursor(
                    pages_read,
                    page.has_more,
                    page.prev_seq,
                    reached_back_to,
                ) else {
                    // The page cap is where this stops on a long room, and the
                    // cursor it stops holding is the only route left to the rows
                    // behind it. Parked unconditionally: the same call answers
                    // `None` when the daemon said the log ran out, which is the
                    // room that must NOT grow an affordance — and, since this
                    // page ANSWERED, the room that may say so out loud.
                    me.settle_older_cursor(transcript_older_cursor(
                        page.has_more,
                        page.prev_seq,
                        reached_back_to,
                    ));
                    return;
                };
                cursor = next;
            }
        });
    }

    /// Park where an on-demand older read should resume, if the room this walk
    /// belongs to is still the open one. Guarded because a walk's failure lands
    /// after an await like everything else here, and writing a retired room's
    /// cursor would offer the operator older history belonging to a room they
    /// have already left.
    fn park_older_cursor(&self, generation_id: u64, key: &str, cursor: Option<u64>) {
        if self.room_is_current(generation_id, key) {
            self.older_cursor.set(cursor);
        }
    }

    /// Publish what a COMPLETED backward page answered: where an on-demand read
    /// resumes, and — inseparably — that a read has answered at all.
    ///
    /// One method rather than two `set` calls at each site, because the two
    /// facts are one fact and splitting them is the failure this whole area
    /// keeps producing. A cursor written without the flag leaves a room that
    /// provably reached the start of its log rendering the "still loading"
    /// silence forever, which is #190's symptom with a fresh coat on it; the
    /// flag written without the cursor claims a beginning that a press could
    /// still walk past. Neither is reachable from here.
    ///
    /// Deliberately NOT used by [`Rooms::park_older_cursor`], which exists for
    /// the opposite case: a request that never answered parks the page it was
    /// reading and settles nothing, because a flaky network is not evidence
    /// about the shape of the log.
    fn settle_older_cursor(&self, cursor: Option<u64>) {
        self.older_cursor.set(cursor);
        self.older_settled.set(true);
    }

    /// What the workspace's older-history affordance renders — see
    /// [`OlderHistory`] and [`older_history_state`].
    ///
    /// Reactive on BOTH halves: the hydration walk publishes them several
    /// page-loads after the first paint, so a view reading either untracked
    /// would have asked before the answer existed.
    pub(crate) fn older_history(&self) -> OlderHistory {
        older_history_state(self.older_cursor.get(), self.older_settled.get())
    }

    /// Whether the operator's older-history press is still in flight, so the
    /// affordance can say so and refuse a second one.
    pub(crate) fn older_transcript_in_flight(&self) -> bool {
        self.older_in_flight.get()
    }

    /// Fetch ONE page older than the parked cursor and prepend it.
    ///
    /// The request is the hydration walk's own — same route, same page size,
    /// same `room_is_current` re-check before anything is written — and the only
    /// difference is what ends it. The walk stops at
    /// [`MAX_TRANSCRIPT_CATCHUP_PAGES`] because it runs unasked on every open;
    /// this runs once per press, so the cursor it leaves behind is
    /// [`transcript_older_cursor`]'s answer and the operator decides whether to
    /// ask again.
    ///
    /// A failed read leaves the cursor untouched on purpose: the affordance
    /// stays on screen and the press can simply be repeated. Clearing it would
    /// turn one dropped request into permanently unreachable history.
    pub(crate) fn load_older_transcript_page(&self) {
        if self.older_in_flight.get_untracked() {
            return;
        }
        let Some(cursor) = self.older_cursor.get_untracked() else {
            return;
        };
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let generation_id = self.generation_snapshot();
        let base = self.base();
        let me = *self;
        self.older_in_flight.set(true);
        spawn_local(async move {
            let url = room_snapshot_tail_url(&base, &key, cursor, BACKFILL_TRANSCRIPT_PAGE_LIMIT);
            let page = match Request::get(&url).send().await {
                Ok(response) => response.json::<RoomSnapshotResponse>().await.ok(),
                Err(_) => None,
            };
            // Nothing below this line may write into a room the operator has
            // left — `reset_room_state` has already cleared both signals, and
            // lowering the flag here would lower the NEXT room's.
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            if let Some(page) = page.filter(|page| page.ok) {
                let reached_back_to = first_transcript_seq(&page.transcript);
                me.transcript
                    .update(|transcript| prepend_transcript_page(transcript, page.transcript));
                me.settle_older_cursor(transcript_older_cursor(
                    page.has_more,
                    page.prev_seq,
                    reached_back_to,
                ));
            }
            me.older_in_flight.set(false);
        });
    }

    /// Start the live tail for `key` at `generation_id`: room-scoped SSE
    /// (`GET /v1/rooms/persistent/{key}/events`) with `?after_seq=` resume for
    /// newly constructed browser connections (TASK-10/TASK-11). Replaces the
    /// 2.5s poll workaround. `resume_seq` is the hydration's cursor — the caller
    /// owns it because only the caller knows whether the rows on screen are the
    /// whole log or one page of it — and it seeds [`Rooms::resume_seq`], the
    /// room's single resume point. The tail advances that signal as it ingests
    /// and re-reads it on every reconnect, so rows a catch-up read pulled in
    /// between are not replayed here, and rows this tail already painted are not
    /// re-read there.
    fn start_live_tail(&self, key: String, generation_id: u64, resume_seq: Option<u64>) {
        let me = *self;
        let base = self.base();
        let tail_state = self.tail_state;
        self.resume_seq.set(resume_seq);

        spawn_local(async move {
            let events_url = format!("{base}/v1/rooms/persistent/{}/events", encode(&key));
            let mut resume_seq = me.resume_seq.get_untracked();
            let mut reconnecting = false;

            loop {
                if !me.room_is_current(generation_id, &key) {
                    break;
                }
                tail_state.set(if reconnecting {
                    TailState::Reconnecting
                } else {
                    TailState::Replaying
                });

                let url = url_with_after_seq(&events_url, resume_seq);
                let mut es = match EventSource::new(&url) {
                    Ok(es) => es,
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let message_sub = match es.subscribe("room_message") {
                    Ok(s) => s
                        .map(|event| event.map(|msg| ("room_message", msg)))
                        .boxed_local(),
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let access_sub = match es.subscribe("room_access") {
                    Ok(s) => s
                        .map(|event| event.map(|msg| ("room_access", msg)))
                        .boxed_local(),
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let read_cursor_sub = match es.subscribe("room_read_cursor") {
                    Ok(s) => s
                        .map(|event| event.map(|msg| ("room_read_cursor", msg)))
                        .boxed_local(),
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                // Only write connection state if this room+generation is still current.
                if me.room_is_current(generation_id, &key) {
                    tail_state.set(match es.state() {
                        gloo_net::eventsource::State::Open => TailState::Live,
                        gloo_net::eventsource::State::Connecting if reconnecting => {
                            TailState::Reconnecting
                        }
                        gloo_net::eventsource::State::Connecting => TailState::Replaying,
                        gloo_net::eventsource::State::Closed => TailState::Reconnecting,
                    });
                }
                let mut stream = futures_util::stream::select(
                    futures_util::stream::select(message_sub, access_sub),
                    read_cursor_sub,
                );
                // Race stream.next() against a 2 s timeout so room close/switch
                // can cancel a stalled connection (blame: gloo EventSource errors
                // are suppressed during CONNECTING, so Reconnecting never fires
                // without an explicit timeout-pump — codex TASK-11 review).
                loop {
                    if !me.room_is_current(generation_id, &key) {
                        break;
                    }
                    let next = stream.next();
                    let timeout = gloo_timers::future::TimeoutFuture::new(2_000);
                    let msg = match futures_util::future::select(Box::pin(next), Box::pin(timeout))
                        .await
                    {
                        Either::Left((Some(msg), _)) => msg,
                        Either::Left((None, _)) => break, // stream ended
                        Either::Right(_) => {
                            // Timeout fired — poll the native connection to
                            // distinguish a quiet stream from a dead one.
                            // gloo wraps web_sys::EventSource; State mirrors
                            // the underlying readyState constants.
                            // Only write tail_state if this room+gen is still current.
                            if me.room_is_current(generation_id, &key) {
                                match es.state() {
                                    gloo_net::eventsource::State::Open => {
                                        tail_state.set(TailState::Live);
                                    }
                                    gloo_net::eventsource::State::Connecting => {
                                        tail_state.set(TailState::Reconnecting);
                                    }
                                    gloo_net::eventsource::State::Closed => {
                                        tail_state.set(TailState::Reconnecting);
                                        break; // fall through to outer reconnect loop
                                    }
                                }
                            } else {
                                break;
                            }
                            continue;
                        }
                    };
                    let Ok((name, msg)) = msg else { continue };
                    let Some(data) = msg.1.data().as_string() else {
                        continue;
                    };
                    let Some(frame) = decode_room_tail_frame(name, &data, &key) else {
                        continue;
                    };
                    let Some(frame) = accept_room_tail_frame(
                        frame,
                        generation_id,
                        me.generation.get_untracked(),
                        &key,
                        me.open_key.get_untracked().as_deref(),
                    ) else {
                        break;
                    };
                    tail_state.set(TailState::Live);
                    match frame {
                        RoomTailFrame::Access(access) => {
                            apply_access_projection(&me.access, access.clone());
                            update_open_summary_from_open_room(
                                &me.read_summaries,
                                Some(&key),
                                &me.transcript.get_untracked(),
                                Some(&access),
                                me.open_read_cursor.get_untracked().as_ref(),
                            );
                        }
                        RoomTailFrame::ReadCursor(cursor) => {
                            // Mirrored SSE cursors merge monotonically with the
                            // cursor already held for this room+generation, so a
                            // lagging frame cannot lower the durable read.
                            let merged = merge_read_cursor_projection(
                                me.open_read_cursor.get_untracked().as_ref(),
                                cursor,
                            );
                            me.open_read_cursor.set(Some(merged.clone()));
                            update_open_summary_from_open_room(
                                &me.read_summaries,
                                Some(&key),
                                &me.transcript.get_untracked(),
                                me.access.get_untracked().as_ref(),
                                Some(&merged),
                            );
                        }
                        RoomTailFrame::Message(entry) => {
                            me.resume_seq
                                .update(|seq| *seq = advanced_resume_seq(*seq, entry.seq));
                            let is_roster_change = matches!(
                                entry.kind,
                                RoomMessageKind::ParticipantJoined
                                    | RoomMessageKind::ParticipantLeft
                            );
                            me.transcript.update(|t| {
                                if t.iter().any(|m| m.seq == entry.seq) {
                                    return;
                                }
                                t.push(entry);
                            });
                            update_open_summary_from_open_room(
                                &me.read_summaries,
                                Some(&key),
                                &me.transcript.get_untracked(),
                                me.access.get_untracked().as_ref(),
                                me.open_read_cursor.get_untracked().as_ref(),
                            );
                            // Refresh the room record (roster) on join/leave frames
                            // so other clients see an accurate participant list.
                            if is_roster_change {
                                let base = base.clone();
                                let key = key.clone();
                                let open_room = me.open_room;
                                spawn_local(async move {
                                    if !me.room_is_current(generation_id, &key) {
                                        return;
                                    }
                                    if let Ok(resp) = Request::get(&format!(
                                        "{base}/v1/rooms/persistent/{}",
                                        encode(&key)
                                    ))
                                    .send()
                                    .await
                                    {
                                        if let Ok(r) = resp.json::<RoomMutateResponse>().await {
                                            if r.ok {
                                                if !me.room_is_current(generation_id, &key) {
                                                    return;
                                                }
                                                if let Some(room) = r.room {
                                                    open_room.set(Some(room));
                                                }
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                resume_seq = me.resume_seq.get_untracked();
                reconnecting = true;
                if !me.room_is_current(generation_id, &key) {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }

    pub fn mark_open_read_if_current(&self, candidate_read_seq: u64) {
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let current_summary = self
            .read_summaries
            .get_untracked()
            .get(&key)
            .copied()
            .unwrap_or(RoomReadSummary {
                latest_seq: None,
                read_seq: None,
            });
        let durable_read_seq = self
            .open_read_cursor
            .get_untracked()
            .as_ref()
            .and_then(current_durable_read_seq);
        let applied_read_seq = applied_open_read_seq(current_summary.read_seq, durable_read_seq);
        if candidate_read_seq <= applied_read_seq {
            return;
        }
        if self
            .read_cursor_in_flight
            .get_untracked()
            .is_some_and(|seq| seq >= candidate_read_seq)
        {
            return;
        }
        if self
            .last_sent_read_cursor
            .get_untracked()
            .is_some_and(|seq| seq >= candidate_read_seq)
        {
            return;
        }

        let base = self.base();
        let me = *self;
        let generation_id = self.generation.get_untracked();
        self.read_cursor_in_flight.set(Some(candidate_read_seq));
        self.last_sent_read_cursor.set(Some(candidate_read_seq));
        spawn_local(async move {
            let patch_url = format!("{base}/v1/rooms/persistent/{}/read-cursor", encode(&key));
            let body = ReadCursorPatchBody {
                read_seq: candidate_read_seq,
            };
            let result = match Request::patch(&patch_url)
                .header("content-type", "application/json")
                .json(&body)
            {
                Ok(req) => match req.send().await {
                    Ok(resp) if resp.ok() => match resp.json::<ReadCursorPatchEnvelope>().await {
                        Ok(envelope) if envelope.ok => {
                            parse_patch_read_cursor_response(&key, envelope.cursor)
                        }
                        Ok(_) => Err("read cursor failed: unknown error".into()),
                        Err(err) => Err(format!("read cursor decode error: {err}")),
                    },
                    Ok(resp) => match resp.json::<RoomErrorResponse>().await {
                        Ok(error) => Err(format!(
                            "read cursor failed: {}",
                            error
                                .error
                                .unwrap_or_else(|| format!("HTTP {}", resp.status()))
                        )),
                        Err(err) => Err(format!(
                            "read cursor failed: HTTP {} ({err})",
                            resp.status()
                        )),
                    },
                    Err(err) => Err(format!("read cursor patch error: {err}")),
                },
                Err(err) => Err(format!("read cursor encode error: {err}")),
            };
            if !me.room_is_current(generation_id, &key) {
                return;
            }
            match result {
                Ok(cursor) => {
                    let merged = merge_read_cursor_projection(
                        me.open_read_cursor.get_untracked().as_ref(),
                        cursor,
                    );
                    me.open_read_cursor.set(Some(merged));
                    update_open_summary_from_open_room(
                        &me.read_summaries,
                        Some(&key),
                        &me.transcript.get_untracked(),
                        me.access.get_untracked().as_ref(),
                        me.open_read_cursor.get_untracked().as_ref(),
                    );
                    me.read_cursor_in_flight.set(None);
                }
                Err(_) => {
                    if me.read_cursor_in_flight.get_untracked() == Some(candidate_read_seq) {
                        me.read_cursor_in_flight.set(None);
                    }
                    if me.last_sent_read_cursor.get_untracked() == Some(candidate_read_seq) {
                        me.last_sent_read_cursor.set(None);
                    }
                }
            }
        });
    }
}

// ---- Helpers ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum RoomTailFrame {
    Message(RoomMessage),
    Access(RoomAccessProjection),
    ReadCursor(RoomReadCursorProjection),
}

fn decode_room_tail_frame(
    name: &str,
    data: &str,
    expected_room_key: &str,
) -> Option<RoomTailFrame> {
    match name {
        "room_message" => serde_json::from_str(data).ok().map(RoomTailFrame::Message),
        "room_access" => serde_json::from_str(data).ok().map(RoomTailFrame::Access),
        // Cursor frames carry a durable read position, so room identity is
        // validated by construction here: the expected key is required and the
        // frame is dropped unless the wire `room_id` matches it exactly.
        "room_read_cursor" => serde_json::from_str::<RoomReadCursorBody>(data)
            .ok()
            .and_then(|body| {
                parse_room_read_cursor_projection(
                    expected_room_key,
                    ReadCursorProjectionTarget::MirroredUpstream,
                    body,
                )
                .ok()
            })
            .map(RoomTailFrame::ReadCursor),
        _ => None,
    }
}

/// Admit a decoded SSE frame only while its captured room generation and key
/// still own the open room. This is the single production boundary before any
/// frame-driven tail state, access, cursor, or transcript mutation.
fn accept_room_tail_frame(
    frame: RoomTailFrame,
    expected_generation: u64,
    current_generation: u64,
    expected_key: &str,
    current_key: Option<&str>,
) -> Option<RoomTailFrame> {
    room_request_is_current(
        expected_generation,
        current_generation,
        expected_key,
        current_key,
    )
    .then_some(frame)
}

fn read_summaries_from_wire(
    read_states: &[RoomReadStateWire],
) -> Result<HashMap<String, RoomReadSummary>, String> {
    let mut summaries = HashMap::with_capacity(read_states.len());
    for state in read_states {
        let latest_seq = parse_optional_decimal_u64(state.latest_seq.as_deref())?;
        let read_seq = parse_optional_decimal_u64(state.read_seq.as_deref())?;
        if summaries
            .insert(
                state.room_id.clone(),
                RoomReadSummary {
                    latest_seq,
                    read_seq,
                },
            )
            .is_some()
        {
            return Err(format!("duplicate read state for room '{}'", state.room_id));
        }
    }
    Ok(summaries)
}

fn parse_optional_decimal_u64(raw: Option<&str>) -> Result<Option<u64>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("empty decimal read state".into());
    }
    trimmed
        .parse::<u64>()
        .map(Some)
        .map_err(|_| format!("invalid decimal read state '{trimmed}'"))
}

fn parse_room_read_cursor_projection(
    expected_room_key: &str,
    target: ReadCursorProjectionTarget,
    body: RoomReadCursorBody,
) -> Result<RoomReadCursorProjection, String> {
    let room_id = body.room_id.trim();
    if room_id.is_empty() {
        return Err("read cursor decode error: empty room_id".into());
    }
    if room_id != expected_room_key {
        return Err(format!(
            "read cursor decode error: wrong room_id '{room_id}' for '{expected_room_key}'"
        ));
    }
    let read_seq = parse_optional_decimal_u64(body.read_seq.as_deref())?;
    Ok(match target {
        ReadCursorProjectionTarget::Local => RoomReadCursorProjection {
            read_seq,
            mirrored_upstream_read_seq: None,
        },
        ReadCursorProjectionTarget::MirroredUpstream => RoomReadCursorProjection {
            read_seq: None,
            mirrored_upstream_read_seq: read_seq,
        },
    })
}

fn parse_patch_read_cursor_response(
    expected_room_key: &str,
    response: RoomReadCursorBody,
) -> Result<RoomReadCursorProjection, String> {
    parse_room_read_cursor_projection(
        expected_room_key,
        ReadCursorProjectionTarget::Local,
        response,
    )
}

/// Fold a newly observed cursor projection into the one already held for the
/// open room. Both the local (PATCH-confirmed) and mirrored (SSE) positions
/// advance monotonically, so a lagging mirrored frame can neither lower the
/// durable read, drop a locally confirmed read, nor resurrect unread — while a
/// later, higher mirrored frame still corrects the durable read upward.
fn merge_read_cursor_projection(
    current: Option<&RoomReadCursorProjection>,
    incoming: RoomReadCursorProjection,
) -> RoomReadCursorProjection {
    let Some(current) = current else {
        return incoming;
    };
    RoomReadCursorProjection {
        read_seq: max_optional_u64(current.read_seq, incoming.read_seq),
        mirrored_upstream_read_seq: max_optional_u64(
            current.mirrored_upstream_read_seq,
            incoming.mirrored_upstream_read_seq,
        ),
    }
}

/// The durable read position is the furthest confirmed read across both the
/// local PATCH projection and the mirrored upstream projection.
fn current_durable_read_seq(cursor: &RoomReadCursorProjection) -> Option<u64> {
    max_optional_u64(cursor.read_seq, cursor.mirrored_upstream_read_seq)
}

/// The read position already applied for the open room: the furthest of the
/// summary's confirmed read and the durable cursor projection. Folding with a
/// monotonic max (rather than preferring the summary when present) keeps a
/// lagging summary from re-sending a PATCH the durable cursor already covers.
fn applied_open_read_seq(summary_read_seq: Option<u64>, durable_read_seq: Option<u64>) -> u64 {
    max_optional_u64(summary_read_seq, durable_read_seq).unwrap_or(0)
}

fn latest_summary_seq_for_open_room(
    transcript: &[RoomMessage],
    access: Option<&RoomAccessProjection>,
) -> Option<u64> {
    match access.map(|projection| projection.state) {
        Some(RoomAccessState::Live) => {
            access.and_then(|projection| projection.last_confirmed_global_sequence)
        }
        _ => transcript.last().map(|message| message.seq),
    }
}

fn merged_room_read_summary(
    current: Option<&RoomReadSummary>,
    incoming: Option<&RoomReadSummary>,
) -> Option<RoomReadSummary> {
    match (current, incoming) {
        (None, None) => None,
        (Some(current), None) => Some(*current),
        (None, Some(incoming)) => Some(*incoming),
        (Some(current), Some(incoming)) => Some(RoomReadSummary {
            latest_seq: max_optional_u64(current.latest_seq, incoming.latest_seq),
            read_seq: max_optional_u64(current.read_seq, incoming.read_seq),
        }),
    }
}

fn max_optional_u64(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

fn merge_room_read_summaries(
    current: &HashMap<String, RoomReadSummary>,
    rooms: &[Room],
    incoming: &HashMap<String, RoomReadSummary>,
) -> HashMap<String, RoomReadSummary> {
    let mut merged = HashMap::with_capacity(rooms.len());
    for room in rooms {
        let room_id = room.id.clone();
        if let Some(summary) =
            merged_room_read_summary(current.get(&room_id), incoming.get(&room_id))
        {
            merged.insert(room_id, summary);
        }
    }
    merged
}

fn update_open_summary_from_open_room(
    summaries: &RwSignal<HashMap<String, RoomReadSummary>>,
    open_key: Option<&str>,
    transcript: &[RoomMessage],
    access: Option<&RoomAccessProjection>,
    cursor: Option<&RoomReadCursorProjection>,
) {
    let Some(open_key) = open_key else {
        return;
    };
    let latest_seq = latest_summary_seq_for_open_room(transcript, access);
    let existing = summaries.get_untracked().get(open_key).copied();
    let read_seq = max_optional_u64(
        cursor.and_then(current_durable_read_seq),
        existing.and_then(|summary| summary.read_seq),
    );
    summaries.update(|map| {
        map.insert(
            open_key.to_string(),
            RoomReadSummary {
                latest_seq: max_optional_u64(
                    existing.and_then(|summary| summary.latest_seq),
                    latest_seq,
                ),
                read_seq,
            },
        );
    });
}

pub(crate) fn room_has_durable_unread(summary: Option<&RoomReadSummary>) -> bool {
    let Some(summary) = summary else {
        return false;
    };
    match (summary.latest_seq, summary.read_seq) {
        (Some(latest), Some(read)) => latest > read,
        (Some(_), None) => true,
        _ => false,
    }
}

fn apply_access_projection(
    signal: &RwSignal<Option<RoomAccessProjection>>,
    next: RoomAccessProjection,
) -> bool {
    let mut current = signal.get_untracked();
    let changed = replace_access_projection(&mut current, next);
    if changed {
        signal.set(current);
    }
    changed
}

fn replace_access_projection(
    current: &mut Option<RoomAccessProjection>,
    next: RoomAccessProjection,
) -> bool {
    if current.as_ref() == Some(&next) {
        return false;
    }
    *current = Some(next);
    true
}

/// Decode the members DELETE response. This route does NOT answer the
/// [`RoomMutateResponse`] envelope the participants DELETE uses: a 200 body
/// is the refreshed [`RoomAccessProjection`] directly, an error body is
/// `{"ok":false,"error":code}` — so the split has to be on HTTP status, not
/// on an `ok` field.
fn decode_remove_member_response(
    http_ok: bool,
    http_status: u16,
    body: &str,
) -> Result<RoomAccessProjection, String> {
    if http_ok {
        serde_json::from_str::<RoomAccessProjection>(body)
            .map_err(|err| format!("remove decode error: {err}"))
    } else {
        let code = serde_json::from_str::<RoomErrorResponse>(body)
            .ok()
            .and_then(|r| r.error);
        Err(remove_member_failure_status(http_status, code.as_deref()))
    }
}

/// Status line for a refused member remove. `federation_forbidden` here is
/// bedrock's owner-or-self policy answering "not yours to remove" — the
/// credential and binding are untouched and a retry is admitted, so the copy
/// must read as a refusal of this one attempt, never as revoked access.
fn remove_member_failure_status(http_status: u16, code: Option<&str>) -> String {
    match code {
        Some("federation_forbidden") => {
            "remove refused: only the room owner or the member's registrant can remove them".into()
        }
        Some(code) => format!("remove failed: {code}"),
        None => format!("remove failed: HTTP {http_status}"),
    }
}

fn last_transcript_seq(transcript: &[RoomMessage]) -> Option<u64> {
    transcript.last().map(|message| message.seq)
}

fn first_transcript_seq(transcript: &[RoomMessage]) -> Option<u64> {
    transcript.first().map(|message| message.seq)
}

/// Where the live tail resumes after a `/snapshot` hydration. The page's own
/// `last_seq` is authoritative — it is the daemon naming what it just served —
/// and the painted rows are the fallback for a response that omits it. No
/// shipped daemon does, so that arm is a decode safety net and not a
/// compatibility window. Both are `None` for an empty room, which resumes from
/// the start of the log exactly as an unhydrated open always has.
fn snapshot_resume_seq(snapshot_last_seq: Option<u64>, transcript: &[RoomMessage]) -> Option<u64> {
    snapshot_last_seq.or_else(|| last_transcript_seq(transcript))
}

/// The hydration read for `key`: the room's NEWEST page, at the store's full
/// window. Both query arguments carry weight. `limit` is spelled out because the
/// route's own default is 200, a fifth of what the unpaged read painted; without
/// `before_seq` the route pages FORWARD from the start of the log, which for a
/// room past the window means opening on its oldest thousand and reaching the
/// rows an operator actually came for only by dragging every row between them
/// through the live tail — and, for a soft-closed room, never, because that room
/// opens no tail at all.
fn room_snapshot_url(base: &str, key: &str) -> String {
    room_snapshot_tail_url(base, key, HYDRATION_TAIL_CURSOR, HYDRATION_TRANSCRIPT_LIMIT)
}

/// One backward page of `/snapshot`: the newest `limit` rows strictly older than
/// `before_seq`, ascending. `after_seq` is never sent beside it — the daemon
/// answers the pair with a typed 400 (`conflicting_transcript_cursors`) rather
/// than picking one, so the two cursors cannot share a builder.
fn room_snapshot_tail_url(base: &str, key: &str, before_seq: u64, limit: usize) -> String {
    format!(
        "{base}/v1/rooms/persistent/{}/snapshot?before_seq={before_seq}&limit={limit}",
        encode(key)
    )
}

/// Append one `/transcript` page to the painted rows, keeping only entries past
/// the last one painted — stricter than the live tail's own ingest, which
/// dedupes on `seq` equality across the whole vector and pushes in arrival
/// order. A page whose rows are all already painted appends nothing: an
/// overlapping read is a duplicate delivery, not a gap.
fn append_transcript_page(transcript: &mut Vec<RoomMessage>, page: Vec<RoomMessage>) {
    for message in page {
        if transcript
            .last()
            .map(|last| last.seq < message.seq)
            .unwrap_or(true)
        {
            transcript.push(message);
        }
    }
}

/// Prepend one backward `/snapshot` page to the painted rows, keeping only
/// entries older than the oldest one painted — the mirror of
/// [`append_transcript_page`], and strict for the same reason: an overlapping
/// read is a duplicate delivery, not a gap. The bound is read ONCE rather than
/// per row because every kept row lands in front of it, so re-reading it after
/// each insert would compare against a row this very page just added.
///
/// The page arrives ascending and entirely below the paint, so splicing the kept
/// rows in at the front preserves the whole vector's `seq` order — which the
/// renderer, the resume point and [`first_transcript_seq`] all depend on.
fn prepend_transcript_page(transcript: &mut Vec<RoomMessage>, page: Vec<RoomMessage>) {
    let oldest_painted = first_transcript_seq(transcript);
    let older: Vec<RoomMessage> = page
        .into_iter()
        .filter(|message| {
            oldest_painted
                .map(|oldest| message.seq < oldest)
                .unwrap_or(true)
        })
        .collect();
    transcript.splice(0..0, older);
}

/// Where the backward hydration walk STARTS, or `None` when the first paint
/// already holds the whole room.
///
/// The page's own length is the signal, not its `has_more`. A backward
/// `/snapshot` page is the last `limit` rows that qualify, so a page shorter
/// than the window provably reached the start of the log and a full one is the
/// only shape that can have anything behind it. Reading the length rather than
/// the flag keeps the hydration decode exactly as wide as it has always been —
/// `closed` reaching the tail gate is mutation-tested on the literal shape of
/// that arm — and costs one request in one case: a room whose length is an exact
/// multiple of the window asks once and is told there is nothing older.
///
/// The cursor is the oldest row painted, which is where the walk's own rule
/// would have put it. `before_seq` is exclusive, so the first page back cannot
/// re-serve it.
fn hydration_backfill_start(transcript: &[RoomMessage], window: usize) -> Option<u64> {
    if transcript.len() < window {
        return None;
    }
    first_transcript_seq(transcript)
}

/// Where the backward hydration walk continues, or `None` when it must stop. The
/// mirror of [`transcript_catchup_cursor`], with the same two stop conditions —
/// the daemon said the log ran out (`has_more` false, meaning no OLDER rows on a
/// backward page), or the walk has taken [`MAX_TRANSCRIPT_CATCHUP_PAGES`] pages.
///
/// `prev_seq` is the daemon's own `before_seq` for the next page;
/// `page_reached_back_to` — the LOWEST `seq` the page just served — is the
/// fallback for a `has_more` page naming no cursor, and is also what forbids an
/// endless walk over one: `before_seq` is exclusive, so every row on a page sits
/// strictly below the cursor that produced it and the replayed cursor strictly
/// decreases. It bottoms out at `before_seq = 0`, which the daemon answers as a
/// terminal empty page because nothing precedes the first message.
fn transcript_backfill_cursor(
    pages_read: usize,
    has_more: bool,
    prev_seq: Option<u64>,
    page_reached_back_to: Option<u64>,
) -> Option<u64> {
    if pages_read >= MAX_TRANSCRIPT_CATCHUP_PAGES {
        return None;
    }
    transcript_older_cursor(has_more, prev_seq, page_reached_back_to)
}

/// Where an ON-DEMAND older read resumes, or `None` once a page has provably
/// reached the start of the log.
///
/// The same cursor [`transcript_backfill_cursor`] answers, minus the page cap —
/// which is the whole difference between the two callers. The hydration walk is
/// bounded because it runs unasked on every open; a press is one page the
/// operator asked for, so only the daemon's own `has_more` may end it. Deriving
/// the bounded answer FROM this one is what stops the two drifting: a walk that
/// stopped because the log ran out and a press that finds nothing older are then
/// the same fact rather than two functions that happen to agree today.
fn transcript_older_cursor(
    has_more: bool,
    prev_seq: Option<u64>,
    page_reached_back_to: Option<u64>,
) -> Option<u64> {
    if !has_more {
        return None;
    }
    prev_seq.or(page_reached_back_to)
}

/// What the transcript can say about the history ABOVE its oldest painted row.
///
/// Three states because the honest answer has three, and the affordance that
/// renders it must not collapse them: a room still hydrating has not earned the
/// claim that its oldest row is the first message in it, and a room that
/// provably reached the start of its log has earned exactly that and should say
/// so rather than silently stop offering a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OlderHistory {
    /// No room open, or no backward read has answered yet. Claim nothing.
    Unknown,
    /// Older rows exist and nothing on screen reaches them.
    Available,
    /// A page provably reached the start of the log: the oldest painted row IS
    /// the first message in this room.
    ReachedBeginning,
}

/// Combine the parked cursor with whether a backward read has answered.
///
/// A cursor dominates: it is only ever written from a page the daemon served or
/// from a request that dropped mid-flight, and either way rows behind the paint
/// are known to exist. `None` is the ambiguous half, and the flag is the whole
/// question — the same `None` means "reached the start of the log" after a page
/// has answered and "we have not asked yet" before one has. Rendering the
/// second as the first is how a room three seconds into hydration would tell an
/// operator its first painted row is the beginning of the room.
pub(crate) fn older_history_state(cursor: Option<u64>, settled: bool) -> OlderHistory {
    match (cursor, settled) {
        (Some(_), _) => OlderHistory::Available,
        (None, true) => OlderHistory::ReachedBeginning,
        (None, false) => OlderHistory::Unknown,
    }
}

/// Where a catch-up read continues after the page it just ingested, or `None`
/// when it must stop — and it stops for two different reasons. The daemon said
/// the log ran out (`has_more` false), or this read has already taken
/// [`MAX_TRANSCRIPT_CATCHUP_PAGES`] pages, which is the bound that keeps a walk
/// running on every join/leave/removal/send off a 12 000-row room.
///
/// `next_seq` is the daemon's own `after_seq` for the next page;
/// `page_covered_through` — the highest `seq` the page just served — is the
/// fallback for a `has_more` page naming no cursor, and is also what forbids an
/// endless walk over one, being strictly past the `after_seq` that produced the
/// page. It mirrors [`snapshot_resume_seq`] with one difference worth naming:
/// the fallback reads the page just served, never the rows on screen.
fn transcript_catchup_cursor(
    pages_read: usize,
    has_more: bool,
    next_seq: Option<u64>,
    page_covered_through: Option<u64>,
) -> Option<u64> {
    if pages_read >= MAX_TRANSCRIPT_CATCHUP_PAGES || !has_more {
        return None;
    }
    next_seq.or(page_covered_through)
}

/// Monotonic advance of the open room's resume point. A page overlapping what is
/// already painted, or a frame replayed after a reconnect, must never lower it:
/// the resume means "how far this client has ingested", and lowering it would
/// re-read rows already on screen on the next catch-up.
fn advanced_resume_seq(held: Option<u64>, ingested: u64) -> Option<u64> {
    match held {
        Some(held) if held >= ingested => Some(held),
        _ => Some(ingested),
    }
}

fn url_with_after_seq(endpoint: &str, after_seq: Option<u64>) -> String {
    match after_seq {
        Some(sequence) => format!("{endpoint}?after_seq={sequence}"),
        None => endpoint.to_string(),
    }
}

fn list_request_is_current(expected_ticket: u64, current_ticket: u64) -> bool {
    expected_ticket == current_ticket
}

fn should_skip_rooms_fetch(mode: RoomsFetchMode, rooms_loading: bool) -> bool {
    matches!(mode, RoomsFetchMode::Silent) && rooms_loading
}

fn finish_rooms_fetch(
    rooms_loaded: &RwSignal<bool>,
    rooms_loading: &RwSignal<bool>,
    mode: RoomsFetchMode,
    is_current: bool,
) {
    if !is_current {
        return;
    }
    rooms_loaded.set(true);
    if matches!(mode, RoomsFetchMode::Interactive) {
        rooms_loading.set(false);
    }
}

fn joined_open_for(
    access: Option<&RoomAccessProjection>,
    room: Option<&Room>,
    identity_id: &str,
) -> bool {
    let Some(access) = access else {
        return false;
    };
    if access.state == RoomAccessState::Local {
        return room.is_some_and(|room| {
            room.participants
                .iter()
                .any(|participant| participant.id == identity_id)
        });
    }
    access.members.iter().any(|member| {
        member.member_id == identity_id || member.owner_member_id.as_deref() == Some(identity_id)
    })
}

/// Which placeholder the rooms list should render, given whether the first
/// fetch has resolved and how many rooms came back. Splitting `Loading` from
/// `Empty` stops the panel from flashing "No rooms yet" while the initial
/// request is still in flight — an empty list only *means* empty once loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RoomsListState {
    Loading,
    Empty,
    Populated,
}

#[allow(dead_code)]
pub(crate) fn rooms_list_state(loaded: bool, room_count: usize) -> RoomsListState {
    if room_count > 0 {
        // Rooms are present — always render them, even if a refetch is in
        // flight. Only the *empty* list is ambiguous between loading and empty.
        RoomsListState::Populated
    } else if loaded {
        RoomsListState::Empty
    } else {
        RoomsListState::Loading
    }
}

/// Whether the open room's transcript should show its "No messages yet" empty
/// state. Only once the live tail is actually connected (`Live`) AND the
/// transcript is empty: during the initial `Replaying` catch-up (or a
/// `Reconnecting` gap) an empty transcript means "still loading", not
/// "genuinely empty", so the stage must not flash the empty copy on room open
/// before history arrives (same bug class as [`rooms_list_state`]).
#[allow(dead_code)]
fn show_transcript_empty(tail: TailState, transcript_empty: bool) -> bool {
    transcript_empty && matches!(tail, TailState::Live)
}

/// Whether the add-agent picker should show its "No agents" hint: only once
/// `/v1/agents` has resolved AND the list is empty. During the initial fetch an
/// empty list means "still loading", not "no agents" (same flash class as the
/// rooms-list and transcript empties).
#[allow(dead_code)]
fn show_no_agents(agents_loaded: bool, agent_count: usize) -> bool {
    agents_loaded && agent_count == 0
}

/// Pure predicate: is `expected_generation`/`expected_key` still the current
/// room admission? `pub(crate)` so sibling modules (`rooms_workspace.rs`) can
/// unit-test the exact rejection logic behind [`Rooms::room_is_current`]
/// without needing a live `Rooms` handle (which requires a browser runtime).
pub(crate) fn room_request_is_current(
    expected_generation: u64,
    current_generation: u64,
    expected_key: &str,
    current_key: Option<&str>,
) -> bool {
    expected_generation == current_generation && current_key == Some(expected_key)
}

/// Derive a url/key-safe slug from a room name (lowercase alnum + `-`).
fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Percent-encode a path segment (room keys can contain `-`/`_`/alnum already,
/// but a defensive encode keeps an unexpected char from breaking the URL).
/// Pure Rust so tests run on native targets.
///
/// `pub(crate)` so `agents.rs` addresses `/v1/agents/{name}` through the same
/// encoder rather than growing a second, subtly different one.
pub(crate) fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

/// A compact "last activity" label from an ISO-8601 timestamp — just the
/// date+time portion, trimmed. Empty input → empty string.
#[allow(dead_code)]
fn short_time(ts: &str) -> String {
    if ts.is_empty() {
        return String::new();
    }
    // "2026-06-05T12:34:56.789Z" → "2026-06-05 12:34"
    let trimmed = ts.split('.').next().unwrap_or(ts).replace('T', " ");
    trimmed.chars().take(16).collect()
}

/// Build the per-room LiveKit token path, percent-encoding the room key.
/// Pure utility used by `daemon.rs` bootstrap; rooms G1 does not call it.
pub(crate) fn livekit_token_path_for_room(key: &str) -> String {
    format!("/v1/rooms/{}/livekit-token", encode(key))
}

/// Whether this surface may act as a room participant yet.
///
/// Both halves are load-bearing. `authoritative` is the daemon having
/// answered; a non-empty `id` alone is not, because the id warm-starts from
/// localStorage and so is non-empty for any browser that has loaded rooms
/// before — including one holding a `web-<random>` ghost or the previous
/// tenant's id.
fn identity_may_act(authoritative: bool, id: &str) -> bool {
    authoritative && !id.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acting_requires_the_daemon_to_have_answered_not_just_a_stored_id() {
        // An id without current authenticated authority must never make the
        // gate pass, even if a future warm-start source grows one again.
        assert!(!identity_may_act(false, "web-18c72b1e64dc22de"));
        assert!(!identity_may_act(false, "smaths"));
        // Nothing to act as, however the flag stands.
        assert!(!identity_may_act(false, ""));
        assert!(!identity_may_act(true, ""));
        // Resolved, and someone to be.
        assert!(identity_may_act(true, "smaths"));
    }

    #[test]
    fn proxy_identity_uses_login_and_normalizes_display_name() {
        assert_eq!(
            RoomIdentity::from_proxy_config("  ocean  ", "  Ocean Operator  ").id,
            "ocean"
        );
        assert_eq!(
            RoomIdentity::from_proxy_config("ocean", "").display_name,
            "ocean"
        );
    }

    #[test]
    fn successful_single_operator_config_uses_stable_identity() {
        let identity = RoomIdentity::from_proxy_config("", "");
        assert_eq!(identity.id, SINGLE_OPERATOR_ROOM_ID);
        assert_eq!(identity.display_name, "Operator");
    }

    #[test]
    fn no_agents_hint_waits_for_agents_fetch() {
        // Empty list before the fetch resolves = still loading, hide the hint.
        assert!(!show_no_agents(false, 0));
        // Once resolved and still empty = genuinely no agents.
        assert!(show_no_agents(true, 0));
        // Any agents present never shows the empty hint.
        assert!(!show_no_agents(true, 3));
        assert!(!show_no_agents(false, 3));
    }

    #[test]
    fn transcript_empty_state_waits_for_live_tail() {
        // During the initial catch-up the tail is Replaying and the transcript
        // is empty — that is "still loading", NOT "no messages", so the empty
        // copy must stay hidden.
        assert!(!show_transcript_empty(TailState::Replaying, true));
        // A reconnect gap with an empty transcript is likewise ambiguous.
        assert!(!show_transcript_empty(TailState::Reconnecting, true));
        // Only once connected (Live) does an empty transcript genuinely mean
        // "no messages yet".
        assert!(show_transcript_empty(TailState::Live, true));
        // A non-empty transcript never shows the empty state, in any tail state.
        assert!(!show_transcript_empty(TailState::Live, false));
        assert!(!show_transcript_empty(TailState::Replaying, false));
    }

    #[test]
    fn rooms_list_state_distinguishes_loading_from_genuinely_empty() {
        // Before the first fetch resolves, an empty list means "still loading",
        // NOT "no rooms" — the panel must not assert emptiness prematurely.
        assert_eq!(rooms_list_state(false, 0), RoomsListState::Loading);
        // A non-empty list mid-load still renders rooms (they arrived).
        assert_eq!(rooms_list_state(false, 3), RoomsListState::Populated);
        // Once loaded, an empty list is genuinely empty.
        assert_eq!(rooms_list_state(true, 0), RoomsListState::Empty);
        // Once loaded with rooms, populated.
        assert_eq!(rooms_list_state(true, 2), RoomsListState::Populated);
    }

    fn access_projection(state: RoomAccessState) -> RoomAccessProjection {
        RoomAccessProjection {
            state,
            last_confirmed_global_sequence: None,
            members: Vec::new(),
            self_member_id: None,
            outbox: Vec::new(),
        }
    }

    fn message(seq: u64) -> RoomMessage {
        RoomMessage {
            seq,
            author_id: "member-1".into(),
            author_kind: RoomParticipantKind::Human,
            kind: RoomMessageKind::Message,
            body: format!("message {seq}"),
            created_at: "2026-07-16T22:00:00Z".into(),
            federated: None,
            thread_parent_seq: None,
            attachment_id: None,
        }
    }

    #[test]
    fn post_message_wire_omits_none_thread_parent_and_includes_some() {
        let root = serde_json::to_value(PostMessageBody {
            author_id: "human-1",
            author_kind: RoomParticipantKind::Human,
            body: "root body",
            thread_parent_seq: None,
        })
        .expect("root post message body should serialize");
        assert_eq!(
            root,
            serde_json::json!({
                "author_id": "human-1",
                "author_kind": "human",
                "body": "root body"
            })
        );

        let reply = serde_json::to_value(PostMessageBody {
            author_id: "human-1",
            author_kind: RoomParticipantKind::Human,
            body: "reply body",
            thread_parent_seq: Some(7),
        })
        .expect("reply post message body should serialize");
        assert_eq!(
            reply,
            serde_json::json!({
                "author_id": "human-1",
                "author_kind": "human",
                "body": "reply body",
                "thread_parent_seq": 7
            })
        );
    }

    fn local_room() -> Room {
        Room {
            id: "room-1".into(),
            name: "Room One".into(),
            participants: vec![
                RoomParticipant {
                    id: "local-agent".into(),
                    kind: RoomParticipantKind::Agent,
                    display_name: "Local Agent".into(),
                },
                RoomParticipant {
                    id: "local-human".into(),
                    kind: RoomParticipantKind::Human,
                    display_name: "Local Human".into(),
                },
            ],
            created_at: String::new(),
            updated_at: String::new(),
            trigger_policy: None,
            workspace_root: None,
        }
    }

    #[test]
    fn slugify_lowercases_and_dashes() {
        assert_eq!(slugify("Map Fix"), "map-fix");
        assert_eq!(slugify("  Ocean   Surface!! "), "ocean-surface");
        assert_eq!(slugify("already-ok_123"), "already-ok-123");
    }

    #[test]
    fn slugify_strips_leading_trailing_separators() {
        assert_eq!(slugify("!!!hi!!!"), "hi");
        assert_eq!(slugify("---"), "");
        assert_eq!(slugify(""), "");
    }

    /// Every policy stored before `on_build_failure` existed decodes with the
    /// flag off — same compat guarantee the daemon's own struct makes.
    #[test]
    fn trigger_policy_without_on_build_failure_decodes_with_flag_off() {
        let policy: RoomTriggerPolicy = serde_json::from_value(serde_json::json!({
            "on_mention": true,
            "on_thread_reply": true
        }))
        .expect("legacy policy should decode");
        assert!(policy.on_mention);
        assert!(policy.on_thread_reply);
        assert!(!policy.on_build_failure);
        assert!(!policy.on_ci_failure);
        assert!(!policy.on_component_event);
        assert_eq!(policy.on_schedule, None);
    }

    /// Same compat guarantee one field later: a policy stored while
    /// `on_build_failure` was the newest flag decodes with `on_ci_failure`
    /// off, so no room silently gains a wake trigger it never opted into.
    #[test]
    fn trigger_policy_without_on_ci_failure_decodes_with_flag_off() {
        let policy: RoomTriggerPolicy = serde_json::from_value(serde_json::json!({
            "on_mention": true,
            "on_thread_reply": false,
            "on_component_event": false,
            "on_build_failure": true
        }))
        .expect("pre-CI policy should decode");
        assert!(policy.on_mention);
        assert!(policy.on_build_failure);
        assert!(!policy.on_ci_failure);
    }

    /// The PATCH body carries the COMPLETE policy under `trigger_policy`
    /// because the daemon replaces the stored policy wholesale. The daemon
    /// refuses dead trigger values by VALUE, not presence (`trigger_unwired`),
    /// so the always-serialized `on_component_event: false` is accepted and
    /// `on_schedule: None` stays omitted (skip_serializing_if), matching the
    /// daemon's "absent = unset" encoding — a normalized policy's body always
    /// passes the write gate. `on_ci_failure` rides along the same way: a
    /// daemon that has never heard of the key drops it (no route denies
    /// unknown fields), so the body stays valid on both sides of that pair.
    #[test]
    fn policy_patch_body_sends_the_complete_policy() {
        let policy = RoomTriggerPolicy {
            on_mention: true,
            on_thread_reply: false,
            on_component_event: false,
            on_build_failure: true,
            on_ci_failure: false,
            on_schedule: None,
        };
        let body = serde_json::to_value(RoomPolicyPatchBody {
            trigger_policy: &policy,
        })
        .expect("body should encode");
        assert_eq!(
            body,
            serde_json::json!({
                "trigger_policy": {
                    "on_mention": true,
                    "on_thread_reply": false,
                    "on_component_event": false,
                    "on_build_failure": true,
                    "on_ci_failure": false
                }
            })
        );
    }

    /// The create body carries `workspace_root` ONLY when the operator filled
    /// the field in. Absent is what the daemon reads as "unbound", so an
    /// always-present `"workspace_root": null` would say the same thing while
    /// looking like a chosen value — and, more importantly, the field being
    /// absent from this body for the whole life of the surface is the defect
    /// this test exists to keep fixed.
    #[test]
    fn create_body_sends_workspace_root_only_when_one_was_given() {
        let bound = serde_json::to_value(CreateRoomBody {
            key: "ocean-surface-map-fix",
            name: "Map fix",
            trigger_policy: None,
            workspace_root: Some("/dev/ocean-surface"),
        })
        .expect("body should encode");
        assert_eq!(
            bound,
            serde_json::json!({
                "key": "ocean-surface-map-fix",
                "name": "Map fix",
                "workspace_root": "/dev/ocean-surface"
            })
        );

        let unbound = serde_json::to_value(CreateRoomBody {
            key: "ocean-surface-map-fix",
            name: "Map fix",
            trigger_policy: None,
            workspace_root: None,
        })
        .expect("body should encode");
        assert_eq!(
            unbound,
            serde_json::json!({"key": "ocean-surface-map-fix", "name": "Map fix"}),
            "an absent binding must be an absent KEY, not an explicit null"
        );
    }

    /// The unbind body, by contrast, MUST carry an explicit null: the daemon
    /// leaves an absent field unchanged, so a skipped `None` here would make
    /// the unbind control a request that changes nothing and reports success.
    #[test]
    fn workspace_patch_body_sends_an_explicit_null_to_unbind() {
        assert_eq!(
            serde_json::to_value(RoomWorkspacePatchBody {
                workspace_root: Some("/dev/ocean-surface"),
            })
            .expect("body should encode"),
            serde_json::json!({"workspace_root": "/dev/ocean-surface"})
        );
        assert_eq!(
            serde_json::to_value(RoomWorkspacePatchBody {
                workspace_root: None
            })
            .expect("body should encode"),
            serde_json::json!({ "workspace_root": null }),
            "unbind is an explicit null; an omitted key means 'leave it alone'"
        );
        // And it carries the binding ALONE, so it can never clobber the stored
        // trigger policy the other control owns.
        let body = serde_json::to_value(RoomWorkspacePatchBody {
            workspace_root: None,
        })
        .expect("body should encode");
        assert_eq!(
            body.as_object().expect("object").len(),
            1,
            "the workspace PATCH must send one field only"
        );
    }

    /// A daemon that predates the field omits it, and an omitted binding must
    /// read as no binding rather than failing the whole decode — this panel
    /// would otherwise go blank against an older daemon.
    #[test]
    fn room_decodes_with_and_without_a_workspace_root() {
        let base = serde_json::json!({"id": "r1", "name": "Room One"});
        let unbound: Room = serde_json::from_value(base.clone()).expect("decodes without the key");
        assert_eq!(unbound.workspace_root, None);

        let mut with_root = base;
        with_root["workspace_root"] = serde_json::json!("/dev/ocean-os");
        let bound: Room = serde_json::from_value(with_root).expect("decodes with the key");
        assert_eq!(bound.workspace_root.as_deref(), Some("/dev/ocean-os"));
    }

    /// The predicate the unbound notice renders from. Whitespace counts as
    /// unbound: the daemon treats a blank value as no binding, so a room
    /// carrying one would otherwise render as bound while its agent turns all
    /// fail closed.
    #[test]
    fn room_is_unbound_reads_absent_and_blank_the_same_way() {
        let room = |root: Option<&str>| Room {
            id: "r1".into(),
            name: "Room One".into(),
            participants: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
            trigger_policy: None,
            workspace_root: root.map(str::to_string),
        };
        assert!(room_is_unbound(&room(None)));
        assert!(room_is_unbound(&room(Some(""))));
        assert!(room_is_unbound(&room(Some("   "))));
        assert!(!room_is_unbound(&room(Some("/dev/ocean-os"))));
    }

    /// The draft re-seeds on a room SWITCH and on nothing else. The seeding
    /// effect has to read `open_room`, so it re-runs whenever anything writes
    /// that signal — a trigger PATCH completing, a hydration refresh — and
    /// re-seeding on those wipes a path the operator is mid-way through
    /// typing.
    #[test]
    fn the_workspace_draft_reseeds_only_when_the_room_identity_changes() {
        // First open: nothing seeded yet, so seed.
        assert!(workspace_draft_should_reseed(None, Some("room-1")));
        // Same room, unrelated update — the draft is the operator's, not the
        // record's.
        assert!(!workspace_draft_should_reseed(
            Some("room-1"),
            Some("room-1")
        ));
        // Switched rooms: the previous room's path must not carry over.
        assert!(workspace_draft_should_reseed(
            Some("room-1"),
            Some("room-2")
        ));
        // Closed the room entirely.
        assert!(workspace_draft_should_reseed(Some("room-1"), None));
        // Nothing open, nothing seeded — no write, so no needless clobber.
        assert!(!workspace_draft_should_reseed(None, None));
    }

    /// An empty create field means "leave it unbound"; a filled one is sent
    /// trimmed, because a trailing space is never part of the path the
    /// operator meant and the daemon would refuse it.
    #[test]
    fn create_workspace_root_trims_and_treats_empty_as_unbound() {
        assert_eq!(create_workspace_root(""), None);
        assert_eq!(create_workspace_root("   "), None);
        assert_eq!(
            create_workspace_root("  /dev/ocean-os  "),
            Some("/dev/ocean-os".to_string())
        );
    }

    /// The daemon's frozen refusal code becomes the one typed status; anything
    /// else is carried verbatim rather than mislabelled as a bad path. Matched
    /// on the EXACT code, never a substring, so an unrelated message quoting it
    /// cannot be retagged.
    #[test]
    fn workspace_bind_status_reads_the_daemons_frozen_refusal_code() {
        assert_eq!(
            WorkspaceBindStatus::from_daemon_error("invalid_workspace_root"),
            WorkspaceBindStatus::InvalidPath
        );
        assert_eq!(
            WorkspaceBindStatus::from_daemon_error("  invalid_workspace_root  "),
            WorkspaceBindStatus::InvalidPath
        );
        assert_eq!(
            WorkspaceBindStatus::from_daemon_error("unknown room"),
            WorkspaceBindStatus::Failed("unknown room".to_string())
        );

        // The sentence names the daemon's host, and is NOT the compute lane's
        // `workspace_unavailable` wording in `room_repo.rs` — that one means
        // Bedrock is unreachable, which is a different condition entirely.
        let message = WorkspaceBindStatus::InvalidPath.message();
        assert!(message.contains("absolute path"), "{message}");
        assert!(message.contains("running the daemon"), "{message}");
        assert!(
            !message.contains("workspace_unavailable"),
            "the compute lane's refusal must not be reused here: {message}"
        );
    }

    #[test]
    fn short_time_trims_iso_to_minute() {
        assert_eq!(short_time("2026-06-05T12:34:56.789Z"), "2026-06-05 12:34");
        assert_eq!(short_time(""), "");
    }

    #[test]
    fn livekit_token_path_percent_encodes_room_key_as_one_path_segment() {
        assert_eq!(
            livekit_token_path_for_room("project/surface demo"),
            "/v1/rooms/project%2Fsurface%20demo/livekit-token"
        );
    }

    #[test]
    fn g1_room_message_without_federation_metadata_decodes_as_none() {
        let message: RoomMessage = serde_json::from_value(serde_json::json!({
            "seq": 1,
            "author_id": "local-human",
            "author_kind": "human",
            "kind": "message",
            "body": "hello",
            "created_at": "2026-07-16T22:00:00Z"
        }))
        .expect("G1 message should decode");

        assert_eq!(message.federated, None);
    }

    #[test]
    fn room_message_thread_parent_seq_decodes_and_defaults_to_none() {
        let reply: RoomMessage = serde_json::from_value(serde_json::json!({
            "seq": 2,
            "author_id": "local-human",
            "author_kind": "human",
            "kind": "message",
            "body": "reply",
            "created_at": "2026-07-16T22:01:00Z",
            "thread_parent_seq": 1
        }))
        .expect("reply should decode");
        assert_eq!(reply.thread_parent_seq, Some(1));

        let root: RoomMessage = serde_json::from_value(serde_json::json!({
            "seq": 1,
            "author_id": "local-human",
            "author_kind": "human",
            "kind": "message",
            "body": "root",
            "created_at": "2026-07-16T22:00:00Z"
        }))
        .expect("root should decode");
        assert_eq!(root.thread_parent_seq, None);
    }

    #[test]
    fn room_snapshot_requires_access_and_local_projection_is_exact() {
        let response: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "access": { "state": "local" }
        }))
        .expect("P1 room envelope should decode");
        assert_eq!(response.access, access_projection(RoomAccessState::Local));

        let missing = serde_json::from_value::<RoomSnapshotResponse>(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": []
        }));
        assert!(missing.is_err(), "access must remain required");

        let error: RoomErrorResponse = serde_json::from_value(serde_json::json!({
            "ok": false,
            "error": "no room with key 'missing'"
        }))
        .expect("non-success responses use the separate error envelope");
        assert_eq!(error.error.as_deref(), Some("no room with key 'missing'"));

        assert_eq!(
            serde_json::to_value(access_projection(RoomAccessState::Local)).unwrap(),
            serde_json::json!({ "state": "local" })
        );
    }

    #[test]
    fn federated_access_projection_uses_only_safe_exact_wire_fields() {
        let projection: RoomAccessProjection = serde_json::from_value(serde_json::json!({
            "state": "live",
            "last_confirmed_global_sequence": 44,
            "members": [{
                "member_id": "member-agent",
                "owner_member_id": "member-owner",
                "actor_type": "agent",
                "role_in_room": "member",
                "display_name": "Fable",
                "public_agent_descriptor": {
                    "display_name": "Fable",
                    "description": "reviewer",
                    "model_alias": "fable",
                    "skills_count": 2,
                    "subagent_names": ["research", "review"]
                },
                "joined_at": "2026-07-16T22:00:00Z",
                "derived_presence": "live",
                "local_binding_available": false
            }],
            "outbox": [{
                "client_event_id": "client-1",
                "source_id": "surface-web",
                "source_sequence": 7,
                "author_member_id": "member-owner",
                "event_type": "room_message",
                "payload": { "body": "hello" },
                "mention_member_ids": ["member-agent"],
                "state": "failed"
            }]
        }))
        .expect("full safe projection should decode");

        let wire = serde_json::to_value(&projection).unwrap();
        assert_eq!(
            wire["members"][0],
            serde_json::json!({
                "member_id": "member-agent",
                "owner_member_id": "member-owner",
                "actor_type": "agent",
                "role_in_room": "member",
                "display_name": "Fable",
                "public_agent_descriptor": {
                    "display_name": "Fable",
                    "description": "reviewer",
                    "model_alias": "fable",
                    "skills_count": 2,
                    "subagent_names": ["research", "review"]
                },
                "joined_at": "2026-07-16T22:00:00Z",
                "derived_presence": "live",
                "local_binding_available": false
            })
        );
        let member = wire["members"][0].as_object().unwrap();
        for secret in [
            "owner_principal_token_id",
            "registration_key",
            "bearer_token",
            "access_token",
        ] {
            assert!(!member.contains_key(secret));
        }
        assert_eq!(
            wire["outbox"][0],
            serde_json::json!({
                "client_event_id": "client-1",
                "source_id": "surface-web",
                "source_sequence": 7,
                "author_member_id": "member-owner",
                "event_type": "room_message",
                "payload": { "body": "hello" },
                "mention_member_ids": ["member-agent"],
                "state": "failed"
            })
        );
    }

    #[test]
    fn access_projection_self_member_id_serde_compat() {
        // Old-daemon payloads carry no `self_member_id` key → `None`; `None`
        // never serializes, so older daemons never see an unknown key back.
        let old: RoomAccessProjection =
            serde_json::from_value(serde_json::json!({ "state": "live" })).unwrap();
        assert_eq!(old.self_member_id, None);
        let none_json = serde_json::to_value(&old).unwrap();
        assert!(none_json.get("self_member_id").is_none());

        let mut projection = access_projection(RoomAccessState::Live);
        projection.self_member_id = Some("member-you".into());
        let json = serde_json::to_value(&projection).unwrap();
        assert_eq!(json["self_member_id"], "member-you");
        let roundtrip: RoomAccessProjection = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip, projection);
    }

    // ── members DELETE response decode ────────────────────────────────

    #[test]
    fn remove_member_success_body_is_the_projection_not_a_mutate_envelope() {
        // The daemon refreshed the roster before answering, so the 200 body
        // already shows the member gone — applying it IS the UI update.
        let body = serde_json::json!({
            "state": "live",
            "last_confirmed_global_sequence": 9
        })
        .to_string();
        let access = decode_remove_member_response(true, 200, &body)
            .expect("200 body decodes as RoomAccessProjection");
        assert_eq!(access.state, RoomAccessState::Live);
        assert!(access.members.is_empty());
    }

    #[test]
    fn remove_member_policy_403_reads_as_refusal_never_revocation() {
        let error = decode_remove_member_response(
            false,
            403,
            r#"{"ok":false,"error":"federation_forbidden"}"#,
        )
        .unwrap_err();
        assert!(error.starts_with("remove refused"), "{error}");
        assert!(!error.to_lowercase().contains("revok"), "{error}");
    }

    #[test]
    fn remove_member_other_failures_carry_their_code_or_http_status() {
        assert_eq!(
            decode_remove_member_response(
                false,
                503,
                r#"{"ok":false,"error":"federation_unavailable"}"#
            )
            .unwrap_err(),
            "remove failed: federation_unavailable"
        );
        // An undecodable error body still names the HTTP status.
        assert_eq!(
            decode_remove_member_response(false, 502, "upstream burp").unwrap_err(),
            "remove failed: HTTP 502"
        );
    }

    #[test]
    fn tail_frame_decoder_tags_access_and_messages_without_cursor_blending() {
        let access =
            serde_json::to_string(&access_projection(RoomAccessState::Recovering)).unwrap();
        let frame = decode_room_tail_frame("room_access", &access, "room-1").unwrap();
        assert_eq!(
            frame,
            RoomTailFrame::Access(access_projection(RoomAccessState::Recovering))
        );

        let frame = decode_room_tail_frame(
            "room_message",
            r#"{"seq":8,"author_id":"member-1","author_kind":"human","kind":"message","body":"hello","created_at":"2026-07-16T22:00:00Z"}"#,
            "room-1",
        )
        .unwrap();
        match frame {
            RoomTailFrame::Message(message) => assert_eq!(message.seq, 8),
            RoomTailFrame::Access(_) => panic!("message frame decoded as access"),
            RoomTailFrame::ReadCursor(_) => panic!("message frame decoded as read cursor"),
        }
        assert!(decode_room_tail_frame("unknown", "{}", "room-1").is_none());
    }

    #[test]
    fn access_projection_replacement_is_idempotent() {
        let live = access_projection(RoomAccessState::Live);
        let mut current = Some(live.clone());
        assert!(!replace_access_projection(&mut current, live));

        let recovering = access_projection(RoomAccessState::Recovering);
        assert!(replace_access_projection(&mut current, recovering.clone()));
        assert_eq!(current, Some(recovering));
    }

    /// Hydration addresses the cursor-bearing route at the store's full page,
    /// from the NEWEST end. Both query arguments are load-bearing and fail
    /// differently. Drop `limit` and the route's own 200-row default silently
    /// costs the first paint four fifths of itself; drop `before_seq` and the
    /// route pages forward from the start of the log instead, which is a full
    /// page of the WRONG rows — every other assertion in this module stays green
    /// through either.
    #[test]
    fn hydration_reads_snapshot_at_the_stores_full_page() {
        assert_eq!(
            room_snapshot_url("http://127.0.0.1:7777", "ocean-surface"),
            "http://127.0.0.1:7777/v1/rooms/persistent/ocean-surface/snapshot\
             ?before_seq=18446744073709551615&limit=1000"
        );
        // Room keys are free-form, so the segment is encoded and the query is
        // not part of what gets encoded.
        assert_eq!(
            room_snapshot_url("https://ocean.example", "call/2026-09-01 standup"),
            "https://ocean.example/v1/rooms/persistent/call%2F2026-09-01%20standup/snapshot\
             ?before_seq=18446744073709551615&limit=1000"
        );
        // The cursor is `u64::MAX` spelled out: a room cannot store a seq at or
        // above it, so the page is unconditionally the newest one. Pinned as a
        // literal because the daemon parses this as a number, and a value that
        // silently became a sentinel string or an i64 would still produce a URL.
        assert_eq!(HYDRATION_TAIL_CURSOR.to_string(), "18446744073709551615");
    }

    /// The backward walk addresses the same route one page at a time, and never
    /// beside `after_seq` — the daemon answers both cursors together with a
    /// typed 400 rather than choosing, so a builder that could emit the pair is
    /// a builder that can emit a request no room will ever answer.
    #[test]
    fn backfill_pages_the_snapshot_backward_at_the_forward_walks_page_size() {
        assert_eq!(
            room_snapshot_tail_url("http://127.0.0.1:7777", "ocean-surface", 800, 200),
            "http://127.0.0.1:7777/v1/rooms/persistent/ocean-surface/snapshot\
             ?before_seq=800&limit=200"
        );
        assert!(
            !room_snapshot_tail_url("http://127.0.0.1:7777", "room-1", 1, 1).contains("after_seq"),
            "after_seq beside before_seq is conflicting_transcript_cursors, not a page"
        );
        assert_eq!(
            BACKFILL_TRANSCRIPT_PAGE_LIMIT * MAX_TRANSCRIPT_CATCHUP_PAGES,
            HYDRATION_TRANSCRIPT_LIMIT,
            "the backward walk is budgeted at exactly one more hydration page, \
             the same bound the forward catch-up runs on"
        );
    }

    /// A `/snapshot` body decodes whole — the forward-only `next_seq` this
    /// envelope still ignores must not break the decode — and the tail resumes
    /// at the sequence the daemon named rather than one re-derived from the
    /// painted rows.
    ///
    /// The body below puts the two sources three rows apart, which the daemon
    /// cannot emit today (it derives `last_seq` from the page's own last row).
    /// That is the point: the rule only has teeth on the day they diverge — a
    /// filtered projection, a trimmed page — and on that day the daemon's number
    /// is the one that names what it actually served.
    /// `closed` is the ONLY thing in this envelope that tells the daemon's
    /// frozen audit view apart from a live room: both answer 200, both carry a
    /// transcript, and `access` describes the federation rail rather than the
    /// room's life — closing never touches the access row, so a frozen room
    /// projects what it always projected. Asserted in both directions on one
    /// fixture — either
    /// half alone stays green against a hardcoded constant, and `open_room`
    /// trusts this field to discriminate before it decides whether to open an
    /// `EventSource` at all.
    ///
    /// The absent case is the compatibility half and is not decoration: four
    /// other fixtures in this module build the envelope with no `closed` key,
    /// as does every daemon shipped before ocean-os#434, and the contract rules
    /// a missing key open. Without `#[serde(default)]` that is a decode error,
    /// which `open_room` reports as "room decode error" — every room on a
    /// pre-field daemon failing to open.
    #[test]
    fn snapshot_closed_is_absent_open_and_reads_both_ways() {
        let body = |closed: Option<bool>| {
            let mut v = serde_json::json!({
                "ok": true,
                "room": null,
                "transcript": [],
                "access": { "state": "local" }
            });
            if let Some(closed) = closed {
                v["closed"] = serde_json::json!(closed);
            }
            serde_json::from_value::<RoomSnapshotResponse>(v)
                .expect("snapshot envelope should decode")
        };

        assert!(
            !body(None).closed,
            "a daemon that predates `closed` says nothing, and nothing means \
             OPEN — reading absence as closed shuts the composer on every room \
             such a daemon serves",
        );
        assert!(
            !body(Some(false)).closed,
            "an explicit `false` is an open room and must stay writable",
        );
        assert!(
            body(Some(true)).closed,
            "`true` is the soft-closed audit view — the flag `open_room` gates \
             the live tail on and the composer refuses every send on",
        );
    }

    /// `agent_owners` decodes off `/snapshot` in ROSTER ORDER, and an envelope
    /// without it decodes to an empty list rather than failing.
    ///
    /// The absent case is the whole compatibility story, and it is not
    /// hypothetical in one direction only: a daemon predating ocean-os#437
    /// omits the key entirely, and a CURRENT daemon omits nothing but answers
    /// `[]` for every room that never recorded an ownership row — which is
    /// every room created before the feature. Both must read as "no ownership
    /// recorded". Without `#[serde(default)]` the first is a decode error,
    /// which `open_room` reports as "room decode error" — every room on such a
    /// daemon failing to open, over a field nothing in the open path needs.
    ///
    /// Order is asserted because the daemon promises it (`ORDER BY p.position`,
    /// the roster's own column) and the rail spends it: the rail walks
    /// `participants` and looks each row up, so a reordering here would be
    /// invisible in the rail and visible the day anything renders the list
    /// whole.
    #[test]
    fn snapshot_agent_owners_decode_in_roster_order_and_default_empty() {
        let with_owners: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "access": { "state": "local" },
            "agent_owners": [
                { "agent_id": "researcher", "owner_id": "alice", "owner_present": true },
                { "agent_id": "scribe", "owner_id": "bob", "owner_present": false },
            ],
        }))
        .expect("a snapshot carrying agent_owners should decode");

        assert_eq!(
            with_owners.agent_owners,
            vec![
                RoomAgentOwner {
                    agent_id: "researcher".into(),
                    owner_id: "alice".into(),
                    owner_present: true,
                },
                RoomAgentOwner {
                    agent_id: "scribe".into(),
                    owner_id: "bob".into(),
                    owner_present: false,
                },
            ],
            "both rows decode, in the roster order the daemon served them",
        );

        let without: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "access": { "state": "local" },
        }))
        .expect(
            "a snapshot with no agent_owners key must still decode — a daemon \
             predating ocean-os#437 omits it and the room must still open",
        );
        assert!(
            without.agent_owners.is_empty(),
            "an absent key is no recorded ownership, which is what the rail \
             renders as unclaimed",
        );

        let empty: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "access": { "state": "local" },
            "agent_owners": [],
        }))
        .expect("an explicit empty list is a room with no owned agents");
        assert!(empty.agent_owners.is_empty());
    }

    /// One `agent_owners` row survives a round trip through the wire shape the
    /// contract names, field for field, and `owner_present` is not lost to a
    /// rename or a type change. `owner_present` is the one field a reader
    /// cannot re-derive — it is the daemon's answer to whether the owning
    /// worker is still on the roster, and the whole reason ownership is
    /// reported as a row instead of filtered down to live claims.
    #[test]
    fn an_agent_owner_row_round_trips_field_for_field() {
        let row = RoomAgentOwner {
            agent_id: "researcher".into(),
            owner_id: "alice".into(),
            owner_present: true,
        };
        let wire = serde_json::to_value(&row).expect("row should serialize");
        assert_eq!(
            wire,
            serde_json::json!({
                "agent_id": "researcher",
                "owner_id": "alice",
                "owner_present": true,
            }),
            "the wire shape is the contract's, not serde's guess at it",
        );
        assert_eq!(
            serde_json::from_value::<RoomAgentOwner>(wire).expect("row should decode"),
            row,
        );

        let absent_flag: RoomAgentOwner = serde_json::from_value(serde_json::json!({
            "agent_id": "scribe",
            "owner_id": "bob",
        }))
        .expect("a row missing owner_present decodes rather than failing the page");
        assert!(
            !absent_flag.owner_present,
            "unstated presence is not a claim of presence: the rail must not \
             assert a worker is here on a field the room never answered",
        );
    }

    #[test]
    fn snapshot_hydration_resumes_from_the_page_cursor() {
        let response: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "participants": [],
            "transcript": [
                {
                    "seq": 996,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "second to last painted row",
                    "created_at": "2026-07-16T22:00:00Z"
                },
                {
                    "seq": 997,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "last painted row",
                    "created_at": "2026-07-16T22:00:01Z"
                }
            ],
            "last_seq": 1000,
            "next_seq": 1000,
            "has_more": true,
            "access": { "state": "local" }
        }))
        .expect("snapshot envelope should decode");
        assert_eq!(response.last_seq, Some(1000));
        assert_eq!(response.transcript.len(), 2);

        let resume = snapshot_resume_seq(response.last_seq, &response.transcript);
        assert_eq!(resume, Some(1000));
        assert_ne!(
            resume,
            last_transcript_seq(&response.transcript),
            "re-deriving the resume from the rows on screen is the behavior this \
             hydration exists to stop"
        );
        // What `start_live_tail` opens first, given what hydration handed it.
        assert_eq!(
            url_with_after_seq("/v1/rooms/persistent/room-1/events", resume),
            "/v1/rooms/persistent/room-1/events?after_seq=1000"
        );

        // An empty room has no cursor from either source, and `None` is what
        // replays it from the start of the log.
        let empty: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "last_seq": null,
            "next_seq": null,
            "has_more": false,
            "access": { "state": "local" }
        }))
        .expect("empty snapshot should decode");
        assert_eq!(snapshot_resume_seq(empty.last_seq, &empty.transcript), None);
        assert_eq!(
            url_with_after_seq(
                "/events",
                snapshot_resume_seq(empty.last_seq, &empty.transcript)
            ),
            "/events"
        );

        // A response that omits `last_seq` still resumes off the painted rows,
        // exactly where the tail always did. No shipped daemon takes this arm;
        // it is pinned so the fallback cannot rot into silence.
        assert_eq!(
            snapshot_resume_seq(None, &[message(3), message(9)]),
            Some(9)
        );
    }

    /// The tail-anchored hydration page, read as `open_room` reads it: the live
    /// tail resumes from the NEWEST row and the backward walk starts at the
    /// oldest, off one body, in opposite directions. Getting the two cursors
    /// crossed is the failure this pins — `last_seq` into `before_seq` re-reads
    /// the page forever, `prev_seq` into `after_seq` replays the whole log.
    #[test]
    fn tail_hydration_resumes_forward_and_backfills_from_opposite_ends() {
        let page: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "participants": [],
            "transcript": [
                {
                    "seq": 4001,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "oldest row on the tail page",
                    "created_at": "2026-07-16T22:00:00Z"
                },
                {
                    "seq": 5000,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "newest row in the room",
                    "created_at": "2026-07-16T22:00:01Z"
                }
            ],
            "last_seq": 5000,
            // Null on every backward page, by construction — the arm that
            // populates it is the forward one this crate never asks for.
            "next_seq": null,
            "prev_seq": 4001,
            "has_more": true,
            "access": { "state": "local" }
        }))
        .expect("a backward snapshot page should decode");
        assert_eq!(page.prev_seq, Some(4001));
        assert!(
            page.has_more,
            "on a backward page this means OLDER rows exist, not newer ones",
        );

        // Forward: the tail picks up past the newest row the page served, so a
        // room opened at its tail replays nothing.
        assert_eq!(
            url_with_after_seq(
                "/v1/rooms/persistent/room-1/events",
                snapshot_resume_seq(page.last_seq, &page.transcript)
            ),
            "/v1/rooms/persistent/room-1/events?after_seq=5000"
        );

        // Backward: the walk starts at the oldest row painted. The window here
        // is the page's own length, standing in for the 1000 `open_room` passes.
        let backfill_from = hydration_backfill_start(&page.transcript, 2);
        assert_eq!(backfill_from, Some(4001));
        assert_eq!(
            room_snapshot_tail_url(
                "",
                "room-1",
                backfill_from.expect("a full page has rows behind it"),
                BACKFILL_TRANSCRIPT_PAGE_LIMIT
            ),
            "/v1/rooms/persistent/room-1/snapshot?before_seq=4001&limit=200"
        );
        assert_ne!(
            backfill_from, page.last_seq,
            "seeding the walk from the NEWEST row would re-request the page it \
             just painted, forever"
        );
    }

    /// A room that fits in the first paint starts no walk, and one that fills it
    /// does. The short case is the one that must stay byte-identical to the
    /// head-anchored read: a room under the window painted its whole log before
    /// this slice and still does, at the cost of no extra request.
    #[test]
    fn a_room_inside_the_first_paint_backfills_nothing() {
        assert_eq!(
            hydration_backfill_start(&[], 1000),
            None,
            "an empty room has nothing to walk back through",
        );
        assert_eq!(
            hydration_backfill_start(&[message(0), message(1), message(2)], 1000),
            None,
            "a short page provably reached the start of the log — a backward \
             page is the LAST `limit` rows that qualify, so fewer than asked \
             for means no more qualify",
        );
        assert_eq!(
            hydration_backfill_start(&[message(7), message(8), message(9)], 3),
            Some(7),
            "a page filled to the window is the only shape that can have rows \
             behind it, and the oldest row painted is where they start",
        );
    }

    /// A pre-#436 daemon ignores `before_seq` and answers a FORWARD page: the
    /// oldest rows in the room, `prev_seq` absent, `has_more` meaning newer rows.
    /// It must decode, and the walk it seeds must terminate rather than spin.
    #[test]
    fn a_daemon_without_backward_paging_still_decodes_and_still_terminates() {
        let legacy: RoomSnapshotResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "last_seq": 1000,
            "next_seq": 1000,
            "has_more": true,
            "access": { "state": "local" }
        }))
        .expect("a pre-backward-paging daemon's body should still decode");
        assert_eq!(
            legacy.prev_seq, None,
            "the field is additive and such a daemon omits it",
        );

        // Such a daemon paints rows 0..window, so the walk seeds at row 0 and
        // asks `before_seq=0` — which is the daemon's own terminal empty page,
        // since nothing precedes the first message. One request, then stop.
        assert_eq!(
            transcript_backfill_cursor(1, false, None, None),
            None,
            "the empty page that request gets back ends the walk",
        );
        // And the fallback cannot keep a walk alive off a page naming no cursor
        // once the daemon says the direction is exhausted.
        assert_eq!(
            transcript_backfill_cursor(1, false, Some(400), Some(400)),
            None
        );
    }

    /// The backward walk: every request starts where the previous page's OLDEST
    /// row was, the cursor strictly decreases so a daemon that keeps saying
    /// "older rows exist" cannot spin it, and the page cap is what ends it.
    #[test]
    fn backfill_walks_older_and_is_bounded_by_the_same_page_cap() {
        let mut cursor = 4001u64;
        let mut pages_read = 0usize;
        let mut requested = Vec::new();
        loop {
            requested.push(room_snapshot_tail_url(
                "",
                "room-1",
                cursor,
                BACKFILL_TRANSCRIPT_PAGE_LIMIT,
            ));
            pages_read += 1;
            // A daemon with older rows to give, forever. Every page reaches back
            // 200 rows below the cursor that produced it, because `before_seq`
            // is exclusive.
            let reached_back_to = cursor - BACKFILL_TRANSCRIPT_PAGE_LIMIT as u64;
            let Some(next) =
                transcript_backfill_cursor(pages_read, true, Some(reached_back_to), None)
            else {
                break;
            };
            assert!(
                next < cursor,
                "a backward cursor that does not fall is a loop"
            );
            cursor = next;
        }
        assert_eq!(
            requested,
            vec![
                "/v1/rooms/persistent/room-1/snapshot?before_seq=4001&limit=200",
                "/v1/rooms/persistent/room-1/snapshot?before_seq=3801&limit=200",
                "/v1/rooms/persistent/room-1/snapshot?before_seq=3601&limit=200",
                "/v1/rooms/persistent/room-1/snapshot?before_seq=3401&limit=200",
                "/v1/rooms/persistent/room-1/snapshot?before_seq=3201&limit=200",
            ],
            "the walk is what keeps the rows before the tail page reachable at \
             all — `/transcript` is forward-only and cannot serve one of them"
        );
        assert_eq!(requested.len(), MAX_TRANSCRIPT_CATCHUP_PAGES);

        // And the page that stopped it is exactly where a press resumes. The
        // walk above ends holding `has_more: true` and a cursor 200 rows below
        // its last request; dropping that pair at this instant is what left the
        // oldest painted row reading as the first message in the room.
        assert_eq!(
            transcript_older_cursor(
                true,
                Some(cursor - BACKFILL_TRANSCRIPT_PAGE_LIMIT as u64),
                None
            ),
            Some(3001),
            "the cursor the page cap stops on is the only route left to the \
             rows behind it",
        );
    }

    /// The two cursors are one rule with one extra stop condition, and the
    /// difference is deliberate: the walk is capped because it runs unasked on
    /// every open, a press is one page the operator asked for. What neither may
    /// do is claim there is older history when the daemon said there is not.
    #[test]
    fn the_on_demand_cursor_is_the_walks_without_the_page_cap() {
        // `has_more` false is the start of the log, from either caller. This is
        // the room that must grow no affordance at all.
        assert_eq!(transcript_older_cursor(false, Some(400), Some(400)), None);
        assert_eq!(transcript_older_cursor(false, None, None), None);

        // With rows behind it, the daemon's own `prev_seq` wins and the page's
        // lowest row is the fallback for a page that names none — the same
        // precedence the walk uses, because it is the same call.
        assert_eq!(
            transcript_older_cursor(true, Some(3801), Some(3802)),
            Some(3801)
        );
        assert_eq!(transcript_older_cursor(true, None, Some(3802)), Some(3802));
        assert_eq!(
            transcript_older_cursor(true, None, None),
            None,
            "a `has_more` page naming no cursor and serving no rows leaves \
             nothing to replay; offering a press that cannot move is worse than \
             offering none",
        );

        // Past the cap the walk stops and the press does not. Below it they are
        // the same answer, which is what makes deriving one from the other the
        // point rather than a tidy-up.
        assert_eq!(
            transcript_backfill_cursor(MAX_TRANSCRIPT_CATCHUP_PAGES, true, Some(3001), None),
            None,
        );
        assert_eq!(
            transcript_older_cursor(true, Some(3001), None),
            Some(3001),
            "the press has no page budget to run out of",
        );
        for pages_read in 0..MAX_TRANSCRIPT_CATCHUP_PAGES {
            assert_eq!(
                transcript_backfill_cursor(pages_read, true, Some(3001), Some(3002)),
                transcript_older_cursor(true, Some(3001), Some(3002)),
            );
            assert_eq!(
                transcript_backfill_cursor(pages_read, false, Some(3001), Some(3002)),
                transcript_older_cursor(false, Some(3001), Some(3002)),
            );
        }
    }

    /// The affordance's three states, and the one that did not exist before
    /// this slice.
    ///
    /// `older_cursor` alone answers two questions with one `None`: "a page
    /// reached the start of the log" and "nothing has asked yet". #192 rendered
    /// both as no affordance at all, so a room that provably held its whole
    /// history looked exactly like a room three seconds into hydration — and
    /// the claim an operator actually wanted, that the oldest row on screen IS
    /// the first message in the room, was the one thing the transcript could
    /// not make.
    #[test]
    fn the_older_history_state_separates_reached_the_start_from_not_asked_yet() {
        assert_eq!(
            older_history_state(None, false),
            OlderHistory::Unknown,
            "before a backward read answers, the transcript knows nothing about \
             what is above it and must claim nothing",
        );
        assert_eq!(
            older_history_state(None, true),
            OlderHistory::ReachedBeginning,
            "a page that answered with no cursor reached the start of the log, \
             and that is the whole claim: this row IS the first message",
        );
        assert_eq!(
            older_history_state(Some(3801), true),
            OlderHistory::Available,
        );
        assert_eq!(
            older_history_state(Some(3801), false),
            OlderHistory::Available,
            "a cursor DOMINATES the flag. It is written from a page the daemon \
             served or from a request that dropped mid-flight, and either way \
             rows behind the paint are known to exist — a dropped request is \
             the case with no settled read behind it, and the press it offers \
             is exactly the retry that room needs",
        );
    }

    /// The states are exclusive by construction, on every input pair. The shape
    /// this replaced — two independent booleans a view reads in sequence —
    /// admits `available && reached_beginning`, which renders a "load older"
    /// button above "beginning of the room".
    #[test]
    fn no_pair_of_inputs_produces_two_states_at_once() {
        for cursor in [None, Some(0u64), Some(3801)] {
            for settled in [false, true] {
                let state = older_history_state(cursor, settled);
                assert_eq!(
                    state == OlderHistory::Available,
                    cursor.is_some(),
                    "Available is exactly a parked cursor, for {cursor:?}/{settled}",
                );
                assert_eq!(
                    state == OlderHistory::ReachedBeginning,
                    cursor.is_none() && settled,
                    "ReachedBeginning is exactly an answered read with no \
                     cursor, for {cursor:?}/{settled}",
                );
            }
        }
    }

    /// The two halves of the transcript's older edge are one rule, walked end
    /// to end: what the daemon says about a page becomes what the affordance
    /// renders, with no state in between that can disagree.
    #[test]
    fn a_daemons_page_answer_reaches_the_affordance_state() {
        let settled_state = |has_more, prev_seq, reached_back_to| {
            older_history_state(
                transcript_older_cursor(has_more, prev_seq, reached_back_to),
                true,
            )
        };

        assert_eq!(
            settled_state(false, Some(400), Some(400)),
            OlderHistory::ReachedBeginning,
            "`has_more` false on a BACKWARD page means no older rows exist — \
             the room whose oldest painted row is genuinely its first message",
        );
        assert_eq!(
            settled_state(true, Some(3801), Some(3802)),
            OlderHistory::Available,
        );
        assert_eq!(
            settled_state(true, None, None),
            OlderHistory::ReachedBeginning,
            "a `has_more` page naming no cursor and serving no rows leaves \
             nothing to replay, so there is no press to offer. Saying the \
             beginning is reached overstates it by one page and is still the \
             better of the two: the alternative is the silence that reads as \
             `still loading` forever",
        );
    }

    /// Ingest: a backward page lands in FRONT of the paint, keeps the vector
    /// ascending, and drops anything already on screen.
    #[test]
    fn backfill_ingest_prepends_before_the_paint_and_keeps_the_order() {
        let mut painted = vec![message(5), message(6)];
        prepend_transcript_page(&mut painted, vec![message(3), message(4)]);
        assert_eq!(
            painted.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![3, 4, 5, 6],
            "older rows go in front, still ascending — the renderer, the resume \
             point and the next backward cursor all read this order"
        );

        prepend_transcript_page(&mut painted, vec![message(4), message(5)]);
        assert_eq!(
            painted.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![3, 4, 5, 6],
            "a page entirely inside the paint is a re-read, not a gap"
        );

        // The overlap case that decides whether the bound is read once or per
        // row: row 2 is older than the paint and belongs; row 3 is already
        // there. Re-reading `first()` after each insert would compare row 3
        // against the row 2 this same page just added, and keep it.
        prepend_transcript_page(&mut painted, vec![message(2), message(3)]);
        assert_eq!(
            painted.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![2, 3, 4, 5, 6]
        );

        // The forward walk's guard is the mirror of this one and neither may
        // stand in for the other: an unpainted room takes everything.
        let mut unpainted = Vec::new();
        prepend_transcript_page(&mut unpainted, vec![message(7), message(8)]);
        assert_eq!(first_transcript_seq(&unpainted), Some(7));
        assert_eq!(last_transcript_seq(&unpainted), Some(8));

        assert_eq!(first_transcript_seq(&[]), None);
    }

    #[test]
    fn transcript_cursor_is_seeded_from_last_hydrated_sequence() {
        assert_eq!(last_transcript_seq(&[]), None);
        assert_eq!(last_transcript_seq(&[message(3), message(9)]), Some(9));
        assert_eq!(url_with_after_seq("/events", None), "/events");
        assert_eq!(
            url_with_after_seq("/events", Some(0)),
            "/events?after_seq=0"
        );
    }

    /// The catch-up read's whole reason to decode a cursor: `/transcript` answers
    /// ONE bounded page, and the body names where the next one starts. A single
    /// request kept the first 200 rows of a burst and dropped the rest in
    /// silence, because nothing here read the two fields the daemon has answered
    /// with since OCEAN-249.
    ///
    /// The first body below puts `next_seq` a row past its own last entry, which
    /// the daemon cannot emit today — it derives the cursor from the page's last
    /// row. That is the point, the same one
    /// `snapshot_hydration_resumes_from_the_page_cursor` makes: the rule only has
    /// teeth on the day the two sources diverge, and on that day the daemon's
    /// number is the one naming what it actually served.
    #[test]
    fn transcript_page_decodes_the_daemon_cursor_and_stops_when_it_says_stop() {
        let more: TranscriptResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "transcript": [
                {
                    "seq": 201,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "first row past the caller's cursor",
                    "created_at": "2026-07-16T22:00:00Z"
                },
                {
                    "seq": 399,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "last row this page served",
                    "created_at": "2026-07-16T22:00:01Z"
                }
            ],
            "next_seq": 400,
            "has_more": true
        }))
        .expect("a paged transcript body should decode");
        assert!(more.ok);
        assert!(more.has_more);
        assert_eq!(more.next_seq, Some(400));
        assert_eq!(last_transcript_seq(&more.transcript), Some(399));
        assert_eq!(
            transcript_catchup_cursor(
                1,
                more.has_more,
                more.next_seq,
                last_transcript_seq(&more.transcript)
            ),
            Some(400),
            "the daemon's own cursor is where the next page starts — the rows it \
             served are only the fallback for a body that names none"
        );

        let done: TranscriptResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "transcript": [
                {
                    "seq": 500,
                    "author_id": "member-1",
                    "author_kind": "human",
                    "kind": "message",
                    "body": "the last row in the log",
                    "created_at": "2026-07-16T22:00:02Z"
                }
            ],
            "next_seq": null,
            "has_more": false
        }))
        .expect("a final page should decode");
        assert_eq!(
            transcript_catchup_cursor(
                1,
                done.has_more,
                done.next_seq,
                last_transcript_seq(&done.transcript)
            ),
            None,
            "a page the daemon says is the last must end the walk even though it \
             served rows the fallback could have continued from"
        );

        // A body carrying neither field still decodes, and reads as "the log ran
        // out" — the only answer that cannot invent a page the daemon never
        // named. No shipped daemon takes this arm; it is pinned so the additive
        // defaults cannot rot into an unbounded walk.
        let bare: TranscriptResponse =
            serde_json::from_value(serde_json::json!({ "ok": true, "transcript": [] }))
                .expect("a body without the cursor should still decode");
        assert_eq!(bare.next_seq, None);
        assert!(!bare.has_more);
        assert_eq!(
            transcript_catchup_cursor(1, bare.has_more, bare.next_seq, None),
            None
        );
    }

    /// The walk itself: every request starts at the cursor the previous page
    /// named — the caller's own resume point seeds the first — and the page cap,
    /// not the daemon, is what ends a room that keeps saying there is more.
    /// Unbounded, this runs on every join, leave, removal and send.
    #[test]
    fn transcript_catchup_follows_the_cursor_and_is_bounded_by_the_page_cap() {
        let endpoint = "/v1/rooms/persistent/room-1/transcript";
        // The caller's resume point, never the rows on screen.
        let mut cursor = Some(200);
        let mut pages_read = 0usize;
        let mut requested = Vec::new();
        loop {
            requested.push(url_with_after_seq(endpoint, cursor));
            pages_read += 1;
            // A daemon with more to give, forever.
            let covered = Some(200 + pages_read as u64 * 200);
            let Some(next) = transcript_catchup_cursor(pages_read, true, covered, None) else {
                break;
            };
            cursor = Some(next);
        }
        assert_eq!(
            requested,
            vec![
                format!("{endpoint}?after_seq=200"),
                format!("{endpoint}?after_seq=400"),
                format!("{endpoint}?after_seq=600"),
                format!("{endpoint}?after_seq=800"),
                format!("{endpoint}?after_seq=1000"),
            ],
            "the second request is the one the old single-shot path never made"
        );
        assert_eq!(requested.len(), MAX_TRANSCRIPT_CATCHUP_PAGES);
    }

    /// Ingest: a page appends only what is past the paint, and the room's resume
    /// point only ever moves forward.
    #[test]
    fn catchup_ingest_appends_past_the_paint_and_never_lowers_the_resume() {
        let mut painted = vec![message(1), message(2)];
        append_transcript_page(&mut painted, vec![message(2), message(3)]);
        assert_eq!(
            painted.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "the overlapping row is a duplicate delivery, not a second row 2"
        );

        append_transcript_page(&mut painted, vec![message(1), message(2)]);
        assert_eq!(
            painted.len(),
            3,
            "a page entirely behind the paint is a re-read, not a gap"
        );

        let mut unpainted = Vec::new();
        append_transcript_page(&mut unpainted, vec![message(7)]);
        assert_eq!(last_transcript_seq(&unpainted), Some(7));

        assert_eq!(advanced_resume_seq(None, 3), Some(3));
        assert_eq!(advanced_resume_seq(Some(3), 9), Some(9));
        assert_eq!(
            advanced_resume_seq(Some(9), 3),
            Some(9),
            "a replayed frame or an overlapping page must not rewind the resume"
        );
    }

    #[test]
    fn retry_wire_requires_exact_body_and_success_access_envelope() {
        assert_eq!(
            serde_json::to_value(RetryOutboxBody {
                client_event_id: "client-1"
            })
            .unwrap(),
            serde_json::json!({ "client_event_id": "client-1" })
        );

        let success: RetryOutboxSuccess = serde_json::from_value(serde_json::json!({
            "ok": true,
            "access": { "state": "live" }
        }))
        .expect("202 envelope should decode");
        assert!(success.ok);
        assert_eq!(success.access.state, RoomAccessState::Live);
        assert!(
            serde_json::from_value::<RetryOutboxSuccess>(serde_json::json!({ "ok": true }))
                .is_err()
        );
    }

    #[test]
    fn retry_projection_guard_requires_generation_and_room_match() {
        assert!(room_request_is_current(4, 4, "room-1", Some("room-1")));
        assert!(!room_request_is_current(4, 5, "room-1", Some("room-1")));
        assert!(!room_request_is_current(4, 4, "room-1", Some("room-2")));
        assert!(!room_request_is_current(4, 4, "room-1", None));
    }

    /// Regression: a request scheduled while room "A" is open at generation N
    /// must be rejected once "A" is closed and reopened under the SAME key —
    /// which bumps the generation to N+1 without changing `open_key`. Key
    /// equality alone (the pre-fix guard) would wrongly admit this stale
    /// request; `room_request_is_current` — the exact predicate backing the
    /// pub(crate) `Rooms::room_is_current` exposed for `rooms_workspace.rs` —
    /// must reject it.
    #[test]
    fn room_request_is_current_rejects_stale_generation_across_same_key_close_reopen() {
        let key = "room-a";
        let scheduled_generation = 3; // captured "gen N" while room-a was open

        // Sanity: the schedule-time snapshot is admitted against itself.
        assert!(room_request_is_current(
            scheduled_generation,
            scheduled_generation,
            key,
            Some(key),
        ));

        // Close + reopen the SAME key: generation advances to N+1, `open_key`
        // is still "room-a" — the pre-fix key-only guard would wrongly admit
        // the stale request here.
        let generation_after_close_reopen = scheduled_generation + 1;
        assert!(!room_request_is_current(
            scheduled_generation,
            generation_after_close_reopen,
            key,
            Some(key),
        ));
        // A freshly-stamped request for the new admission is admitted.
        assert!(room_request_is_current(
            generation_after_close_reopen,
            generation_after_close_reopen,
            key,
            Some(key),
        ));
    }

    #[test]
    fn joined_open_uses_only_the_authoritative_roster_for_access_mode() {
        let room = local_room();
        let local = access_projection(RoomAccessState::Local);
        assert!(joined_open_for(Some(&local), Some(&room), "local-human"));
        assert!(!joined_open_for(Some(&local), Some(&room), "remote-owner"));
        assert!(!joined_open_for(None, Some(&room), "local-human"));

        let mut federated = access_projection(RoomAccessState::Live);
        federated.members = vec![FederatedRoomMemberProjection {
            member_id: "federated-user".into(),
            owner_member_id: Some("local-human".into()),
            actor_type: FederatedActorType::User,
            role_in_room: FederatedRoomRole::Member,
            display_name: "Federated User".into(),
            public_agent_descriptor: None,
            joined_at: String::new(),
            derived_presence: None,
            local_binding_available: Some(true),
        }];
        assert!(joined_open_for(Some(&federated), None, "federated-user"));
        assert!(joined_open_for(Some(&federated), None, "local-human"));
        assert!(!joined_open_for(
            Some(&federated),
            Some(&room),
            "local-agent"
        ));
    }

    #[test]
    fn room_list_ticket_is_strictly_latest_request_wins() {
        assert!(list_request_is_current(8, 8));
        assert!(!list_request_is_current(7, 8));
        assert!(!list_request_is_current(8, 9));
    }

    #[test]
    fn outbox_states_keep_pending_and_failed_distinct() {
        assert_eq!(
            serde_json::from_str::<OutboxItemState>(r#""pending""#).unwrap(),
            OutboxItemState::Pending
        );
        assert_eq!(
            serde_json::from_str::<OutboxItemState>(r#""failed""#).unwrap(),
            OutboxItemState::Failed
        );
        assert_eq!(
            serde_json::to_string(&OutboxItemState::Pending).unwrap(),
            r#""pending""#
        );
        assert_eq!(
            serde_json::to_string(&OutboxItemState::Failed).unwrap(),
            r#""failed""#
        );
    }

    // ── TASK-21 tail guard: stale SSE frames after close/switch ────────
    #[test]
    fn production_frame_boundary_accepts_only_the_current_room_for_both_variants() {
        let frames = [
            RoomTailFrame::Message(message(8)),
            RoomTailFrame::Access(access_projection(RoomAccessState::Recovering)),
        ];

        for frame in frames {
            assert_eq!(
                accept_room_tail_frame(frame.clone(), 4, 4, "room-a", Some("room-a")),
                Some(frame.clone()),
                "current frame must be admitted"
            );
            assert_eq!(
                accept_room_tail_frame(frame.clone(), 4, 5, "room-a", Some("room-a")),
                None,
                "generation-stale frame must be a total no-op"
            );
            assert_eq!(
                accept_room_tail_frame(frame.clone(), 4, 4, "room-a", Some("room-b")),
                None,
                "wrong-room frame must be a total no-op"
            );
            assert_eq!(
                accept_room_tail_frame(frame, 4, 4, "room-a", None),
                None,
                "closed-room frame must be a total no-op"
            );
        }
    }

    #[test]
    fn read_summaries_fail_closed_on_duplicate_room_ids() {
        let duplicate = vec![
            RoomReadStateWire {
                room_id: "room-1".into(),
                latest_seq: Some("7".into()),
                read_seq: Some("3".into()),
            },
            RoomReadStateWire {
                room_id: "room-1".into(),
                latest_seq: Some("8".into()),
                read_seq: Some("4".into()),
            },
        ];
        assert!(read_summaries_from_wire(&duplicate).is_err());
    }

    #[test]
    fn read_summaries_fail_closed_on_malformed_decimal() {
        let malformed = vec![RoomReadStateWire {
            room_id: "room-1".into(),
            latest_seq: Some("oops".into()),
            read_seq: Some("1".into()),
        }];
        assert!(read_summaries_from_wire(&malformed).is_err());
    }

    #[test]
    fn patch_response_parses_canonical_cursor_body_exactly() {
        let local: ReadCursorPatchEnvelope = serde_json::from_value(serde_json::json!({
            "ok": true,
            "cursor": {
                "room_id": "room-1",
                "read_seq": "9"
            }
        }))
        .unwrap();
        assert!(local.ok);
        assert_eq!(
            parse_patch_read_cursor_response("room-1", local.cursor).unwrap(),
            RoomReadCursorProjection {
                read_seq: Some(9),
                mirrored_upstream_read_seq: None,
            }
        );
    }

    #[test]
    fn patch_response_parses_js_safe_decimal_strings_and_null() {
        let big: ReadCursorPatchEnvelope = serde_json::from_value(serde_json::json!({
            "ok": true,
            "cursor": {
                "room_id": "room-1",
                "read_seq": "9007199254740993"
            }
        }))
        .unwrap();
        assert_eq!(
            parse_patch_read_cursor_response("room-1", big.cursor).unwrap(),
            RoomReadCursorProjection {
                read_seq: Some(9_007_199_254_740_993),
                mirrored_upstream_read_seq: None,
            }
        );

        let null: ReadCursorPatchEnvelope = serde_json::from_value(serde_json::json!({
            "ok": true,
            "cursor": {
                "room_id": "room-1",
                "read_seq": null
            }
        }))
        .unwrap();
        assert_eq!(
            parse_patch_read_cursor_response("room-1", null.cursor).unwrap(),
            RoomReadCursorProjection {
                read_seq: None,
                mirrored_upstream_read_seq: None,
            }
        );
    }

    #[test]
    fn patch_response_rejects_bad_decimal_string() {
        let live: ReadCursorPatchEnvelope = serde_json::from_value(serde_json::json!({
            "ok": true,
            "cursor": {
                "room_id": "room-1",
                "read_seq": "NaN"
            }
        }))
        .unwrap();
        assert!(parse_patch_read_cursor_response("room-1", live.cursor).is_err());
    }

    #[test]
    fn patch_response_rejects_wrong_or_empty_room_id() {
        let wrong: ReadCursorPatchEnvelope = serde_json::from_value(serde_json::json!({
            "ok": true,
            "cursor": {
                "room_id": "room-2",
                "read_seq": "44"
            }
        }))
        .unwrap();
        assert!(parse_patch_read_cursor_response("room-1", wrong.cursor).is_err());

        let empty: ReadCursorPatchEnvelope = serde_json::from_value(serde_json::json!({
            "ok": true,
            "cursor": {
                "room_id": "",
                "read_seq": "44"
            }
        }))
        .unwrap();
        assert!(parse_patch_read_cursor_response("room-1", empty.cursor).is_err());
    }

    #[test]
    fn sse_read_cursor_decodes_canonical_wire_and_rejects_malformed_or_wrong_room_id() {
        assert_eq!(
            decode_room_tail_frame(
                "room_read_cursor",
                r#"{"room_id":"room-1","read_seq":"9007199254740993"}"#,
                "room-1",
            ),
            Some(RoomTailFrame::ReadCursor(RoomReadCursorProjection {
                read_seq: None,
                mirrored_upstream_read_seq: Some(9_007_199_254_740_993),
            }))
        );

        assert_eq!(
            decode_room_tail_frame(
                "room_read_cursor",
                r#"{"room_id":"room-1","read_seq":null}"#,
                "room-1",
            ),
            Some(RoomTailFrame::ReadCursor(RoomReadCursorProjection {
                read_seq: None,
                mirrored_upstream_read_seq: None,
            }))
        );

        assert_eq!(
            decode_room_tail_frame(
                "room_read_cursor",
                r#"{"room_id":"room-2","read_seq":"44"}"#,
                "room-1",
            ),
            None
        );
        assert_eq!(
            decode_room_tail_frame(
                "room_read_cursor",
                r#"{"room_id":"","read_seq":"44"}"#,
                "room-1",
            ),
            None
        );
        assert_eq!(
            decode_room_tail_frame(
                "room_read_cursor",
                r#"{"room_id":"room-1","read_seq":"oops"}"#,
                "room-1",
            ),
            None
        );
    }

    #[test]
    fn open_hydration_preserves_existing_read_seq_until_cursor_arrives() {
        let summaries = RwSignal::new(HashMap::from([(
            "room-1".to_string(),
            RoomReadSummary {
                latest_seq: Some(3),
                read_seq: Some(2),
            },
        )]));
        let transcript = vec![message(7)];
        let access = access_projection(RoomAccessState::Local);

        update_open_summary_from_open_room(
            &summaries,
            Some("room-1"),
            &transcript,
            Some(&access),
            None,
        );

        assert_eq!(
            summaries.get_untracked().get("room-1"),
            Some(&RoomReadSummary {
                latest_seq: Some(7),
                read_seq: Some(2),
            })
        );
    }

    #[test]
    fn lagging_mirrored_sse_cursor_cannot_lower_local_confirmed_read() {
        // Local PATCH confirms read 100.
        let local = parse_patch_read_cursor_response(
            "room-1",
            RoomReadCursorBody {
                room_id: "room-1".into(),
                read_seq: Some("100".into()),
            },
        )
        .unwrap();
        assert_eq!(
            local,
            RoomReadCursorProjection {
                read_seq: Some(100),
                mirrored_upstream_read_seq: None,
            }
        );
        assert_eq!(current_durable_read_seq(&local), Some(100));

        // A lagging mirrored SSE frame reports 90.
        let Some(RoomTailFrame::ReadCursor(lagging)) = decode_room_tail_frame(
            "room_read_cursor",
            r#"{"room_id":"room-1","read_seq":"90"}"#,
            "room-1",
        ) else {
            panic!("mirrored cursor frame should decode");
        };
        let merged = merge_read_cursor_projection(Some(&local), lagging);
        assert_eq!(
            merged,
            RoomReadCursorProjection {
                read_seq: Some(100),
                mirrored_upstream_read_seq: Some(90),
            }
        );
        assert_eq!(current_durable_read_seq(&merged), Some(100));

        // The room summary keeps the confirmed read; unread stays cleared.
        let summaries = RwSignal::new(HashMap::from([(
            "room-1".to_string(),
            RoomReadSummary {
                latest_seq: Some(100),
                read_seq: Some(100),
            },
        )]));
        update_open_summary_from_open_room(
            &summaries,
            Some("room-1"),
            &[message(100)],
            Some(&access_projection(RoomAccessState::Local)),
            Some(&merged),
        );
        assert_eq!(
            summaries.get_untracked().get("room-1"),
            Some(&RoomReadSummary {
                latest_seq: Some(100),
                read_seq: Some(100),
            })
        );
        assert!(!room_has_durable_unread(
            summaries.get_untracked().get("room-1")
        ));

        // A later, higher mirrored frame still corrects the durable read up.
        let Some(RoomTailFrame::ReadCursor(ahead)) = decode_room_tail_frame(
            "room_read_cursor",
            r#"{"room_id":"room-1","read_seq":"110"}"#,
            "room-1",
        ) else {
            panic!("mirrored cursor frame should decode");
        };
        let corrected = merge_read_cursor_projection(Some(&merged), ahead);
        assert_eq!(
            corrected,
            RoomReadCursorProjection {
                read_seq: Some(100),
                mirrored_upstream_read_seq: Some(110),
            }
        );
        assert_eq!(current_durable_read_seq(&corrected), Some(110));

        update_open_summary_from_open_room(
            &summaries,
            Some("room-1"),
            &[message(110)],
            Some(&access_projection(RoomAccessState::Local)),
            Some(&corrected),
        );
        assert_eq!(
            summaries.get_untracked().get("room-1"),
            Some(&RoomReadSummary {
                latest_seq: Some(110),
                read_seq: Some(110),
            })
        );
        assert!(!room_has_durable_unread(
            summaries.get_untracked().get("room-1")
        ));
    }

    #[test]
    fn read_cursor_merge_seeds_from_empty_and_never_clears_known_positions() {
        let mirrored = RoomReadCursorProjection {
            read_seq: None,
            mirrored_upstream_read_seq: Some(7),
        };
        assert_eq!(
            merge_read_cursor_projection(None, mirrored.clone()),
            mirrored
        );

        // An empty (null read_seq) frame cannot erase either known position.
        let known = RoomReadCursorProjection {
            read_seq: Some(12),
            mirrored_upstream_read_seq: Some(9),
        };
        let Some(RoomTailFrame::ReadCursor(empty)) = decode_room_tail_frame(
            "room_read_cursor",
            r#"{"room_id":"room-1","read_seq":null}"#,
            "room-1",
        ) else {
            panic!("null cursor frame should decode");
        };
        assert_eq!(merge_read_cursor_projection(Some(&known), empty), known);
    }

    #[test]
    fn applied_open_read_seq_folds_summary_and_durable_cursor_monotonically() {
        // Absent on both sides keeps the historical zero floor.
        assert_eq!(applied_open_read_seq(None, None), 0);
        // Either side alone still applies.
        assert_eq!(applied_open_read_seq(Some(5), None), 5);
        assert_eq!(applied_open_read_seq(None, Some(9)), 9);
        // A lagging summary can no longer mask a further durable cursor.
        assert_eq!(applied_open_read_seq(Some(5), Some(100)), 100);
        // A further summary still wins over a lagging durable cursor.
        assert_eq!(applied_open_read_seq(Some(100), Some(5)), 100);
    }

    #[test]
    fn merge_room_read_summaries_is_monotonic_and_removes_deleted_rooms() {
        let current = HashMap::from([
            (
                "room-1".to_string(),
                RoomReadSummary {
                    latest_seq: Some(9),
                    read_seq: Some(4),
                },
            ),
            (
                "room-2".to_string(),
                RoomReadSummary {
                    latest_seq: Some(8),
                    read_seq: Some(6),
                },
            ),
        ]);
        let incoming = HashMap::from([(
            "room-1".to_string(),
            RoomReadSummary {
                latest_seq: Some(5),
                read_seq: None,
            },
        )]);
        let rooms = vec![Room {
            id: "room-1".into(),
            name: "Room One".into(),
            participants: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
            trigger_policy: None,
            workspace_root: None,
        }];

        let merged = merge_room_read_summaries(&current, &rooms, &incoming);

        assert_eq!(
            merged.get("room-1"),
            Some(&RoomReadSummary {
                latest_seq: Some(9),
                read_seq: Some(4),
            })
        );
        assert!(!merged.contains_key("room-2"));
    }

    #[test]
    fn silent_fetch_skips_during_interactive_loading_and_cleanup_is_ticket_safe() {
        assert!(should_skip_rooms_fetch(RoomsFetchMode::Silent, true));
        assert!(!should_skip_rooms_fetch(RoomsFetchMode::Silent, false));
        assert!(!should_skip_rooms_fetch(RoomsFetchMode::Interactive, true));

        let rooms_loaded = RwSignal::new(false);
        let rooms_loading = RwSignal::new(true);
        finish_rooms_fetch(
            &rooms_loaded,
            &rooms_loading,
            RoomsFetchMode::Interactive,
            false,
        );
        assert!(!rooms_loaded.get_untracked());
        assert!(rooms_loading.get_untracked());

        finish_rooms_fetch(
            &rooms_loaded,
            &rooms_loading,
            RoomsFetchMode::Interactive,
            true,
        );
        assert!(rooms_loaded.get_untracked());
        assert!(!rooms_loading.get_untracked());
    }

    #[test]
    fn unread_dot_helper_requires_latest_ahead_of_read() {
        assert!(room_has_durable_unread(Some(&RoomReadSummary {
            latest_seq: Some(5),
            read_seq: Some(4),
        })));
        assert!(room_has_durable_unread(Some(&RoomReadSummary {
            latest_seq: Some(5),
            read_seq: None,
        })));
        assert!(!room_has_durable_unread(Some(&RoomReadSummary {
            latest_seq: Some(5),
            read_seq: Some(5),
        })));
    }
}
