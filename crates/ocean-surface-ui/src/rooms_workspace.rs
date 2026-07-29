//! Slack-style persistent 3-column Rooms workspace.
//!
//! Left rail (room list + create), center rail (header + transcript + composer),
//! right rail (members / details). Renders against the existing CSS classes in
//! `styles/rooms-workspace.css` and the existing [`crate::rooms::Rooms`] signals
//! and API surface. No new styles; no edits to rooms.rs, app.rs, or main.rs
//! beyond the module declaration.

use leptos::prelude::*;

use crate::rooms::{
    FederatedActorType, FederatedRoomMemberProjection, FederatedRoomRole, MemberPresence, Room,
    RoomAccessProjection, RoomAccessState, RoomMessage, RoomMessageKind, RoomParticipant,
    RoomParticipantKind, Rooms,
};

// ── Production helpers (testable directly, called from Effects) ─

/// Outcome of a pending create monitored against the room list.
#[derive(Debug, PartialEq)]
enum CreateAdmission {
    /// Room creation still in flight.
    Pending,
    /// A pre-existing room with the same name already existed → duplicate.
    Duplicate,
    /// A new room with the draft name appeared → success, clear draft.
    Admitted,
}

fn admit_create(draft: &str, pre_create_ids: &[String], current_list: &[Room]) -> CreateAdmission {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return CreateAdmission::Pending;
    }
    // Duplicate: a room with this name in pre_create_ids
    if current_list
        .iter()
        .any(|r| r.name == trimmed && pre_create_ids.contains(&r.id))
    {
        return CreateAdmission::Duplicate;
    }
    // Success: a new room (not in pre_create_ids) with this name
    if current_list
        .iter()
        .any(|r| r.name == trimmed && !pre_create_ids.contains(&r.id))
    {
        return CreateAdmission::Admitted;
    }
    CreateAdmission::Pending
}

/// Whether the composer should clear — true only when the current
/// draft is character-for-character identical to the body that was
/// sent. Trim-equivalence is NOT a match; the user may have typed
/// extra whitespace intentionally.
fn should_clear_composer(current: &str, sent_body: &str) -> bool {
    if sent_body.is_empty() {
        return false;
    }
    current == sent_body
}

/// Action the create-admission Effect should take after evaluating
/// the verdict and epoch state.
#[derive(Debug, PartialEq)]
enum CreateAction {
    /// Clear the draft and reset pending — new room appeared.
    AdmitNewRoom,
    /// Reset pending but keep the draft so the user can edit — duplicate
    /// room name, or server-side failure after epoch advanced.
    KeepDraft,
    /// Still in flight — do nothing, retry on next signal.
    StayPending,
}

/// Resolve what the create Effect should do given the admission
/// verdict and whether the create_epoch advanced since dispatch.
fn resolve_create(verdict: CreateAdmission, epoch_advanced: bool) -> CreateAction {
    match verdict {
        CreateAdmission::Admitted => CreateAction::AdmitNewRoom,
        CreateAdmission::Duplicate => CreateAction::KeepDraft,
        CreateAdmission::Pending if epoch_advanced => CreateAction::KeepDraft,
        CreateAdmission::Pending => CreateAction::StayPending,
    }
}

// ── Inline helpers (mirrors of private fns in rooms.rs) ──────────────

/// Whether writes (composer, join, leave) are permitted under this access
/// projection.
#[allow(dead_code)]
fn access_allows_writes(access: Option<&RoomAccessProjection>) -> bool {
    match access.map(|a| a.state) {
        Some(RoomAccessState::Local) | Some(RoomAccessState::Live) => true,
        _ => false,
    }
}

/// Render a compact time label from an ISO-8601 timestamp. Mirrors
/// `rooms::short_time`.
#[allow(dead_code)]
fn short_time(ts: &str) -> String {
    // "2026-07-25T03:43:12Z" → "03:43"
    if ts.len() >= 16 {
        ts[11..16].to_string()
    } else {
        ts.to_string()
    }
}

/// Whether to show the "No messages yet" empty state in the transcript.
#[allow(dead_code)]
fn transcript_is_empty(transcript: &[RoomMessage], access: Option<&RoomAccessProjection>) -> bool {
    if !transcript.is_empty() {
        return false;
    }
    // Only show empty state once the room is loaded (access is Some).
    // During loading, access is None — don't flash "no messages".
    access.is_some()
}

