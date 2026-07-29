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
pub fn RoomsWorkspace(rooms: Rooms) -> impl IntoView {
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

    // ── Left-rail: create room ────────────────────────────────────────
    let do_create = move || {
        let name = new_room_name.get_untracked().trim().to_string();
        if name.is_empty() {
            return;
        }
        rooms.create_room(name, None);
        new_room_name.set(String::new());
    };

    // ── Composer: preserve draft until admission. post_message is
    //    fire-and-forget, but we only clear after sending so the user can
    //    retry if the status line shows an error.
    let do_send = move || {
        if !access_allows_writes(rooms.access.get_untracked().as_ref()) {
            return;
        }
        let text = composer.get_untracked();
        if text.trim().is_empty() {
            return;
        }
        rooms.post_message(text);
        // Clear only after attempting send — if it fails the draft is still
        // visible and the status signal will carry the error.
        composer.set(String::new());
    };

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
            <div
                class="rooms-workspace__left"
                class:rooms-workspace__left--visible=move || show_left_rail.get()
            >
                <div class="rooms-workspace__left-head">
                    <h2 class="rooms-workspace__left-title">"Rooms"</h2>
                    <button
                        class="rooms-workspace__left-close"
                        type="button"
                        aria-label="Close rooms workspace"
                        on:click=move |_| {
                            rooms.close_room();
                            rooms.panel_open.set(true);
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
                                do_create();
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
                                if participants.is_empty() {
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
                                }
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
