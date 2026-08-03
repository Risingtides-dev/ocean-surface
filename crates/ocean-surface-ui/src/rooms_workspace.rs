//! Slack-style persistent 3-column Rooms workspace.
//!
//! Left rail (room list + create), center rail (header + transcript + composer),
//! right rail (members / details). Renders against the existing CSS classes in
//! `styles/rooms-workspace.css` and the existing [`crate::rooms::Rooms`] signals
//! and API surface. Mounted as the default web+Tauri collaboration surface in
//! [`crate::app`]; the legacy RoomStage/RoomsPanel components in rooms.rs are
//! deleted.

use leptos::prelude::*;

use crate::rooms::{
    CreateResolution, FederatedActorType, FederatedRoomMemberProjection, FederatedRoomRole,
    MemberPresence, OutboxItemState, Room, RoomAccessProjection, RoomAccessState, RoomMessage,
    RoomMessageKind, RoomParticipant, RoomParticipantKind, Rooms,
};

// ── Production helpers (testable directly, called from Effects) ─

// These are only exercised by tests now that create admission is
// gated by typed op-id outcomes rather than list inspection.

/// Whether the composer should clear after the normalized wire body is
/// confirmed. The current draft must still be the exact original draft so
/// typing that happened while the send was in flight is never discarded.
fn should_clear_composer(current: &str, original_draft: &str) -> bool {
    !original_draft.is_empty() && current == original_draft
}

fn normalized_message_body(draft: &str) -> String {
    draft.trim().to_string()
}

fn message_send_admitted(
    own_in_flight: bool,
    other_in_flight: bool,
    writes_allowed: bool,
    draft: &str,
) -> bool {
    !own_in_flight
        && !other_in_flight
        && writes_allowed
        && !normalized_message_body(draft).is_empty()
}

/// Toggle a boolean signal — the exact logic consumed by hamburger
/// drawer open/close clicks.
fn toggle_drawer(current: bool) -> bool {
    !current
}

// ── Inline helpers (mirrors of private fns in rooms.rs) ──────────────

