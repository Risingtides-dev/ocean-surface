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
//!   GET    /v1/rooms/persistent/{key}/transcript?after_seq=N → live tail
//!
//! Live updates: the daemon emits an unscoped `room_trigger` extension event on
//! the agent event bus when an @-mention auto-convenes (OCEAN-65). That frame is
//! council-wide (`scope: None`), so it only reaches the `?all=1` firehose — the
//! main session stream in `daemon.rs` drops unscoped events on purpose. We open
//! our own `?all=1` listener here (mirroring `connect_permission_stream`) scoped
//! to the open room, and additionally poll the transcript on a short interval
//! while a room is open so new messages from any author tail in even when no
//! trigger fires. Both paths request `after_seq` so we only append new entries.
//!
//! The whole module is self-contained — it carries its own request layer rather
//! than threading rooms state through the `Daemon` handle — so it never touches
//! the live agent loop / session SSE code.

use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use gloo_net::http::Request;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;

/// How often (ms) we re-poll the open room's transcript for new entries. SSE
/// `room_trigger` only fires on auto-convene; this catches plain messages from
/// other participants too. Cheap: `after_seq` means each poll returns only the
/// tail since the last seq we hold.
const TRANSCRIPT_POLL_MS: u32 = 2_500;

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
    /// A short glyph for the author/roster chip.
    fn glyph(self) -> &'static str {
        match self {
            RoomParticipantKind::Human => "🧑",
            RoomParticipantKind::Agent => "🤖",
            RoomParticipantKind::Bot => "⚙",
            RoomParticipantKind::Tool => "🔧",
            RoomParticipantKind::System => "✦",
        }
    }

    /// A lowercase word for the kind — shown next to the glyph so the roster makes
    /// it explicit who's an agent (i.e. auto-convene-able) vs. a human.
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

