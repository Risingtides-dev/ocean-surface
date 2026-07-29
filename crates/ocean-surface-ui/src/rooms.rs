//! Persistent Rooms panel — list, create, join/leave, transcript + composer.
//!
//! The web counterpart to the daemon's persistent-rooms surface (OCEAN-65,
//! routes under `/v1/rooms/persistent/*`). A room is a durable, named
//! collaboration space with a participant roster and an append-only transcript.
//! This module owns:
//!
//!   GET    /v1/rooms/persistent                       → list rooms
//!   POST   /v1/rooms/persistent                       → create a room
//!   GET    /v1/rooms/persistent/{key}                 → room + transcript
//!   POST   /v1/rooms/persistent/{key}/participants    → join
//!   DELETE /v1/rooms/persistent/{key}/participants/{id}→ leave
//!   POST   /v1/rooms/persistent/{key}/messages        → post a message
//!   GET    /v1/rooms/persistent/{key}/events           → live SSE tail (TASK-10)
//!
//! Live updates: the daemon's room-scoped SSE (TASK-10, `GET
//! /v1/rooms/persistent/{key}/events`) streams every transcript row as a
//! `room_message` frame with `id:=seq`. The surface hydrates once, then tails
//! live with sequence resume (`?after_seq=` on each newly constructed browser
//! connection) — no poll, no global-stream workaround (TASK-11).
//!
//! The whole module is self-contained — it carries its own request layer rather
//! than threading rooms state through the `Daemon` handle — so it never touches
//! the live agent loop / session SSE code.

use futures_util::future::Either;
use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

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

/// localStorage key for this surface's stable room participant id, so a given
/// browser keeps the same identity across reloads (join/leave/author are keyed
/// on it).
const ROOM_IDENTITY_KEY: &str = "ocean.room_identity";

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
}

// ---- Federated wire types (exact mirror of ocean-core 786c6ba4) -------------

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outbox: Vec<RoomOutboxItem>,
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

/// How a room's agents are auto-woken. Mirrors `ocean_core::RoomTriggerPolicy`.
/// All flags default off; the daemon reads this on `room_create` and evaluates
/// it on every non-agent-authored message (OCEAN-65 / OCEAN-111).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RoomTriggerPolicy {
    /// Wake an agent when it is @-mentioned in the transcript (the common case).
    #[serde(default)]
    pub on_mention: bool,
    /// Wake an agent when someone replies in a thread it participates in.
    #[serde(default)]
    pub on_thread_reply: bool,
    /// Wake an agent when a rendered component emits an interaction event.
    #[serde(default)]
    pub on_component_event: bool,
    /// Optional cron expression for scheduled wake-ups. `None`/empty = no schedule.
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
}

// ---- Response envelopes (the daemon's `json!({ "ok": .., .. })` shapes) ------

#[derive(Debug, Clone, Deserialize)]
struct RoomsListResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    rooms: Vec<Room>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RoomGetResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    room: Option<Room>,
    #[serde(default)]
    transcript: Vec<RoomMessage>,
    /// Required on every successful room open, including local rooms.
    access: RoomAccessProjection,
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

#[derive(Debug, Clone, Deserialize)]
struct TranscriptResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    transcript: Vec<RoomMessage>,
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

/// Identity of this surface as a room participant. Stable per browser via
/// localStorage so join/leave/author all key on the same id.
#[derive(Debug, Clone)]
pub struct RoomIdentity {
    pub id: String,
    pub display_name: String,
}

impl RoomIdentity {
    fn current() -> Self {
        // Reuse a persisted id if present; otherwise mint one and store it.
        let id = local_storage()
            .and_then(|s| s.get_item(ROOM_IDENTITY_KEY).ok().flatten())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let minted = format!("web-{}", mint_suffix());
                if let Some(s) = local_storage() {
                    let _ = s.set_item(ROOM_IDENTITY_KEY, &minted);
                }
                minted
            });
        Self {
            display_name: id.clone(),
            id,
        }
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