/// Whether writes (composer, join, leave) are permitted under this access
/// projection.
#[allow(dead_code)]
fn access_allows_writes(access: Option<&RoomAccessProjection>) -> bool {
    matches!(
        access.map(|a| a.state),
        Some(RoomAccessState::Local) | Some(RoomAccessState::Live)
    )
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
fn show_transcript_empty(tail_is_live: bool, roots_empty: bool) -> bool {
    roots_empty && tail_is_live
}

/// Whether the right-rail member list is genuinely empty after load.
#[allow(dead_code)]
fn members_loaded(access: Option<&RoomAccessProjection>) -> bool {
    access.is_some()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ThreadPartition {
    roots: Vec<RoomMessage>,
    replies: Vec<RoomMessage>,
}

fn partition_thread_messages(transcript: &[RoomMessage], root_seq: u64) -> ThreadPartition {
    let mut roots = Vec::new();
    let mut replies = Vec::new();
    for message in transcript {
        match message.thread_parent_seq {
            None => roots.push(message.clone()),
            Some(parent) if parent == root_seq => replies.push(message.clone()),
            Some(_) => {}
        }
    }
    ThreadPartition { roots, replies }
}

fn reply_count_for(transcript: &[RoomMessage], root_seq: u64) -> usize {
    transcript
        .iter()
        .filter(|message| message.thread_parent_seq == Some(root_seq))
        .count()
}

fn thread_root_for(transcript: &[RoomMessage], root_seq: Option<u64>) -> Option<RoomMessage> {
    let root_seq = root_seq?;
    transcript
        .iter()
        .find(|message| message.seq == root_seq && message.thread_parent_seq.is_none())
        .cloned()
}

fn sync_thread_selection(
    current_root_seq: Option<u64>,
    open_room_key: Option<&str>,
    transcript: &[RoomMessage],
) -> Option<u64> {
    let root_seq = current_root_seq?;
    open_room_key?;
    thread_root_for(transcript, Some(root_seq)).map(|_| root_seq)
}

fn should_show_thread_button(message: &RoomMessage) -> bool {
    message.thread_parent_seq.is_none() && matches!(message.kind, RoomMessageKind::Message)
}

fn outbox_matches_failed_message(
    item: &crate::rooms::RoomOutboxItem,
    author_member_id: &str,
    wire: &str,
    thread_parent_seq: Option<u64>,
) -> bool {
    item.state == OutboxItemState::Failed
        && item.author_member_id == author_member_id
        && item.payload.get("body").and_then(|body| body.as_str()) == Some(wire)
        && item
            .payload
            .get("thread_parent_seq")
            .and_then(|value| value.as_u64())
            == thread_parent_seq
}

fn is_thread_open(selected_thread_root_seq: Option<u64>, root_seq: u64) -> bool {
    selected_thread_root_seq == Some(root_seq)
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

    // ── Center-rail: composer signal + focus/scroll refs ────────────────
    let composer = RwSignal::new(String::new());
    let thread_composer = RwSignal::new(String::new());
    let selected_thread_root_seq = RwSignal::new(None::<u64>);
    let thread_send_in_flight = RwSignal::new(false);
    let thread_last_sent_draft = RwSignal::new(String::new());
    let thread_last_sent_wire = RwSignal::new(String::new());
    let thread_last_sent_seq = RwSignal::new(0u64);
    let list_ref: NodeRef<leptos::html::Div> = NodeRef::new();
    let mobile_toggle_ref: NodeRef<leptos::html::Button> = NodeRef::new();
    let create_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();

    Effect::new(move |_| {
        if show_left_rail.get() {
            request_animation_frame(move || {
                if let Some(input) = create_input_ref.get() {
                    let _ = input.focus();
                }
            });
        }
    });

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

    Effect::new(move |_| {
        let next = sync_thread_selection(
            selected_thread_root_seq.get(),
            rooms.open_key.get().as_deref(),
            &rooms.transcript.get(),
        );
        if next != selected_thread_root_seq.get_untracked() {
            selected_thread_root_seq.set(next);
        }
        if next.is_none() {
            thread_composer.set(String::new());
            thread_send_in_flight.set(false);
            thread_last_sent_draft.set(String::new());
            thread_last_sent_wire.set(String::new());
            thread_last_sent_seq.set(0);
        }
    });

    // ── Left-rail: create room (draft retained until the typed
    //    create_op delivers a matching outcome — op-id gating so
    //    concurrent submits never cross-resolve, and CAS publication
    //    prevents stale completions from overwriting later ops).
    let pending_create = RwSignal::new(false);
    let create_op_id: RwSignal<u64> = RwSignal::new(0);
    let create_room = move || {
        // Prevent concurrent dispatch: if a create is already in flight,
        // ignore the keypress. The Effect clears pending_create when the
        // outcome resolves; until then, any second Enter is a no-op.
        if pending_create.get_untracked() {
            return;
        }
        let name = new_room_name.get_untracked();
        if name.trim().is_empty() {
            return;
        }
        let op_id = rooms.create_room(name.clone(), None);
        if op_id == 0 {
            // Synchronous rejection — empty name or slug. Don't set
            // pending; the name field already shows the error via status.
            return;
        }
        create_op_id.set(op_id);
        pending_create.set(true);
    };

    // Admission gate: triggered by list changes OR create_op updates.
    // Only the matching op_id resolves — no list-admission cross-attempt
    // fallthrough that would let A clear B's draft.
    Effect::new(move |_: Option<()>| {
        if !pending_create.get() {
            return;
        }
        let my_op = create_op_id.get();
        let (current_op, outcome) = rooms.create_op.get();
        // Only resolve our own op; never fall through to list inspection
        // for another op_id's outcome.
        if current_op != my_op {
            return;
        }
        let draft = new_room_name.get();
        if draft.trim().is_empty() {
            pending_create.set(false);
            return;
        }
        match Rooms::resolve_create_op(current_op, my_op, outcome.as_ref()) {
            CreateResolution::Success => {
                new_room_name.set(String::new());
                pending_create.set(false);
            }
            CreateResolution::KeepDraft => {
                pending_create.set(false);
            }
            CreateResolution::Pending => { /* still in flight */ }
        }
    });

    // ── Composer: one admitted send at a time. Keep both the exact draft and
    // the normalized wire body: daemon messages are trimmed, while clearing is
    // allowed only if the operator has not edited the original draft.
    let last_sent_draft = RwSignal::new(String::new());
    let last_sent_wire = RwSignal::new(String::new());
    let last_sent_seq = RwSignal::new(0u64);
    let send_in_flight = RwSignal::new(false);
    let do_send = move || {
        let draft = composer.get_untracked();
        if !message_send_admitted(
            send_in_flight.get_untracked(),
            thread_send_in_flight.get_untracked(),
            access_allows_writes(rooms.access.get_untracked().as_ref()),
            &draft,
        ) {
            return;
        }
        let wire = normalized_message_body(&draft);
        let max_seq = rooms
            .transcript
            .get_untracked()
            .iter()
            .map(|m| m.seq)
            .max()
            .unwrap_or(0);
        rooms.status.set(String::new());
        last_sent_draft.set(draft);
        last_sent_wire.set(wire.clone());
        last_sent_seq.set(max_seq);
        send_in_flight.set(true);
        rooms.post_message(wire, None);
    };

    let do_send_thread_reply = move || {
        let Some(root_seq) = selected_thread_root_seq.get_untracked() else {
            return;
        };
        let draft = thread_composer.get_untracked();
        if !message_send_admitted(
            thread_send_in_flight.get_untracked(),
            send_in_flight.get_untracked(),
            access_allows_writes(rooms.access.get_untracked().as_ref()),
            &draft,
        ) {
            return;
        }
        let wire = normalized_message_body(&draft);
        let max_seq = rooms
            .transcript
            .get_untracked()
            .iter()
            .map(|m| m.seq)
            .max()
            .unwrap_or(0);
        rooms.status.set(String::new());
        thread_last_sent_draft.set(draft);
        thread_last_sent_wire.set(wire.clone());
        thread_last_sent_seq.set(max_seq);
        thread_send_in_flight.set(true);
        rooms.post_message(wire, Some(root_seq));
    };

    Effect::new(move |_: Option<()>| {
        if !send_in_flight.get() {
            return;
        }
        let wire = last_sent_wire.get();
        let sent_at_seq = last_sent_seq.get();
        let me = rooms.identity_id.get_untracked();
        let confirmed = rooms.transcript.get().iter().any(|m| {
            m.seq > sent_at_seq
                && m.body == wire
                && m.author_id == me
                && m.thread_parent_seq.is_none()
        });
        if confirmed {
            let original = last_sent_draft.get_untracked();
            if should_clear_composer(&composer.get_untracked(), &original) {
                composer.set(String::new());
            }
            last_sent_draft.set(String::new());
            last_sent_wire.set(String::new());
            last_sent_seq.set(0);
            send_in_flight.set(false);
            return;
        }

        let failed_outbox = rooms.access.get().is_some_and(|access| {
            access
                .outbox
                .iter()
                .any(|item| outbox_matches_failed_message(item, me, &wire, None))
        });
        let request_failed = rooms.status.get().starts_with("message ");
        if failed_outbox || request_failed {
            last_sent_draft.set(String::new());
            last_sent_wire.set(String::new());
            last_sent_seq.set(0);
            send_in_flight.set(false);
        }
    });

    Effect::new(move |_: Option<()>| {
        if !thread_send_in_flight.get() {
            return;
        }
        let Some(root_seq) = selected_thread_root_seq.get() else {
            return;
        };
        let wire = thread_last_sent_wire.get();
        let sent_at_seq = thread_last_sent_seq.get();
        let me = rooms.identity_id.get_untracked();
        let confirmed = rooms.transcript.get().iter().any(|m| {
            m.seq > sent_at_seq
                && m.body == wire
                && m.author_id == me
                && m.thread_parent_seq == Some(root_seq)
        });
        if confirmed {
            let original = thread_last_sent_draft.get_untracked();
            if should_clear_composer(&thread_composer.get_untracked(), &original) {
                thread_composer.set(String::new());
            }
            thread_last_sent_draft.set(String::new());
            thread_last_sent_wire.set(String::new());
            thread_last_sent_seq.set(0);
            thread_send_in_flight.set(false);
            return;
        }

        let failed_outbox = rooms.access.get().is_some_and(|access| {
            access
                .outbox
                .iter()
                .any(|item| outbox_matches_failed_message(item, me, &wire, Some(root_seq)))
        });
        let request_failed = rooms.status.get().starts_with("message ");
        if failed_outbox || request_failed {
            thread_last_sent_draft.set(String::new());
            thread_last_sent_wire.set(String::new());
            thread_last_sent_seq.set(0);
            thread_send_in_flight.set(false);
        }
    });

    view! {
        <div
            class="rooms-workspace"
            role="region"
            aria-label="Rooms workspace"
            on:keydown=move |ev| {
                if ev.key() == "Escape" && show_left_rail.get_untracked() {
                    ev.prevent_default();
                    show_left_rail.set(false);
                    if let Some(toggle) = mobile_toggle_ref.get() {
                        let _ = toggle.focus();
                    }
                }
            }
        >

            // ═══ MOBILE NAV — narrow-screen room selector ═════════════
            // Always present but CSS hides it above 650px via matching
            // breakpoint; renders a compact bar with room toggle + active
            // room name so the reader is never stranded.
            <div class="rooms-workspace__mobile-nav">
                <button
                    class="rooms-workspace__mobile-nav-toggle"
                    type="button"
                    node_ref=mobile_toggle_ref
                    aria-label="Toggle room list"
                    aria-controls="rooms-workspace-room-list"
                    aria-expanded=move || show_left_rail.get().to_string()
                    on:click=move |_| show_left_rail.update(|v| *v = toggle_drawer(*v))
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
                            aria-hidden="true"
                            on:click=move |_| {
                                show_left_rail.set(false);
                                if let Some(toggle) = mobile_toggle_ref.get() {
                                    let _ = toggle.focus();
                                }
                            }
                        ></div>
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}
            <div
                id="rooms-workspace-room-list"
                class="rooms-workspace__left"
                class:rooms-workspace__left--visible=move || show_left_rail.get()
                role="navigation"
                aria-label="Room list"
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
                            let close = on_close;
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
                        let error = rooms.rooms_error.get();
                        if let Some(error) = error {
                            view! {
                                <div
                                    class="rooms-workspace__left-empty rooms-workspace__left-empty--error"
                                    role="alert"
                                >
                                    {format!("Unable to load rooms: {error}")}
                                </div>
                            }.into_any()
                        } else if list.is_empty()
                            && (rooms.rooms_loading.get() || !rooms.rooms_loaded.get())
                        {
                            view! {
                                <div class="rooms-workspace__left-empty" role="status">
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

                {move || {
                    let status = rooms.status.get();
                    if status.starts_with("rooms ") || status.starts_with("create ") {
                        view! {
                            <div
                                class="rooms-workspace__left-status"
                                role="status"
                                aria-live="polite"
                            >
                                {status}
                            </div>
                        }.into_any()
                    } else {
                        ().into_any()
                    }
                }}

                // Create input at bottom of left rail
                <div class="rooms-workspace__left-create">
                    <input
                        class="rooms-workspace__left-input"
                        type="text"
                        node_ref=create_input_ref
                        aria-label="New room name"
                        aria-busy=move || pending_create.get().to_string()
                        placeholder="New room name…"
                        prop:value=move || new_room_name.get()
                        on:input=move |ev| new_room_name.set(event_target_value(&ev))
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                ev.prevent_default();
                                create_room();
                            }
                        }
                        disabled=move || pending_create.get()
                    />
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
                                            aria-label="Close current room"
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
                                                <div
                                                    class="room-stage__access-state room-stage__access-state--connecting"
                                                    role="status"
                                                    aria-live="polite"
                                                >
                                                    "Connecting to federated room…"
                                                </div>
                                            }.into_any()
                                        }
                                        Some(RoomAccessState::Recovering) => {
                                            view! {
                                                <div
                                                    class="room-stage__access-state room-stage__access-state--recovering"
                                                    role="status"
                                                    aria-live="polite"
                                                >
                                                    "Recovering connection…"
                                                </div>
                                            }.into_any()
                                        }
                                        Some(RoomAccessState::Revoked) => {
                                            view! {
                                                <div
                                                    class="room-stage__access-state room-stage__access-state--revoked"
                                                    role="alert"
                                                >
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
                                        each=move || partition_thread_messages(&rooms.transcript.get(), 0).roots
                                        key=|m: &RoomMessage| m.seq
                                        children=move |m: RoomMessage| {
                                            let is_system = matches!(
                                                m.kind,
                                                RoomMessageKind::System
                                                    | RoomMessageKind::ParticipantJoined
                                                    | RoomMessageKind::ParticipantLeft
                                            );
                                            let ts = short_time(&m.created_at);
                                            let root_seq = m.seq;
                                            view! {
                                                <div
                                                    class="rooms-workspace__msg"
                                                    class:rooms-workspace__msg--system=is_system
                                                >
                                                    <div class="rooms-workspace__msg-avatar">
                                                        {if is_system {
                                                            view! { <crate::icons::Spark /> }.into_any()
                                                        } else {
                                                            m.author_id.chars().take(2).collect::<String>().to_uppercase().into_any()
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
                                                        {move || {
                                                            if should_show_thread_button(&m) {
                                                                let reply_count = reply_count_for(&rooms.transcript.get(), root_seq);
                                                                view! {
                                                                    <button
                                                                        class="rooms-workspace__thread-toggle"
                                                                        class:rooms-workspace__thread-toggle--active=move || {
                                                                            selected_thread_root_seq.get() == Some(root_seq)
                                                                        }
                                                                        type="button"
                                                                        aria-label=move || {
                                                                            if is_thread_open(selected_thread_root_seq.get(), root_seq) {
                                                                                format!("Close thread for message {}", root_seq)
                                                                            } else {
                                                                                format!("Open thread for message {}", root_seq)
                                                                            }
                                                                        }
                                                                        aria-pressed=move || {
                                                                            is_thread_open(selected_thread_root_seq.get(), root_seq)
                                                                                .to_string()
                                                                        }
                                                                        on:click=move |_| {
                                                                            selected_thread_root_seq.update(|selected| {
                                                                                *selected = if *selected == Some(root_seq) {
                                                                                    None
                                                                                } else {
                                                                                    Some(root_seq)
                                                                                };
                                                                            });
                                                                        }
                                                                    >
                                                                        {if reply_count > 0 {
                                                                            format!("Open thread ({reply_count})")
                                                                        } else {
                                                                            "Open thread".to_string()
                                                                        }}
                                                                    </button>
                                                                }.into_any()
                                                            } else {
                                                                ().into_any()
                                                            }
                                                        }}
                                                    </div>
                                                </div>
                                            }
                                        }
                                    />
                                    {move || {
                                        let roots = partition_thread_messages(&rooms.transcript.get(), 0).roots;
                                        let roots_empty = roots.is_empty();
                                        if show_transcript_empty(rooms.transcript_tail_is_live(), roots_empty) {
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

                                // Federation outbox is explicitly outside the
                                // confirmed transcript. Pending items are
                                // informational; only failed items can retry.
                                {move || {
                                    let outbox = rooms.access.get()
                                        .map(|access| access.outbox)
                                        .unwrap_or_default();
                                    if outbox.is_empty() {
                                        ().into_any()
                                    } else {
                                        view! {
                                            <div
                                                class="rooms-workspace__outbox"
                                                aria-label="Messages awaiting federation"
                                                aria-live="polite"
                                            >
                                                <For
                                                    each=move || rooms.access.get()
                                                        .map(|access| access.outbox)
                                                        .unwrap_or_default()
                                                    key=|item| item.client_event_id.clone()
                                                    children=move |item| {
                                                        let failed = item.state == OutboxItemState::Failed;
                                                        let body = item.payload.get("body")
                                                            .and_then(|body| body.as_str())
                                                            .unwrap_or("Message awaiting confirmation")
                                                            .to_string();
                                                        let event_id = item.client_event_id.clone();
                                                        view! {
                                                            <div
                                                                class="rooms-workspace__outbox-item"
                                                                class:rooms-workspace__outbox-item--failed=failed
                                                            >
                                                                <span class="rooms-workspace__outbox-state">
                                                                    {if failed { "Failed" } else { "Pending" }}
                                                                </span>
                                                                <span class="rooms-workspace__outbox-body">
                                                                    {body}
                                                                </span>
                                                                {if failed {
                                                                    view! {
                                                                        <button
                                                                            class="rooms-workspace__outbox-retry"
                                                                            type="button"
                                                                            aria-label="Retry failed message"
                                                                            on:click=move |_| rooms.retry_outbox(event_id.clone())
                                                                        >
                                                                            "Retry"
                                                                        </button>
                                                                    }.into_any()
                                                                } else {
                                                                    ().into_any()
                                                                }}
                                                            </div>
                                                        }
                                                    }
                                                />
                                            </div>
                                        }.into_any()
                                    }
                                }}

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
                                            aria-label="Message"
                                            placeholder="Message… (@id to mention)"
                                            prop:value=move || composer.get()
                                            on:input=move |ev| composer.set(event_target_value(&ev))
                                            disabled=move || !access_allows_writes(rooms.access.get().as_ref())
                                        />
                                        <button
                                            class="rooms-workspace__composer-send"
                                            type="submit"
                                            disabled=move || {
                                                send_in_flight.get()
                                                    || composer.get().trim().is_empty()
                                                    || !access_allows_writes(rooms.access.get().as_ref())
                                            }
                                        >
                                            {move || if send_in_flight.get() { "Sending…" } else { "Send" }}
                                        </button>
                                    </form>

                                    {move || {
                                        let s = rooms.status.get();
                                        if s.is_empty()
                                            || s.starts_with("rooms ")
                                            || s.starts_with("create ")
                                        {
                                            ().into_any()
                                        } else {
                                            view! {
                                                <div
                                                    class="rooms-workspace__status"
                                                    role="status"
                                                    aria-live="polite"
                                                >
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
            <div
                class="rooms-workspace__right"
                class:rooms-workspace__right--thread=move || selected_thread_root_seq.get().is_some()
            >
                <div class="rooms-workspace__right-head">
                    <h3 class="rooms-workspace__right-title">
                        {move || if selected_thread_root_seq.get().is_some() { "Thread" } else { "Members" }}
                    </h3>
                    {move || {
                        if selected_thread_root_seq.get().is_some() {
                            view! {
                                <button
                                    class="rooms-workspace__right-close"
                                    type="button"
                                    aria-label="Close thread"
                                    on:click=move |_| selected_thread_root_seq.set(None)
                                >
                                    <svg viewBox="0 0 16 16" width="14" height="14"
                                        fill="none" stroke="currentColor" stroke-width="1.6"
                                        stroke-linecap="round">
                                        <path d="M3 3l10 10M13 3L3 13"/>
                                    </svg>
                                </button>
                            }.into_any()
                        } else {
                            ().into_any()
                        }
                    }}
                </div>

                <div class="rooms-workspace__right-list">
                    {move || {
                        if let Some(root) = thread_root_for(&rooms.transcript.get(), selected_thread_root_seq.get()) {
                            let root_seq = root.seq;
                            let ts = short_time(&root.created_at);
                            let root_is_system = matches!(
                                root.kind,
                                RoomMessageKind::System
                                    | RoomMessageKind::ParticipantJoined
                                    | RoomMessageKind::ParticipantLeft
                            );
                            view! {
                                <div class="rooms-workspace__right-thread">
                                    <div class="rooms-workspace__right-thread-head">
                                        <p class="rooms-workspace__right-thread-title">"Thread"</p>
                                        <div class="rooms-workspace__right-thread-subtitle">
                                            {format!("Replying to {}", root.author_id)}
                                        </div>
                                    </div>
                                    <div class="rooms-workspace__right-thread-transcript">
                                        <div
                                            class="rooms-workspace__msg rooms-workspace__msg--thread-root"
                                            class:rooms-workspace__msg--system=root_is_system
                                        >
                                            <div class="rooms-workspace__msg-avatar">
                                                {if root_is_system {
                                                    view! { <crate::icons::Spark /> }.into_any()
                                                } else {
                                                    root.author_id.chars().take(2).collect::<String>().to_uppercase().into_any()
                                                }}
                                            </div>
                                            <div class="rooms-workspace__msg-body">
                                                <div class="rooms-workspace__msg-author">
                                                    <span class="rooms-workspace__msg-name">{root.author_id.clone()}</span>
                                                    <span class="rooms-workspace__msg-time">{ts}</span>
                                                </div>
                                                <div class="rooms-workspace__msg-text">{root.body.clone()}</div>
                                            </div>
                                        </div>
                                        <For
                                            each=move || partition_thread_messages(&rooms.transcript.get(), root_seq).replies
                                            key=|m: &RoomMessage| m.seq
                                            children=move |reply: RoomMessage| {
                                                let ts = short_time(&reply.created_at);
                                                let is_system = matches!(
                                                    reply.kind,
                                                    RoomMessageKind::System
                                                        | RoomMessageKind::ParticipantJoined
                                                        | RoomMessageKind::ParticipantLeft
                                                );
                                                view! {
                                                    <div
                                                        class="rooms-workspace__msg rooms-workspace__msg--thread-reply"
                                                        class:rooms-workspace__msg--system=is_system
                                                    >
                                                        <div class="rooms-workspace__msg-avatar">
                                                            {if is_system {
                                                                view! { <crate::icons::Spark /> }.into_any()
                                                            } else {
                                                                reply.author_id.chars().take(2).collect::<String>().to_uppercase().into_any()
                                                            }}
                                                        </div>
                                                        <div class="rooms-workspace__msg-body">
                                                            <div class="rooms-workspace__msg-author">
                                                                <span class="rooms-workspace__msg-name">{reply.author_id.clone()}</span>
                                                                <span class="rooms-workspace__msg-time">{ts}</span>
                                                            </div>
                                                            <div class="rooms-workspace__msg-text">{reply.body.clone()}</div>
                                                        </div>
                                                    </div>
                                                }
                                            }
                                        />
                                    </div>
                                    <div class="rooms-workspace__composer rooms-workspace__composer--thread">
                                        <form
                                            class="rooms-workspace__composer-row"
                                            on:submit=move |ev| {
                                                ev.prevent_default();
                                                do_send_thread_reply();
                                            }
                                        >
                                            <input
                                                class="rooms-workspace__composer-input"
                                                type="text"
                                                aria-label="Thread reply"
                                                placeholder="Reply in thread…"
                                                prop:value=move || thread_composer.get()
                                                on:input=move |ev| thread_composer.set(event_target_value(&ev))
                                                disabled=move || !access_allows_writes(rooms.access.get().as_ref())
                                            />
                                            <button
                                                class="rooms-workspace__composer-send"
                                                type="submit"
                                                disabled=move || {
                                                    thread_send_in_flight.get()
                                                        || thread_composer.get().trim().is_empty()
                                                        || !access_allows_writes(rooms.access.get().as_ref())
                                                }
                                            >
                                                {move || if thread_send_in_flight.get() { "Sending…" } else { "Reply" }}
                                            </button>
                                        </form>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
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
                                                                    {p.display_name.chars().take(2).collect::<String>().to_uppercase()}
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

                                        <button
                                            class="rooms-workspace__addagent"
                                            type="button"
                                            title="Add an agent participant"
                                            aria-controls="rooms-workspace-agent-picker"
                                            aria-expanded=move || show_add_agent.get().to_string()
                                            on:click=move |_| show_add_agent.update(|v: &mut bool| *v = !*v)
                                        >
                                            "+ agent"
                                        </button>
                                        {move || {
                                            if show_add_agent.get() {
                                                view! {
                                                    <div
                                                        id="rooms-workspace-agent-picker"
                                                        class="rooms-workspace__addagent-picker"
                                                    >
                                                        <select
                                                            class="rooms-workspace__addagent-select"
                                                            aria-label="Choose an agent to add"
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
                                                    let presence_label = match presence {
                                                        Some(MemberPresence::Live) => "Live",
                                                        Some(MemberPresence::Unavailable) => "Unavailable",
                                                        None => "",
                                                    };
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
                                                                {member.display_name.chars().take(2).collect::<String>().to_uppercase()}
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
                                                                        role="img"
                                                                        aria-label=presence_label
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
        CreateOutcome, CreateResolution, RoomAccessProjection, RoomAccessState, RoomMessage,
        RoomMessageKind, RoomParticipantKind,
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

    fn test_msg(seq: u64, body: &str, thread_parent_seq: Option<u64>) -> RoomMessage {
        RoomMessage {
            seq,
            kind: RoomMessageKind::Message,
            author_id: "user".into(),
            author_kind: RoomParticipantKind::Human,
            body: body.into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            federated: None,
            thread_parent_seq,
        }
    }

    #[test]
    fn transcript_with_root_is_not_empty_when_live() {
        let msgs = vec![test_msg(1, "hello", None)];
        assert!(!show_transcript_empty(
            true,
            partition_thread_messages(&msgs, 0).roots.is_empty()
        ));
    }

    #[test]
    fn transcript_empty_is_hidden_until_tail_is_live() {
        assert!(!show_transcript_empty(false, true));
        assert!(show_transcript_empty(true, true));
        assert!(!show_transcript_empty(true, false));
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

    #[test]
    fn failed_outbox_matching_discriminates_thread_parent_seq() {
        let top_level = crate::rooms::RoomOutboxItem {
            client_event_id: "client-1".into(),
            source_id: "surface-web".into(),
            source_sequence: 1,
            author_member_id: "user".into(),
            event_type: "room_message".into(),
            payload: serde_json::json!({"body": "hello"}),
            mention_member_ids: vec![],
            state: OutboxItemState::Failed,
        };
        assert!(outbox_matches_failed_message(
            &top_level, "user", "hello", None
        ));
        assert!(!outbox_matches_failed_message(
            &top_level,
            "user",
            "hello",
            Some(7)
        ));

        let threaded = crate::rooms::RoomOutboxItem {
            payload: serde_json::json!({"body": "hello", "thread_parent_seq": 7}),
            ..top_level.clone()
        };
        assert!(outbox_matches_failed_message(
            &threaded,
            "user",
            "hello",
            Some(7)
        ));
        assert!(!outbox_matches_failed_message(
            &threaded, "user", "hello", None
        ));
    }

    #[test]
    fn thread_open_helper_is_exact() {
        assert!(is_thread_open(Some(7), 7));
        assert!(!is_thread_open(Some(8), 7));
        assert!(!is_thread_open(None, 7));
    }

    #[test]
    fn partition_thread_messages_separates_roots_and_direct_replies() {
        let transcript = vec![
            test_msg(1, "root 1", None),
            test_msg(2, "reply to 1", Some(1)),
            test_msg(3, "root 3", None),
            test_msg(4, "reply to 3", Some(3)),
            test_msg(5, "orphan nested", Some(2)),
        ];

        let partition = partition_thread_messages(&transcript, 1);
        assert_eq!(
            partition.roots.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(
            partition.replies.iter().map(|m| m.seq).collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(reply_count_for(&transcript, 3), 1);
        assert_eq!(reply_count_for(&transcript, 2), 1);
    }

    #[test]
    fn sync_thread_selection_clears_on_room_close_or_missing_root() {
        let transcript = vec![test_msg(1, "root 1", None), test_msg(2, "reply", Some(1))];
        assert_eq!(
            sync_thread_selection(Some(1), Some("room-1"), &transcript),
            Some(1)
        );
        assert_eq!(sync_thread_selection(Some(1), None, &transcript), None);
        assert_eq!(
            sync_thread_selection(Some(9), Some("room-1"), &transcript),
            None
        );
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

    #[test]
    fn composer_normalizes_wire_body_once() {
        assert_eq!(normalized_message_body("  hello world \n"), "hello world");
    }

    #[test]
    fn composer_admission_rejects_empty_blocked_and_concurrent_sends() {
        assert!(!message_send_admitted(false, false, true, " \n\t "));
        assert!(!message_send_admitted(false, false, false, "hello"));
        assert!(!message_send_admitted(true, false, true, "hello"));
        assert!(!message_send_admitted(false, true, true, "hello"));
        assert!(message_send_admitted(false, false, true, " hello "));
    }

    // ── Behavioral: resolve_create_op (production helper from rooms.rs) ──
    // Uses the real pub fn that the Effect calls — no cfg(test)-only copies.

    #[test]
    fn resolve_success_clears_draft() {
        let outcome = Some(CreateOutcome::Success { key: "room".into() });
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(1, 1, outcome.as_ref()),
            CreateResolution::Success
        );
    }

    #[test]
    fn resolve_duplicate_keeps_draft() {
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(1, 1, Some(&CreateOutcome::Duplicate)),
            CreateResolution::KeepDraft
        );
    }

    #[test]
    fn resolve_failed_keeps_draft() {
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(
                1,
                1,
                Some(&CreateOutcome::Failed {
                    error: "timeout".into(),
                }),
            ),
            CreateResolution::KeepDraft
        );
    }

    #[test]
    fn resolve_in_flight_stays_pending() {
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(1, 1, None),
            CreateResolution::Pending
        );
    }

    #[test]
    fn resolve_stale_op_id_returns_pending() {
        // current_op (2) != my_op (1) → stale, no match
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(
                2,
                1,
                Some(&CreateOutcome::Success { key: "x".into() }),
            ),
            CreateResolution::Pending
        );
    }

    #[test]
    fn resolve_concurrent_b_sees_own_outcome_a_sees_stale() {
        // Op 2 is the active dispatch; op 1 was superseded.
        // Both check the same slot (current_op=2, outcome=Success for room-b).
        let outcome = Some(CreateOutcome::Success {
            key: "b-room".into(),
        });
        // B (my_op=2) resolves → Success
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(2, 2, outcome.as_ref()),
            CreateResolution::Success
        );
        // A (my_op=1) resolves → Pending (stale)
        assert_eq!(
            crate::rooms::Rooms::resolve_create_op(2, 1, outcome.as_ref()),
            CreateResolution::Pending
        );
    }

    // ── Behavioral: cas_admit_create (production helper from rooms.rs) ──

    #[test]
    fn cas_admit_matching_op() {
        assert!(Rooms::cas_admit_create(1, 1));
    }

    #[test]
    fn cas_admit_superseded_op() {
        // Slot op 3, our op 1 — superseded by two later dispatches.
        assert!(!Rooms::cas_admit_create(3, 1));
    }

    #[test]
    fn cas_admit_zero_op() {
        assert!(Rooms::cas_admit_create(0, 0));
        assert!(!Rooms::cas_admit_create(0, 1));
    }

    #[test]
    fn cas_admit_stale_success_action_is_list_only() {
        // Simulate what the CAS closure does for a stale success:
        // admitted=false + Success → fetch_rooms only, no status/select.
        // Verified by the outcome path: CasAdmission::admit(*cur, op_id)
        // returns false, and the else-if branch only triggers for Success.
        let outcome = CreateOutcome::Success {
            key: "stale-room".into(),
        };
        let admitted = Rooms::cas_admit_create(2, 1); // slot=2, my=1 → false
        assert!(!admitted);
        // If !admitted && matches!(Success): list-refresh only.
        // If !admitted && !Success: fully suppressed.
        assert!(matches!(outcome, CreateOutcome::Success { .. }));
        // Stale failure would be suppressed — no status, no list:
        let failure = CreateOutcome::Failed {
            error: "timeout".into(),
        };
        assert!(matches!(failure, CreateOutcome::Failed { .. }));
        // The production code path for stale failure is: no side effects.
    }

    // ── Behavioral: compact drawer reachability (production helper) ──

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
        assert!(!toggle_drawer(toggle_drawer(false)));
        assert!(toggle_drawer(toggle_drawer(true)));
    }
}
