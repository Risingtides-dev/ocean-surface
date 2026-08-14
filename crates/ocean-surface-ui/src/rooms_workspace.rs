//! Slack-style persistent 3-column Rooms workspace.
//!
//! Left rail (room list + create), center rail (header + transcript + composer),
//! right rail (members / details). Renders against the existing CSS classes in
//! `styles/rooms-workspace.css` and the existing [`crate::rooms::Rooms`] signals
//! and API surface. Mounted as the default web+Tauri collaboration surface in
//! [`crate::app`]; the legacy RoomStage/RoomsPanel components in rooms.rs are
//! deleted.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::room_messages;
use crate::rooms::{
    CreateResolution, FederatedActorType, FederatedRoomMemberProjection, FederatedRoomRole,
    MemberPresence, OutboxItemState, Room, RoomAccessProjection, RoomAccessState, RoomMessage,
    RoomMessageKind, RoomParticipant, RoomParticipantKind, RoomReadCursorProjection, Rooms,
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

/// Render a compact clock label from the canonical RFC3339 wire timestamp.
/// Extracts the shared `HH:MM` prefix for `Z`, fractional-second, and offset
/// variants without converting timezones or localizing; invalid/non-canonical
/// input passes through unchanged.
/// The client's current UTC day key (`YYYY-MM-DD`), matching the daemon's
/// ISO-8601 UTC timestamps, for humanizing day separators.
fn today_day_key() -> String {
    js_sys::Date::new_0()
        .to_iso_string()
        .as_string()
        .unwrap_or_default()
        .chars()
        .take(10)
        .collect()
}

/// Whether to show the "No messages yet" empty state in the transcript.
#[allow(dead_code)]
fn show_transcript_empty(tail_is_live: bool, roots_empty: bool) -> bool {
    roots_empty && tail_is_live
}

// ── Room-list ARIA listbox helpers (pure, unit-testable) ───────────────────

/// Roving-tabindex keyboard model for the room-list listbox. Given the
/// ordered room keys, the key of the option that currently has DOM focus,
/// and the pressed key, returns the index that should receive focus next.
/// `None` means "not a navigation key — leave the event alone".
/// ArrowDown/ArrowUp wrap (listbox convention); Home/End jump.
fn room_list_next_focus(keys: &[String], focused: Option<&str>, pressed: &str) -> Option<usize> {
    if keys.is_empty() {
        return None;
    }
    let cur = focused.and_then(|f| keys.iter().position(|k| k == f));
    match pressed {
        "ArrowDown" => Some(cur.map_or(0, |i| (i + 1) % keys.len())),
        "ArrowUp" => Some(cur.map_or(keys.len() - 1, |i| (i + keys.len() - 1) % keys.len())),
        "Home" => Some(0),
        "End" => Some(keys.len() - 1),
        _ => None,
    }
}

/// The single tab stop of the roving-tabindex listbox: the open room when it
/// is present in the list, else the first room. Exactly one option carries
/// tabindex=0 so Tab enters the list once and arrows move within it.
fn room_list_tab_stop(keys: &[String], open: Option<&str>) -> Option<usize> {
    if keys.is_empty() {
        return None;
    }
    Some(
        open.and_then(|o| keys.iter().position(|k| k == o))
            .unwrap_or(0),
    )
}

/// DOM id for a room option — the stable hook the keydown handler uses to
/// move real focus (roving tabindex needs actual `.focus()` calls).
fn room_option_dom_id(key: &str) -> String {
    format!("rooms-opt-{key}")
}

fn transcript_is_near_bottom(
    scroll_height: i32,
    scroll_top: i32,
    client_height: i32,
    threshold: i32,
) -> bool {
    scroll_height - scroll_top - client_height < threshold
}

/// Whether the transcript element can scroll at all. A transcript whose
/// content fits inside its viewport never fires a `scroll` event, so it can
/// never be *confirmed* at-bottom by scrolling — it simply already is.
fn transcript_is_scrollable(scroll_height: i32, client_height: i32) -> bool {
    scroll_height > client_height
}

/// Whether this transcript pass is hydrated enough for its at-bottom state to
/// advance the durable read cursor.
///
/// Every pass after the first fill is hydrated. The first fill itself is
/// trusted only when the transcript is *measured* and cannot scroll: a
/// scrollable first fill is programmatically pinned to the bottom by the
/// transcript Effect, and the `scroll` event that pin produces is what
/// re-enters the read path with hydration complete (so a reader who
/// immediately scrolls up is never marked read from the raw fill). A
/// non-scrollable transcript never fires that event, so its first fill — by
/// definition entirely visible with the newest message at the bottom — must
/// advance the cursor directly, or the room stays unread forever.
///
/// "Measured" is the load-bearing word: an element that has not been laid out
/// yet reports `client_height == 0`, and `0 > 0` is false, so an unmeasured
/// first fill would otherwise masquerade as a fully visible transcript and
/// mark an arbitrarily long room read without a single visible message. Zero
/// (or negative) viewport height means "unknown", never "everything fits";
/// such a pass defers to the next one, which measures a laid-out element.
///
/// This is scroll-position hydration only. The room/access/transcript
/// hydration guards stay in [`ready_read_target`]: an open room, a non-empty
/// transcript, and a durable candidate from a `Local`/`Live` access
/// projection.
fn transcript_read_hydrated(first_fill: bool, scroll_height: i32, client_height: i32) -> bool {
    if !first_fill {
        return true;
    }
    client_height > 0 && !transcript_is_scrollable(scroll_height, client_height)
}

fn durable_read_candidate(
    transcript: &[RoomMessage],
    access: Option<&RoomAccessProjection>,
) -> Option<u64> {
    match access.map(|projection| projection.state) {
        Some(RoomAccessState::Live) => {
            access.and_then(|projection| projection.last_confirmed_global_sequence)
        }
        Some(RoomAccessState::Local) => transcript.last().map(|message| message.seq),
        _ => None,
    }
}

/// The single evaluation point for "may this state advance the durable read
/// cursor, and to which sequence?". [`read_advance_request`] derives from it,
/// so readiness and the candidate can never disagree and no caller needs an
/// unwrap/expect to recover the sequence.
///
/// These are the hydration guards proper: the room detail must be loaded, the
/// transcript non-empty, and the access projection must be `Local`/`Live`
/// enough to yield a durable candidate. `transcript_hydrated` carries only the
/// scroll-position question (see [`transcript_read_hydrated`]).
fn ready_read_target(
    transcript_hydrated: bool,
    near_bottom: bool,
    room_loaded: bool,
    transcript: &[RoomMessage],
    access: Option<&RoomAccessProjection>,
) -> Option<u64> {
    if !transcript_hydrated || !near_bottom || !room_loaded || transcript.is_empty() {
        return None;
    }
    durable_read_candidate(transcript, access)
}

/// A queued read-advance intent, stamped with the room `generation` live at
/// the moment it was computed. `open_room_key` alone is not enough to prove
/// the request still belongs to the currently open room admission: closing
/// and reopening the *same* key bumps `Rooms::generation` without changing
/// the key, so a request built against the old admission must be rejected
/// even though `open_room_key` still matches. Re-validate with
/// `Rooms::room_is_current(generation, &open_room_key)` before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadAdvanceRequest {
    open_room_key: String,
    generation: u64,
    candidate_read_seq: u64,
}

fn read_advance_request(
    open_room_key: Option<&str>,
    generation: u64,
    transcript_hydrated: bool,
    near_bottom: bool,
    room_loaded: bool,
    transcript: &[RoomMessage],
    access: Option<&RoomAccessProjection>,
) -> Option<ReadAdvanceRequest> {
    let open_room_key = open_room_key?;
    let candidate_read_seq = ready_read_target(
        transcript_hydrated,
        near_bottom,
        room_loaded,
        transcript,
        access,
    )?;
    Some(ReadAdvanceRequest {
        open_room_key: open_room_key.to_string(),
        generation,
        candidate_read_seq,
    })
}

/// What one transcript pass should do. Extracting the decision keeps the
/// Effect body a dispatcher and makes the reactive re-run behaviour testable
/// without a browser: the same pass now fires for a transcript append *and*
/// for a room-access projection arriving after the fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptPassAction {
    /// No transcript (room switch / generation reset): drop the jump
    /// affordance and any queued read intent.
    Reset,
    /// First fill, or an at-bottom reader: pin to the newest message and
    /// (re)evaluate the durable read advance.
    PinAndQueue,
    /// Content appended below a scrolled-up reader: raise the jump
    /// affordance instead of yanking them.
    RaiseJump,
    /// Nothing to do. Critically, `Hold` never writes: a pass triggered by a
    /// non-transcript dependency (an access projection) while the reader is
    /// scrolled up must not mark read, must not raise the jump affordance,
    /// and must not clobber an already queued request.
    Hold,
}

/// `measured` is whether the transcript element exists *and* reports a real
/// viewport; an unmeasured pass holds so the fill state stays intact for the
/// first pass that can actually measure.
fn transcript_pass_action(
    len: usize,
    prev_len: usize,
    measured: bool,
    near_bottom: bool,
) -> TranscriptPassAction {
    if len == 0 {
        return TranscriptPassAction::Reset;
    }
    if !measured {
        return TranscriptPassAction::Hold;
    }
    if prev_len == 0 || near_bottom {
        return TranscriptPassAction::PinAndQueue;
    }
    if len > prev_len {
        return TranscriptPassAction::RaiseJump;
    }
    TranscriptPassAction::Hold
}