/// Reactive handle for the rooms panel. Holds the room list, the open room +
/// its transcript, status text, and the SSE-tail generation counter. Cloned
/// freely (all fields are `Copy` signal handles), like [`crate::daemon::Daemon`].
#[derive(Clone, Copy)]
pub struct Rooms {
    /// Daemon base URL signal, shared with `Daemon::url` so requests follow the
    /// origin learned at bootstrap (phone-via-tunnel resolves it asynchronously,
    /// so we must read it live at request time, not snapshot it at construction).
    pub url: RwSignal<String>,
    /// All persistent rooms (from `GET /v1/rooms/persistent`).
    pub list: RwSignal<Vec<Room>>,
    /// Whether the first `fetch_rooms` has resolved (success or failure). Starts
    /// false so the panel shows a loading placeholder instead of falsely
    /// asserting "No rooms yet" during the initial in-flight fetch.
    pub rooms_loaded: RwSignal<bool>,
    /// The currently selected room key, if any.
    pub open_key: RwSignal<Option<String>>,
    /// The open room's full record (roster + metadata).
    pub open_room: RwSignal<Option<Room>>,
    /// The open room's transcript, ascending by `seq`.
    pub transcript: RwSignal<Vec<RoomMessage>>,
    /// Free-form status line (errors, in-flight notices).
    pub status: RwSignal<String>,
    /// Monotonic generation: bumped when the open room changes so a stale
    /// poll/SSE loop retires instead of writing into the wrong room.
    generation: RwSignal<u64>,
    /// This browser's stable participant id, used for join/leave/post.
    pub identity_id: RwSignal<&'static str>,
    /// This browser's display name.
    pub identity_name: RwSignal<&'static str>,
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
    /// Typed operation-id pair for create-room: (next_op_id, last_outcome).
    /// Surfaces snapshot the op_id before dispatching and gate only on the
    /// outcome carrying a matching id — concurrent submits never cross-resolve.
    pub create_op: RwSignal<(u64, Option<CreateOutcome>)>,
}