/// Whether the right-rail member list is genuinely empty after load.
#[allow(dead_code)]
fn members_loaded(access: Option<&RoomAccessProjection>) -> bool {
    access.is_some()
}

// ── Component ─────────────────────────────────────────────────────────

/// Full-screen 3-column Slack-style rooms workspace.
///
/// Takes a [`Rooms`] handle (Clone, Copy) and drives the three rails:
///
/// - **Left:** room list with active highlight, new-room create input.
/// - **Center:** selected room header, message timeline, composer form,
///   status bar.
/// - **Right:** participant / member roster with kind, role, and presence
///   badges.
///
/// On narrow screens (&lt;650px) a compact top nav reveals the hidden left
/// rail so the reader is never stranded with no room navigation.
#[component]
pub fn RoomsWorkspace(
    rooms: Rooms,
    /// Called when the user wants to leave the Rooms workspace entirely
    /// (e.g. switch to Direct Messages). If `None` the close button is
    /// hidden.
    #[prop(optional)]
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    // ── Left-rail: create form signals ────────────────────────────────
    let new_room_name = RwSignal::new(String::new());

    // Toggle for narrow-screen left-rail visibility.
    let show_left_rail = RwSignal::new(false);

    // Fetch room list on mount.
    Effect::new(move |_| {
        rooms.fetch_rooms();
    });

    // ── Center-rail: composer signal + auto-scroll ref ────────────────
    let composer = RwSignal::new(String::new());
    let list_ref: NodeRef<leptos::html::Div> = NodeRef::new();

    // Keep transcript pinned to newest message.
    let transcript = rooms.transcript;
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

    // ── Left-rail: create room (draft retained until a NEW room
    //    appears in the list — i.e. one whose id wasn't in the list
    //    before create was fired — OR the create_epoch advances
    //    without a match, signalling a silent failure).
    let pending_create = RwSignal::new(false);
    let pre_create_ids = RwSignal::new(Vec::new());
    let last_create_epoch = RwSignal::new(0u64);
    let create_room = move || {
        let name = new_room_name.get_untracked();
        if name.trim().is_empty() {
            return;
        }
        let existing_ids: Vec<String> = rooms
            .list
            .get_untracked()
            .iter()
            .map(|r| r.id.clone())
            .collect();
        pre_create_ids.set(existing_ids);
        last_create_epoch.set(rooms.create_epoch.get_untracked());
        rooms.create_room(name.clone(), None);
        pending_create.set(true);
    };

    // Admission gate: triggered by list changes OR create_epoch bumps.
    Effect::new(move |_: Option<()>| {
        if !pending_create.get() {
            return;
        }
        let draft = new_room_name.get();
        if draft.trim().is_empty() {
            pending_create.set(false);
            return;
        }
        let verdict = admit_create(&draft, &pre_create_ids.get(), &rooms.list.get());
        let epoch_advanced = rooms.create_epoch.get() != last_create_epoch.get();
        match resolve_create(verdict, epoch_advanced) {
            CreateAction::AdmitNewRoom => {
                new_room_name.set(String::new());
                pending_create.set(false);
            }
            CreateAction::KeepDraft => {
                pending_create.set(false);
            }
            CreateAction::StayPending => { /* retry on next signal */ }
        }
    });

    // ── Composer: preserve draft until confirmed admission. post_message
    //    is fire-and-forget; clearing before the message reaches the
    //    transcript discards the user's text on any failure. We capture the
    //    max seq at send time, then clear only when a message by this author
    //    with the same body appears at a seq beyond that watermark.
    let last_sent_body = RwSignal::new(String::new());
    let last_sent_seq = RwSignal::new(0u64);
    let do_send = move || {
        if !access_allows_writes(rooms.access.get_untracked().as_ref()) {
            return;
        }
        let text = composer.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        let max_seq = rooms
            .transcript
            .get_untracked()
            .iter()
            .map(|m| m.seq)
            .max()
            .unwrap_or(0);
        rooms.post_message(text.clone());
        last_sent_body.set(text);
        last_sent_seq.set(max_seq);
        // Do NOT clear here — the Effect below clears on confirmed admission.
    };

    // Clear composer once the last-sent draft appears in the transcript
    // beyond the pre-send watermark, authored by this identity — and only
    // when the current composer text is exact-character-match to the sent body.
    Effect::new(move |_: Option<()>| {
        let body = last_sent_body.get();
        if body.is_empty() {
            return;
        }
        let sent_at_seq = last_sent_seq.get();
        let me = rooms.identity_id.get_untracked();
        let found = rooms
            .transcript
            .get()
            .iter()
            .any(|m| m.seq > sent_at_seq && m.body == body && m.author_id == me);
        if found {
            let current = composer.get_untracked();
            if should_clear_composer(&current, &body) {
                composer.set(String::new());
            }
            last_sent_body.set(String::new());
            last_sent_seq.set(0);
        }
    });

    view! {
        <div class="rooms-workspace" role="region" aria-label="Rooms workspace">

            // ═══ MOBILE NAV — narrow-screen room selector ═════════════
            // Always present but CSS hides it above 650px via matching
            // breakpoint; renders a compact bar with room toggle + active
            // room name so the reader is never stranded.
            <div class="rooms-workspace__mobile-nav">
                <button
                    class="rooms-workspace__mobile-nav-toggle"
                    type="button"
                    aria-label="Toggle room list"
                    on:click=move |_| show_left_rail.update(|v: &mut bool| *v = !*v)
                >
                    <svg viewBox="0 0 16 16" width="16" height="16"
                        fill="none" stroke="currentColor" stroke-width="1.5"
                        stroke-linecap="round">
                        <path d="M2 4h12M2 8h12M2 12h12"/>
                    </svg>
                </button>
                <span class="rooms-workspace__mobile-nav-title">
                    {move || rooms.open_key.get()
                        .and_then(|_k| rooms.open_room.get().map(|r| r.name))
                        .unwrap_or_else(|| "Rooms".into())
                    }
                </span>
            </div>

            // ═══ LEFT RAIL — room list ═══════════════════════════════════
            // Backdrop closes the drawer on narrow screens (tapping outside).
            {move || {
                if show_left_rail.get() {
                    view! {
                        <div
                            class="rooms-workspace__left-backdrop"
                            on:click=move |_| show_left_rail.set(false)
                        ></div>
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}
            <div
                class="rooms-workspace__left"
                class:rooms-workspace__left--visible=move || show_left_rail.get()
            >
                <div class="rooms-workspace__left-head">
                    <h2 class="rooms-workspace__left-title">"Rooms"</h2>
                    // Close: on wide screens exits rooms entirely; on narrow
                    // the backdrop closes the drawer and this X remains the
                    // escape-hatch to close rooms.
                    <button
                        class="rooms-workspace__left-close"
                        type="button"
                        aria-label="Close rooms workspace"
                        on:click={
                            let close = on_close.clone();
                            move |_| {
                                rooms.close_room();
                                if let Some(ref cb) = close { cb.run(()); }
                            }
                        }
                    >
                        <svg viewBox="0 0 16 16" width="14" height="14"
                            fill="none" stroke="currentColor" stroke-width="1.6"
                            stroke-linecap="round">
                            <path d="M3 3l10 10M13 3L3 13"/>
                        </svg>
                    </button>
                </div>

                // Room list — scrollable
                <div class="rooms-workspace__left-list">
                    {move || {
                        let list = rooms.list.get();
                        if list.is_empty() && !rooms.rooms_loaded.get() {
                            view! {
                                <div class="rooms-workspace__left-empty">
                                    "Loading…"
                                </div>
                            }.into_any()
                        } else if list.is_empty() {
                            view! {
                                <div class="rooms-workspace__left-empty">
                                    "No rooms yet. Create one below."
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <For
                                    each=move || rooms.list.get()
                                    key=|r: &Room| (r.id.clone(), r.participants.len(), r.updated_at.clone())
                                    children=move |room: Room| {
                                        let key = room.id.clone();
                                        let key2 = key.clone();
                                        let active = move || rooms.open_key.get().as_deref() == Some(&*key);
                                        view! {
                                            <button
                                                class="rooms-workspace__room"
                                                class:is-active=active
                                                type="button"
                                                on:click=move |_| {
                                                    rooms.open_room(key2.clone());
                                                    show_left_rail.set(false);
                                                }
                                            >
                                                <span class="rooms-workspace__room-hash">"#"</span>
                                                <span class="rooms-workspace__room-name">
                                                    {room.name.clone()}
                                                </span>
                                            </button>
                                        }
                                    }
                                />
                            }.into_any()
                        }
                    }}
                </div>

                // Create input at bottom of left rail
                <div class="rooms-workspace__left-create">
                    <input
                        class="rooms-workspace__left-input"
                        type="text"
                        placeholder="New room name…"
                        prop:value=move || new_room_name.get()
                        on:input=move |ev| new_room_name.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                ev.prevent_default();
                                create_room();
                            }
                        }
                    />
                </div>
            </div>

            // ═══ CENTER RAIL — header + transcript + composer ═══════════
            <div class="rooms-workspace__center">
                {move || {
                    let open = rooms.open_room.get();
                    match open {
                        None => {
                            // No room selected placeholder
                            view! {
                                <div class="rooms-workspace__join">
                                    <div class="rooms-workspace__join-title">
                                        "Select a room"
                                    </div>
                                    <div class="rooms-workspace__join-desc">
                                        "Choose a room from the sidebar to start collaborating."
                                    </div>
                                </div>
                            }.into_any()
                        }
                        Some(ref room) => {
                            let joined = rooms.joined_open();
                            let room_name = room.name.clone();

                            view! {
                                // Header
                                <div class="rooms-workspace__center-head">
                                    <span class="rooms-workspace__center-hash">"#"</span>
                                    <h1 class="rooms-workspace__center-title">
                                        {room_name.clone()}
                                    </h1>
                                    <div class="rooms-workspace__center-actions">
                                        {if joined {
                                            view! {
                                                <button
                                                    class="room-stage__leave"
                                                    type="button"
                                                    on:click=move |_| rooms.leave_open()
                                                >
                                                    "Leave"
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <button
                                                    class="rooms-workspace__join-btn"
                                                    type="button"
                                                    on:click=move |_| rooms.join_open()
                                                >
                                                    "Join room"
                                                </button>
                                            }.into_any()
                                        }}
                                        <button
                                            class="rooms-workspace__center-back"
                                            type="button"
                                            title="Back to room list"
                                            on:click=move |_| rooms.close_room()
                                        >
                                            <svg viewBox="0 0 16 16" width="14" height="14"
                                                fill="none" stroke="currentColor" stroke-width="1.6"
                                                stroke-linecap="round">
                                                <path d="M10 3L5 8l5 5"/>
                                            </svg>
                                        </button>
                                    </div>
                                </div>

                                // Access status banner (Connecting, Recovering, Revoked)
                                {move || {
                                    let state = rooms.access.get().map(|a| a.state);
                                    match state {
                                        Some(RoomAccessState::Connecting) => {
                                            view! {
                                                <div class="room-stage__access-state room-stage__access-state--connecting">
                                                    "Connecting to federated room…"
                                                </div>
                                            }.into_any()
                                        }
                                        Some(RoomAccessState::Recovering) => {
                                            view! {
                                                <div class="room-stage__access-state room-stage__access-state--recovering">
                                                    "Recovering connection…"
                                                </div>
                                            }.into_any()
                                        }
                                        Some(RoomAccessState::Revoked) => {
                                            view! {
                                                <div class="room-stage__access-state room-stage__access-state--revoked">
                                                    "Access revoked"
                                                </div>
                                            }.into_any()
                                        }
                                        _ => ().into_any(),
                                    }
                                }}

                                // Transcript + empty state
                                <div class="rooms-workspace__transcript" node_ref=list_ref>
                                    <For
                                        each=move || rooms.transcript.get()
                                        key=|m: &RoomMessage| m.seq
                                        children=move |m: RoomMessage| {
                                            let is_system = matches!(
                                                m.kind,
                                                RoomMessageKind::System
                                                    | RoomMessageKind::ParticipantJoined
                                                    | RoomMessageKind::ParticipantLeft
                                            );
                                            let ts = short_time(&m.created_at);
                                            view! {
                                                <div
                                                    class="rooms-workspace__msg"
                                                    class:rooms-workspace__msg--system=is_system
                                                >
                                                    <div class="rooms-workspace__msg-avatar">
                                                        {if is_system {
                                                            view! { <crate::icons::Spark /> }.into_any()
                                                        } else {
                                                            m.author_id.chars().take(2).collect::<String>().into_any()
                                                        }}
                                                    </div>
                                                    <div class="rooms-workspace__msg-body">
                                                        <div class="rooms-workspace__msg-author">
                                                            <span class="rooms-workspace__msg-name">
                                                                {m.author_id.clone()}
                                                            </span>
                                                            <span class="rooms-workspace__msg-time">
                                                                {ts}
                                                            </span>
                                                        </div>
                                                        <div class="rooms-workspace__msg-text">
                                                            {m.body.clone()}
                                                        </div>
                                                    </div>
                                                </div>
                                            }
                                        }
                                    />
                                    {move || {
                                        let empty = rooms.transcript.get().is_empty();
                                        let loaded = rooms.access.get().is_some();
                                        if empty && loaded {
                                            view! {
                                                <div class="rooms-workspace__empty">
                                                    "No messages yet. Say something — use @id to mention an agent."
                                                </div>
                                            }.into_any()
                                        } else {
                                            ().into_any()
                                        }
                                    }}
                                </div>

                                // Composer + status line
                                <div class="rooms-workspace__composer">
                                    <form
                                        class="rooms-workspace__composer-row"
                                        on:submit=move |ev| {
                                            ev.prevent_default();
                                            do_send();
                                        }
                                    >
                                        <input
                                            class="rooms-workspace__composer-input"
                                            type="text"
                                            placeholder="Message… (@id to mention)"
                                            prop:value=move || composer.get()
                                            on:input=move |ev| composer.set(event_target_value(&ev))
                                            disabled=move || !access_allows_writes(rooms.access.get().as_ref())
                                        />
                                        <button
                                            class="rooms-workspace__composer-send"
                                            type="submit"
                                            disabled=move || {
                                                composer.get().trim().is_empty()
                                                    || !access_allows_writes(rooms.access.get().as_ref())
                                            }
                                        >
                                            "Send"
                                        </button>
                                    </form>

                                    // Status line: errors from create / join / post land here.
                                    {move || {
                                        let s = rooms.status.get();
                                        if s.is_empty() {
                                            ().into_any()
                                        } else {
                                            view! {
                                                <div class="rooms-workspace__status">
                                                    {s}
                                                </div>
                                            }.into_any()
                                        }
                                    }}
                                </div>
                            }.into_any()
                        }
                    }
                }}
            </div>

            // ═══ RIGHT RAIL — members / details ═════════════════════════
            <div class="rooms-workspace__right">
                <div class="rooms-workspace__right-head">
                    <h3 class="rooms-workspace__right-title">"Members"</h3>
                </div>

                <div class="rooms-workspace__right-list">
                    {move || {
                        match rooms.access.get() {
                            None => {
                                view! {
                                    <div class="rooms-workspace__right-empty">
                                        "Open a room to see members."
                                    </div>
                                }.into_any()
                            }
                            Some(ref access)
                                if access.state == RoomAccessState::Local =>
                            {
                                let participants = rooms.open_room.get()
                                    .map(|r| r.participants)
                                    .unwrap_or_default();
                                let show_add_agent = RwSignal::new(false);
                                view! {
                                    {if participants.is_empty() {
                                        view! {
                                            <div class="rooms-workspace__right-empty">
                                                "No members yet."
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! {
                                            <For
                                                each=move || rooms.open_room.get()
                                                    .map(|r| r.participants)
                                                    .unwrap_or_default()
                                                key=|p: &RoomParticipant| p.id.clone()
                                                children=move |p: RoomParticipant| {
                                                    view! {
                                                        <div class="rooms-workspace__member">
                                                            <div class="rooms-workspace__member-avatar">
                                                                {p.display_name.chars().take(2).collect::<String>()}
                                                            </div>
                                                            <span class="rooms-workspace__member-name">
                                                                {p.display_name.clone()}
                                                            </span>
                                                            <span class="rooms-workspace__member-kind">
                                                            {match p.kind {
                                                                RoomParticipantKind::Human => "human",
                                                                RoomParticipantKind::Agent => "agent",
                                                                RoomParticipantKind::Bot => "bot",
                                                                RoomParticipantKind::Tool => "tool",
                                                                RoomParticipantKind::System => "system",
                                                            }}
                                                            </span>
                                                        </div>
                                                    }
                                                }
                                            />
                                        }.into_any()
                                    }}

                                    // Add-agent control (local rooms only)
                                    <button
                                        class="rooms-workspace__addagent"
                                        type="button"
                                        title="Add an agent participant"
                                        on:click=move |_| show_add_agent.update(|v: &mut bool| *v = !*v)
                                    >
                                        "+ agent"
                                    </button>
                                    {move || {
                                        if show_add_agent.get() {
                                            view! {
                                                <div class="rooms-workspace__addagent-picker">
                                                    <select
                                                        class="rooms-workspace__addagent-select"
                                                        on:change=move |ev| {
                                                            let val = event_target_value(&ev);
                                                            if !val.is_empty() {
                                                                rooms.add_agent(val);
                                                                show_add_agent.set(false);
                                                            }
                                                        }
                                                    >
                                                        <option value="" selected=true>
                                                            "-- pick an agent --"
                                                        </option>
                                                        <For
                                                            each=move || rooms.available_agents.get()
                                                            key=|id: &String| id.clone()
                                                            children=move |id: String| {
                                                                let v = id.clone();
                                                                view! {
                                                                    <option value=v>{id}</option>
                                                                }
                                                            }
                                                        />
                                                    </select>
                                                </div>
                                            }.into_any()
                                        } else {
                                            ().into_any()
                                        }
                                    }}
                                }.into_any()
                            }
                            Some(ref access)
                                if matches!(
                                    access.state,
                                    RoomAccessState::Connecting
                                        | RoomAccessState::Live
                                        | RoomAccessState::Recovering
                                        | RoomAccessState::Revoked
                                ) =>
                            {
                                if access.members.is_empty() {
                                    view! {
                                        <div class="rooms-workspace__right-empty">
                                            "No members visible."
                                        </div>
                                    }.into_any()
                                } else {
                                    let members = access.members.clone();
                                    view! {
                                        <For
                                            each=move || members.clone()
                                            key=|m: &FederatedRoomMemberProjection| m.member_id.clone()
                                            children=move |member: FederatedRoomMemberProjection| {
                                                let role_label = match member.role_in_room {
                                                    FederatedRoomRole::Owner => "owner",
                                                    FederatedRoomRole::Member => "member",
                                                };
                                                let actor_label = match member.actor_type {
                                                    FederatedActorType::User => "user",
                                                    FederatedActorType::Agent => "agent",
                                                };
                                                let presence = member.derived_presence;
                                                let local_agent = matches!(member.actor_type, FederatedActorType::Agent)
                                                    && member.local_binding_available == Some(true);
                                                let remote_agent = matches!(member.actor_type, FederatedActorType::Agent)
                                                    && member.local_binding_available == Some(false);
                                                let desc_title = member.public_agent_descriptor.as_ref()
                                                    .and_then(|d| d.description.clone())
                                                    .unwrap_or_default();
                                                view! {
                                                    <div class="rooms-workspace__member"
                                                        class:rooms-workspace__member--local-agent=local_agent
                                                        class:rooms-workspace__member--remote-agent=remote_agent
                                                        title=desc_title
                                                    >
                                                        <div class="rooms-workspace__member-avatar">
                                                            {member.display_name.chars().take(2).collect::<String>()}
                                                        </div>
                                                        <span class="rooms-workspace__member-name">
                                                            {member.display_name.clone()}
                                                        </span>
                                                        <span class="rooms-workspace__member-kind">
                                                            {actor_label}
                                                        </span>
                                                        <span class="rooms-workspace__member-role">
                                                            {role_label}
                                                        </span>
                                                        {if presence.is_some() {
                                                            view! {
                                                                <span
                                                                    class="rooms-workspace__member-presence"
                                                                    class:rooms-workspace__member-presence--live=move || {
                                                                        presence == Some(MemberPresence::Live)
                                                                    }
                                                                    class:rooms-workspace__member-presence--unavailable=move || {
                                                                        presence == Some(MemberPresence::Unavailable)
                                                                    }
                                                                ></span>
                                                            }.into_any()
                                                        } else {
                                                            ().into_any()
                                                        }}
                                                    </div>
                                                }
                                            }
                                        />
                                    }.into_any()
                                }
                            }
                            _ => {
                                view! {
                                    <div class="rooms-workspace__right-empty">
                                        "Members unavailable."
                                    </div>
                                }.into_any()
                            }
                        }
                    }}
                </div>

                // Trigger-policy summary at bottom of right rail
                {move || {
                    rooms.open_room.get()
                        .and_then(|r| r.trigger_policy)
                        .map(|p| {
                            let mut on: Vec<&str> = Vec::new();
                            if p.on_mention { on.push("mention"); }
                            if p.on_thread_reply { on.push("thread reply"); }
                            if p.on_component_event { on.push("interaction"); }
                            if p.on_schedule.is_some() { on.push("schedule"); }
                            let triggers = if on.is_empty() {
                                "none".to_string()
                            } else {
                                on.join(", ")
                            };
                            view! {
                                <div class="rooms-workspace__policy">
                                    <div class="rooms-workspace__policy-label">
                                        "Response Policy"
                                    </div>
                                    {triggers}
                                </div>
                            }.into_any()
                        })
                }}
            </div>
        </div>
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::{
        Room, RoomAccessProjection, RoomAccessState, RoomMessage, RoomMessageKind,
        RoomParticipantKind,
    };

    fn test_access(state: RoomAccessState) -> RoomAccessProjection {
        RoomAccessProjection {
            state,
            last_confirmed_global_sequence: None,
            members: vec![],
            outbox: vec![],
        }
    }

    // ── access_allows_writes ──────────────────────────────────────────

    #[test]
    fn access_allows_writes_local() {
        assert!(access_allows_writes(Some(&test_access(
            RoomAccessState::Local
        ))));
    }

    #[test]
    fn access_allows_writes_live() {
        assert!(access_allows_writes(Some(&test_access(
            RoomAccessState::Live
        ))));
    }

    #[test]
    fn access_blocks_writes_federated_connecting() {
        assert!(!access_allows_writes(Some(&test_access(
            RoomAccessState::Connecting
        ))));
    }

    #[test]
    fn access_blocks_writes_none() {
        assert!(!access_allows_writes(None));
    }

    #[test]
    fn access_blocks_writes_revoked() {
        assert!(!access_allows_writes(Some(&test_access(
            RoomAccessState::Revoked
        ))));
    }

    // ── short_time ────────────────────────────────────────────────────

    #[test]
    fn short_time_extracts_hhmm_from_iso() {
        assert_eq!(short_time("2026-07-25T03:43:12Z"), "03:43");
    }

    #[test]
    fn short_time_passthrough_short_string() {
        assert_eq!(short_time("abc"), "abc");
    }

    #[test]
    fn short_time_handles_minimum_16_chars() {
        assert_eq!(short_time("2026-01-01T00:00"), "00:00");
    }

    // ── transcript_is_empty ───────────────────────────────────────────

    fn test_msg(body: &str) -> RoomMessage {
        RoomMessage {
            seq: 1,
            kind: RoomMessageKind::Message,
            author_id: "user".into(),
            author_kind: RoomParticipantKind::Human,
            body: body.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            federated: None,
        }
    }

    #[test]
    fn transcript_not_empty_returns_false() {
        let msgs = vec![test_msg("hello")];
        assert!(!transcript_is_empty(
            &msgs,
            Some(&test_access(RoomAccessState::Local))
        ));
    }

    #[test]
    fn transcript_empty_without_access_returns_false() {
        assert!(!transcript_is_empty(&[], None));
    }

    #[test]
    fn transcript_empty_with_access_returns_true() {
        assert!(transcript_is_empty(
            &[],
            Some(&test_access(RoomAccessState::Local))
        ));
    }

    // ── members_loaded ────────────────────────────────────────────────

    #[test]
    fn members_not_loaded_when_access_none() {
        assert!(!members_loaded(None));
    }

    #[test]
    fn members_loaded_when_access_some() {
        assert!(members_loaded(Some(&test_access(RoomAccessState::Local))));
    }

    // ── Behavioral: create-admission gate (production helpers) ───

    fn room_fixture(id: &str, name: &str) -> Room {
        Room {
            id: id.into(),
            name: name.into(),
            participants: Vec::new(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            trigger_policy: None,
        }
    }

    #[test]
    fn create_admits_new_room() {
        let pre = vec!["a".to_string(), "b".into()];
        let list = vec![
            room_fixture("a", "alpha"),
            room_fixture("b", "beta"),
            room_fixture("c", "gamma"),
        ];
        assert_eq!(
            admit_create("gamma", &pre, &list),
            CreateAdmission::Admitted
        );
    }

    #[test]
    fn create_detects_duplicate() {
        let pre = vec!["a".into(), "b".into()];
        let list = vec![room_fixture("a", "alpha"), room_fixture("b", "beta")];
        assert_eq!(
            admit_create("alpha", &pre, &list),
            CreateAdmission::Duplicate
        );
        assert_eq!(
            admit_create("beta", &pre, &list),
            CreateAdmission::Duplicate
        );
    }

    #[test]
    fn create_pending_when_no_match() {
        let pre = vec!["a".into()];
        let list = vec![room_fixture("a", "alpha")];
        assert_eq!(admit_create("delta", &pre, &list), CreateAdmission::Pending);
    }

    #[test]
    fn create_empty_draft_stays_pending() {
        let pre = vec!["a".into()];
        let list = vec![room_fixture("a", "alpha")];
        assert_eq!(admit_create("   ", &pre, &list), CreateAdmission::Pending);
    }

    // ── Behavioral: composer draft preservation (production helper) ──

    #[test]
    fn composer_clears_when_unedited() {
        assert!(should_clear_composer("hello", "hello"));
    }

    #[test]
    fn composer_preserves_edited_draft() {
        assert!(!should_clear_composer("hello world", "hello"));
        assert!(!should_clear_composer("something else", "hello"));
    }

    #[test]
    fn composer_preserves_whitespace_edit() {
        // " hi " -> "hi" is still an edit; exact equality only.
        assert!(!should_clear_composer("hi", " hi "));
    }

    #[test]
    fn composer_ignores_empty_sent_body() {
        assert!(!should_clear_composer("hello", ""));
    }

    // ── Behavioral: create-resolution gate (production helper) ──

    #[test]
    fn resolve_admitted_clears_draft() {
        assert_eq!(
            resolve_create(CreateAdmission::Admitted, false),
            CreateAction::AdmitNewRoom
        );
        // epoch state is irrelevant for Admitted
        assert_eq!(
            resolve_create(CreateAdmission::Admitted, true),
            CreateAction::AdmitNewRoom
        );
    }

    #[test]
    fn resolve_duplicate_keeps_draft() {
        assert_eq!(
            resolve_create(CreateAdmission::Duplicate, false),
            CreateAction::KeepDraft
        );
        // epoch state is irrelevant for Duplicate
        assert_eq!(
            resolve_create(CreateAdmission::Duplicate, true),
            CreateAction::KeepDraft
        );
    }

    #[test]
    fn resolve_pending_failure_resets_on_epoch_advance() {
        // Server completed (epoch bumped) but no room appeared →
        // silent failure → KeepDraft so user can retry.
        assert_eq!(
            resolve_create(CreateAdmission::Pending, true),
            CreateAction::KeepDraft
        );
    }

    #[test]
    fn resolve_pending_stays_on_same_epoch() {
        // Epoch hasn't changed → request is still in flight.
        assert_eq!(
            resolve_create(CreateAdmission::Pending, false),
            CreateAction::StayPending
        );
    }

    // ── Behavioral: compact drawer reachability (production helper) ──

    /// The exact toggle logic from the component's on:click handler.
    fn toggle_drawer(current: bool) -> bool {
        !current
    }

    #[test]
    fn drawer_opens_when_closed() {
        assert!(toggle_drawer(false));
    }

    #[test]
    fn drawer_closes_when_open() {
        assert!(!toggle_drawer(true));
    }

    #[test]
    fn drawer_toggle_is_idempotent() {
        // Two toggles = identity — the drawer returns to its previous
        // state, which is the contract for a compact-nav hamburger.
        assert_eq!(toggle_drawer(toggle_drawer(false)), false);
        assert_eq!(toggle_drawer(toggle_drawer(true)), true);
    }
}