/// The unscoped `room_trigger` extension frame the daemon emits on auto-convene
/// (OCEAN-65). We only need the room key to know which room to re-tail.
#[derive(Debug, Clone, Deserialize)]
struct RoomTriggerPayload {
    #[serde(default)]
    room: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AllStreamEvent {
    Extension {
        #[serde(default)]
        extension: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
    #[serde(other)]
    Other,
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

/// Reactive handle for the rooms panel. Holds the room list, the open room +
/// its transcript, status text, and the SSE/poll generation counter. Cloned
/// freely (all fields are `Copy` signal handles), like [`crate::daemon::Daemon`].
#[derive(Clone, Copy)]
pub struct Rooms {
    /// Daemon base URL signal, shared with `Daemon::url` so requests follow the
    /// origin learned at bootstrap (phone-via-tunnel resolves it asynchronously,
    /// so we must read it live at request time, not snapshot it at construction).
    pub url: RwSignal<String>,
    /// All persistent rooms (from `GET /v1/rooms/persistent`).
    pub list: RwSignal<Vec<Room>>,
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
    /// The LiveKit room id for the current call (shared with Daemon/LiveKitPanel).
    pub livekit_room_id: RwSignal<String>,
    /// The LiveKit token path for the current call (shared with Daemon/LiveKitPanel).
    pub livekit_token_path: RwSignal<String>,
    /// Whether the LiveKit controls utility row / room stage is visible.
    pub show_livekit_controls: RwSignal<bool>,
    /// Whether the rooms browse panel itself is open.
    pub panel_open: RwSignal<bool>,
}

impl Rooms {
    /// Construct a rooms handle that shares the live `Daemon::url` signal, so it
    /// always targets the origin resolved by bootstrap. Accepts the shared
    /// LiveKit signals from the daemon + app shell so room join/leave/close can
    /// route calls and toggle the utility row.
    pub fn new(
        daemon: &crate::daemon::Daemon,
        livekit_room_id: RwSignal<String>,
        livekit_token_path: RwSignal<String>,
        show_livekit_controls: RwSignal<bool>,
        panel_open: RwSignal<bool>,
    ) -> Self {
        let identity = RoomIdentity::current();
        // Leak the small, app-lifetime identity strings to obtain `&'static str`
        // signals, so the panel can pass them into request closures without a
        // per-call clone.
        let id_static: &'static str = Box::leak(identity.id.into_boxed_str());
        let name_static: &'static str = Box::leak(identity.display_name.into_boxed_str());
        Self {
            url: daemon.url,
            list: RwSignal::new(Vec::new()),
            open_key: RwSignal::new(None),
            open_room: RwSignal::new(None),
            transcript: RwSignal::new(Vec::new()),
            status: RwSignal::new(String::new()),
            generation: RwSignal::new(0),
            identity_id: RwSignal::new(id_static),
            identity_name: RwSignal::new(name_static),
            livekit_room_id,
            livekit_token_path,
            show_livekit_controls,
            panel_open,
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
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
        });
    }

    /// Create a room (`POST /v1/rooms/persistent`) with an optional auto-convene
    /// `trigger_policy`, then select it. The daemon keys rooms by `key`; we
    /// derive a url-safe key from the name but keep the human name intact.
    pub fn create_room(&self, name: String, policy: Option<RoomTriggerPolicy>) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let key = slugify(&name);
        if key.is_empty() {
            self.status
                .set("room name needs at least one letter/number".into());
            return;
        }
        let base = self.base();
        let me = *self;
        let status = self.status;
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
                    return;
                }
            };
            match res {
                Ok(resp) => match resp.json::<RoomMutateResponse>().await {
                    Ok(r) if r.ok => {
                        status.set(format!("room '{name}' created"));
                        // Refresh the list and open the new room.
                        me.fetch_rooms();
                        me.open_room(key.clone());
                    }
                    Ok(r) => status.set(format!(
                        "create failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => status.set(format!("create decode error: {err}")),
                },
                Err(err) => status.set(format!("create post error: {err}")),
            }
        });
    }

    /// Open a room: load its record + full transcript, bump the generation, and
    /// start the live tail (SSE `?all=1` + transcript poll).
    pub fn open_room(&self, key: String) {
        let base = self.base();
        let me = *self;
        let open_key = self.open_key;
        let open_room = self.open_room;
        let transcript = self.transcript;
        let status = self.status;
        let generation = self.generation;

        // Retire any prior room's live loops.
        let gen = generation.get_untracked().wrapping_add(1);
        generation.set(gen);
        open_key.set(Some(key.clone()));
        open_room.set(None);
        transcript.set(Vec::new());
        status.set("loading room…".into());
        // Entering a room takes the stage (RoomStage swaps in for the chat
        // surface), so the browser panel folds away.
        self.panel_open.set(false);

        spawn_local(async move {
            let get_url = format!("{base}/v1/rooms/persistent/{}", encode(&key));
            match Request::get(&get_url).send().await {
                Ok(resp) => match resp.json::<RoomGetResponse>().await {
                    Ok(r) if r.ok => {
                        // Guard against a fast re-select before this landed.
                        if generation.get_untracked() != gen {
                            return;
                        }
                        open_room.set(r.room);
                        transcript.set(r.transcript);
                        status.set(String::new());
                        // Start live updates for this generation.
                        me.start_live_tail(key.clone(), gen);
                    }
                    Ok(r) => status.set(format!(
                        "room load failed: {}",
                        r.error.unwrap_or_else(|| "unknown error".into())
                    )),
                    Err(err) => status.set(format!("room decode error: {err}")),
                },
                Err(err) => status.set(format!("room fetch error: {err}")),
            }
        });
    }
    /// Close the open room and stop its live loops.
    pub fn close_room(&self) {
        let closing_key = self.open_key.get_untracked();
        self.generation.update(|g| *g = g.wrapping_add(1));
        self.open_key.set(None);
        self.open_room.set(None);
        self.transcript.set(Vec::new());
        if let Some(key) = &closing_key {
            if clear_livekit_room_call_if_current(
                key,
                &self.livekit_room_id,
                &self.livekit_token_path,
            ) {
                self.show_livekit_controls.set(false);
                crate::livekit::disconnect_livekit_bridge();
            }
        }
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
                            route_livekit_room_call(
                                &key,
                                &me.livekit_room_id,
                                &me.livekit_token_path,
                            );
                            me.show_livekit_controls.set(true);
                            me.panel_open.set(false);
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

    /// Add an **agent** participant to the open room
    /// (`POST .../participants` with `kind = agent`). Once present, the agent's
    /// id is mentionable (`@id`) and — if the room's trigger policy has
    /// `on_mention` — auto-convenes when mentioned (OCEAN-111). The daemon's
    /// `room_join` route accepts the `kind` field directly, so this needs no
    /// daemon change.
    pub fn add_agent(&self, agent_id: String, display_name: String) {
        let agent_id = agent_id.trim().to_string();
        if agent_id.is_empty() {
            self.status.set("agent id required".into());
            return;
        }
        let display_name = {
            let trimmed = display_name.trim();
            if trimmed.is_empty() {
                agent_id.clone()
            } else {
                trimmed.to_string()
            }
        };
        let Some(key) = self.open_key.get_untracked() else {
            return;
        };
        let base = self.base();
        let me = *self;
        let status = self.status;
        spawn_local(async move {
            let body = JoinBody {
                id: &agent_id,
                display_name: &display_name,
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
    pub fn agent_ids(&self) -> Vec<String> {
        self.open_room
            .get()
            .map(|r| {
                r.participants
                    .iter()
                    .filter(|p| p.kind == RoomParticipantKind::Agent)
                    .map(|p| p.id.clone())
                    .collect()
            })
            .unwrap_or_default()
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
                        if clear_livekit_room_call_if_current(
                            &key,
                            &me.livekit_room_id,
                            &me.livekit_token_path,
                        ) {
                            me.show_livekit_controls.set(false);
                            crate::livekit::disconnect_livekit_bridge();
                        }
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

    /// Re-fetch the open room's transcript tail (`after_seq` = our highest seq)
    /// and append only new entries. Used after our own writes and by the poll /
    /// SSE live tail.
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

    /// Start the live tail for `key` at generation `gen`: a transcript poll loop
    /// and an `?all=1` SSE listener for `room_trigger`. Both stop when the
    /// generation advances (room change / panel close).
    fn start_live_tail(&self, key: String, gen: u64) {
        // 1) Transcript poll — catches every author's messages, not just
        //    auto-convene. Cheap via `after_seq`.
        {
            let me = *self;
            let generation = self.generation;
            let key = key.clone();
            spawn_local(async move {
                loop {
                    gloo_timers::future::TimeoutFuture::new(TRANSCRIPT_POLL_MS).await;
                    if generation.get_untracked() != gen {
                        break;
                    }
                    me.refresh_open_transcript(&key);
                }
            });
        }

        // 2) `?all=1` SSE — the daemon's unscoped `room_trigger` frame fires on
        //    @-mention auto-convene. When one names our open room, re-tail
        //    immediately (don't wait for the poll). Mirrors the per-name
        //    subscription pattern in daemon.rs's permission stream.
        {
            let me = *self;
            let generation = self.generation;
            let base = self.base();
            let key = key.clone();
            spawn_local(async move {
                let events_url = format!("{base}/v1/agent/events?all=1");
                loop {
                    if generation.get_untracked() != gen {
                        break;
                    }
                    let mut es = match EventSource::new(&events_url) {
                        Ok(es) => es,
                        Err(_) => {
                            gloo_timers::future::TimeoutFuture::new(2_000).await;
                            continue;
                        }
                    };
                    let sub = match es.subscribe("extension") {
                        Ok(s) => s,
                        Err(_) => {
                            gloo_timers::future::TimeoutFuture::new(2_000).await;
                            continue;
                        }
                    };
                    let mut stream = sub;
                    while let Some(msg) = stream.next().await {
                        if generation.get_untracked() != gen {
                            break;
                        }
                        let Ok((_name, msg)) = msg else { continue };
                        let Some(data) = msg.data().as_string() else {
                            continue;
                        };
                        let Ok(evt) = serde_json::from_str::<AllStreamEvent>(&data) else {
                            continue;
                        };
                        if let AllStreamEvent::Extension { extension, payload } = evt {
                            if extension != "room_trigger" {
                                continue;
                            }
                            // Only react to triggers for the open room.
                            if let Ok(p) = serde_json::from_value::<RoomTriggerPayload>(payload) {
                                if p.room == key {
                                    me.refresh_open_transcript(&key);
                                }
                            }
                        }
                    }
                    if generation.get_untracked() != gen {
                        break;
                    }
                    gloo_timers::future::TimeoutFuture::new(1_000).await;
                }
            });
        }
    }
}

// ---- Helpers ----------------------------------------------------------------

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
fn short_time(ts: &str) -> String {
    if ts.is_empty() {
        return String::new();
    }
    // "2026-06-05T12:34:56.789Z" → "2026-06-05 12:34"
    let trimmed = ts.split('.').next().unwrap_or(ts).replace('T', " ");
    trimmed.chars().take(16).collect()
}


// ---- LiveKit room-call routing helpers ------------------------------------

/// Build the per-room LiveKit token path, percent-encoding the room key
/// exactly like the proxy: `/v1/rooms/{encoded}/livekit-token`.
fn livekit_token_path_for_room(key: &str) -> String {
    format!("/v1/rooms/{}/livekit-token", encode(key))
}

/// Set the LiveKit room id + token path signals for a room key.
fn route_livekit_room_call(
    key: &str,
    room_id: &RwSignal<String>,
    token_path: &RwSignal<String>,
) {
    room_id.set(key.to_string());
    token_path.set(livekit_token_path_for_room(key));
}

/// Clear the LiveKit room id + token path signals only if they match `key`.
fn clear_livekit_room_call_if_current(
    key: &str,
    room_id: &RwSignal<String>,
    token_path: &RwSignal<String>,
) -> bool {
    if room_id.get_untracked() == key {
        room_id.set(String::new());
        token_path.set(String::new());
        true
    } else {
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use leptos::prelude::{Get, RwSignal};

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
    fn route_livekit_room_call_sets_room_key_and_token_path_signals() {
        let room_key: RwSignal<String> = RwSignal::new(String::new());
        let token_path: RwSignal<String> = RwSignal::new(String::new());

        route_livekit_room_call("my-room", &room_key, &token_path);

        assert_eq!(room_key.get(), "my-room");
        assert_eq!(token_path.get(), livekit_token_path_for_room("my-room"));
    }

    #[test]
    fn clear_livekit_room_call_clears_matching_room() {
        let room_key: RwSignal<String> = RwSignal::new("room-a".to_string());
        let token_path: RwSignal<String> =
            RwSignal::new(livekit_token_path_for_room("room-a"));

        clear_livekit_room_call_if_current("room-a", &room_key, &token_path);

        assert_eq!(room_key.get(), "");
        assert_eq!(token_path.get(), "");
    }

    #[test]
    fn clear_livekit_room_call_preserves_non_matching_room() {
        let room_key: RwSignal<String> = RwSignal::new("room-a".to_string());
        let token_path: RwSignal<String> =
            RwSignal::new(livekit_token_path_for_room("room-a"));

        clear_livekit_room_call_if_current("room-b", &room_key, &token_path);

        assert_eq!(room_key.get(), "room-a");
        assert_eq!(token_path.get(), livekit_token_path_for_room("room-a"));
    }
}

// ---- View -------------------------------------------------------------------

/// Rooms browser panel — slides in from the right (same overlay pattern as the
/// sessions panel). Lists persistent rooms + create-with-policy; selecting a
/// room ENTERS it: the panel closes and [`RoomStage`] takes over the main
/// surface (rooms are a mode, not a drawer).
#[component]
pub fn RoomsPanel(rooms: Rooms, open: RwSignal<bool>) -> impl IntoView {
    // Fetch the room list whenever the panel opens.
    Effect::new(move |_| {
        if open.get() {
            rooms.fetch_rooms();
        }
    });

    let is_open = move || open.get();
    let new_room_name = RwSignal::new(String::new());

    // ---- Trigger-policy toggles for room creation ---------------------------
    // `on_mention` defaults on (the common auto-convene case); the rest default
    // off. `on_schedule` is a free-form cron string (empty = no schedule).
    let policy_on_mention = RwSignal::new(true);
    let policy_on_thread_reply = RwSignal::new(false);
    let policy_on_component_event = RwSignal::new(false);
    let policy_on_schedule = RwSignal::new(String::new());

    // Assemble the trigger policy from the toggles, or `None` if nothing is set
    // (so the daemon stores no policy rather than an all-off one).
    let collect_policy = move || -> Option<RoomTriggerPolicy> {
        let cron = policy_on_schedule.get_untracked().trim().to_string();
        let on_schedule = if cron.is_empty() { None } else { Some(cron) };
        let policy = RoomTriggerPolicy {
            on_mention: policy_on_mention.get_untracked(),
            on_thread_reply: policy_on_thread_reply.get_untracked(),
            on_component_event: policy_on_component_event.get_untracked(),
            on_schedule,
        };
        if policy == RoomTriggerPolicy::default() {
            None
        } else {
            Some(policy)
        }
    };


    let room_list = rooms.list;
    let status = rooms.status;

    view! {
        <div
            class="rooms-overlay"
            class:rooms-overlay--open=is_open
            on:click=move |ev| {
                let target = event_target::<web_sys::HtmlElement>(&ev);
                if target.class_list().contains("rooms-overlay") {
                    open.set(false);
                }
            }
        >
            <div class="rooms-panel">
                <div class="rooms-panel__head">
                    <h2 class="rooms-panel__title">"Rooms"</h2>
                    <button
                        class="rooms-panel__close"
                        type="button"
                        aria-label="close rooms panel"
                        on:click=move |_| open.set(false)
                    >
                        "✕"
                    </button>
                </div>

                // ---- List view (no room open) -------------------------------
                    <div class="rooms-panel__create">
                        <input
                            class="rooms-panel__create-input"
                            type="text"
                            placeholder="New room name…"
                            prop:value=move || new_room_name.get()
                            on:input=move |ev| new_room_name.set(event_target_value(&ev))
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" {
                                    ev.prevent_default();
                                    let name = new_room_name.get_untracked();
                                    rooms.create_room(name, collect_policy());
                                    new_room_name.set(String::new());
                                }
                            }
                        />
                        <button
                            class="rooms-panel__create-btn"
                            type="button"
                            on:click=move |_| {
                                let name = new_room_name.get_untracked();
                                rooms.create_room(name, collect_policy());
                                new_room_name.set(String::new());
                            }
                        >
                            "+ Create"
                        </button>
                    </div>

                    // Trigger-policy toggles applied at room creation (OCEAN-117).
                    // These wire into the daemon's `room_create` body; there is no
                    // room-update route yet, so policy is set once at create time.
                    <div class="rooms-policy">
                        <div class="rooms-policy__title">
                            "Auto-convene triggers"
                        </div>
                        <label class="rooms-policy__row">
                            <input
                                type="checkbox"
                                prop:checked=move || policy_on_mention.get()
                                on:change=move |ev| policy_on_mention.set(event_target_checked(&ev))
                            />
                            <span>"On @mention"</span>
                            <span class="rooms-policy__hint">"wake a mentioned agent"</span>
                        </label>
                        <label class="rooms-policy__row">
                            <input
                                type="checkbox"
                                prop:checked=move || policy_on_thread_reply.get()
                                on:change=move |ev| policy_on_thread_reply.set(event_target_checked(&ev))
                            />
                            <span>"On thread reply"</span>
                        </label>
                        <label class="rooms-policy__row">
                            <input
                                type="checkbox"
                                prop:checked=move || policy_on_component_event.get()
                                on:change=move |ev| policy_on_component_event.set(event_target_checked(&ev))
                            />
                            <span>"On component event"</span>
                        </label>
                        <label class="rooms-policy__row rooms-policy__row--cron">
                            <span>"On schedule (cron)"</span>
                            <input
                                class="rooms-policy__cron"
                                type="text"
                                placeholder="e.g. 0 9 * * *"
                                prop:value=move || policy_on_schedule.get()
                                on:input=move |ev| policy_on_schedule.set(event_target_value(&ev))
                            />
                        </label>
                    </div>

                    <div class="rooms-panel__list">
                        <For
                            each=move || room_list.get()
                            key=|r| (r.id.clone(), r.participants.len(), r.updated_at.clone())
                            children=move |room: Room| {
                                let key = room.id.clone();
                                let count = room.participants.len();
                                let last = short_time(&room.updated_at);
                                view! {
                                    <button
                                        class="rooms-item"
                                        type="button"
                                        on:click=move |_| rooms.open_room(key.clone())
                                    >
                                        <div class="rooms-item__name">{room.name.clone()}</div>
                                        <div class="rooms-item__meta">
                                            <span class="rooms-item__count">
                                                {format!("{count} participant{}", if count == 1 { "" } else { "s" })}
                                            </span>
                                            <Show when={
                                                let last = last.clone();
                                                move || !last.is_empty()
                                            }>
                                                <span class="rooms-item__time">{last.clone()}</span>
                                            </Show>
                                        </div>
                                    </button>
                                }
                            }
                        />
                    </div>

                    <Show when=move || room_list.get().is_empty()>
                        <div class="rooms-panel__empty">
                            "No rooms yet. Create one above to start collaborating."
                        </div>
                    </Show>


                // Status line (errors / notices).
                <Show when=move || !status.get().is_empty()>
                    <div class="rooms-panel__status">{move || status.get()}</div>
                </Show>
            </div>
        </div>
    }
}

/// Full-surface room mode: entering a room from the browser panel promotes it
/// to the main stage — the chat transcript/composer swap out and the room's
/// own roster, transcript, and composer take the surface over (a room is a
/// mode of operation you enter, not a drawer you peek at). Works with ZERO
/// LiveKit configuration: the text room is daemon-native
/// (`/v1/rooms/persistent/*`); the call strip above the stage upgrades the
/// room to audio when LiveKit credentials exist.
#[component]
pub fn RoomStage(rooms: Rooms) -> impl IntoView {
    let composer = RwSignal::new(String::new());
    let new_agent_id = RwSignal::new(String::new());
    let new_agent_name = RwSignal::new(String::new());
    // Add-agent inputs are power-user chrome: hidden behind the "+ agent"
    // ghost chip until asked for (control density: reveal-on-intent).
    let show_add_agent = RwSignal::new(false);

    let open_room = rooms.open_room;
    let transcript = rooms.transcript;
    let status = rooms.status;

    let submit_agent = move || {
        rooms.add_agent(
            new_agent_id.get_untracked(),
            new_agent_name.get_untracked(),
        );
        new_agent_id.set(String::new());
        new_agent_name.set(String::new());
        show_add_agent.set(false);
    };

    // Keep the transcript pinned to the newest message: jump to the bottom
    // when the room's history first fills, and follow new messages tailing in
    // unless the reader has scrolled up into history (same stick pattern as
    // the chat transcript). The effect returns the seen length so the first
    // fill is distinguishable from a live append.
    let list_ref: NodeRef<leptos::html::Div> = NodeRef::new();
    Effect::new(move |prev: Option<usize>| {
        let len = transcript.with(|t| t.len());
        if len > 0 {
            if let Some(el) = list_ref.get() {
                let first_fill = prev.unwrap_or(0) == 0;
                let near_bottom = el.scroll_height() - el.scroll_top() - el.client_height() < 120;
                if first_fill || near_bottom {
                    request_animation_frame(move || el.set_scroll_top(el.scroll_height()));
                }
            }
        }
        len
    });

    view! {
        <div class="room-stage">
            <div class="room-stage__head">
                <button
                    class="room-stage__back"
                    type="button"
                    title="Back to rooms"
                    on:click=move |_| {
                        rooms.close_room();
                        rooms.panel_open.set(true);
                    }
                >
                    "‹ Rooms"
                </button>
                <h2 class="room-stage__title">
                    {move || open_room.get().map(|r| r.name).unwrap_or_default()}
                </h2>
                <Show
                    when=move || rooms.joined_open()
                    fallback=move || view! {
                        <button
                            class="room-stage__join"
                            type="button"
                            on:click=move |_| rooms.join_open()
                        >
                            "Join room"
                        </button>
                    }
                >
                    <button
                        class="room-stage__leave"
                        type="button"
                        on:click=move |_| rooms.leave_open()
                    >
                        "Leave"
                    </button>
                </Show>
            </div>

            // Roster: kind-tinted chips + reveal-on-intent add-agent.
            <div class="room-stage__roster">
                <For
                    each=move || open_room.get().map(|r| r.participants).unwrap_or_default()
                    key=|p| p.id.clone()
                    children=move |p: RoomParticipant| {
                        let is_agent = p.kind == RoomParticipantKind::Agent;
                        view! {
                            <span
                                class="rooms-chip"
                                class:rooms-chip--agent=is_agent
                                title=format!("{} ({})", p.id, p.kind.label())
                            >
                                <span class="rooms-chip__glyph">{p.kind.glyph()}</span>
                                {p.display_name.clone()}
                                <span class="rooms-chip__kind">{p.kind.label()}</span>
                            </span>
                        }
                    }
                />
                <button
                    class="room-stage__addagent-toggle"
                    type="button"
                    title="Add an agent participant"
                    on:click=move |_| show_add_agent.update(|v| *v = !*v)
                >
                    "+ agent"
                </button>
            </div>

            <Show when=move || show_add_agent.get()>
                <div class="rooms-addagent">
                    <input
                        class="rooms-addagent__input"
                        type="text"
                        placeholder="agent id (e.g. flux)"
                        prop:value=move || new_agent_id.get()
                        on:input=move |ev| new_agent_id.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                ev.prevent_default();
                                submit_agent();
                            }
                        }
                    />
                    <input
                        class="rooms-addagent__input"
                        type="text"
                        placeholder="display name (optional)"
                        prop:value=move || new_agent_name.get()
                        on:input=move |ev| new_agent_name.set(event_target_value(&ev))
                    />
                    <button
                        class="rooms-addagent__btn"
                        type="button"
                        disabled=move || new_agent_id.get().trim().is_empty()
                        on:click=move |_| submit_agent()
                    >
                        "Add agent"
                    </button>
                </div>
            </Show>

            // Trigger-policy summary — read-only (no daemon room-update route).
            <Show when=move || {
                open_room.get().and_then(|r| r.trigger_policy).is_some()
            }>
                <div class="rooms-policy-summary">
                    {move || {
                        let p = open_room.get().and_then(|r| r.trigger_policy)
                            .unwrap_or_default();
                        let mut on: Vec<&str> = Vec::new();
                        if p.on_mention { on.push("@mention"); }
                        if p.on_thread_reply { on.push("thread-reply"); }
                        if p.on_component_event { on.push("component-event"); }
                        if p.on_schedule.is_some() { on.push("schedule"); }
                        let triggers = if on.is_empty() {
                            "none".to_string()
                        } else {
                            on.join(", ")
                        };
                        format!("Auto-convene: {triggers}")
                    }}
                </div>
            </Show>

            // Transcript — the main column.
            <div class="room-stage__transcript" node_ref=list_ref>
                <For
                    each=move || transcript.get()
                    key=|m| m.seq
                    children=move |m: RoomMessage| {
                        let is_system = matches!(
                            m.kind,
                            RoomMessageKind::System
                                | RoomMessageKind::ParticipantJoined
                                | RoomMessageKind::ParticipantLeft
                        );
                        view! {
                            <div
                                class="rooms-msg"
                                class:rooms-msg--system=is_system
                            >
                                <Show when=move || !is_system>
                                    <div class="rooms-msg__author">
                                        <span class="rooms-msg__glyph">
                                            {m.author_kind.glyph()}
                                        </span>
                                        {m.author_id.clone()}
                                    </div>
                                </Show>
                                <div class="rooms-msg__body">{m.body.clone()}</div>
                            </div>
                        }
                    }
                />
                <Show when=move || transcript.get().is_empty()>
                    <div class="room-stage__empty">
                        "No messages yet. Say something — use @id to convene an agent."
                    </div>
                </Show>
            </div>

            <div class="room-stage__foot">
                // @mention discoverability: click a chip to insert `@id `.
                <Show when=move || !rooms.agent_ids().is_empty()>
                    <div class="rooms-mention-hint">
                        <span class="rooms-mention-hint__label">"@agents:"</span>
                        <For
                            each=move || rooms.agent_ids()
                            key=|id| id.clone()
                            children=move |id: String| {
                                let insert = id.clone();
                                view! {
                                    <button
                                        class="rooms-mention-hint__chip"
                                        type="button"
                                        title="insert mention"
                                        on:click=move |_| {
                                            composer.update(|c| {
                                                if !c.is_empty() && !c.ends_with(' ') {
                                                    c.push(' ');
                                                }
                                                c.push('@');
                                                c.push_str(&insert);
                                                c.push(' ');
                                            });
                                        }
                                    >
                                        {format!("@{id}")}
                                    </button>
                                }
                            }
                        />
                    </div>
                </Show>

                <form
                    class="rooms-composer"
                    on:submit=move |ev| {
                        ev.prevent_default();
                        let text = composer.get_untracked();
                        rooms.post_message(text);
                        composer.set(String::new());
                    }
                >
                    <input
                        class="rooms-composer__input"
                        type="text"
                        placeholder="Message… (@id to mention)"
                        prop:value=move || composer.get()
                        on:input=move |ev| composer.set(event_target_value(&ev))
                    />
                    <button
                        class="rooms-composer__send"
                        type="submit"
                        disabled=move || composer.get().trim().is_empty()
                    >
                        "Send"
                    </button>
                </form>

                <Show when=move || !status.get().is_empty()>
                    <div class="rooms-panel__status">{move || status.get()}</div>
                </Show>
            </div>
        </div>
    }
}