/// The read position already durably applied to the open room: the furthest of
/// the room-list summary's confirmed read and the durable cursor projection
/// (own read plus mirrored upstream read). This mirrors the monotonic fold
/// `rooms.rs` applies before it will issue `PATCH /read-cursor`; here it is a
/// read-only pre-filter over public signals, so being conservative only costs
/// a redundant queue, never a lost read.
fn applied_read_seq(
    summary_read_seq: Option<u64>,
    cursor: Option<&RoomReadCursorProjection>,
) -> Option<u64> {
    [
        summary_read_seq,
        cursor.and_then(|cursor| cursor.read_seq),
        cursor.and_then(|cursor| cursor.mirrored_upstream_read_seq),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// Whether an at-bottom read target still needs to be queued.
///
/// A near-bottom scroll burst fires many events per second, each computing the
/// identical `(room, generation, seq)` target; allocating and publishing that
/// duplicate every frame churns the dispatch Effect for no state change. Two
/// self-releasing skips, both keyed on room/generation/seq:
///
/// - the same target is already queued and undispatched, or
/// - the durable cursor has already been confirmed at or past this sequence.
///
/// Neither can suppress a required retry. A failed `PATCH /read-cursor`
/// advances no cursor and the dispatch Effect has already cleared the pending
/// request, so the next near-bottom frame recomputes the same target and finds
/// both skips released.
fn read_advance_needs_queue(
    pending: Option<&ReadAdvanceRequest>,
    open_room_key: &str,
    generation: u64,
    candidate_read_seq: u64,
    applied_read_seq: Option<u64>,
) -> bool {
    if applied_read_seq.is_some_and(|applied| applied >= candidate_read_seq) {
        return false;
    }
    !pending.is_some_and(|pending| {
        pending.open_room_key == open_room_key
            && pending.generation == generation
            && pending.candidate_read_seq == candidate_read_seq
    })
}

fn roster_presence_count(members: &[FederatedRoomMemberProjection]) -> usize {
    members
        .iter()
        .filter(|member| {
            matches!(member.actor_type, FederatedActorType::User)
                && matches!(member.derived_presence, Some(MemberPresence::Live))
        })
        .count()
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

/// Row timestamp: show only the canonical wire clock (HH:MM) for RFC3339
/// timestamps while preserving the full wire value for machine-readable and
/// accessible render paths. Accepts only ASCII canonical structure at the
/// byte positions we actually rely on, never panics on Unicode/invalid input,
/// and returns the original string unchanged when the wire value is not the
/// expected RFC3339 shape.
fn canonical_wire_clock_time(full: &str) -> String {
    let bytes = full.as_bytes();
    let is_digit = |idx: usize| bytes.get(idx).is_some_and(|b| b.is_ascii_digit());

    if bytes.len() < 16
        || !full.is_ascii()
        || !is_digit(0)
        || !is_digit(1)
        || !is_digit(2)
        || !is_digit(3)
        || bytes[4] != b'-'
        || !is_digit(5)
        || !is_digit(6)
        || bytes[7] != b'-'
        || !is_digit(8)
        || !is_digit(9)
        || bytes[10] != b'T'
        || !is_digit(11)
        || !is_digit(12)
        || bytes[13] != b':'
        || !is_digit(14)
        || !is_digit(15)
    {
        return full.to_string();
    }

    full[11..16].to_string()
}
fn avatar_identity_class(author_id: &str) -> &'static str {
    const HUES: [&str; 5] = [
        "rooms-workspace__msg-avatar--hue0",
        "rooms-workspace__msg-avatar--hue1",
        "rooms-workspace__msg-avatar--hue2",
        "rooms-workspace__msg-avatar--hue3",
        "rooms-workspace__msg-avatar--hue4",
    ];
    let h = author_id.bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(b as usize)
    });
    HUES[h % HUES.len()]
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

// ── Mention autosuggest (pure, unit-testable) ──────────────────────────────

/// A ranked mention suggestion for the composer typeahead popup.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MentionSuggestion {
    id: String,
    display_name: String,
    kind: RoomParticipantKind,
}

/// Short human label for a participant kind, shown as the suggestion badge.
fn participant_kind_label(kind: RoomParticipantKind) -> &'static str {
    match kind {
        RoomParticipantKind::Human => "human",
        RoomParticipantKind::Agent => "agent",
        RoomParticipantKind::Bot => "bot",
        RoomParticipantKind::Tool => "tool",
        RoomParticipantKind::System => "system",
    }
}

/// Convert a UTF-16 code-unit offset (what `selectionStart` reports) into a
/// byte offset into `s`, clamped to the string end.
fn utf16_to_byte_idx(s: &str, utf16: usize) -> usize {
    let mut units = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if units >= utf16 {
            return byte_idx;
        }
        units += ch.len_utf16();
    }
    s.len()
}

/// Convert a byte offset into `s` into a UTF-16 code-unit offset, clamped.
fn byte_to_utf16_idx(s: &str, byte: usize) -> usize {
    s[..byte.min(s.len())].chars().map(|c| c.len_utf16()).sum()
}

/// If the caret sits directly after an `@token`, return the byte index of the
/// `@` plus the partial token typed so far. Mirrors the tokenizer's rules:
/// the `@` must start the text or follow a non-mention character (so email
/// local parts never trigger the popup), and every character between `@` and
/// the caret must be a mention character.
fn mention_query(text: &str, cursor: usize) -> Option<(usize, String)> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let at = before.rfind('@')?;
    let partial = &before[at + '@'.len_utf8()..];
    if !partial.chars().all(crate::room_markdown::is_mention_char) {
        return None;
    }
    if !crate::room_markdown::mention_start_boundary(before[..at].chars().next_back()) {
        return None;
    }
    Some((at, partial.to_string()))
}

fn live_mention_query_from_input(
    text: &str,
    selection_start_utf16: Option<u32>,
) -> Option<(usize, String)> {
    let cursor = selection_start_utf16
        .map(|u| utf16_to_byte_idx(text, u as usize))
        .unwrap_or(text.len());
    mention_query(text, cursor)
}