impl Rooms {
    /// Construct a rooms handle that shares the live `Daemon::url` signal, so it
    /// always targets the origin resolved by bootstrap. Room collaboration is
    /// daemon-native text; LiveKit state is intentionally outside this type.
    pub fn new(daemon: &crate::daemon::Daemon) -> Self {
        let identity = RoomIdentity::current();
        // Leak the small, app-lifetime identity strings to obtain `&'static str`
        // signals, so the panel can pass them into request closures without a
        // per-call clone.
        let id_static: &'static str = Box::leak(identity.id.into_boxed_str());
        let name_static: &'static str = Box::leak(identity.display_name.into_boxed_str());
        Self {
            url: daemon.url,
            list: RwSignal::new(Vec::new()),
            rooms_loaded: RwSignal::new(false),
            open_key: RwSignal::new(None),
            open_room: RwSignal::new(None),
            transcript: RwSignal::new(Vec::new()),
            status: RwSignal::new(String::new()),
            generation: RwSignal::new(0),
            identity_id: RwSignal::new(id_static),
            identity_name: RwSignal::new(name_static),
            tail_state: RwSignal::new(TailState::Replaying),
            available_agents: RwSignal::new(Vec::new()),
            agents_loaded: RwSignal::new(false),
            access: RwSignal::new(None),
            create_op: RwSignal::new((0, None)),
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Current-room predicate over the live `generation` and `open_key`
    /// signals. Async tail work checks it before any non-frame state write;
    /// decoded frames pass through `accept_room_tail_frame` below.
    fn room_is_current(&self, generation_id: u64, key: &str) -> bool {
        room_request_is_current(
            generation_id,
            self.generation.get_untracked(),
            key,
            self.open_key.get_untracked().as_deref(),
        )
    }

    /// Synchronously clear the open-room signals and pin `tail_state` to
    /// `Replaying` so no prior room state leaks into the next open. Shared
    /// by `open_room` (pre-hydrate) and `close_room`.
    fn reset_room_state(&self) {
        self.open_room.set(None);
        self.transcript.set(Vec::new());
        self.access.set(None);
        self.tail_state.set(TailState::Replaying);
    }

    /// Whether the current identity is in the open room's roster.
    pub fn joined_open(&self) -> bool {
        let me = self.identity_id.get();
        self.open_room
            .get()
            .map(|r| r.participants.iter().any(|p| p.id == me))
            .unwrap_or(false)
    }

    /// Fetch the room list (`GET /v1/rooms/persistent`).
    pub fn fetch_rooms(&self) {
        let base = self.base();
        let list = self.list;
        let status = self.status;
        let loaded = self.rooms_loaded;
        spawn_local(async move {
            let get_url = format!("{base}/v1/rooms/persistent");
            match Request::get(&get_url).send().await {
                Ok(resp) => match resp.json::<RoomsListResponse>().await {
                    Ok(r) if r.ok => list.set(r.rooms),
                    Ok(r) => status.set(format!(
                        "rooms list failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => status.set(format!("rooms decode error: {err}")),
                },
                Err(err) => status.set(format!("rooms fetch error: {err}")),
            }
            // The first fetch has now resolved — the panel may distinguish
            // "still loading" from "genuinely empty". Set on every outcome so a
            // failed fetch stops claiming the list is loading forever.
            loaded.set(true);
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
    pub fn create_room(&self, name: String, policy: Option<RoomTriggerPolicy>) -> u64 {
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
            };
            let post_url = format!("{base}/v1/rooms/persistent");
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            let res = match res {
                Ok(req) => req.send().await,
                Err(err) => {
                    status.set(format!("create encode error: {err}"));
                    // CAS: only publish if the slot still belongs to this op.
                    signal.update(|(cur, out)| {
                        if *cur == op_id {
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
                    Ok(r) if r.ok => {
                        status.set(format!("room '{name}' created"));
                        // Refresh the list and open the new room.
                        me.fetch_rooms();
                        me.open_room(key.clone());
                        CreateOutcome::Success { key: key.clone() }
                    }
                    Ok(r) => {
                        let msg = r.error.unwrap_or_else(|| "unknown error".into());
                        status.set(format!("create failed: {msg}"));
                        if msg.to_lowercase().contains("duplicate") {
                            CreateOutcome::Duplicate
                        } else {
                            CreateOutcome::Failed { error: msg }
                        }
                    }
                    Err(err) => {
                        let msg = format!("create decode error: {err}");
                        status.set(msg.clone());
                        CreateOutcome::Failed { error: msg }
                    }
                },
                Err(err) => {
                    let msg = format!("create post error: {err}");
                    status.set(msg.clone());
                    CreateOutcome::Failed { error: msg }
                }
            };
            // CAS: only publish if the slot still belongs to this op.
            signal.update(|(cur, out)| {
                if *cur == op_id {
                    *out = Some(outcome);
                }
            });
        });
        op_id
    }

    /// Open a room: load its record + full transcript, bump the generation, and
    /// start the room-scoped SSE live tail (TASK-10/TASK-11).
    pub fn open_room(&self, key: String) {
        let base = self.base();
        let me = *self;
        let open_key = self.open_key;
        let open_room = self.open_room;
        let transcript = self.transcript;
        let status = self.status;
        let generation = self.generation;

        // Retire any prior room's live loops and reset state synchronously.
        let generation_id = generation.get_untracked().wrapping_add(1);
        generation.set(generation_id);
        open_key.set(Some(key.clone()));
        me.reset_room_state();
        status.set("loading room…".into());
        // Keep the workspace visible when opening a room (no panel toggle).
        // The workspace is a persistent shell — opening a room selects it
        // in-place, it does not unmount or overlay-swap.

        spawn_local(async move {
            let get_url = format!("{base}/v1/rooms/persistent/{}", encode(&key));
            match Request::get(&get_url).send().await {
                Ok(resp) if resp.ok() => match resp.json::<RoomGetResponse>().await {
                    Ok(r) if r.ok => {
                        // Guard against a fast re-select before this landed.
                        if generation.get_untracked() != generation_id {
                            return;
                        }
                        open_room.set(r.room);
                        transcript.set(r.transcript);
                        me.access.set(Some(r.access));
                        status.set(String::new());
                        // Pre-fetch agent list for the picker (TASK-11).
                        me.fetch_agents();
                        // Start live updates for this generation.
                        me.start_live_tail(key.clone(), generation_id);
                    }
                    Ok(r) => status.set(format!(
                        "room load failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => status.set(format!("room decode error: {err}")),
                },
                Ok(resp) => {
                    let http_status = resp.status();
                    match resp.json::<RoomErrorResponse>().await {
                        Ok(r) => status.set(format!(
                            "room load failed: {}",
                            r.error.unwrap_or_else(|| format!("HTTP {http_status}"))
                        )),
                        Err(err) => {
                            status.set(format!("room load failed: HTTP {http_status} ({err})"))
                        }
                    }
                }
                Err(err) => status.set(format!("room fetch error: {err}")),
            }
        });
    }
    /// Close the open room and stop its live loops.
    pub fn close_room(&self) {
        self.generation.update(|g| *g = g.wrapping_add(1));
        self.open_key.set(None);
        self.reset_room_state();
    }

    /// Join the open room as the current identity
    /// (`POST .../participants`).
    pub fn join_open(&self) {
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let status = self.status;
        let id = self.identity_id.get_untracked();
        let name = self.identity_name.get_untracked();
        spawn_local(async move {
            let body = JoinBody {
                id,
                display_name: name,
                kind: RoomParticipantKind::Human,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/participants", encode(&key));
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            match res {
                Ok(req) => match req.send().await {
                    Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                        Ok(r) if r.ok => {
                            me.open_room.set(r.room);
                            status.set("joined".into());
                            me.refresh_open_transcript(&key);
                            me.fetch_rooms();
                        }
                        Ok(r) => status.set(format!(
                            "join failed: {}",
                            r.error.unwrap_or_else(|| "unknown error".into())
                        )),
                        Err(err) => status.set(format!("join decode error: {err}")),
                    },
                    Err(err) => status.set(format!("join post error: {err}")),
                },
                Err(err) => status.set(format!("join encode error: {err}")),
            }
        });
    }

    /// Add a real named agent to the open room via the daemon's validated join
    /// (`POST .../participants` with `kind = agent`). TASK-9/TASK-11: the daemon
    /// resolves `agent_id` against `agentdir::resolve` and rejects bogus ids
    /// with a typed 400. The surface picks from `available_agents` (fetched via
    /// `GET /v1/agents`) — free-text fake agents are gone. Once joined, the
    /// agent is mentionable and auto-convenes per the room's trigger policy.
    pub fn add_agent(&self, agent_id: String) {
        let agent_id = agent_id.trim().to_string();
        if agent_id.is_empty() {
            self.status.set("agent id required".into());
            return;
        }
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let status = self.status;
        spawn_local(async move {
            let body = JoinBody {
                id: &agent_id,
                display_name: &agent_id,
                kind: RoomParticipantKind::Agent,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/participants", encode(&key));
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&body);
            match res {
                Ok(req) => match req.send().await {
                    Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                        Ok(r) if r.ok => {
                            me.open_room.set(r.room);
                            status.set(format!("agent '{agent_id}' added — mention @{agent_id}"));
                            me.refresh_open_transcript(&key);
                            me.fetch_rooms();
                        }
                        Ok(r) => status.set(format!(
                            "add agent failed: {}",
                            r.error.unwrap_or_else(|| "unknown error".into())
                        )),
                        Err(err) => status.set(format!("add agent decode error: {err}")),
                    },
                    Err(err) => status.set(format!("add agent post error: {err}")),
                },
                Err(err) => status.set(format!("add agent encode error: {err}")),
            }
        });
    }

    /// Ids of the open room's **agent** participants — the actors a human can
    /// `@mention` to auto-convene. Used to render the composer's discoverability
    /// hint.
    #[allow(dead_code)]
    pub fn agent_ids(&self) -> Vec<String> {
        let access = self.access.get();
        let room = self.open_room.get();
        agent_ids_for(access.as_ref(), room.as_ref())
    }

    /// Leave the open room (`DELETE .../participants/{id}`).
    pub fn leave_open(&self) {
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let status = self.status;
        let id = self.identity_id.get_untracked();
        spawn_local(async move {
            let del_url = format!(
                "{base}/v1/rooms/persistent/{}/participants/{}",
                encode(&key),
                encode(id)
            );
            match Request::delete(&del_url).send().await {
                Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                    Ok(r) if r.ok => {
                        me.open_room.set(r.room);
                        status.set("left".into());
                        me.refresh_open_transcript(&key);
                        me.fetch_rooms();
                    }
                    Ok(r) => status.set(format!(
                        "leave failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => status.set(format!("leave decode error: {err}")),
                },
                Err(err) => status.set(format!("leave error: {err}")),
            }
        });
    }

    /// Post a message to the open room (`POST .../messages`). `@id` mentions in
    /// the body drive the daemon's trigger-policy auto-convene.
    pub fn post_message(&self, body: String) {
        if !access_allows_writes(self.access.get_untracked().as_ref()) {
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
        let status = self.status;
        let id = self.identity_id.get_untracked();
        spawn_local(async move {
            let payload = PostMessageBody {
                author_id: id,
                author_kind: RoomParticipantKind::Human,
                body: &body,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/messages", encode(&key));
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&payload);
            match res {
                Ok(req) => match req.send().await {
                    Ok(resp) if resp.ok() => {
                        // The daemon also appends a System line on auto-convene;
                        // re-tail to pick up our message + any trigger notice.
                        me.refresh_open_transcript(&key);
                    }
                    Ok(resp) => {
                        let text = resp.text().await.unwrap_or_default();
                        status.set(format!("message failed: {text}"));
                    }
                    Err(err) => status.set(format!("message post error: {err}")),
                },
                Err(err) => status.set(format!("message encode error: {err}")),
            }
        });
    }

    /// Retry a failed outbox item (`POST …/outbox/retry`). On 202 the daemon
    /// returns the fresh access projection; apply it immediately with
    /// generation + open_key guard, then idempotently accept the duplicate SSE
    #[allow(dead_code)]
    /// `room_access` wake when it arrives later.
    pub fn retry_outbox(&self, client_event_id: String) {
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let status = self.status;
        let generation_id = self.generation.get_untracked();
        spawn_local(async move {
            let payload = RetryOutboxBody {
                client_event_id: &client_event_id,
            };
            let post_url = format!("{base}/v1/rooms/persistent/{}/outbox/retry", encode(&key));
            let res = Request::post(&post_url)
                .header("content-type", "application/json")
                .json(&payload);
            match res {
                Ok(req) => match req.send().await {
                    Ok(resp) if resp.status() == 202 => {
                        match resp.json::<RetryOutboxSuccess>().await {
                            Ok(r) if r.ok => {
                                if !room_request_is_current(
                                    generation_id,
                                    me.generation.get_untracked(),
                                    &key,
                                    me.open_key.get_untracked().as_deref(),
                                ) {
                                    return;
                                }
                                apply_access_projection(&me.access, r.access);
                                status.set("retry queued".into());
                            }
                            Ok(_) => status.set("retry response invalid".into()),
                            Err(err) => status.set(format!("retry decode error: {err}")),
                        }
                    }
                    Ok(resp) => {
                        let http_status = resp.status();
                        let error = resp.json::<RetryOutboxErrorResponse>().await;
                        if !room_request_is_current(
                            generation_id,
                            me.generation.get_untracked(),
                            &key,
                            me.open_key.get_untracked().as_deref(),
                        ) {
                            return;
                        }
                        match error {
                            Ok(r) => {
                                let detail = match (r.code, r.error) {
                                    (Some(code), Some(error)) => format!("{code}: {error}"),
                                    (Some(code), None) => code,
                                    (None, Some(error)) => error,
                                    (None, None) => format!("HTTP {http_status}"),
                                };
                                status.set(format!("retry failed: {detail}"));
                            }
                            Err(err) => {
                                status.set(format!("retry failed: HTTP {http_status} ({err})"))
                            }
                        }
                    }
                    Err(err) => status.set(format!("retry post error: {err}")),
                },
                Err(err) => status.set(format!("retry encode error: {err}")),
            }
        });
    }

    /// Re-fetch the open room's transcript tail (`after_seq` = our highest seq)
    /// and append only new entries. Used after our own writes; the SSE stream
    /// remains the primary source for remote updates.
    fn refresh_open_transcript(&self, key: &str) {
        let base = self.base();
        let transcript = self.transcript;
        let open_key = self.open_key;
        let key = key.to_string();
        spawn_local(async move {
            // Only tail if this is still the open room.
            if open_key.get_untracked().as_deref() != Some(key.as_str()) {
                return;
            }
            let after = transcript
                .get_untracked()
                .last()
                .map(|m| m.seq)
                .unwrap_or(0);
            let get_url = format!(
                "{base}/v1/rooms/persistent/{}/transcript?after_seq={after}",
                encode(&key)
            );
            if let Ok(resp) = Request::get(&get_url).send().await {
                if let Ok(r) = resp.json::<TranscriptResponse>().await {
                    if r.ok && !r.transcript.is_empty() {
                        // Guard: room may have changed during the await.
                        if open_key.get_untracked().as_deref() != Some(key.as_str()) {
                            return;
                        }
                        transcript.update(|t| {
                            for m in r.transcript {
                                if t.last().map(|l| l.seq).unwrap_or(0) < m.seq {
                                    t.push(m);
                                }
                            }
                        });
                    }
                }
            }
        });
    }

    /// Start the live tail for `key` at `generation_id`: room-scoped SSE
    /// (`GET /v1/rooms/persistent/{key}/events`) with `?after_seq=` resume for
    /// newly constructed browser connections (TASK-10/TASK-11). Replaces the
    /// 2.5s poll workaround.
    fn start_live_tail(&self, key: String, generation_id: u64) {
        let me = *self;
        let base = self.base();
        let last_seq = RwSignal::new(last_transcript_seq(&self.transcript.get_untracked()));
        let tail_state = self.tail_state;

        spawn_local(async move {
            let events_url = format!("{base}/v1/rooms/persistent/{}/events", encode(&key));
            let mut resume_seq = last_seq.get_untracked();
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

                let url = format!("{events_url}?after_seq={resume_seq}");
                let mut es = match EventSource::new(&url) {
                    Ok(es) => es,
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let message_sub = match es.subscribe("room_message") {
                    Ok(s) => s,
                    Err(_) => {
                        gloo_timers::future::TimeoutFuture::new(2_000).await;
                        continue;
                    }
                };
                let access_sub = match es.subscribe("room_access") {
                    Ok(s) => s,
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
                let mut stream = futures_util::stream::select(message_sub, access_sub);
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
                    let Some(data) = msg.data().as_string() else {
                        continue;
                    };
                    let Some(frame) = decode_room_tail_frame(&name, &data) else {
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
                            apply_access_projection(&me.access, access);
                        }
                        RoomTailFrame::Message(entry) => {
                            if entry.seq > last_seq.get_untracked() {
                                last_seq.set(entry.seq);
                            }
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
                resume_seq = last_seq.get_untracked();
                reconnecting = true;
                if !me.room_is_current(generation_id, &key) {
                    break;
                }
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
        });
    }
}

// ---- Helpers ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum RoomTailFrame {
    Message(RoomMessage),
    Access(RoomAccessProjection),
}

fn decode_room_tail_frame(name: &str, data: &str) -> Option<RoomTailFrame> {
    match name {
        "room_message" => serde_json::from_str(data).ok().map(RoomTailFrame::Message),
        "room_access" => serde_json::from_str(data).ok().map(RoomTailFrame::Access),
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

fn last_transcript_seq(transcript: &[RoomMessage]) -> u64 {
    transcript.last().map(|message| message.seq).unwrap_or(0)
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

fn room_request_is_current(
    expected_generation: u64,
    current_generation: u64,
    expected_key: &str,
    current_key: Option<&str>,
) -> bool {
    expected_generation == current_generation && current_key == Some(expected_key)
}

fn access_allows_writes(access: Option<&RoomAccessProjection>) -> bool {
    matches!(
        access.map(|projection| projection.state),
        Some(RoomAccessState::Local | RoomAccessState::Live)
    )
}

#[allow(dead_code)]
fn access_banner(access: Option<&RoomAccessProjection>) -> Option<&'static str> {
    match access.map(|projection| projection.state) {
        Some(RoomAccessState::Connecting) => Some("Connecting"),
        Some(RoomAccessState::Recovering) => Some("Recovering"),
        Some(RoomAccessState::Revoked) => Some("Access revoked"),
        None | Some(RoomAccessState::Local | RoomAccessState::Live) => None,
    }
}

#[allow(dead_code)]
fn agent_ids_for(access: Option<&RoomAccessProjection>, room: Option<&Room>) -> Vec<String> {
    let Some(access) = access else {
        return Vec::new();
    };
    if access.state != RoomAccessState::Local {
        return access
            .members
            .iter()
            .filter(|member| member.actor_type == FederatedActorType::Agent)
            .map(|member| member.member_id.clone())
            .collect();
    }
    room.map(|room| {
        room.participants
            .iter()
            .filter(|participant| participant.kind == RoomParticipantKind::Agent)
            .map(|participant| participant.id.clone())
            .collect()
    })
    .unwrap_or_default()
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// A short, reasonably-unique suffix for a minted identity. We don't have a UUID
/// crate in this WASM bundle, so derive one from the wall clock (`js_sys::Date`,
/// no web-sys feature needed) XOR'd with a random.
fn mint_suffix() -> String {
    let now = js_sys::Date::now();
    let rand = js_sys::Math::random();
    format!(
        "{:x}",
        (now as u64).wrapping_mul(1_000_000) ^ (rand * 1e9) as u64
    )
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
fn encode(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        }
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
    fn room_get_requires_access_and_local_projection_is_exact() {
        let response: RoomGetResponse = serde_json::from_value(serde_json::json!({
            "ok": true,
            "room": null,
            "transcript": [],
            "access": { "state": "local" }
        }))
        .expect("P1 room envelope should decode");
        assert_eq!(response.access, access_projection(RoomAccessState::Local));

        let missing = serde_json::from_value::<RoomGetResponse>(serde_json::json!({
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
    fn all_access_states_pin_write_and_banner_policy() {
        let cases = [
            (RoomAccessState::Local, true, None),
            (RoomAccessState::Connecting, false, Some("Connecting")),
            (RoomAccessState::Live, true, None),
            (RoomAccessState::Recovering, false, Some("Recovering")),
            (RoomAccessState::Revoked, false, Some("Access revoked")),
        ];

        assert!(!access_allows_writes(None));
        assert_eq!(access_banner(None), None);
        for (state, writes, banner) in cases {
            let access = access_projection(state);
            assert_eq!(access_allows_writes(Some(&access)), writes);
            assert_eq!(access_banner(Some(&access)), banner);
        }
    }

    #[test]
    fn tail_frame_decoder_tags_access_and_messages_without_cursor_blending() {
        let access =
            serde_json::to_string(&access_projection(RoomAccessState::Recovering)).unwrap();
        let frame = decode_room_tail_frame("room_access", &access).unwrap();
        assert_eq!(
            frame,
            RoomTailFrame::Access(access_projection(RoomAccessState::Recovering))
        );

        let frame = decode_room_tail_frame(
            "room_message",
            r#"{"seq":8,"author_id":"member-1","author_kind":"human","kind":"message","body":"hello","created_at":"2026-07-16T22:00:00Z"}"#,
        )
        .unwrap();
        match frame {
            RoomTailFrame::Message(message) => assert_eq!(message.seq, 8),
            RoomTailFrame::Access(_) => panic!("message frame decoded as access"),
        }
        assert!(decode_room_tail_frame("unknown", "{}").is_none());
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

    #[test]
    fn transcript_cursor_is_seeded_from_last_hydrated_sequence() {
        assert_eq!(last_transcript_seq(&[]), 0);
        assert_eq!(last_transcript_seq(&[message(3), message(9)]), 9);
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

    #[test]
    fn agent_ids_switch_strictly_between_local_and_federated_rosters() {
        let room = local_room();
        let local = access_projection(RoomAccessState::Local);
        assert_eq!(
            agent_ids_for(Some(&local), Some(&room)),
            vec!["local-agent"]
        );
        assert!(agent_ids_for(None, Some(&room)).is_empty());

        let mut federated = access_projection(RoomAccessState::Live);
        federated.members = vec![
            FederatedRoomMemberProjection {
                member_id: "opaque-agent".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::Agent,
                role_in_room: FederatedRoomRole::Member,
                display_name: "Remote Agent".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: None,
                local_binding_available: Some(false),
            },
            FederatedRoomMemberProjection {
                member_id: "opaque-user".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::User,
                role_in_room: FederatedRoomRole::Owner,
                display_name: "User".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: None,
                local_binding_available: Some(true),
            },
        ];
        assert_eq!(
            agent_ids_for(Some(&federated), Some(&room)),
            vec!["opaque-agent"]
        );
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
    fn production_room_is_current_and_reset_use_shared_predicate() {
        let rooms = Rooms {
            url: RwSignal::new(String::new()),
            list: RwSignal::new(Vec::new()),
            rooms_loaded: RwSignal::new(false),
            open_key: RwSignal::new(None),
            open_room: RwSignal::new(None),
            transcript: RwSignal::new(Vec::new()),
            status: RwSignal::new(String::new()),
            generation: RwSignal::new(0),
            identity_id: RwSignal::new("test-member"),
            identity_name: RwSignal::new("Test Member"),
            tail_state: RwSignal::new(TailState::Replaying),
            available_agents: RwSignal::new(Vec::new()),
            agents_loaded: RwSignal::new(false),
            access: RwSignal::new(None),
            create_op: RwSignal::new((0, None)),
        };

        // Not-yet-opened: no key, gen=0 → room_is_current rejects.
        assert!(!rooms.room_is_current(0, "room-1"));
        assert!(!rooms.room_is_current(13, "room-1"));

        // Manually set open_key + gen to simulate an open room.
        rooms.generation.set(42);
        rooms.open_key.set(Some("room-1".into()));
        assert!(rooms.room_is_current(42, "room-1"));
        assert!(!rooms.room_is_current(41, "room-1")); // wrong gen
        assert!(!rooms.room_is_current(42, "room-2")); // wrong key

        // Seed some state, then call reset_room_state and assert it's cleared.
        rooms.open_room.set(Some(local_room()));
        rooms.transcript.set(vec![message(1)]);
        rooms
            .access
            .set(Some(access_projection(RoomAccessState::Local)));
        rooms.tail_state.set(TailState::Live);

        rooms.reset_room_state();
        assert!(rooms.open_room.get_untracked().is_none());
        assert!(rooms.transcript.get_untracked().is_empty());
        assert!(rooms.access.get_untracked().is_none());
        assert_eq!(
            rooms.tail_state.get_untracked(),
            TailState::Replaying,
            "reset_room_state must pin tail_state to Replaying"
        );
    }
}