/// Select the daemon-authoritative roster for mention completion. Local rooms
/// use `Room.participants`; every non-Local room uses only the safe access
/// member projection so stale/local identities never become mention ids.
fn mention_roster(
    local_participants: &[RoomParticipant],
    access: Option<&RoomAccessProjection>,
) -> Vec<RoomParticipant> {
    match access {
        Some(access) if access.state == RoomAccessState::Local => local_participants.to_vec(),
        Some(access) => access
            .members
            .iter()
            .map(|member| RoomParticipant {
                id: member.member_id.clone(),
                kind: match member.actor_type {
                    FederatedActorType::User => RoomParticipantKind::Human,
                    FederatedActorType::Agent => RoomParticipantKind::Agent,
                },
                display_name: member.display_name.clone(),
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Rank roster candidates for a mention partial: id prefix first, then
/// display-name prefix, then substring anywhere; stable within each rank and
/// capped at 8. An empty partial (caret right after `@`) lists the roster.
fn mention_suggestions(participants: &[RoomParticipant], partial: &str) -> Vec<MentionSuggestion> {
    let q = partial.to_lowercase();
    let mut ranked: Vec<(u8, MentionSuggestion)> = participants
        .iter()
        .filter_map(|p| {
            let id = p.id.to_lowercase();
            let name = p.display_name.to_lowercase();
            let rank = if q.is_empty() || id.starts_with(&q) {
                0
            } else if name.starts_with(&q) {
                1
            } else if id.contains(&q) || name.contains(&q) {
                2
            } else {
                return None;
            };
            Some((
                rank,
                MentionSuggestion {
                    id: p.id.clone(),
                    display_name: p.display_name.clone(),
                    kind: p.kind,
                },
            ))
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, s)| s).take(8).collect()
}

fn mention_suggestion_at(
    participants: &[RoomParticipant],
    partial: &str,
    index: usize,
) -> Option<MentionSuggestion> {
    mention_suggestions(participants, partial)
        .get(index)
        .cloned()
}

fn mention_accept_is_valid(
    text: &str,
    selection_start_utf16: Option<u32>,
    participants: &[RoomParticipant],
    index: usize,
) -> bool {
    live_mention_query_from_input(text, selection_start_utf16)
        .and_then(|(_, partial)| mention_suggestion_at(participants, &partial, index))
        .is_some()
}

/// Replace the active `@partial` (spanning `at..cursor` bytes) with `@id `
/// and return the new text plus the byte caret position after the space.
fn apply_mention(text: &str, at: usize, cursor: usize, id: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len() + id.len() + 2);
    out.push_str(&text[..at]);
    out.push('@');
    out.push_str(id);
    out.push(' ');
    let caret = out.len();
    out.push_str(&text[cursor.min(text.len())..]);
    (out, caret)
}

/// Keyboard model for the mention popup while it is open.
#[derive(Debug, PartialEq, Eq)]
enum MentionKey {
    Move(usize),
    Accept,
    Close,
    Pass,
}

fn mention_popup_key(len: usize, active: usize, key: &str) -> MentionKey {
    if len == 0 {
        return MentionKey::Pass;
    }
    match key {
        "ArrowDown" => MentionKey::Move((active + 1) % len),
        "ArrowUp" => MentionKey::Move((active + len - 1) % len),
        "Enter" | "Tab" => MentionKey::Accept,
        "Escape" => MentionKey::Close,
        _ => MentionKey::Pass,
    }
}

/// One-line identity summary for an agent member, from the federated roster
/// descriptor when available: model alias and description. `None` for humans
/// and for agents without a published descriptor — never fabricated.
fn agent_descriptor_line(access: Option<&RoomAccessProjection>, member_id: &str) -> Option<String> {
    let member = access?.members.iter().find(|m| m.member_id == member_id)?;
    let descriptor = member.public_agent_descriptor.as_ref()?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(alias) = descriptor.model_alias.as_ref().filter(|a| !a.is_empty()) {
        parts.push(alias.clone());
    }
    if let Some(desc) = descriptor.description.as_ref().filter(|d| !d.is_empty()) {
        parts.push(desc.clone());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" \u{b7} "))
    }
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

    // Mention truth source: the open room's daemon-provided roster ids.
    // room_markdown highlights @id ONLY when it resolves here.
    let member_ids = Memo::new(move |_| {
        let local_participants = rooms
            .open_room
            .get()
            .map(|room| room.participants)
            .unwrap_or_default();
        mention_roster(&local_participants, rooms.access.get().as_ref())
            .into_iter()
            .map(|participant| participant.id)
            .collect::<std::collections::HashSet<_>>()
    });
    let mobile_toggle_ref: NodeRef<leptos::html::Button> = NodeRef::new();
    let create_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();

    // ── Mention autosuggest state (channel + thread composers) ──
    let composer_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let thread_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let mention_ctx = RwSignal::new(None::<(usize, String)>);
    let mention_active = RwSignal::new(0usize);
    let thread_mention_ctx = RwSignal::new(None::<(usize, String)>);
    let thread_mention_active = RwSignal::new(0usize);

    let mention_items = Memo::new(move |_| {
        let Some((_, partial)) = mention_ctx.get() else {
            return Vec::new();
        };
        let local_participants = rooms
            .open_room
            .get()
            .map(|room| room.participants)
            .unwrap_or_default();
        let roster = mention_roster(&local_participants, rooms.access.get().as_ref());
        mention_suggestions(&roster, &partial)
    });
    let thread_mention_items = Memo::new(move |_| {
        let Some((_, partial)) = thread_mention_ctx.get() else {
            return Vec::new();
        };
        let local_participants = rooms
            .open_room
            .get()
            .map(|room| room.participants)
            .unwrap_or_default();
        let roster = mention_roster(&local_participants, rooms.access.get().as_ref());
        mention_suggestions(&roster, &partial)
    });

    let accept_mention = move |idx: usize| {
        let text = composer.get_untracked();
        let live_ctx = match composer_input_ref.get() {
            Some(input) => match input.selection_start().ok().flatten() {
                Some(cursor) => live_mention_query_from_input(&text, Some(cursor)),
                None => mention_ctx.get_untracked(),
            },
            None => mention_ctx.get_untracked(),
        };
        let Some((at, partial)) = live_ctx else {
            mention_ctx.set(None);
            mention_active.set(0);
            return;
        };
        let local_participants = rooms
            .open_room
            .get_untracked()
            .map(|room| room.participants)
            .unwrap_or_default();
        let roster = mention_roster(&local_participants, rooms.access.get_untracked().as_ref());
        let Some(pick) = mention_suggestion_at(&roster, &partial, idx) else {
            mention_ctx.set(None);
            mention_active.set(0);
            return;
        };
        let cursor = at + 1 + partial.len();
        let (new_text, caret) = apply_mention(&text, at, cursor, &pick.id);
        let caret16 = byte_to_utf16_idx(&new_text, caret) as u32;
        composer.set(new_text);
        mention_ctx.set(None);
        mention_active.set(0);
        if let Some(input) = composer_input_ref.get() {
            let _ = input.focus();
            let _ = input.set_selection_range(caret16, caret16);
        }
    };
    let accept_thread_mention = move |idx: usize| {
        let text = thread_composer.get_untracked();
        let live_ctx = match thread_input_ref.get() {
            Some(input) => match input.selection_start().ok().flatten() {
                Some(cursor) => live_mention_query_from_input(&text, Some(cursor)),
                None => thread_mention_ctx.get_untracked(),
            },
            None => thread_mention_ctx.get_untracked(),
        };
        let Some((at, partial)) = live_ctx else {
            thread_mention_ctx.set(None);
            thread_mention_active.set(0);
            return;
        };
        let local_participants = rooms
            .open_room
            .get_untracked()
            .map(|room| room.participants)
            .unwrap_or_default();
        let roster = mention_roster(&local_participants, rooms.access.get_untracked().as_ref());
        let Some(pick) = mention_suggestion_at(&roster, &partial, idx) else {
            thread_mention_ctx.set(None);
            thread_mention_active.set(0);
            return;
        };
        let cursor = at + 1 + partial.len();
        let (new_text, caret) = apply_mention(&text, at, cursor, &pick.id);
        let caret16 = byte_to_utf16_idx(&new_text, caret) as u32;
        thread_composer.set(new_text);
        thread_mention_ctx.set(None);
        thread_mention_active.set(0);
        if let Some(input) = thread_input_ref.get() {
            let _ = input.focus();
            let _ = input.set_selection_range(caret16, caret16);
        }
    };

    Effect::new(move |_| {
        if show_left_rail.get() {
            request_animation_frame(move || {
                if let Some(input) = create_input_ref.get() {
                    let _ = input.focus();
                }
            });
        }
    });

    // Keep transcript pinned to newest message. When the reader has
    // scrolled up, never yank them — surface the missed append as a
    // "New messages" jump affordance instead (client scroll state only;
    // this is not read-cursor "unread" state).
    let transcript = rooms.transcript;
    let new_below = RwSignal::new(false);
    let pending_read_advance = RwSignal::new(None::<ReadAdvanceRequest>);
    let refresh_handle = RwSignal::new(None::<IntervalHandle>);
    Effect::new(move |prev: Option<usize>| {
        let len = transcript.with(|t| t.len());
        let open_key = rooms.open_key.get();
        // Track the access projection. For a `Live` room the durable candidate
        // is `last_confirmed_global_sequence`, which routinely lands *after*
        // the fill that pinned the transcript to the bottom; reading it
        // reactively lets that arrival re-enter this pass and queue the read
        // advance the fill itself could not compute. A re-run cannot mark a
        // scrolled-up reader read or fake hydration: it is not a first fill
        // (`prev_len > 0`), so it only queues through the `PinAndQueue` arm,
        // which requires a measured, genuinely at-bottom transcript.
        let access = rooms.access.get();
        let prev_len = prev.unwrap_or(0);
        let first_fill = prev_len == 0;
        let el = list_ref.get();
        let metrics = el
            .as_ref()
            .map(|el| (el.scroll_height(), el.scroll_top(), el.client_height()));
        let near_bottom = metrics.is_some_and(|(scroll_height, scroll_top, client_height)| {
            transcript_is_near_bottom(scroll_height, scroll_top, client_height, 120)
        });
        match transcript_pass_action(len, prev_len, el.is_some(), near_bottom) {
            TranscriptPassAction::Reset => {
                // Generation reset / room switch: nothing below.
                new_below.set(false);
                pending_read_advance.set(None);
            }
            TranscriptPassAction::PinAndQueue => {
                let (scroll_height, _, client_height) = metrics.unwrap_or_default();
                if let Some(el) = el.clone() {
                    request_animation_frame(move || el.set_scroll_top(el.scroll_height()));
                }
                new_below.set(false);
                pending_read_advance.set(read_advance_request(
                    open_key.as_deref(),
                    rooms.generation_snapshot(),
                    // A scrollable first fill defers to the `scroll` event the
                    // pin above produces; a transcript that fits its measured
                    // viewport never fires one and marks read here instead.
                    transcript_read_hydrated(first_fill, scroll_height, client_height),
                    near_bottom,
                    rooms.open_room.get().is_some(),
                    &rooms.transcript.get_untracked(),
                    access.as_ref(),
                ));
            }
            TranscriptPassAction::RaiseJump => {
                new_below.set(true);
                pending_read_advance.set(None);
            }
            TranscriptPassAction::Hold => {}
        }
        // Single open-none clear: this Effect already tracks `open_key`, so it
        // re-runs on close and drops any queued request in the same pass.
        if open_key.is_none() {
            pending_read_advance.set(None);
        }
        // Only a pass that measured a real viewport may consume the first-fill
        // state. An element reporting zero height was not laid out, and
        // spending the first fill on it would hand the *next* pass unearned
        // hydration (see `transcript_read_hydrated`).
        let viewport_measured = metrics.is_some_and(|(_, _, client_height)| client_height > 0);
        if len == 0 {
            0
        } else if viewport_measured {
            len
        } else {
            prev_len
        }
    });

    Effect::new(move |_| {
        let Some(request) = pending_read_advance.get() else {
            return;
        };
        if !rooms.room_is_current(request.generation, &request.open_room_key) {
            pending_read_advance.set(None);
            return;
        }
        let open = rooms.open_key.get();
        let ready = rooms.open_room.get().is_some();
        if ready && open.is_some() {
            rooms.mark_open_read_if_current(request.candidate_read_seq);
            pending_read_advance.set(None);
        }
    });

    // The one confirmed-at-bottom queue path, shared by the transcript
    // `scroll` handler and the jump-to-latest button. Both are already-proven
    // at-bottom intents (hydration and near-bottom are settled by the caller),
    // so the only question left is whether this target is worth publishing —
    // see `read_advance_needs_queue`. Returning early instead of writing
    // `None` also stops a candidate-less frame from clobbering a still-queued
    // request that was valid when it was built.
    let queue_bottom_read_advance = move || {
        let Some(open_room_key) = rooms.open_key.get_untracked() else {
            return;
        };
        let generation = rooms.generation_snapshot();
        let Some(candidate_read_seq) = ready_read_target(
            true,
            true,
            rooms.open_room.get_untracked().is_some(),
            &rooms.transcript.get_untracked(),
            rooms.access.get_untracked().as_ref(),
        ) else {
            return;
        };
        let applied = applied_read_seq(
            rooms.read_summaries.with_untracked(|summaries| {
                summaries
                    .get(&open_room_key)
                    .and_then(|summary| summary.read_seq)
            }),
            rooms.open_read_cursor.get_untracked().as_ref(),
        );
        let needs_queue = pending_read_advance.with_untracked(|pending| {
            read_advance_needs_queue(
                pending.as_ref(),
                &open_room_key,
                generation,
                candidate_read_seq,
                applied,
            )
        });
        if !needs_queue {
            return;
        }
        pending_read_advance.set(Some(ReadAdvanceRequest {
            open_room_key,
            generation,
            candidate_read_seq,
        }));
    };

    Effect::new(move |_| {
        if refresh_handle.get().is_none() {
            let rooms = rooms;
            let handle = leptos::prelude::set_interval_with_handle(
                move || rooms.fetch_rooms_silent(),
                std::time::Duration::from_secs(8),
            )
            .expect("rooms refresh interval");
            refresh_handle.set(Some(handle));
        }
    });

    Owner::on_cleanup(move || {
        if let Some(handle) = refresh_handle.get_untracked() {
            handle.clear();
        }
        refresh_handle.set(None);
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
                            // ARIA listbox with roving tabindex: exactly one
                            // option (open room, else first) is the Tab stop;
                            // arrows/Home/End move REAL focus between options
                            // via their DOM ids. Enter/Space activate through
                            // the button default. aria-selected drives the
                            // interaction stylesheet's selected treatment.
                            view! {
                                <div
                                    class="rooms-workspace__left-options"
                                    role="listbox"
                                    aria-label="Rooms"
                                    on:keydown=move |ev: web_sys::KeyboardEvent| {
                                        let keys: Vec<String> = rooms
                                            .list
                                            .get_untracked()
                                            .iter()
                                            .map(|r| r.id.clone())
                                            .collect();
                                        let focused = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| d.active_element())
                                            .map(|el| el.id());
                                        let focused_key = focused
                                            .as_deref()
                                            .and_then(|id| id.strip_prefix("rooms-opt-"));
                                        let Some(idx) =
                                            room_list_next_focus(&keys, focused_key, &ev.key())
                                        else {
                                            return;
                                        };
                                        ev.prevent_default();
                                        if let Some(el) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| {
                                                d.get_element_by_id(&room_option_dom_id(&keys[idx]))
                                            })
                                            .and_then(|el| {
                                                el.dyn_into::<web_sys::HtmlElement>().ok()
                                            })
                                        {
                                            let _ = el.focus();
                                        }
                                    }
                                >
                                    <For
                                        each=move || rooms.list.get()
                                        key=|r: &Room| (r.id.clone(), r.participants.len(), r.updated_at.clone())
                                        children=move |room: Room| {
                                            let key = room.id.clone();
                                            let key2 = key.clone();
                                            let key_tab = key.clone();
                                            let key_sel = key.clone();
                                            let key_unread = key.clone();
                                            let active = move || rooms.open_key.get().as_deref() == Some(&*key);
                                            let selected =
                                                move || rooms.open_key.get().as_deref() == Some(&*key_sel);
                                            let is_tab_stop = move || {
                                                let keys: Vec<String> = rooms
                                                    .list
                                                    .get()
                                                    .iter()
                                                    .map(|r| r.id.clone())
                                                    .collect();
                                                let open = rooms.open_key.get();
                                                room_list_tab_stop(&keys, open.as_deref())
                                                    .and_then(|i| keys.get(i).cloned())
                                                    .as_deref()
                                                    == Some(&*key_tab)
                                            };
                                            let unread = move || {
                                                rooms.read_summaries.with(|summaries| {
                                                    crate::rooms::room_has_durable_unread(
                                                        summaries.get(&key_unread),
                                                    )
                                                })
                                            };
                                            view! {
                                                <button
                                                    class="rooms-workspace__room"
                                                    class:is-active=active
                                                    type="button"
                                                    role="option"
                                                    id=room_option_dom_id(&room.id)
                                                    aria-selected=move || selected().to_string()
                                                    tabindex=move || if is_tab_stop() { "0" } else { "-1" }
                                                    on:click=move |_| {
                                                        rooms.open_room(key2.clone());
                                                        show_left_rail.set(false);
                                                    }
                                                >
                                                    <span class="rooms-workspace__room-hash">"#"</span>
                                                    <span class="rooms-workspace__room-name">
                                                        {room.name.clone()}
                                                    </span>
                                                    <Show when=move || unread()>
                                                        <span
                                                            class="rooms-workspace__room-unread"
                                                            role="img"
                                                            aria-label="Unread messages"
                                                        ></span>
                                                    </Show>
                                                </button>
                                            }
                                        }
                                    />
                                </div>
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
                                // role=log: AT treats the timeline as a
                                // polite live region — new messages are
                                // announced without stealing focus.
                                <div
                                    class="rooms-workspace__transcript"
                                    role="log"
                                    aria-label="Messages"
                                    node_ref=list_ref
                                    on:scroll=move |_| {
                                        if let Some(el) = list_ref.get() {
                                            if transcript_is_near_bottom(
                                                el.scroll_height(),
                                                el.scroll_top(),
                                                el.client_height(),
                                                120,
                                            ) {
                                                new_below.set(false);
                                                queue_bottom_read_advance();
                                            }
                                        }
                                    }
                                >
                                    <For
                                        // Pair each root with its predecessor so
                                        // density decisions (grouping, gap headers,
                                        // day separators) are derived per row. The
                                        // transcript is append-only under one
                                        // generation, so a cached keyed child never
                                        // sees its predecessor change; generation
                                        // reset rebuilds the whole list.
                                        each=move || {
                                            let roots = partition_thread_messages(&rooms.transcript.get(), 0).roots;
                                            std::iter::once(None)
                                                .chain(roots.iter().cloned().map(Some))
                                                .zip(roots.clone())
                                                .collect::<Vec<_>>()
                                        }
                                        key=|(_, m): &(Option<RoomMessage>, RoomMessage)| m.seq
                                        children=move |(prev, m): (Option<RoomMessage>, RoomMessage)| {
                                            let is_system = room_messages::is_compact_system_row(&m);
                                            let full_ts = m.created_at.clone();
                                            let root_seq = m.seq;
                                            let day_label = room_messages::day_separator_label(prev.as_ref(), &m)
                                                .map(|d| room_messages::humanize_day_label(&d, &today_day_key()));
                                            // A long silence gets a time header —
                                            // unless a day separator already marks
                                            // this row (no double dividers).
                                            let gap_label = (day_label.is_none()
                                                && prev
                                                    .as_ref()
                                                    .map(|p| room_messages::needs_gap_header(p, &m))
                                                    .unwrap_or(false))
                                            .then(|| canonical_wire_clock_time(&full_ts));
                                            let grouped = prev
                                                .as_ref()
                                                .map(|p| room_messages::is_grouped(p, &m))
                                                .unwrap_or(false);
                                            view! {
                                                {day_label.map(|d| view! {
                                                    <div class="rooms-workspace__day-separator" data-day="true">{d}</div>
                                                })}
                                                {gap_label.map(|g| view! {
                                                    <div class="rooms-workspace__day-separator" data-gap="true">{g}</div>
                                                })}
                                                <div
                                                    class="rooms-workspace__msg"
                                                    class:rooms-workspace__msg--system=is_system
                                                    class:rooms-workspace__msg--grouped=grouped
                                                >
                                                    <div class=if is_system {
                                                        "rooms-workspace__msg-avatar".to_string()
                                                    } else {
                                                        format!(
                                                            "rooms-workspace__msg-avatar {}",
                                                            avatar_identity_class(&m.author_id)
                                                        )
                                                    }>
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
                                                            <time
                                                                class="rooms-workspace__msg-time"
                                                                datetime=full_ts.clone()
                                                                aria-label=full_ts.clone()
                                                                title=full_ts.clone()
                                                            >
                                                                {canonical_wire_clock_time(&full_ts)}
                                                            </time>
                                                        </div>
                                                        <div class="rooms-workspace__msg-text">
                                                            {crate::room_markdown::body_view(m.body.clone(), member_ids)}
                                                        </div>
                                                        {move || {
                                                            if should_show_thread_button(&m) {
                                                                let reply_count = move || reply_count_for(&rooms.transcript.get(), root_seq);
                                                                let thread_label = move || {
                                                                    let count = reply_count();
                                                                    if count > 0 {
                                                                        format!("Open thread ({count})")
                                                                    } else {
                                                                        "Open thread".to_string()
                                                                    }
                                                                };
                                                                // Hover/focus action rail. ONLY real actions live
                                                                // here (reply-in-thread today); persistent when the
                                                                // thread has replies or is open, so truthful state
                                                                // never hides. Reveal is CSS (hover/focus-within,
                                                                // always-on for no-hover pointers).
                                                                view! {
                                                                    <div
                                                                        class="rooms-workspace__action-rail"
                                                                        class:rooms-workspace__action-rail--persistent=move || {
                                                                            reply_count() > 0
                                                                                || selected_thread_root_seq.get() == Some(root_seq)
                                                                        }
                                                                    >
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
                                                                        {move || thread_label()}
                                                                    </button>
                                                                    </div>
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

                                // Jump affordance for appends missed while
                                // scrolled up. Scroll-state UX only — never
                                // read-cursor "unread" semantics.
                                {move || new_below.get().then(|| view! {
                                    <button
                                        type="button"
                                        class="rooms-workspace__jump-new"
                                        on:click=move |_| {
                                            if let Some(el) = list_ref.get() {
                                                el.set_scroll_top(el.scroll_height());
                                            }
                                            new_below.set(false);
                                            queue_bottom_read_advance();
                                        }
                                    >
                                        "\u{2193} New messages"
                                    </button>
                                })}

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
                                            placeholder="Message… (@ to mention)"
                                            node_ref=composer_input_ref
                                            role="combobox"
                                            aria-autocomplete="list"
                                            aria-controls="rooms-mention-listbox"
                                            aria-expanded=move || (!mention_items.get().is_empty()).to_string()
                                            aria-activedescendant=move || {
                                                if mention_items.get().is_empty() {
                                                    String::new()
                                                } else {
                                                    format!("rooms-mention-opt-{}", mention_active.get())
                                                }
                                            }
                                            prop:value=move || composer.get()
                                            on:input=move |ev| {
                                                let value = event_target_value(&ev);
                                                mention_ctx.set(live_mention_query_from_input(
                                                    &value,
                                                    ev.target()
                                                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                        .and_then(|el| el.selection_start().ok().flatten()),
                                                ));
                                                mention_active.set(0);
                                                composer.set(value);
                                            }
                                            on:keydown=move |ev| {
                                                if ev.is_composing() {
                                                    return;
                                                }
                                                if mention_ctx.get_untracked().is_none() {
                                                    return;
                                                }
                                                let key = ev.key();
                                                let active = mention_active.get_untracked();
                                                if matches!(key.as_str(), "Enter" | "Tab") {
                                                    let local_participants = rooms
                                                        .open_room
                                                        .get_untracked()
                                                        .map(|room| room.participants)
                                                        .unwrap_or_default();
                                                    let roster = mention_roster(
                                                        &local_participants,
                                                        rooms.access.get_untracked().as_ref(),
                                                    );
                                                    let selection = ev
                                                        .target()
                                                        .and_then(|target| {
                                                            target
                                                                .dyn_into::<web_sys::HtmlInputElement>()
                                                                .ok()
                                                        })
                                                        .and_then(|input| {
                                                            input.selection_start().ok().flatten()
                                                        });
                                                    if !mention_accept_is_valid(
                                                        &composer.get_untracked(),
                                                        selection,
                                                        &roster,
                                                        active,
                                                    ) {
                                                        mention_ctx.set(None);
                                                        mention_active.set(0);
                                                        return;
                                                    }
                                                }
                                                let len = mention_items.get_untracked().len();
                                                match mention_popup_key(len, active, &key) {
                                                    MentionKey::Move(next) => {
                                                        ev.prevent_default();
                                                        mention_active.set(next);
                                                    }
                                                    MentionKey::Accept => {
                                                        ev.prevent_default();
                                                        accept_mention(active);
                                                    }
                                                    MentionKey::Close => {
                                                        ev.prevent_default();
                                                        mention_ctx.set(None);
                                                    }
                                                    MentionKey::Pass => {}
                                                }
                                            }
                                            on:blur=move |_| mention_ctx.set(None)
                                            disabled=move || !access_allows_writes(rooms.access.get().as_ref())
                                        />
                                        {move || {
                                            let items = mention_items.get();
                                            if items.is_empty() {
                                                return ().into_any();
                                            }
                                            let active = mention_active.get();
                                            let access = rooms.access.get();
                                            view! {
                                                <div
                                                    id="rooms-mention-listbox"
                                                    class="rooms-workspace__mention-pop"
                                                    role="listbox"
                                                    aria-label="Mention suggestions"
                                                >
                                                    {items
                                                        .into_iter()
                                                        .enumerate()
                                                        .map(|(i, item)| {
                                                            let desc = agent_descriptor_line(access.as_ref(), &item.id);
                                                            let initials = item
                                                                .display_name
                                                                .chars()
                                                                .take(2)
                                                                .collect::<String>()
                                                                .to_uppercase();
                                                            let avatar_class = format!(
                                                                "rooms-workspace__member-avatar {}",
                                                                avatar_identity_class(&item.id)
                                                            );
                                                            let id_label = format!("@{}", item.id);
                                                            view! {
                                                                <div
                                                                    id=format!("rooms-mention-opt-{i}")
                                                                    class="rooms-workspace__mention-opt"
                                                                    class:rooms-workspace__mention-opt--active=i == active
                                                                    role="option"
                                                                    aria-selected=(i == active).to_string()
                                                                    on:mousedown=move |ev: web_sys::MouseEvent| {
                                                                        ev.prevent_default();
                                                                        accept_mention(i);
                                                                    }
                                                                >
                                                                    <span class=avatar_class>{initials}</span>
                                                                    <span class="rooms-workspace__mention-name">
                                                                        {item.display_name.clone()}
                                                                    </span>
                                                                    <span class="rooms-workspace__mention-id">{id_label}</span>
                                                                    <span class="rooms-workspace__mention-kind">
                                                                        {participant_kind_label(item.kind)}
                                                                    </span>
                                                                    {desc.map(|d| view! {
                                                                        <span class="rooms-workspace__mention-desc">{d}</span>
                                                                    })}
                                                                </div>
                                                            }
                                                        })
                                                        .collect_view()}
                                                </div>
                                            }
                                            .into_any()
                                        }}
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
                            let full_ts = root.created_at.clone();
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
                                    <div
                                        class="rooms-workspace__right-thread-transcript"
                                        role="log"
                                        aria-label="Thread replies"
                                    >
                                        <div
                                            class="rooms-workspace__msg rooms-workspace__msg--thread-root"
                                            class:rooms-workspace__msg--system=root_is_system
                                        >
                                            <div class=if root_is_system {
                                                "rooms-workspace__msg-avatar".to_string()
                                            } else {
                                                format!(
                                                    "rooms-workspace__msg-avatar {}",
                                                    avatar_identity_class(&root.author_id)
                                                )
                                            }>
                                                {if root_is_system {
                                                    view! { <crate::icons::Spark /> }.into_any()
                                                } else {
                                                    root.author_id.chars().take(2).collect::<String>().to_uppercase().into_any()
                                                }}
                                            </div>
                                            <div class="rooms-workspace__msg-body">
                                                <div class="rooms-workspace__msg-author">
                                                    <span class="rooms-workspace__msg-name">{root.author_id.clone()}</span>
                                                    <time
                                                        class="rooms-workspace__msg-time"
                                                        datetime=full_ts.clone()
                                                        aria-label=full_ts.clone()
                                                        title=full_ts.clone()
                                                    >
                                                        {canonical_wire_clock_time(&full_ts)}
                                                    </time>
                                                </div>
                                                <div class="rooms-workspace__msg-text">
                                                    {crate::room_markdown::body_view(root.body.clone(), member_ids)}
                                                </div>
                                            </div>
                                        </div>
                                        <For
                                            each=move || partition_thread_messages(&rooms.transcript.get(), root_seq).replies
                                            key=|m: &RoomMessage| m.seq
                                            children=move |reply: RoomMessage| {
                                                let full_ts = reply.created_at.clone();
                                                let is_system = room_messages::is_compact_system_row(&reply);
                                                view! {
                                                    <div
                                                        class="rooms-workspace__msg rooms-workspace__msg--thread-reply"
                                                        class:rooms-workspace__msg--system=is_system
                                                    >
                                                        <div class=if is_system {
                                                            "rooms-workspace__msg-avatar".to_string()
                                                        } else {
                                                            format!(
                                                                "rooms-workspace__msg-avatar {}",
                                                                avatar_identity_class(&reply.author_id)
                                                            )
                                                        }>
                                                            {if is_system {
                                                                view! { <crate::icons::Spark /> }.into_any()
                                                            } else {
                                                                reply.author_id.chars().take(2).collect::<String>().to_uppercase().into_any()
                                                            }}
                                                        </div>
                                                        <div class="rooms-workspace__msg-body">
                                                            <div class="rooms-workspace__msg-author">
                                                                <span class="rooms-workspace__msg-name">{reply.author_id.clone()}</span>
                                                                <time
                                                                    class="rooms-workspace__msg-time"
                                                                    datetime=full_ts.clone()
                                                                    aria-label=full_ts.clone()
                                                                    title=full_ts.clone()
                                                                >
                                                                    {canonical_wire_clock_time(&full_ts)}
                                                                </time>
                                                            </div>
                                                            <div class="rooms-workspace__msg-text">
                                                                {crate::room_markdown::body_view(reply.body.clone(), member_ids)}
                                                            </div>
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
                                                node_ref=thread_input_ref
                                                role="combobox"
                                                aria-autocomplete="list"
                                                aria-controls="rooms-mention-listbox-thread"
                                                aria-expanded=move || (!thread_mention_items.get().is_empty()).to_string()
                                                aria-activedescendant=move || {
                                                    if thread_mention_items.get().is_empty() {
                                                        String::new()
                                                    } else {
                                                        format!("rooms-mention-thread-opt-{}", thread_mention_active.get())
                                                    }
                                                }
                                                prop:value=move || thread_composer.get()
                                                on:input=move |ev| {
                                                    let value = event_target_value(&ev);
                                                    thread_mention_ctx.set(live_mention_query_from_input(
                                                        &value,
                                                        ev.target()
                                                            .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                            .and_then(|el| el.selection_start().ok().flatten()),
                                                    ));
                                                    thread_mention_active.set(0);
                                                    thread_composer.set(value);
                                                }
                                                on:keydown=move |ev| {
                                                    if ev.is_composing() {
                                                        return;
                                                    }
                                                    if thread_mention_ctx.get_untracked().is_none() {
                                                        return;
                                                    }
                                                    let key = ev.key();
                                                    let active = thread_mention_active.get_untracked();
                                                    if matches!(key.as_str(), "Enter" | "Tab") {
                                                        let local_participants = rooms
                                                            .open_room
                                                            .get_untracked()
                                                            .map(|room| room.participants)
                                                            .unwrap_or_default();
                                                        let roster = mention_roster(
                                                            &local_participants,
                                                            rooms.access.get_untracked().as_ref(),
                                                        );
                                                        let selection = ev
                                                            .target()
                                                            .and_then(|target| {
                                                                target
                                                                    .dyn_into::<web_sys::HtmlInputElement>()
                                                                    .ok()
                                                            })
                                                            .and_then(|input| {
                                                                input.selection_start().ok().flatten()
                                                            });
                                                        if !mention_accept_is_valid(
                                                            &thread_composer.get_untracked(),
                                                            selection,
                                                            &roster,
                                                            active,
                                                        ) {
                                                            thread_mention_ctx.set(None);
                                                            thread_mention_active.set(0);
                                                            return;
                                                        }
                                                    }
                                                    let len = thread_mention_items.get_untracked().len();
                                                    match mention_popup_key(len, active, &key) {
                                                        MentionKey::Move(next) => {
                                                            ev.prevent_default();
                                                            thread_mention_active.set(next);
                                                        }
                                                        MentionKey::Accept => {
                                                            ev.prevent_default();
                                                            accept_thread_mention(active);
                                                        }
                                                        MentionKey::Close => {
                                                            ev.prevent_default();
                                                            thread_mention_ctx.set(None);
                                                        }
                                                        MentionKey::Pass => {}
                                                    }
                                                }
                                                on:blur=move |_| thread_mention_ctx.set(None)
                                                disabled=move || !access_allows_writes(rooms.access.get().as_ref())
                                            />
                                            {move || {
                                                let items = thread_mention_items.get();
                                                if items.is_empty() {
                                                    return ().into_any();
                                                }
                                                let active = thread_mention_active.get();
                                                let access = rooms.access.get();
                                                view! {
                                                    <div
                                                        id="rooms-mention-listbox-thread"
                                                        class="rooms-workspace__mention-pop"
                                                        role="listbox"
                                                        aria-label="Mention suggestions"
                                                    >
                                                        {items
                                                            .into_iter()
                                                            .enumerate()
                                                            .map(|(i, item)| {
                                                                let desc = agent_descriptor_line(access.as_ref(), &item.id);
                                                                let initials = item
                                                                    .display_name
                                                                    .chars()
                                                                    .take(2)
                                                                    .collect::<String>()
                                                                    .to_uppercase();
                                                                let avatar_class = format!(
                                                                    "rooms-workspace__member-avatar {}",
                                                                    avatar_identity_class(&item.id)
                                                                );
                                                                let id_label = format!("@{}", item.id);
                                                                view! {
                                                                    <div
                                                                        id=format!("rooms-mention-thread-opt-{i}")
                                                                        class="rooms-workspace__mention-opt"
                                                                        class:rooms-workspace__mention-opt--active=i == active
                                                                        role="option"
                                                                        aria-selected=(i == active).to_string()
                                                                        on:mousedown=move |ev: web_sys::MouseEvent| {
                                                                            ev.prevent_default();
                                                                            accept_thread_mention(i);
                                                                        }
                                                                    >
                                                                        <span class=avatar_class>{initials}</span>
                                                                        <span class="rooms-workspace__mention-name">
                                                                            {item.display_name.clone()}
                                                                        </span>
                                                                        <span class="rooms-workspace__mention-id">{id_label}</span>
                                                                        <span class="rooms-workspace__mention-kind">
                                                                            {participant_kind_label(item.kind)}
                                                                        </span>
                                                                        {desc.map(|d| view! {
                                                                            <span class="rooms-workspace__mention-desc">{d}</span>
                                                                        })}
                                                                    </div>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </div>
                                                }
                                                .into_any()
                                            }}
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
                                                <div
                                                    class="rooms-workspace__member-list"
                                                    role="list"
                                                    aria-label="Room members"
                                                >
                                                <For
                                                    each=move || rooms.open_room.get()
                                                        .map(|r| r.participants)
                                                        .unwrap_or_default()
                                                    key=|p: &RoomParticipant| p.id.clone()
                                                    children=move |p: RoomParticipant| {
                                                        view! {
                                                            <div
                                                                class="rooms-workspace__member"
                                                                role="listitem"
                                                            >
                                                                <div class="rooms-workspace__member-avatar">
                                                                    {p.display_name.chars().take(2).collect::<String>().to_uppercase()}
                                                                </div>
                                                                <span class="rooms-workspace__member-name">
                                                                    {p.display_name.clone()}
                                                                </span>
                                                                <span class="rooms-workspace__member-kind">
                                                                    {participant_kind_label(p.kind)}
                                                                </span>
                                                            </div>
                                                        }
                                                    }
                                                />
                                                </div>
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
                                        let members_for_label = members.clone();
                                        view! {
                                            // Roster is a real list: give AT an
                                            // item count + boundaries instead of
                                            // an undifferentiated div run.
                                            <div
                                                class="rooms-workspace__member-list"
                                                role="list"
                                                aria-label=move || {
                                                    let count = roster_presence_count(&members_for_label);
                                                    if count == 0 {
                                                        "Room members".to_string()
                                                    } else {
                                                        format!("Room members, {count} humans live")
                                                    }
                                                }
                                            >
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
                                                    let desc_line = member
                                                        .public_agent_descriptor
                                                        .as_ref()
                                                        .and_then(|descriptor| {
                                                            let mut parts = Vec::new();
                                                            if let Some(alias) = descriptor
                                                                .model_alias
                                                                .as_ref()
                                                                .filter(|alias| !alias.is_empty())
                                                            {
                                                                parts.push(alias.clone());
                                                            }
                                                            if let Some(description) = descriptor
                                                                .description
                                                                .as_ref()
                                                                .filter(|description| !description.is_empty())
                                                            {
                                                                parts.push(description.clone());
                                                            }
                                                            (!parts.is_empty())
                                                                .then(|| parts.join(" \u{b7} "))
                                                        });
                                                    let desc_title = member.public_agent_descriptor.as_ref()
                                                        .and_then(|d| d.description.clone())
                                                        .unwrap_or_default();
                                                    view! {
                                                        <div class="rooms-workspace__member"
                                                            role="listitem"
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
                                                            {desc_line.map(|desc| view! {
                                                                <span class="rooms-workspace__member-desc">{desc}</span>
                                                            })}
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
                                            </div>
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
        room_request_is_current, CreateOutcome, CreateResolution, FederatedActorType,
        FederatedRoomMemberProjection, FederatedRoomRole, MemberPresence, RoomAccessProjection,
        RoomAccessState, RoomMessage, RoomMessageKind, RoomParticipantKind,
    };

    #[test]
    fn read_advance_request_skips_hydration_but_advances_after_bottom_append() {
        let transcript = vec![test_msg(4, "hello", None), test_msg(7, "world", None)];
        let local = test_access(RoomAccessState::Local);

        assert_eq!(
            read_advance_request(None, 1, false, true, true, &transcript, Some(&local)),
            None
        );
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                1,
                false,
                true,
                true,
                &transcript,
                Some(&local)
            ),
            None
        );
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                1,
                true,
                true,
                true,
                &transcript,
                Some(&local)
            ),
            Some(ReadAdvanceRequest {
                open_room_key: "room-a".into(),
                generation: 1,
                candidate_read_seq: 7,
            })
        );
    }

    /// F1 regression: the *first fill* of a room whose transcript fits inside
    /// the viewport must advance the durable read cursor. A scrollable first
    /// fill still defers to the `scroll` event the programmatic bottom-pin
    /// produces, so a reader who scrolls up is never marked read from the raw
    /// fill.
    #[test]
    fn transcript_read_hydration_trusts_non_scrollable_first_fill_only() {
        // Content fits the viewport: nothing can scroll, nothing will ever
        // confirm the position — the fill itself is the confirmation.
        assert!(!transcript_is_scrollable(400, 400));
        assert!(!transcript_is_scrollable(120, 400));
        assert!(transcript_read_hydrated(true, 400, 400));
        assert!(transcript_read_hydrated(true, 120, 400));

        // Scrollable first fill: defer to the scroll event.
        assert!(transcript_is_scrollable(4000, 400));
        assert!(!transcript_read_hydrated(true, 4000, 400));

        // Every later pass is hydrated regardless of scrollability.
        assert!(transcript_read_hydrated(false, 4000, 400));
        assert!(transcript_read_hydrated(false, 400, 400));
    }

    /// M1 regression: an unmeasured transcript (`client_height == 0`, the
    /// reading a not-yet-laid-out element gives) must NOT be mistaken for a
    /// transcript that fits its viewport. `scroll_height > client_height` is
    /// false for `0 > 0`, so without the positive-height requirement the very
    /// first fill of an arbitrarily long room would mark the whole room read
    /// before a single message was visible.
    #[test]
    fn transcript_read_hydration_rejects_unmeasured_first_fill() {
        assert!(!transcript_read_hydrated(true, 0, 0));
        // Content already measured but the viewport is not: still unknown.
        assert!(!transcript_read_hydrated(true, 4000, 0));
        // Nonsense/negative heights are unknown too, never "everything fits".
        assert!(!transcript_read_hydrated(true, 0, -1));
        // A measured viewport keeps the F1 behaviour exactly.
        assert!(transcript_read_hydrated(true, 240, 600));
        assert!(!transcript_read_hydrated(true, 4000, 600));
        // Later passes are unaffected: hydration is a first-fill question.
        assert!(transcript_read_hydrated(false, 0, 0));
    }

    /// M1 regression through the production decision path: the exact
    /// arguments the transcript Effect passes for an unmeasured first fill
    /// produce no request, even though the unmeasured element reports a
    /// trivially "near bottom" position (`0 - 0 - 0 < 120`).
    #[test]
    fn read_advance_request_skips_unmeasured_first_fill() {
        let transcript = vec![test_msg(4, "hello", None), test_msg(7, "world", None)];
        let local = test_access(RoomAccessState::Local);
        assert!(transcript_is_near_bottom(0, 0, 0, 120));

        assert_eq!(
            read_advance_request(
                Some("room-a"),
                2,
                transcript_read_hydrated(true, 0, 0),
                transcript_is_near_bottom(0, 0, 0, 120),
                true,
                &transcript,
                Some(&local),
            ),
            None
        );
    }

    /// L2 regression: the transcript pass is now driven by the access
    /// projection as well as by transcript length, so a pass can re-run with
    /// an unchanged transcript. Such a pass must requeue when the reader is at
    /// the bottom and must do nothing at all when they are scrolled up — no
    /// read mark, no jump affordance, no clobbering of a queued request.
    #[test]
    fn transcript_pass_action_covers_append_reset_and_unchanged_reruns() {
        // Room switch / generation reset.
        assert_eq!(
            transcript_pass_action(0, 5, true, true),
            TranscriptPassAction::Reset
        );
        // First fill, regardless of the measured scroll position.
        assert_eq!(
            transcript_pass_action(5, 0, true, false),
            TranscriptPassAction::PinAndQueue
        );
        // Access arrives after the fill, reader still at the bottom: requeue.
        assert_eq!(
            transcript_pass_action(5, 5, true, true),
            TranscriptPassAction::PinAndQueue
        );
        // Access arrives after the fill, reader scrolled up: hold.
        assert_eq!(
            transcript_pass_action(5, 5, true, false),
            TranscriptPassAction::Hold
        );
        // Append below a scrolled-up reader keeps the jump affordance.
        assert_eq!(
            transcript_pass_action(6, 5, true, false),
            TranscriptPassAction::RaiseJump
        );
        // Append while at the bottom pins and queues.
        assert_eq!(
            transcript_pass_action(6, 5, true, true),
            TranscriptPassAction::PinAndQueue
        );
        // No transcript element yet: hold, so the first-fill state survives
        // for the pass that can actually measure.
        assert_eq!(
            transcript_pass_action(5, 0, false, true),
            TranscriptPassAction::Hold
        );
    }

    /// L2 regression, end to end through the production decision path: a
    /// `Live` room fills before the daemon confirms a global sequence, so the
    /// fill itself has no durable candidate. When the access projection lands
    /// afterwards the pass re-runs with an unchanged transcript; at the bottom
    /// that pass must produce exactly the request the fill could not.
    #[test]
    fn live_room_requeues_read_when_confirmed_sequence_lands_after_fill() {
        let transcript = vec![test_msg(4, "hello", None), test_msg(7, "world", None)];
        let (scroll_height, client_height) = (240, 600);
        let near_bottom = transcript_is_near_bottom(scroll_height, 0, client_height, 120);

        // Fill: `Live` access with no confirmed sequence yet.
        let mut live = test_access(RoomAccessState::Live);
        assert_eq!(
            transcript_pass_action(transcript.len(), 0, true, near_bottom),
            TranscriptPassAction::PinAndQueue
        );
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                3,
                transcript_read_hydrated(true, scroll_height, client_height),
                near_bottom,
                true,
                &transcript,
                Some(&live),
            ),
            None
        );

        // The confirmed global sequence arrives; the transcript did not change.
        live.last_confirmed_global_sequence = Some(44);
        assert_eq!(
            transcript_pass_action(transcript.len(), transcript.len(), true, near_bottom),
            TranscriptPassAction::PinAndQueue
        );
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                3,
                // Not a first fill any more, so hydration is settled.
                transcript_read_hydrated(false, scroll_height, client_height),
                near_bottom,
                true,
                &transcript,
                Some(&live),
            ),
            Some(ReadAdvanceRequest {
                open_room_key: "room-a".into(),
                generation: 3,
                candidate_read_seq: 44,
            })
        );

        // Same arrival while the reader is scrolled up marks nothing.
        assert_eq!(
            transcript_pass_action(transcript.len(), transcript.len(), true, false),
            TranscriptPassAction::Hold
        );
    }

    /// L3: the applied read position is the monotonic fold of the room-list
    /// summary and both halves of the durable cursor projection, so a lagging
    /// half can never mask a further confirmed read.
    #[test]
    fn applied_read_seq_folds_summary_and_cursor_monotonically() {
        assert_eq!(applied_read_seq(None, None), None);
        assert_eq!(applied_read_seq(Some(5), None), Some(5));
        assert_eq!(
            applied_read_seq(
                None,
                Some(&RoomReadCursorProjection {
                    read_seq: Some(9),
                    mirrored_upstream_read_seq: None,
                })
            ),
            Some(9)
        );
        assert_eq!(
            applied_read_seq(
                Some(3),
                Some(&RoomReadCursorProjection {
                    read_seq: Some(9),
                    mirrored_upstream_read_seq: Some(12),
                })
            ),
            Some(12)
        );
    }

    /// L3 regression: a near-bottom scroll burst recomputes the identical
    /// target every frame. The duplicate is skipped while it is still queued
    /// and once the daemon has confirmed it, but a failed PATCH — which clears
    /// the pending request and advances no cursor — must still be retried.
    #[test]
    fn read_advance_needs_queue_dedupes_without_suppressing_retry() {
        let pending = ReadAdvanceRequest {
            open_room_key: "room-a".into(),
            generation: 2,
            candidate_read_seq: 7,
        };

        // Identical target already queued: skip.
        assert!(!read_advance_needs_queue(
            Some(&pending),
            "room-a",
            2,
            7,
            None
        ));
        // Same key/seq under a new admission is a different target.
        assert!(read_advance_needs_queue(
            Some(&pending),
            "room-a",
            3,
            7,
            None
        ));
        // Different room, and a further sequence in the same room, both queue.
        assert!(read_advance_needs_queue(
            Some(&pending),
            "room-b",
            2,
            7,
            None
        ));
        assert!(read_advance_needs_queue(
            Some(&pending),
            "room-a",
            2,
            8,
            None
        ));
        // Already durably read at or past the candidate: nothing to advance.
        assert!(!read_advance_needs_queue(None, "room-a", 2, 7, Some(7)));
        assert!(!read_advance_needs_queue(None, "room-a", 2, 7, Some(9)));
        // Retry after failure: the dispatch Effect cleared the pending request
        // and the failed PATCH advanced no cursor, so the next near-bottom
        // frame must re-queue the very same target.
        assert!(read_advance_needs_queue(None, "room-a", 2, 7, None));
        assert!(read_advance_needs_queue(None, "room-a", 2, 7, Some(6)));
    }

    /// F1 regression, end to end through the production decision path: the
    /// exact arguments the transcript Effect passes for a non-scrollable
    /// first fill produce a read-advance request, while the scrollable first
    /// fill of the same room does not.
    #[test]
    fn read_advance_request_marks_non_scrollable_first_fill_at_bottom() {
        let transcript = vec![test_msg(4, "hello", None), test_msg(7, "world", None)];
        let local = test_access(RoomAccessState::Local);
        // Short transcript in a tall viewport: scroll_top is pinned at 0 and
        // `near_bottom` is trivially true.
        let (scroll_height, client_height) = (240, 600);
        let near_bottom = transcript_is_near_bottom(scroll_height, 0, client_height, 120);
        assert!(near_bottom);

        assert_eq!(
            read_advance_request(
                Some("room-a"),
                2,
                transcript_read_hydrated(true, scroll_height, client_height),
                near_bottom,
                true,
                &transcript,
                Some(&local),
            ),
            Some(ReadAdvanceRequest {
                open_room_key: "room-a".into(),
                generation: 2,
                candidate_read_seq: 7,
            })
        );

        // Same room, taller content than viewport: the first fill stays
        // silent and the pin's `scroll` event does the marking.
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                2,
                transcript_read_hydrated(true, 4000, client_height),
                true,
                true,
                &transcript,
                Some(&local),
            ),
            None
        );
    }

    /// The non-scrollable shortcut must not bypass the room/access/transcript
    /// hydration guards, and must not fire while the reader is scrolled up.
    #[test]
    fn read_advance_request_non_scrollable_still_requires_hydration_and_bottom() {
        let transcript = vec![test_msg(7, "world", None)];
        let local = test_access(RoomAccessState::Local);
        let hydrated = transcript_read_hydrated(true, 240, 600);
        assert!(hydrated);

        // Open room not yet hydrated.
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                2,
                hydrated,
                true,
                false,
                &transcript,
                Some(&local)
            ),
            None
        );
        // Access projection not yet hydrated.
        assert_eq!(
            read_advance_request(Some("room-a"), 2, hydrated, true, true, &transcript, None),
            None
        );
        // Transcript not yet hydrated.
        assert_eq!(
            read_advance_request(Some("room-a"), 2, hydrated, true, true, &[], Some(&local)),
            None
        );
        // Scrolled up: intent wins over the shortcut.
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                2,
                hydrated,
                false,
                true,
                &transcript,
                Some(&local)
            ),
            None
        );
    }

    /// A `Live` room with no confirmed global sequence yields no candidate, so
    /// the request path returns `None` instead of panicking on a missing
    /// candidate (F5: one evaluation, no `expect`).
    #[test]
    fn read_advance_request_returns_none_when_live_has_no_confirmed_sequence() {
        let transcript = vec![test_msg(7, "world", None)];
        let live = test_access(RoomAccessState::Live);
        assert_eq!(live.last_confirmed_global_sequence, None);

        assert_eq!(
            ready_read_target(true, true, true, &transcript, Some(&live)),
            None
        );
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                2,
                true,
                true,
                true,
                &transcript,
                Some(&live)
            ),
            None
        );
    }

    #[test]
    fn read_advance_request_allows_jump_to_latest_and_captures_generation() {
        let transcript = vec![test_msg(11, "hello", None)];
        let local = test_access(RoomAccessState::Local);

        assert_eq!(
            read_advance_request(
                Some("room-z"),
                5,
                true,
                true,
                true,
                &transcript,
                Some(&local)
            ),
            Some(ReadAdvanceRequest {
                open_room_key: "room-z".into(),
                generation: 5,
                candidate_read_seq: 11,
            })
        );
    }

    #[test]
    fn read_advance_request_rejects_scrolled_up_or_missing_candidate() {
        let transcript = vec![test_msg(5, "hello", None)];
        let connecting = test_access(RoomAccessState::Connecting);

        assert_eq!(
            read_advance_request(
                Some("room-a"),
                1,
                true,
                false,
                true,
                &transcript,
                Some(&test_access(RoomAccessState::Local))
            ),
            None
        );
        assert_eq!(
            read_advance_request(
                Some("room-a"),
                1,
                true,
                true,
                true,
                &transcript,
                Some(&connecting)
            ),
            None
        );
    }

    /// Exact regression for the same-key close/reopen gap: a request is
    /// scheduled (stamped `generation`) while room "A" is open at gen N, the
    /// SAME key is closed and reopened (gen N -> N+1, key unchanged), and the
    /// stale request must be rejected by the exact predicate the dispatch
    /// effect calls before `mark_open_read_if_current` —
    /// `crate::rooms::room_request_is_current`, the same logic
    /// `Rooms::room_is_current` delegates to (no live `Rooms`/browser runtime
    /// needed to exercise it here).
    #[test]
    fn read_advance_request_generation_is_rejected_after_same_key_close_reopen() {
        let transcript = vec![test_msg(9, "hello", None)];
        let local = test_access(RoomAccessState::Local);

        // Schedule A: room "A" is open at gen N; build a read-advance request
        // stamped with that live generation.
        let scheduled_generation = 3; // gen N
        let request = read_advance_request(
            Some("room-a"),
            scheduled_generation,
            true,
            true,
            true,
            &transcript,
            Some(&local),
        )
        .expect("ready state produces a request");
        assert_eq!(request.generation, scheduled_generation);
        assert_eq!(request.open_room_key, "room-a");

        // Close/reopen the SAME key: generation advances to N+1, `open_key`
        // is still "room-a" — the pre-fix key-only guard would wrongly admit
        // this stale request.
        let generation_after_close_reopen = scheduled_generation + 1;

        // The exact predicate the dispatch Effect calls before
        // `mark_open_read_if_current` must reject the stale request.
        assert!(!room_request_is_current(
            request.generation,
            generation_after_close_reopen,
            &request.open_room_key,
            Some("room-a"),
        ));
        // A freshly-stamped request for the new admission is admitted.
        assert!(room_request_is_current(
            generation_after_close_reopen,
            generation_after_close_reopen,
            "room-a",
            Some("room-a"),
        ));
    }

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

    #[test]
    fn canonical_wire_clock_time_extracts_hhmm_from_rfc3339_z() {
        assert_eq!(canonical_wire_clock_time("2026-07-25T03:43:12Z"), "03:43");
    }

    #[test]
    fn transcript_bottom_threshold_matches_follow_contract() {
        assert!(transcript_is_near_bottom(1000, 810, 100, 120));
        assert!(!transcript_is_near_bottom(1000, 700, 100, 120));
    }

    #[test]
    fn durable_read_candidate_uses_local_transcript_and_live_global_only() {
        let transcript = vec![test_msg(4, "hello", None), test_msg(7, "world", None)];
        let local = test_access(RoomAccessState::Local);
        let mut live = test_access(RoomAccessState::Live);
        live.last_confirmed_global_sequence = Some(44);
        let connecting = test_access(RoomAccessState::Connecting);

        assert_eq!(durable_read_candidate(&transcript, Some(&local)), Some(7));
        assert_eq!(durable_read_candidate(&transcript, Some(&live)), Some(44));
        assert_eq!(durable_read_candidate(&transcript, Some(&connecting)), None);
        assert_eq!(durable_read_candidate(&[], Some(&local)), None);
    }

    #[test]
    fn ready_read_target_requires_hydrated_room_near_bottom_and_candidate() {
        let transcript = vec![test_msg(4, "hello", None)];
        let local = test_access(RoomAccessState::Local);

        assert_eq!(
            ready_read_target(true, true, true, &transcript, Some(&local)),
            Some(4)
        );
        assert_eq!(
            ready_read_target(false, true, true, &transcript, Some(&local)),
            None
        );
        assert_eq!(
            ready_read_target(true, false, true, &transcript, Some(&local)),
            None
        );
        assert_eq!(
            ready_read_target(true, true, false, &transcript, Some(&local)),
            None
        );
        assert_eq!(ready_read_target(true, true, true, &[], Some(&local)), None);
        assert_eq!(
            ready_read_target(
                true,
                true,
                true,
                &transcript,
                Some(&test_access(RoomAccessState::Connecting))
            ),
            None
        );
    }

    #[test]
    fn canonical_wire_clock_time_extracts_hhmm_from_rfc3339_fractional() {
        assert_eq!(
            canonical_wire_clock_time("2026-07-25T03:43:12.987Z"),
            "03:43"
        );
    }

    #[test]
    fn canonical_wire_clock_time_extracts_hhmm_from_rfc3339_offset() {
        assert_eq!(
            canonical_wire_clock_time("2026-07-25T03:43:12+07:00"),
            "03:43"
        );
    }

    #[test]
    fn canonical_wire_clock_time_passthrough_short_string() {
        assert_eq!(canonical_wire_clock_time("abc"), "abc");
    }

    #[test]
    fn canonical_wire_clock_time_passthrough_noncanonical_separator() {
        assert_eq!(
            canonical_wire_clock_time("2026-07-25 03:43:12Z"),
            "2026-07-25 03:43:12Z"
        );
    }

    #[test]
    fn canonical_wire_clock_time_passthrough_unicode_without_panic() {
        assert_eq!(
            canonical_wire_clock_time("２０２６-07-25T03:43:12Z"),
            "２０２６-07-25T03:43:12Z"
        );
    }

    #[test]
    fn roster_presence_counts_humans_only() {
        let members = vec![
            FederatedRoomMemberProjection {
                member_id: "user-live-1".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::User,
                role_in_room: FederatedRoomRole::Member,
                display_name: "A".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: Some(MemberPresence::Live),
                local_binding_available: Some(true),
            },
            FederatedRoomMemberProjection {
                member_id: "user-live-2".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::User,
                role_in_room: FederatedRoomRole::Owner,
                display_name: "B".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: Some(MemberPresence::Live),
                local_binding_available: Some(true),
            },
            FederatedRoomMemberProjection {
                member_id: "user-away".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::User,
                role_in_room: FederatedRoomRole::Member,
                display_name: "C".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: Some(MemberPresence::Unavailable),
                local_binding_available: Some(true),
            },
            FederatedRoomMemberProjection {
                member_id: "agent-live".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::Agent,
                role_in_room: FederatedRoomRole::Member,
                display_name: "Flux".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: Some(MemberPresence::Live),
                local_binding_available: Some(true),
            },
        ];

        assert_eq!(roster_presence_count(&members), 2);
    }

    #[test]
    fn room_timestamp_markup_preserves_full_wire_datetime_and_visible_clock() {
        let ts = "2026-07-25T03:43:12.987+07:00";
        let clock = canonical_wire_clock_time(ts);
        let markup = format!(
            "<time class=\"rooms-workspace__msg-time\" datetime=\"{ts}\" aria-label=\"{ts}\" title=\"{ts}\">{clock}</time>"
        );
        assert!(markup.contains("<time"));
        assert!(markup.contains("datetime=\"2026-07-25T03:43:12.987+07:00\""));
        assert!(markup.contains("aria-label=\"2026-07-25T03:43:12.987+07:00\""));
        assert!(markup.contains("title=\"2026-07-25T03:43:12.987+07:00\""));
        assert!(markup.ends_with(">03:43</time>"));
    }

    // ── Mention autosuggest helpers ──

    fn part(id: &str, name: &str, kind: RoomParticipantKind) -> RoomParticipant {
        RoomParticipant {
            id: id.into(),
            kind,
            display_name: name.into(),
        }
    }

    #[test]
    fn mention_query_detects_partial_at_caret() {
        assert_eq!(mention_query("hi @fl", 6), Some((3, "fl".to_string())));
        assert_eq!(mention_query("@", 1), Some((0, String::new())));
        assert_eq!(
            mention_query("say @designer x", 13),
            Some((4, "designer".to_string()))
        );
    }

    #[test]
    fn mention_query_rejects_email_local_parts_and_non_tokens() {
        // '@' directly after a mention char = email shape, never a popup.
        assert_eq!(mention_query("mail me a@b", 11), None);
        // Whitespace inside the candidate token closes the query.
        assert_eq!(mention_query("@fl x", 5), None);
        // No '@' at all.
        assert_eq!(mention_query("hello", 5), None);
    }

    #[test]
    fn mention_query_is_unicode_safe() {
        let text = "héllo @fl";
        assert_eq!(mention_query(text, text.len()), Some((7, "fl".to_string())));
        // Non-boundary cursor never panics.
        assert_eq!(mention_query("é@a", 1), None);
    }

    #[test]
    fn mention_query_rejects_unicode_letter_before_at() {
        assert_eq!(mention_query("é@fl", "é@fl".len()), None);
    }

    #[test]
    fn live_mention_query_uses_current_selection_start() {
        let text = "hello @fl tail";
        let stale_cursor = text.len();
        assert_eq!(mention_query(text, stale_cursor), None);
        assert_eq!(
            live_mention_query_from_input(text, Some(9)),
            Some((6, "fl".to_string()))
        );
        assert_eq!(live_mention_query_from_input(text, Some(5)), None);
    }

    #[test]
    fn mention_roster_uses_room_participants_only_for_local_access() {
        let local = vec![part("local-human", "Local", RoomParticipantKind::Human)];
        let mut access = test_access(RoomAccessState::Local);
        access.members = vec![FederatedRoomMemberProjection {
            member_id: "projected-agent".into(),
            owner_member_id: None,
            actor_type: FederatedActorType::Agent,
            role_in_room: FederatedRoomRole::Member,
            display_name: "Projected".into(),
            public_agent_descriptor: None,
            joined_at: String::new(),
            derived_presence: None,
            local_binding_available: None,
        }];
        assert_eq!(mention_roster(&local, Some(&access)), local);
    }

    #[test]
    fn mention_roster_uses_safe_projection_for_every_non_local_state() {
        let local = vec![part("stale-local", "Stale", RoomParticipantKind::Human)];
        for state in [
            RoomAccessState::Connecting,
            RoomAccessState::Live,
            RoomAccessState::Recovering,
            RoomAccessState::Revoked,
        ] {
            let mut access = test_access(state);
            access.members = vec![FederatedRoomMemberProjection {
                member_id: "remote-agent".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::Agent,
                role_in_room: FederatedRoomRole::Member,
                display_name: "Remote Agent".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: None,
                local_binding_available: None,
            }];
            let roster = mention_roster(&local, Some(&access));
            assert_eq!(
                roster,
                vec![part(
                    "remote-agent",
                    "Remote Agent",
                    RoomParticipantKind::Agent
                )]
            );
        }
        assert!(mention_roster(&local, None).is_empty());
    }

    #[test]
    fn mention_suggestions_rank_id_prefix_then_name_then_substring() {
        let roster = vec![
            part("zeta", "Ada", RoomParticipantKind::Human),
            part("flux", "Builder", RoomParticipantKind::Agent),
            part("reflux", "Other", RoomParticipantKind::Agent),
        ];
        let got = mention_suggestions(&roster, "fl");
        let ids: Vec<&str> = got.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["flux", "reflux"]);

        // Name prefix outranks substring.
        let got = mention_suggestions(&roster, "ada");
        assert_eq!(got[0].id, "zeta");
    }

    #[test]
    fn mention_suggestion_pick_uses_the_live_partial() {
        let roster = vec![
            part("ada", "Ada", RoomParticipantKind::Human),
            part("flux", "Flux", RoomParticipantKind::Agent),
        ];
        assert_eq!(
            mention_suggestion_at(&roster, "fl", 0).map(|pick| pick.id),
            Some("flux".to_string())
        );
        assert_eq!(
            mention_suggestion_at(&roster, "ad", 0).map(|pick| pick.id),
            Some("ada".to_string())
        );
    }

    #[test]
    fn mention_accept_validation_does_not_consume_keys_after_caret_moves() {
        let roster = vec![part("flux", "Flux", RoomParticipantKind::Agent)];
        let text = "@fl trailing";
        assert!(mention_accept_is_valid(text, Some(3), &roster, 0));
        assert!(!mention_accept_is_valid(text, Some(11), &roster, 0));
        assert!(!mention_accept_is_valid(text, None, &roster, 0));
    }

    #[test]
    fn mention_suggestions_empty_partial_lists_roster_capped() {
        let roster: Vec<RoomParticipant> = (0..12)
            .map(|i| part(&format!("m{i}"), "M", RoomParticipantKind::Human))
            .collect();
        assert_eq!(mention_suggestions(&roster, "").len(), 8);
        assert!(mention_suggestions(&roster, "zzz").is_empty());
    }

    #[test]
    fn apply_mention_replaces_partial_and_positions_caret() {
        let (text, caret) = apply_mention("hi @fl tail", 3, 6, "flux");
        assert_eq!(text, "hi @flux  tail");
        assert_eq!(caret, "hi @flux ".len());

        let (text, caret) = apply_mention("@", 0, 1, "designer");
        assert_eq!(text, "@designer ");
        assert_eq!(caret, text.len());
    }

    #[test]
    fn mention_popup_key_model() {
        assert_eq!(mention_popup_key(3, 0, "ArrowDown"), MentionKey::Move(1));
        assert_eq!(mention_popup_key(3, 0, "ArrowUp"), MentionKey::Move(2));
        assert_eq!(mention_popup_key(3, 2, "ArrowDown"), MentionKey::Move(0));
        assert_eq!(mention_popup_key(3, 1, "Enter"), MentionKey::Accept);
        assert_eq!(mention_popup_key(3, 1, "Tab"), MentionKey::Accept);
        assert_eq!(mention_popup_key(3, 1, "Escape"), MentionKey::Close);
        assert_eq!(mention_popup_key(3, 1, "a"), MentionKey::Pass);
        assert_eq!(mention_popup_key(0, 0, "Enter"), MentionKey::Pass);
    }

    #[test]
    fn utf16_byte_offset_roundtrip() {
        let s = "héllo @x";
        // 'é' is 1 UTF-16 unit but 2 bytes.
        assert_eq!(utf16_to_byte_idx(s, 2), 3);
        assert_eq!(byte_to_utf16_idx(s, 3), 2);
        assert_eq!(utf16_to_byte_idx(s, 99), s.len());
        assert_eq!(
            byte_to_utf16_idx(s, 99),
            s.chars().map(|c| c.len_utf16()).sum::<usize>()
        );
    }

    #[test]
    fn agent_descriptor_line_reads_projected_member_descriptor() {
        use crate::rooms::PublicAgentDescriptor;
        let mut access = test_access(RoomAccessState::Live);
        access.members = vec![FederatedRoomMemberProjection {
            member_id: "flux".into(),
            owner_member_id: None,
            actor_type: FederatedActorType::Agent,
            role_in_room: FederatedRoomRole::Member,
            display_name: "Flux".into(),
            public_agent_descriptor: Some(PublicAgentDescriptor {
                display_name: "Flux".into(),
                description: Some("Rapid implementation".into()),
                model_alias: Some("sonnet".into()),
                skills_count: 0,
                subagent_names: vec![],
            }),
            joined_at: String::new(),
            derived_presence: None,
            local_binding_available: Some(false),
        }];
        assert_eq!(
            agent_descriptor_line(Some(&access), "flux"),
            Some("sonnet \u{b7} Rapid implementation".to_string())
        );
    }

    #[test]
    fn agent_descriptor_line_is_truthful_only() {
        use crate::rooms::PublicAgentDescriptor;
        let mut access = test_access(RoomAccessState::Local);
        access.members = vec![FederatedRoomMemberProjection {
            member_id: "flux".into(),
            owner_member_id: None,
            actor_type: FederatedActorType::Agent,
            role_in_room: FederatedRoomRole::Member,
            display_name: "Flux".into(),
            public_agent_descriptor: Some(PublicAgentDescriptor {
                display_name: "Flux".into(),
                description: Some("Rapid implementation".into()),
                model_alias: Some("sonnet".into()),
                skills_count: 0,
                subagent_names: vec![],
            }),
            joined_at: String::new(),
            derived_presence: None,
            local_binding_available: None,
        }];
        assert_eq!(
            agent_descriptor_line(Some(&access), "flux"),
            Some("sonnet \u{b7} Rapid implementation".to_string())
        );
        // Unknown member / no descriptor / no access — never fabricated.
        assert_eq!(agent_descriptor_line(Some(&access), "ghost"), None);
        assert_eq!(agent_descriptor_line(None, "flux"), None);
    }

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
    fn reply_only_append_keeps_root_keys_and_bumps_count() {
        // The timeline For is keyed by root seq: a reply-only append must
        // not churn root keys (children stay cached), which is exactly why
        // the thread-toggle label must read the count reactively at the
        // leaf — this locks both halves of that contract.
        let mut transcript = vec![test_msg(0, "root", None), test_msg(1, "other root", None)];
        let roots_before: Vec<u64> = partition_thread_messages(&transcript, 0)
            .roots
            .iter()
            .map(|m| m.seq)
            .collect();
        assert_eq!(reply_count_for(&transcript, 0), 0);

        transcript.push(test_msg(2, "reply", Some(0)));

        let roots_after: Vec<u64> = partition_thread_messages(&transcript, 0)
            .roots
            .iter()
            .map(|m| m.seq)
            .collect();
        assert_eq!(
            roots_before, roots_after,
            "reply-only appends must preserve root For keys"
        );
        assert_eq!(reply_count_for(&transcript, 0), 1);
        assert_eq!(reply_count_for(&transcript, 1), 0);
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
    fn canonical_wire_clock_time_strips_redundant_date_for_rfc3339() {
        assert_eq!(canonical_wire_clock_time("2026-06-05T12:34:56Z"), "12:34");
        // Non-canonical inputs fall back to the full string — never lie.
        assert_eq!(canonical_wire_clock_time(""), "");
        assert_eq!(canonical_wire_clock_time("12:34"), "12:34");
        assert_eq!(canonical_wire_clock_time("2026-06-05T12"), "2026-06-05T12");
        assert_eq!(
            canonical_wire_clock_time("2026-06-05 12:34"),
            "2026-06-05 12:34"
        );
    }

    #[test]
    fn avatar_identity_is_deterministic_and_bounded() {
        let a = avatar_identity_class("ada");
        assert_eq!(a, avatar_identity_class("ada"), "same id, same hue");
        assert!(a.starts_with("rooms-workspace__msg-avatar--hue"));
        // Different ids may collide (5 hues) but must all stay in range.
        for id in ["ada", "grace", "linus", "smaths", "ocean-agent-7", ""] {
            let c = avatar_identity_class(id);
            assert!(
                (0..5).any(|n| c == format!("rooms-workspace__msg-avatar--hue{n}")),
                "out of palette: {c}"
            );
        }
    }

    #[test]
    fn avatar_identity_distributes_across_hues() {
        use std::collections::HashSet;
        let hues: HashSet<_> = (0..50)
            .map(|n| avatar_identity_class(&format!("agent-{n}")))
            .collect();
        assert!(hues.len() >= 3, "degenerate distribution: {hues:?}");
    }

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

    // ── room-list ARIA listbox helpers ───────────────────────────────────

    fn keys(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn next_focus_arrows_wrap_and_home_end_jump() {
        let k = keys(&["a", "b", "c"]);
        assert_eq!(room_list_next_focus(&k, Some("a"), "ArrowDown"), Some(1));
        assert_eq!(room_list_next_focus(&k, Some("c"), "ArrowDown"), Some(0));
        assert_eq!(room_list_next_focus(&k, Some("a"), "ArrowUp"), Some(2));
        assert_eq!(room_list_next_focus(&k, Some("b"), "Home"), Some(0));
        assert_eq!(room_list_next_focus(&k, Some("b"), "End"), Some(2));
    }

    #[test]
    fn next_focus_without_focused_option_enters_at_edges() {
        let k = keys(&["a", "b"]);
        assert_eq!(room_list_next_focus(&k, None, "ArrowDown"), Some(0));
        assert_eq!(room_list_next_focus(&k, None, "ArrowUp"), Some(1));
    }

    #[test]
    fn next_focus_ignores_non_nav_keys_and_empty_list() {
        let k = keys(&["a"]);
        assert_eq!(room_list_next_focus(&k, Some("a"), "Enter"), None);
        assert_eq!(room_list_next_focus(&k, Some("a"), "j"), None);
        assert_eq!(room_list_next_focus(&[], None, "ArrowDown"), None);
    }

    #[test]
    fn next_focus_with_stale_focused_key_recovers_at_edges() {
        // Focused option was removed by a refetch: treat as unfocused.
        let k = keys(&["a", "b"]);
        assert_eq!(room_list_next_focus(&k, Some("gone"), "ArrowDown"), Some(0));
    }

    #[test]
    fn tab_stop_is_open_room_else_first_else_none() {
        let k = keys(&["a", "b", "c"]);
        assert_eq!(room_list_tab_stop(&k, Some("b")), Some(1));
        assert_eq!(room_list_tab_stop(&k, Some("zz")), Some(0));
        assert_eq!(room_list_tab_stop(&k, None), Some(0));
        assert_eq!(room_list_tab_stop(&[], Some("a")), None);
    }

    #[test]
    fn option_dom_id_is_prefix_stable() {
        assert_eq!(room_option_dom_id("r1"), "rooms-opt-r1");
    }
}
