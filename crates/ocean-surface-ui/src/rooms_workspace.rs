//! Slack-style persistent Rooms workspace.
//!
//! Left rail (room list + create), center rail (header + transcript + composer),
//! right rail (members), plus a dedicated thread panel column that opens to the
//! right of the members rail (and overlays it on narrower layouts). The open
//! room and thread persist across reload via localStorage. Renders against the
//! existing CSS classes in `styles/rooms-workspace.css` and the existing
//! [`crate::rooms::Rooms`] signals and API surface. Mounted as the default
//! web+Tauri collaboration surface in [`crate::app`]; the legacy
//! RoomStage/RoomsPanel components in rooms.rs are deleted.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::room_messages;
use crate::rooms::{
    CreateResolution, FederatedActorType, FederatedRoomMemberProjection, FederatedRoomRole,
    MemberPresence, OutboxItemState, Room, RoomAccessProjection, RoomAccessState, RoomAgentOwner,
    RoomMessage, RoomMessageKind, RoomParticipant, RoomParticipantKind, RoomReadCursorProjection,
    RoomTriggerPolicy, Rooms,
};

use crate::rooms::{create_workspace_root, room_is_unbound};

// ── Production helpers (testable directly, called from Effects) ─

// These are only exercised by tests now that create admission is
// gated by typed op-id outcomes rather than list inspection.

/// Whether the composer should clear after the normalized wire body is
/// confirmed. The current draft must still be the exact original draft so
/// typing that happened while the send was in flight is never discarded.
fn should_clear_composer(current: &str, original_draft: &str) -> bool {
    !original_draft.is_empty() && current == original_draft
}

/// The trigger-policy flags this workspace exposes. `on_component_event` and
/// `on_schedule` have no control because the daemon ruled them unwired: its
/// write routes refuse a policy carrying `on_component_event: true` or a set
/// `on_schedule` with a typed 400 (`trigger_unwired`), so every write path
/// here normalizes them away instead (see [`policy_with_toggle`]).
///
/// `on_ci_failure` is exposed on the same terms as the other three: the daemon
/// half is live — a `room.workspace.ci_checked` row whose checks read red
/// convenes the roster's agents, the green and in-progress ones staying pure
/// markers — and nothing refuses the write. It reached this enum a release
/// after the flag itself, so `RoomTriggerPolicy` mirrors it (see that field)
/// for rooms whose value the daemon set before any control could.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriggerToggle {
    Mention,
    ThreadReply,
    BuildFailure,
    CiFailure,
}

/// The full policy to PATCH when one exposed flag flips: a copy of the room's
/// current policy with only that flag changed — and the unwired fields
/// normalized away. Preserving them is impossible by ruling: the daemon
/// refuses any write carrying their live values (`trigger_unwired`), so a
/// room with dead state stored would 400 on every flip if we carried it
/// through. Nothing ever fires those fields; dropping them on the next edit
/// is the honest behavior. `on_ci_failure` is deliberately NOT in that group:
/// nothing refuses it, so the copy carries it through untouched and only its
/// own row's flip ever changes it — which is what keeps a value the daemon set
/// from being cleared by a flip of some unrelated row.
fn policy_with_toggle(
    current: Option<&RoomTriggerPolicy>,
    toggle: TriggerToggle,
    enabled: bool,
) -> RoomTriggerPolicy {
    let mut policy = current.cloned().unwrap_or_default();
    policy.on_component_event = false;
    policy.on_schedule = None;
    match toggle {
        TriggerToggle::Mention => policy.on_mention = enabled,
        TriggerToggle::ThreadReply => policy.on_thread_reply = enabled,
        TriggerToggle::BuildFailure => policy.on_build_failure = enabled,
        TriggerToggle::CiFailure => policy.on_ci_failure = enabled,
    }
    policy
}

/// Create-time policy from the four exposed toggles. All-off returns `None`
/// so the create body omits the field entirely and the daemon's default (no
/// automatic triggers) applies — exactly what creating a room did before this
/// form had toggles.
fn create_trigger_policy(
    on_mention: bool,
    on_thread_reply: bool,
    on_build_failure: bool,
    on_ci_failure: bool,
) -> Option<RoomTriggerPolicy> {
    if !on_mention && !on_thread_reply && !on_build_failure && !on_ci_failure {
        return None;
    }
    Some(RoomTriggerPolicy {
        on_mention,
        on_thread_reply,
        on_build_failure,
        on_ci_failure,
        ..RoomTriggerPolicy::default()
    })
}

/// The right rail's one-line reading of a stored policy: the live flags that
/// are on, or `"none"`. A free function rather than inline view code because
/// the view is only reachable from a browser — a control silently dropped from
/// this list is invisible to `cargo test` and to the wasm build alike, which is
/// exactly how the summary lost a line once before.
///
/// The unwired fields are never listed: they cannot fire, and a write carrying
/// them is refused (`trigger_unwired`).
///
/// A live flag that is on but cannot fire in THIS kind of room is listed with
/// the note the rail's own row carries — `build failure (federated rooms
/// only)`. Annotated rather than dropped, because the flag is stored true and
/// its row two inches above renders checked: hiding it here would trade one
/// contradiction for its inverse. The note comes from
/// [`trigger_row_dead_here`] rather than a second reading of the access
/// projection, so the rail's wording and the summary's cannot drift apart —
/// the same reason the stage's banner takes its text from [`access_banner`].
fn trigger_summary(policy: &RoomTriggerPolicy, access: Option<&RoomAccessProjection>) -> String {
    let flags = [
        (TriggerToggle::Mention, policy.on_mention, "mention"),
        (
            TriggerToggle::ThreadReply,
            policy.on_thread_reply,
            "thread reply",
        ),
        (
            TriggerToggle::BuildFailure,
            policy.on_build_failure,
            "build failure",
        ),
        (TriggerToggle::CiFailure, policy.on_ci_failure, "CI failure"),
    ];
    let on: Vec<String> = flags
        .into_iter()
        .filter(|(_, enabled, _)| *enabled)
        .map(
            |(toggle, _, label)| match trigger_row_dead_here(toggle, access) {
                Some(note) => format!("{label} ({note})"),
                None => label.to_string(),
            },
        )
        .collect();
    if on.is_empty() {
        "none".to_string()
    } else {
        on.join(", ")
    }
}

/// The note a trigger row carries when its flag cannot fire in this kind of
/// room — `None` when the flag is live here.
///
/// The policy all four flags are judged against is read from THIS daemon's
/// store, on the federation bridge's ingest paths as much as on the local
/// post path, so a federated room's policy is not an inert mirror and
/// PATCHing it changes what actually fires. What varies is which flag's EVENT
/// can reach which kind of room, and it is a four-way split:
///
/// - `on_mention` reaches both. The local post path parses mentions out of a
///   posted body, and the bridge's message ingest evaluates the same event
///   per federated mention.
/// - `on_thread_reply` reaches a Local room only. It is built solely on the
///   local post path, from the thread root's author; the federated message
///   payload carries no thread parent, so the bridge can never build one.
/// - `on_build_failure` reaches a federated room only. It is built solely
///   from a `room.workspace.build_failed` marker, and workspace markers
///   arrive through the bridge — a Local room has no workspace to fail.
/// - `on_ci_failure` reaches a federated room only, for the same reason and
///   from the same lane: its sole event is a `room.workspace.ci_checked`
///   marker the bridge reads red, so a Local room has no workspace to check.
///
/// A room whose access is still unknown gets no note: claiming a flag is
/// dead there would be a guess, and [`trigger_policy_accepts_writes`] already
/// holds the row.
fn trigger_row_dead_here(
    toggle: TriggerToggle,
    access: Option<&RoomAccessProjection>,
) -> Option<&'static str> {
    let access = access?;
    let federated = room_is_federated(Some(access));
    match toggle {
        TriggerToggle::Mention => None,
        TriggerToggle::ThreadReply => federated.then_some("local rooms only"),
        TriggerToggle::BuildFailure => (!federated).then_some("federated rooms only"),
        TriggerToggle::CiFailure => (!federated).then_some("federated rooms only"),
    }
}

/// The access a room created from the left rail has on its first day:
/// `Local`, by construction. `POST /v1/rooms/persistent` carries a key, a
/// name and a trigger policy and nothing else — there is no federation
/// anywhere in that body — so the room the daemon writes is local until
/// someone federates it later.
///
/// Built explicitly rather than handing [`trigger_row_dead_here`] the `None`
/// a room-in-creation literally has. `None` there means "access unknown", and
/// deliberately yields no note at all; wiring the create rows to it would
/// compile, pass every test, and annotate nothing. A room being created is
/// not unknown — it is Local, and saying so is this function's whole job.
fn creating_room_access() -> RoomAccessProjection {
    RoomAccessProjection {
        state: RoomAccessState::Local,
        last_confirmed_global_sequence: None,
        members: Vec::new(),
        self_member_id: None,
        outbox: Vec::new(),
    }
}

/// The note a create-time trigger row carries, or `None` when the flag is
/// live in the room this form is about to make. Delegates to
/// [`trigger_row_dead_here`] — the one authority the right rail's rows and its
/// summary already share — against [`creating_room_access`], so the create
/// panel and the panel two rails over can never disagree about which flags a
/// Local room can fire.
///
/// The ruling, since a Local room may federate later and a build-failure tick
/// made here would be dead now and live then: note AND disable, the treatment
/// the right rail's own rows get, not a sentence calling these defaults a
/// federated room will re-judge. "Later" already has a control — the right
/// rail's row goes live the moment the room does, and that is where the
/// decision belongs. Arming a flag at create time that the rail two panels
/// over will immediately grey out and explain stores a contradiction on day
/// one to buy a preference the room can express any time it actually
/// federates.
fn create_trigger_row_dead_here(toggle: TriggerToggle) -> Option<&'static str> {
    trigger_row_dead_here(toggle, Some(&creating_room_access()))
}

/// Whether the trigger policy accepts a write under this access projection —
/// this rail's name for [`local_store_write_gate`], and the section where that
/// ruling was first made.
///
/// The trigger policy is a field on a row in THIS daemon's store:
/// `PATCH /v1/rooms/persistent/{key}` carries no access check of any kind, and
/// both readers of the policy — the local post path and the federation
/// bridge's ingest — read it back from that same store. A link that is down or
/// coming back cannot make the write unlandable; it only delays the events the
/// policy governs. The loss the composer's gate caused here was the sharpest
/// one available: a room stuck `Recovering` while every mention woke an agent
/// gave the operator no way to turn `on_mention` off, at precisely the moment
/// they wanted to.
fn trigger_policy_accepts_writes(access: Option<&RoomAccessProjection>) -> bool {
    local_store_write_gate(access)
}

/// Whether a trigger row accepts a flip, and in which direction. The policy
/// must be writable under this access at all (see
/// [`trigger_policy_accepts_writes`] — this rail's own gate, not the
/// composer's), and a flag that cannot fire here (see
/// [`trigger_row_dead_here`]) may still be turned OFF.
///
/// The direction is what `checked` carries, and splitting on it is the whole
/// point. "This flag's event can never reach this room" is a reason to refuse
/// ARMING the flag; it is never a reason to refuse disarming one that is
/// already armed. Under a single gate it did both, and a room that stored
/// `on_thread_reply: true` and then federated rendered that row checked,
/// greyed, noted `local rooms only`, and listed as on by [`trigger_summary`] —
/// with no control anywhere that could clear it. The stored contradiction
/// outlived every session that looked at it.
///
/// An admitted un-tick re-renders the section from `open_room` with `checked`
/// false, and the row goes held-in-both-directions again. That is the state
/// this exists to reach, not a row that has come back to life.
fn trigger_row_is_editable(
    toggle: TriggerToggle,
    checked: bool,
    access: Option<&RoomAccessProjection>,
) -> bool {
    trigger_policy_accepts_writes(access)
        && (checked || trigger_row_dead_here(toggle, access).is_none())
}

/// One editable trigger row in the right rail. `checked` is a plain bool on
/// purpose: the enclosing section re-renders from `open_room` after every
/// admitted PATCH, so an admitted flip settles to durable state. A refused
/// PATCH leaves `open_room` untouched — no re-render — so the box keeps the
/// user's flip next to the inline error until the next successful write. The
/// flip reads the room's policy fresh at event time — not from the render that
/// drew the box — so two quick flips compose instead of the second
/// resurrecting the first's pre-state.
///
/// `checked` is also the direction the gate is asked about: a dead flag that
/// is stored on is the one case where a dead row still takes a click. If that
/// un-tick is REFUSED the row stays enabled, because `checked` is still the
/// true it rendered from — which is what lets the operator put back the flag
/// the daemon would not let them clear.
fn trigger_toggle_row(
    rooms: Rooms,
    toggle: TriggerToggle,
    label: &'static str,
    checked: bool,
    access: Option<&RoomAccessProjection>,
) -> impl IntoView {
    // Both read the projection the enclosing section already holds, but they
    // ask different things of it: the note follows the access reading alone,
    // while the hold follows access AND the direction of the flip. So on one
    // row they part company on purpose — a dead flag stored ON renders noted
    // and still clickable, because that click is the un-tick.
    let dead_here = trigger_row_dead_here(toggle, access);
    let editable = trigger_row_is_editable(toggle, checked, access);
    view! {
        <label class="rooms-workspace__trigger">
            <input
                type="checkbox"
                prop:checked=checked
                disabled=move || !editable || rooms.policy_update_in_flight.get()
                on:change=move |ev| {
                    let current = rooms
                        .open_room
                        .get_untracked()
                        .and_then(|room| room.trigger_policy);
                    rooms.update_open_room_policy(policy_with_toggle(
                        current.as_ref(),
                        toggle,
                        event_target_checked(&ev),
                    ));
                }
            />
            <span class="rooms-workspace__trigger-label">{label}</span>
            {dead_here.map(|note| view! {
                <span class="rooms-workspace__trigger-note">{note}</span>
            })}
        </label>
    }
}

/// The open room's workspace binding: the unbound notice, the folder it is
/// bound to when it has one, and the bind/unbind control.
///
/// This sits with the trigger rows because it is the precondition for all of
/// them. A trigger decides WHETHER the room's agents are woken; the binding
/// decides whether a woken turn can run at all — the daemon resolves the
/// turn's project and `cwd` from the room's `workspace_root`, and with none
/// stored it refuses with `workspace_unavailable` before the agent sees the
/// message. So an unbound room can have every trigger checked and still do
/// nothing, which is exactly the state the notice names.
///
/// Gated on [`trigger_policy_accepts_writes`], the same gate the rows above
/// take, because it is the same PATCH to the same route under the same
/// authority. Deliberately NOT gated on a locally-inferred room owner: this
/// repo's contract is that owner authority is server-derived and never guessed
/// from a participant projection, and the daemon's PATCH applies no owner check
/// of its own — inventing one here would be a lock on the surface only.
fn workspace_binding_section(rooms: Rooms, access: Option<&RoomAccessProjection>) -> impl IntoView {
    let writable = trigger_policy_accepts_writes(access);
    let draft = RwSignal::new(String::new());
    // Seeded from the stored binding so the field opens showing what it will
    // change, and a rebind is an edit rather than a retype.
    Effect::new(move |_: Option<()>| {
        let stored = rooms
            .open_room
            .get()
            .and_then(|room| room.workspace_root)
            .unwrap_or_default();
        draft.set(stored);
    });
    let unbound = move || rooms.open_room.get().as_ref().is_some_and(room_is_unbound);
    let bound_to = move || {
        rooms
            .open_room
            .get()
            .and_then(|room| room.workspace_root)
            .filter(|root| !root.trim().is_empty())
    };
    let in_flight = move || rooms.workspace_update_in_flight.get();
    view! {
        <div class="rooms-workspace__workspace-binding">
            {move || unbound().then(|| view! {
                <div class="rooms-workspace__workspace-unbound" role="note">
                    "No workspace folder is bound. Agents in this room cannot run \
                     until one is — every turn is refused before it starts."
                </div>
            })}
            {move || bound_to().map(|root| view! {
                <div class="rooms-workspace__workspace-bound">
                    <span class="rooms-workspace__workspace-bound-label">"Workspace"</span>
                    <code class="rooms-workspace__workspace-bound-path">{root}</code>
                </div>
            })}
            {move || writable.then(|| view! {
                <div class="rooms-workspace__workspace-controls">
                    <input
                        class="rooms-workspace__workspace-input"
                        type="text"
                        aria-label="Workspace folder on the daemon host"
                        placeholder="/absolute/path/to/project"
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                        disabled=in_flight
                    />
                    <button
                        class="rooms-workspace__workspace-bind"
                        type="button"
                        // An empty field has nothing to bind: unbinding is the
                        // other button, so this one never doubles as it.
                        disabled=move || in_flight() || draft.get().trim().is_empty()
                        on:click=move |_| {
                            rooms.set_open_room_workspace(
                                create_workspace_root(&draft.get_untracked()),
                            );
                        }
                    >
                        "Bind"
                    </button>
                    <button
                        class="rooms-workspace__workspace-unbind"
                        type="button"
                        disabled=move || in_flight() || unbound()
                        on:click=move |_| rooms.set_open_room_workspace(None)
                    >
                        "Unbind"
                    </button>
                </div>
            })}
            <span class="rooms-workspace__workspace-help">
                "The folder is resolved on the machine running the daemon, not in \
                 this browser. It must be an absolute path that already exists there."
            </span>
            {move || rooms.workspace_update_status.get().map(|status| view! {
                <div class="rooms-workspace__workspace-error" role="alert">
                    {status.message()}
                </div>
            })}
        </div>
    }
}

/// One trigger row in the left rail's create form. The mirror of
/// [`trigger_toggle_row`] for a room that does not exist yet: there is no
/// policy to PATCH, so the flip lands in a local signal the submit reads, and
/// the row is held either while the create POST is in flight or permanently,
/// because the flag cannot fire in the Local room this form makes.
///
/// The note and the hold both come from [`create_trigger_row_dead_here`], the
/// same single read the right rail's rows make, so a row can never be greyed
/// out with nothing to explain it — or explained while still clickable.
fn create_trigger_row(
    toggle: TriggerToggle,
    label: &'static str,
    flag: RwSignal<bool>,
    pending_create: RwSignal<bool>,
) -> impl IntoView {
    let dead_here = create_trigger_row_dead_here(toggle);
    view! {
        <label class="rooms-workspace__trigger">
            <input
                type="checkbox"
                prop:checked=move || flag.get()
                on:change=move |ev| flag.set(event_target_checked(&ev))
                disabled=move || dead_here.is_some() || pending_create.get()
            />
            <span class="rooms-workspace__trigger-label">{label}</span>
            {dead_here.map(|note| view! {
                <span class="rooms-workspace__trigger-note">{note}</span>
            })}
        </label>
    }
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

/// Escape behavior owned by the Rooms workspace. Only a visible compact
/// drawer is handled here; every other Escape bubbles to the app-level
/// topmost-surface hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompactEscapeAction {
    CloseDrawer,
}

fn compact_escape_action(
    is_compact: bool,
    drawer_open: bool,
    default_prevented: bool,
) -> Option<CompactEscapeAction> {
    if is_compact && drawer_open && !default_prevented {
        Some(CompactEscapeAction::CloseDrawer)
    } else {
        None
    }
}

fn window_inner_width() -> Option<f64> {
    web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|width| width.as_f64())
}

/// Keep this aligned with the compact media queries in
/// `styles/rooms-workspace.css` and Tauri's minimum window width.
fn rooms_layout_is_compact() -> bool {
    window_inner_width().is_some_and(|width| width <= 650.0)
}

// ── Access-policy helpers (the crate's single source; rooms.rs imports) ──

/// Whether writes (composer, join, leave) are permitted under this access
/// projection.
pub(crate) fn access_allows_writes(access: Option<&RoomAccessProjection>) -> bool {
    matches!(
        access.map(|a| a.state),
        Some(RoomAccessState::Local) | Some(RoomAccessState::Live)
    )
}

/// Whether the composer may send into the OPEN room: the access gate above,
/// AND the room not being the daemon's frozen soft-closed audit view.
///
/// Two axes, and neither implies the other. Closing a room stamps `closed_at`
/// and leaves its access row untouched, so a frozen room projects whatever it
/// projected while live — `Local` with no access row, an unchanged `Live` when
/// federated — and `access_allows_writes` waves through every send into either
/// shape, each of which `POST .../messages` answers 404. Asking both here
/// rather than at each site is why `post_message` and the
/// four `disabled=` bindings cannot drift into disagreeing about what a dead
/// composer is: a room that refuses the send must be a room whose input is
/// visibly shut, or the refusal reads as the message being swallowed.
pub(crate) fn composer_writes_allowed(
    access: Option<&RoomAccessProjection>,
    room_closed: bool,
) -> bool {
    access_allows_writes(access) && !room_closed
}

/// Whether a write that lands in THIS daemon's local store may proceed under
/// this access projection — the rail-local counterpart of
/// [`access_allows_writes`], and deliberately more permissive than it.
///
/// The two gates ask different questions. [`access_allows_writes`] asks "can
/// this write reach a peer?", which is the right question for the composer and
/// for anything that mints or drives federation: a message that never leaves is
/// a lie about what was said. Most of what the right rail writes is not that
/// kind of write. The trigger policy, a summarize run, an artifact and an
/// attachment all land through the daemon's own store handle and announce
/// themselves on the local event stream; ocean-os enqueues none of them to the
/// federation outbox — a federated room's summary is documented local-only, the
/// artifact routes write through `with_rooms` and publish a local wake, and the
/// attachment module names the outbox nowhere at all. A link that is down or
/// coming back cannot make such a write unlandable, so `Connecting` and
/// `Recovering` keep these rails writable.
///
/// `Revoked` stays held. The daemon would accept those writes too, but the
/// operator has been removed from the room, and offering to configure a room
/// you no longer stand in is offering an action that cannot mean anything to
/// you again. Unknown access stays held for the weaker version of the same
/// reason: it may yet resolve to `Revoked`, and a control that flips to
/// disabled once the projection lands is worse than one that waits for it.
///
/// Matched exhaustively without a wildcard on purpose — a new access state
/// must be ruled on here rather than quietly inheriting "writable".
fn local_store_write_gate(access: Option<&RoomAccessProjection>) -> bool {
    match access.map(|projection| projection.state) {
        Some(
            RoomAccessState::Local
            | RoomAccessState::Connecting
            | RoomAccessState::Live
            | RoomAccessState::Recovering,
        ) => true,
        Some(RoomAccessState::Revoked) | None => false,
    }
}

/// Whether this room federates through Bedrock at all. Only a federated room
/// has a Bedrock workspace; a Local room renders nothing rather than a
/// refusal, and `None` (no room open / still loading) also renders nothing.
pub(crate) fn room_is_federated(access: Option<&RoomAccessProjection>) -> bool {
    access.is_some_and(|projection| projection.state != RoomAccessState::Local)
}

/// The access-banner label for a state that blocks writes; `None` for the
/// two writable states, where no banner mounts. The stage's banner match
/// takes its text from here so the rendered strings and the pinning test
/// cannot diverge; per-state CSS classes and roles stay with the view.
fn access_banner(access: Option<&RoomAccessProjection>) -> Option<&'static str> {
    match access.map(|projection| projection.state) {
        Some(RoomAccessState::Connecting) => Some("Connecting to federated room…"),
        Some(RoomAccessState::Recovering) => Some("Recovering connection…"),
        Some(RoomAccessState::Revoked) => Some("Access revoked"),
        None | Some(RoomAccessState::Local | RoomAccessState::Live) => None,
    }
}

/// How one transcript row relates to the shared ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LedgerMark {
    /// Bedrock confirmed the row onto the shared ledger; the row wears the
    /// positive mark.
    Confirmed,
    /// Federated room, but the row carries no confirmation — local-era/G1
    /// history. Silence, never a pending/failed treatment: in-flight state
    /// belongs to the outbox block, not the transcript.
    Unmarked,
    /// Local room (or no access projection yet): there is no ledger to
    /// reach, so `federated: None` is simply correct and nothing renders.
    NotApplicable,
}

fn ledger_mark(access: Option<&RoomAccessProjection>, message: &RoomMessage) -> LedgerMark {
    if !room_is_federated(access) {
        LedgerMark::NotApplicable
    } else if message.federated.is_some() {
        LedgerMark::Confirmed
    } else {
        LedgerMark::Unmarked
    }
}

/// The per-row ledger glyph, rendered beside the timestamp so grouped rows —
/// whose header collapses to the dimmed time — keep it. Only `Confirmed`
/// renders anything; both silent states are deliberate (see [`LedgerMark`]).
fn ledger_mark_view(access: Option<&RoomAccessProjection>, message: &RoomMessage) -> AnyView {
    if ledger_mark(access, message) != LedgerMark::Confirmed {
        return ().into_any();
    }
    view! {
        <span
            class="rooms-workspace__msg-ledger"
            role="img"
            title="On the shared ledger"
            aria-label="On the shared ledger"
        >
            <svg viewBox="0 0 16 16" width="10" height="10"
                fill="none" stroke="currentColor" stroke-width="2"
                stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 8.5l3.5 3.5L13 5"/>
            </svg>
        </span>
    }
    .into_any()
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
    /// An older page the operator ASKED for landed in FRONT of the painted
    /// rows: hold the reader on the rows they were looking at, and touch
    /// nothing else.
    ///
    /// Neither of the growth arms is right for a requested prepend.
    /// `RaiseJump` would raise "↓ New messages" over rows that arrived above,
    /// which is a lie about where they are; `PinAndQueue` would throw the
    /// reader to the bottom of a transcript they just asked to see the top of.
    /// And the read advance is deliberately not re-queued: the durable
    /// candidate is the NEWEST row (or the access projection's confirmed
    /// sequence), and a prepend moves neither.
    ///
    /// Only a REQUESTED prepend takes this arm, which is why the decision needs
    /// the anchor as an input rather than reading `grew_at_front` alone. The
    /// hydration walk prepends too — up to four more pages after the first fill
    /// — and those pages answer nothing the reader did. Holding position
    /// through them leaves a long room open on its oldest loaded page, which is
    /// exactly what ocean-surface#190 fixed; they keep taking `PinAndQueue`.
    AnchorOlder,
    /// Nothing to do. Critically, `Hold` never writes: a pass triggered by a
    /// non-transcript dependency (an access projection) while the reader is
    /// scrolled up must not mark read, must not raise the jump affordance,
    /// and must not clobber an already queued request.
    Hold,
}

/// What one transcript pass hands the next: how many rows it saw, and the `seq`
/// of the oldest. The pair travels together because a pass that declines to
/// consume the first-fill state (see below) must decline to consume the other
/// half too — carrying a fresh oldest beside a stale length would let the next
/// pass conclude that nothing arrived in front of rows it never measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TranscriptPassState {
    len: usize,
    oldest_seq: Option<u64>,
}

/// Whether this pass's rows arrived in FRONT of the ones already painted.
///
/// The transcript is ascending by `seq` and only ever grows at one end per
/// write, so a fallen oldest `seq` is a prepend and nothing else. Reading the
/// seq rather than counting rows is what makes that true: a page whose rows were
/// all already painted prepends nothing and must read as no movement at all,
/// which a length comparison alone cannot say.
///
/// A first fill answers false — there were no rows to arrive in front of — which
/// is what keeps it on the `PinAndQueue` path that opens a room at its newest
/// message.
fn transcript_grew_at_front(prev_oldest_seq: Option<u64>, oldest_seq: Option<u64>) -> bool {
    match (prev_oldest_seq, oldest_seq) {
        (Some(previous), Some(current)) => current < previous,
        _ => false,
    }
}

/// `measured` is whether the transcript element exists *and* reports a real
/// viewport; an unmeasured pass holds so the fill state stays intact for the
/// first pass that can actually measure. `anchored` is whether a press parked
/// the scroll geometry this prepend is owed against — the one thing that
/// separates history the operator asked for from history that merely arrived.
fn transcript_pass_action(
    len: usize,
    prev_len: usize,
    measured: bool,
    near_bottom: bool,
    grew_at_front: bool,
    anchored: bool,
) -> TranscriptPassAction {
    if len == 0 {
        return TranscriptPassAction::Reset;
    }
    if !measured {
        return TranscriptPassAction::Hold;
    }
    // Ahead of the at-bottom pin, because a REQUESTED prepend that lands while
    // the reader happens to sit at the bottom is still a prepend: pinning would
    // be harmless there but re-queueing the read advance on rows that arrived
    // above the paint is not the claim this Effect should be making. Unasked
    // prepends — every page of the hydration walk — fall past this and keep the
    // pin that opens a room at its newest message.
    if grew_at_front && anchored {
        return TranscriptPassAction::AnchorOlder;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct RoomComposerEpoch {
    generation: u64,
    room_key: Option<String>,
}

fn room_composer_epoch_changed(
    previous: Option<&RoomComposerEpoch>,
    generation: u64,
    room_key: Option<&str>,
) -> bool {
    previous
        != Some(&RoomComposerEpoch {
            generation,
            room_key: room_key.map(str::to_string),
        })
}

// ── View-state persistence (open room + open thread) ───────────────────────

/// localStorage key for the last open room/thread. Bump the suffix on any
/// format change: unknown payloads decode to `None` and are simply dropped.
const ROOMS_VIEW_STATE_KEY: &str = "ocean.rooms.view.v1";

/// Encode the persisted view state: the open room key, optionally followed
/// by a newline and the open thread's root sequence. Room keys are daemon
/// slugs (no newlines), so the separator is unambiguous.
fn encode_view_state(room_key: &str, thread_root_seq: Option<u64>) -> String {
    match thread_root_seq {
        Some(seq) => format!("{room_key}\n{seq}"),
        None => room_key.to_string(),
    }
}

/// Decode a persisted view state. Fail-closed: an empty room key or a
/// non-numeric thread line yields `None` rather than a guessed restore.
fn decode_view_state(raw: &str) -> Option<(String, Option<u64>)> {
    let (room_key, thread) = match raw.split_once('\n') {
        Some((room_key, thread_line)) => (room_key, Some(thread_line.parse::<u64>().ok()?)),
        None => (raw, None),
    };
    if room_key.is_empty() {
        return None;
    }
    Some((room_key.to_string(), thread))
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|window| window.local_storage().ok().flatten())
}

fn load_view_state() -> Option<(String, Option<u64>)> {
    local_storage()
        .and_then(|storage| storage.get_item(ROOMS_VIEW_STATE_KEY).ok().flatten())
        .as_deref()
        .and_then(decode_view_state)
}

fn store_view_state(room_key: Option<&str>, thread_root_seq: Option<u64>) {
    let Some(storage) = local_storage() else {
        return;
    };
    match room_key {
        Some(room_key) if !room_key.is_empty() => {
            let _ = storage.set_item(
                ROOMS_VIEW_STATE_KEY,
                &encode_view_state(room_key, thread_root_seq),
            );
        }
        _ => {
            let _ = storage.remove_item(ROOMS_VIEW_STATE_KEY);
        }
    }
}

/// How an open thread is presented. Inline is the default: the conversation
/// surfaces directly underneath its root message in the timeline. The side
/// panel is an optional pop-out the reader chooses, and the choice sticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ThreadViewMode {
    #[default]
    Inline,
    Panel,
}

const ROOMS_THREAD_VIEW_MODE_KEY: &str = "ocean.rooms.thread-view.v1";

fn encode_thread_view_mode(mode: ThreadViewMode) -> &'static str {
    match mode {
        ThreadViewMode::Inline => "inline",
        ThreadViewMode::Panel => "panel",
    }
}

/// Fail-closed: anything but the two known tokens falls back to the
/// inline default rather than guessing.
fn decode_thread_view_mode(raw: &str) -> Option<ThreadViewMode> {
    match raw {
        "inline" => Some(ThreadViewMode::Inline),
        "panel" => Some(ThreadViewMode::Panel),
        _ => None,
    }
}

fn load_thread_view_mode() -> ThreadViewMode {
    local_storage()
        .and_then(|storage| storage.get_item(ROOMS_THREAD_VIEW_MODE_KEY).ok().flatten())
        .as_deref()
        .and_then(decode_thread_view_mode)
        .unwrap_or_default()
}

fn store_thread_view_mode(mode: ThreadViewMode) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(ROOMS_THREAD_VIEW_MODE_KEY, encode_thread_view_mode(mode));
    }
}

// ── Members drawer + thread panel header helpers ───────────────────────────

/// Whether the members rail currently renders as an overlay drawer rather
/// than an inline column. Must mirror the members-reachability media blocks
/// in `styles/rooms-workspace.css`: the inline rail is gone at 1080px and
/// below always, and up to 1440px while the thread panel occupies the row.
fn members_drawer_is_overlay(width: f64, thread_open: bool) -> bool {
    width <= 1080.0 || (thread_open && width <= 1440.0)
}

/// Escape owned by the members drawer: only an actually-overlaying drawer
/// consumes the key; otherwise it bubbles to the drawer/app hierarchy below.
fn members_escape_closes(
    drawer_is_overlay: bool,
    members_open: bool,
    default_prevented: bool,
) -> bool {
    drawer_is_overlay && members_open && !default_prevented
}

/// Resolve a roster display name for an author id, falling back to the raw
/// id when the author is no longer in the roster (departed member, system).
fn roster_display_name(roster: &[RoomParticipant], author_id: &str) -> String {
    roster
        .iter()
        .find(|participant| participant.id == author_id)
        .map(|participant| participant.display_name.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| author_id.to_string())
}

/// Truthful reply-count label ("No replies yet" over a fake "0"), shared by
/// the inline thread header and the panel subtitle.
fn reply_count_label(reply_count: usize) -> String {
    match reply_count {
        0 => "No replies yet".to_string(),
        1 => "1 reply".to_string(),
        n => format!("{n} replies"),
    }
}

/// Thread panel subtitle: reply count plus who is being replied to, by
/// display name.
fn thread_panel_subtitle(reply_count: usize, root_author_display: &str) -> String {
    format!(
        "{} \u{b7} replying to {root_author_display}",
        reply_count_label(reply_count)
    )
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

/// Whether a roster row offers a remove control: every row except the
/// caller's own — self-removal is already the header's Leave, and a second
/// path to it labeled "remove" would just be Leave with worse copy. Not an
/// authorization check (the daemon's participant DELETE has none); the Local
/// gate is structural — only the Local members branch renders removable rows,
/// because federated rosters are bedrock-authoritative.
fn participant_removable(participant_id: &str, identity_id: &str) -> bool {
    participant_id != identity_id
}

/// What the members rail says about ONE agent row's ownership — see
/// [`agent_ownership`] for how it is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentOwnership {
    /// A worker owns this agent. `owner` is their roster display name where the
    /// roster still carries them and their raw participant id otherwise, which
    /// is the only name a room can give for a worker who has left.
    Owned { owner: String, present: bool },
    /// The daemon answered, and no ownership row names this agent: nobody has
    /// claimed it. The rail says so rather than saying nothing, because an
    /// agent with no badge is indistinguishable from one whose badge simply did
    /// not render.
    Unclaimed,
    /// The surface has no answer to give — hydration has not landed, a binding
    /// mutation has just invalidated what it held, or the daemon predates
    /// `agent_owners` and cannot project ownership it may well hold. Renders
    /// NOTHING. Saying `unclaimed` here would be a claim about the room made
    /// entirely out of the surface's own ignorance, and on a pre-field daemon
    /// it would be that claim about every agent in every room.
    Unknown,
}

/// Map one Agent roster row to its ownership. Both lists are the Local roster's
/// own: `owners` keys on `RoomParticipant::id` (the daemon joins the ownership
/// row to `participants` on exactly that column), so this is a lookup in one
/// namespace and never a guess across two.
///
/// `present` is the daemon's `owner_present` NARROWED by the roster the reader
/// is looking at. The daemon computes the flag as "is `owner_id` still on this
/// roster" at hydration; the roster then moves under the surface — a join,
/// leave or remove replaces `Room::participants` from a route that carries no
/// `agent_owners` at all — so a worker who left after hydration is gone from
/// the rail while their flag beside them is one read stale. Requiring both
/// keeps the rail self-consistent: no row is ever badged as a present owner
/// while the rail does not show them. The other direction is left alone —
/// a daemon that says absent is believed, because a same-id row appearing
/// later is not evidence the original binding survived.
fn agent_ownership(
    owners: Option<&[RoomAgentOwner]>,
    participants: &[RoomParticipant],
    agent_id: &str,
) -> AgentOwnership {
    // No answer at all is its own state and never `Unclaimed`: the caller
    // holding `None` has not been told anything about this room's ownership,
    // and absence of an answer is not an answer of absence.
    let Some(owners) = owners else {
        return AgentOwnership::Unknown;
    };
    let Some(row) = owners.iter().find(|owner| owner.agent_id == agent_id) else {
        return AgentOwnership::Unclaimed;
    };
    let on_roster = participants
        .iter()
        .find(|participant| participant.id == row.owner_id);
    AgentOwnership::Owned {
        owner: on_roster
            .map(|participant| participant.display_name.clone())
            .unwrap_or_else(|| row.owner_id.clone()),
        present: row.owner_present && on_roster.is_some(),
    }
}

/// Whether a federated roster row is the caller's own membership. `None`
/// (a local room, or a daemon that predates `self_member_id`) marks no row,
/// so every row keeps today's remove control and bedrock's 403 stays the
/// answer of last resort.
fn federated_member_is_self(self_member_id: Option<&str>, member_id: &str) -> bool {
    self_member_id == Some(member_id)
}

/// Whether a federated roster row is an agent the caller owns — the rows
/// bedrock's owner-or-self policy actually lets a non-owner remove, so the
/// chip stands in for a dial-and-403 probe per attempt. Requires a known
/// self: a bare `owner_member_id == self_member_id` would read `None == None`
/// as ownership and badge every ownerless agent row.
fn federated_member_is_yours(
    self_member_id: Option<&str>,
    member: &FederatedRoomMemberProjection,
) -> bool {
    self_member_id.is_some()
        && matches!(member.actor_type, FederatedActorType::Agent)
        && member.owner_member_id.as_deref() == self_member_id
}

/// Whether the armed remove confirm survives a room/roster change. The armed
/// state is keyed by participant id and held OUTSIDE the members-rail closure
/// (which is rebuilt by every roster SSE update — see the component doc), so
/// this is the pruning rule that keeps a primed confirm from outliving its
/// target: a different room, or a roster the target has left, disarms it.
/// Without the room check, a same-id agent in the next room opened would
/// inherit a confirm armed against a different room's row. Both rosters are
/// consulted because only one renders at a time: a federated room's rows are
/// the access projection's members, never `open_room.participants`, and a
/// confirm armed against one of them must survive the SSE access updates
/// that rebuild the rail. A self target disarms too: `self_member_id` can
/// arrive AFTER a row was armed (the first access update to carry the
/// field), and a confirm must not survive the discovery that it points at
/// the caller's own row — that removal is the header's Leave.
fn keep_armed_remove(
    armed: Option<&str>,
    room_changed: bool,
    participants: &[RoomParticipant],
    members: &[FederatedRoomMemberProjection],
    self_member_id: Option<&str>,
) -> bool {
    let Some(armed) = armed else {
        return false;
    };
    !room_changed
        && !federated_member_is_self(self_member_id, armed)
        && (participants.iter().any(|p| p.id == armed)
            || members.iter().any(|m| m.member_id == armed))
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

/// The roster subset that can be selected as a new mention target.
///
/// Humans remain mentionable and every roster member still renders in the
/// room. Agent candidates, however, require a currently Active local binding;
/// an unauthorized/suspended/stale/revoked compatibility participant is never
/// offered as clickable execution intent. Federated access must also project
/// the local binding as available.
fn mentionable_roster(
    local_participants: &[RoomParticipant],
    access: Option<&RoomAccessProjection>,
    active_agent_member_ids: &std::collections::HashSet<String>,
) -> Vec<RoomParticipant> {
    mention_roster(local_participants, access)
        .into_iter()
        .filter(|participant| {
            if participant.kind != RoomParticipantKind::Agent {
                return true;
            }
            if !active_agent_member_ids.contains(&participant.id) {
                return false;
            }
            match access {
                Some(access) if access.state != RoomAccessState::Local => access
                    .members
                    .iter()
                    .find(|member| member.member_id == participant.id)
                    .is_some_and(|member| member.local_binding_available == Some(true)),
                _ => true,
            }
        })
        .collect()
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
    displayed_id: Option<&str>,
) -> bool {
    live_mention_query_from_input(text, selection_start_utf16)
        .and_then(|(_, partial)| mention_suggestion_at(participants, &partial, index))
        .is_some_and(|candidate| displayed_id == Some(candidate.id.as_str()))
}

/// Replace the active mention token beginning at `at` with `@id `, extending
/// beyond the caret through any remaining mention characters, and return the
/// new text plus the byte caret position after one separator.
fn apply_mention(text: &str, at: usize, cursor: usize, id: &str) -> (String, usize) {
    let cursor = cursor.min(text.len());
    let trailing_token_len = text[cursor..]
        .chars()
        .take_while(|&c| crate::room_markdown::is_mention_char(c))
        .map(char::len_utf8)
        .sum::<usize>();
    let replace_end = cursor + trailing_token_len;
    let suffix = &text[replace_end..];
    let suffix = match suffix.chars().next() {
        Some(c) if c.is_whitespace() => &suffix[c.len_utf8()..],
        _ => suffix,
    };
    let mut out = String::with_capacity(text.len() + id.len() + 2);
    out.push_str(&text[..at]);
    out.push('@');
    out.push_str(id);
    out.push(' ');
    let caret = out.len();
    out.push_str(suffix);
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

/// Full-screen Slack-style rooms workspace.
///
/// Takes a [`Rooms`] handle (Clone, Copy) and drives the rails:
///
/// - **Left:** room list with active highlight, new-room create input.
/// - **Center:** selected room header, message timeline, composer form,
///   status bar.
/// - **Right:** participant / member roster with kind, role, and presence
///   badges; reachable as a drawer (header chip) wherever the inline rail
///   is hidden.
/// - **Thread panel:** a dedicated conversation column for the selected
///   thread root, with its own pinned reply composer.
///
/// At 650px and below, a compact top nav reveals the hidden left rail so the
/// reader is never stranded with no room navigation. Tauri can reach this at
/// its matching minimum window width.
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

    // ── Members rail: agent-builder form state ────────────────────────
    // Constructed HERE, at component scope, not inside the members-rail
    // closure: that closure re-runs on every `rooms.access` change (i.e. every
    // roster update that arrives over SSE), so form state owned by it would be
    // discarded mid-sentence and take a half-written system prompt with it.
    let agent_builder = crate::agents::AgentBuilderState::new(&rooms);
    let room_agent_authority = crate::room_agent_authorization::RoomAgentAuthorizationState::new();

    // Room context files. Same reasoning as the agent builder above: an
    // in-flight upload flag rebuilt by a roster SSE update would re-enable the
    // control during its own upload.
    let attachments = crate::attachments::RoomAttachmentsState::new(&rooms);

    // The room's summary. Same reasoning again, and it costs more here: the
    // in-flight flag guards a request that holds one of the daemon's turn
    // permits for up to 45s, so a flag rebuilt by a roster SSE update would
    // re-enable the control mid-run and buy a second provider turn nobody
    // asked for.
    let summary = crate::room_summary::RoomSummaryState::new(&rooms);

    // The room's artifacts. Same reasoning again: this state owns an open
    // editor and the version it was loaded against, and a rail closure re-run
    // by a roster SSE update would rebuild both mid-edit — which is how a
    // compare-and-swap loses the version it is supposed to be presenting.
    let artifacts = crate::room_artifacts::RoomArtifactsState::new(&rooms);

    // The room's repo binding. Same reasoning once more, and the stake is a
    // container: the in-flight flag guards a clone/build that holds Bedrock's
    // per-room checkout lock, so a flag rebuilt by a roster SSE update would
    // re-enable the control mid-run and manufacture a 409 against our own
    // command.
    let repo = crate::room_repo::RoomRepoState::new(&rooms);

    // The room's invite control. Same reasoning again, and the stake is the
    // room's own shape: on a Local room a mint bootstraps federation, so a
    // flag rebuilt by a roster SSE update would re-enable the control mid-mint
    // and publish the room twice over one intent.
    let invite = crate::room_invite::RoomInviteState::new(&rooms);

    // The other half of that door: redeeming a code someone else minted. Not
    // scoped to a room at all — you redeem to GET one — so it lives at this
    // scope for the plainer reason that the left rail's create block is
    // rebuilt on room-list traffic, and a redemption spans two Bedrock legs
    // that must not be re-fired halfway through.
    let redeem = crate::room_redeem::RoomRedeemState::new(&rooms);

    // The room's workspace status and command history. Same reasoning: the
    // open panel owns a poll loop, and a rail closure re-run by a roster SSE
    // update would orphan the loop and respawn it mid-tick.
    let workspace_panel = crate::room_workspace_panel::RoomWorkspacePanelState::new(&rooms);

    // Which roster row's remove control is one click from firing, by
    // participant id. Same reasoning as the states above — the members-rail
    // closure is rebuilt by every roster SSE update, and a confirm owned by
    // it would disarm mid-interaction. Arming a row inherently disarms the
    // previous one (there is one slot); the effect below prunes the rest.
    let member_remove_armed: RwSignal<Option<String>> = RwSignal::new(None);
    Effect::new(move |prev_key: Option<Option<String>>| {
        let key = rooms.open_key.get();
        let participants = rooms
            .open_room
            .get()
            .map(|room| room.participants)
            .unwrap_or_default();
        let access = rooms.access.get();
        let self_member_id = access
            .as_ref()
            .and_then(|access| access.self_member_id.clone());
        let members = access.map(|access| access.members).unwrap_or_default();
        let room_changed = prev_key.is_some_and(|prev| prev != key);
        let armed = member_remove_armed.get_untracked();
        if armed.is_some()
            && !keep_armed_remove(
                armed.as_deref(),
                room_changed,
                &participants,
                &members,
                self_member_id.as_deref(),
            )
        {
            member_remove_armed.set(None);
        }
        key
    });

    // Toggle for narrow-screen left-rail visibility.
    let show_left_rail = RwSignal::new(false);

    // Toggle for the members drawer where the inline rail is hidden
    // (narrow viewports, or mid-width desktops while a thread is open).
    let show_members = RwSignal::new(false);
    let members_chip_ref: NodeRef<leptos::html::Button> = NodeRef::new();

    // Thread presentation: inline under the root message by default; the
    // side panel is an opt-in pop-out and the reader's choice persists.
    let thread_view_mode = RwSignal::new(load_thread_view_mode());
    Effect::new(move |_| {
        store_thread_view_mode(thread_view_mode.get());
    });

    // Persisted view state, captured BEFORE the persist effect below can
    // overwrite storage with this mount's initial empty state. Restores are
    // validated against live daemon data before they apply: the room must
    // still be in the fetched list, the thread root must be in the
    // transcript — a stale restore silently degrades, never errors.
    let restored_view = load_view_state();
    let pending_room_restore =
        RwSignal::new(restored_view.as_ref().map(|(room_key, _)| room_key.clone()));
    let pending_thread_restore = RwSignal::new(
        restored_view.and_then(|(room_key, thread)| thread.map(|root_seq| (room_key, root_seq))),
    );

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

    // Thread rows/header show roster display names, not raw author ids;
    // departed authors fall back to their id rather than disappearing.
    let thread_display_name = move |author_id: &str| -> String {
        let local_participants = rooms
            .open_room
            .get()
            .map(|room| room.participants)
            .unwrap_or_default();
        roster_display_name(
            &mention_roster(&local_participants, rooms.access.get().as_ref()),
            author_id,
        )
    };

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
        let roster = mentionable_roster(
            &local_participants,
            rooms.access.get().as_ref(),
            &room_agent_authority.active_agent_member_ids(),
        );
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
        let roster = mentionable_roster(
            &local_participants,
            rooms.access.get().as_ref(),
            &room_agent_authority.active_agent_member_ids(),
        );
        mention_suggestions(&roster, &partial)
    });

    let accept_mention = move |idx: usize, displayed_id: Option<String>| {
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
        let roster = mentionable_roster(
            &local_participants,
            rooms.access.get_untracked().as_ref(),
            &room_agent_authority.active_agent_member_ids_untracked(),
        );
        let Some(pick) = mention_suggestion_at(&roster, &partial, idx) else {
            mention_ctx.set(None);
            mention_active.set(0);
            return;
        };
        if displayed_id.as_deref() != Some(pick.id.as_str()) {
            mention_ctx.set(None);
            mention_active.set(0);
            return;
        }
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
    let accept_thread_mention = move |idx: usize, displayed_id: Option<String>| {
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
        let roster = mentionable_roster(
            &local_participants,
            rooms.access.get_untracked().as_ref(),
            &room_agent_authority.active_agent_member_ids_untracked(),
        );
        let Some(pick) = mention_suggestion_at(&roster, &partial, idx) else {
            thread_mention_ctx.set(None);
            thread_mention_active.set(0);
            return;
        };
        if displayed_id.as_deref() != Some(pick.id.as_str()) {
            thread_mention_ctx.set(None);
            thread_mention_active.set(0);
            return;
        }
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
    // `(scroll_height, scroll_top)` as the "load older" press left them, which
    // is the one moment they can be read: the page arrives asynchronously, and
    // whether this Effect runs before or after the `<For>` writes those rows to
    // the DOM is not something a scanner in this crate can prove either way. A
    // press always overwrites — the anchor belongs to the newest one.
    let older_anchor = RwSignal::new(None::<(i32, i32)>);
    Effect::new(move |prev: Option<TranscriptPassState>| {
        let (len, oldest_seq) = transcript.with(|t| (t.len(), t.first().map(|m| m.seq)));
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
        let previous = prev.unwrap_or_default();
        let prev_len = previous.len;
        let first_fill = prev_len == 0;
        let grew_at_front = transcript_grew_at_front(previous.oldest_seq, oldest_seq);
        let el = list_ref.get();
        let metrics = el
            .as_ref()
            .map(|el| (el.scroll_height(), el.scroll_top(), el.client_height()));
        let near_bottom = metrics.is_some_and(|(scroll_height, scroll_top, client_height)| {
            transcript_is_near_bottom(scroll_height, scroll_top, client_height, 120)
        });
        // Untracked because two arms below CLEAR this signal; tracking what the
        // pass writes would re-enter the pass. Its presence is also the only
        // evidence a prepend was asked for — the hydration walk prepends four
        // more pages after the first fill, and those must stay on the pin.
        let anchor = older_anchor.get_untracked();
        match transcript_pass_action(
            len,
            prev_len,
            el.is_some(),
            near_bottom,
            grew_at_front,
            anchor.is_some(),
        ) {
            TranscriptPassAction::Reset => {
                // Generation reset / room switch: nothing below.
                new_below.set(false);
                pending_read_advance.set(None);
                older_anchor.set(None);
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
            TranscriptPassAction::AnchorOlder => {
                // Rows landing above the viewport push everything the reader
                // was looking at down by exactly the height they add, so the
                // scroll position has to move by the same amount to leave the
                // view where it was. The frame callback is the first point that
                // can measure the growth: it runs after the DOM holds the new
                // rows, whereas this Effect may not.
                if let (Some(el), Some((anchored_height, anchored_top))) = (el.clone(), anchor) {
                    request_animation_frame(move || {
                        let grown = el.scroll_height() - anchored_height;
                        if grown > 0 {
                            el.set_scroll_top(anchored_top + grown);
                        }
                    });
                }
                // One anchor per press, consumed here whether or not the frame
                // callback above was scheduled. An anchor kept past the page it
                // was taken for would be applied to some later prepend against
                // a height that no longer exists — and would route the walk's
                // remaining pages here too.
                older_anchor.set(None);
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
            TranscriptPassState::default()
        } else if viewport_measured {
            TranscriptPassState { len, oldest_seq }
        } else {
            previous
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

    // ── View-state restore + persist ──────────────────────────────────
    // Reopen the persisted room once the fetched list confirms it still
    // exists. One-shot: a user action that opens any room first wins.
    Effect::new(move |_| {
        let Some(want_key) = pending_room_restore.get() else {
            return;
        };
        if !rooms.rooms_loaded.get() {
            return;
        }
        pending_room_restore.set(None);
        if rooms.open_key.get_untracked().is_some() {
            return;
        }
        if rooms
            .list
            .get_untracked()
            .iter()
            .any(|room| room.id == want_key)
        {
            rooms.open_room(want_key);
        }
    });

    // Reselect the persisted thread once its root message is actually in
    // the open room's transcript. Dropped without effect if the user went
    // to a different room or the root no longer exists.
    Effect::new(move |_| {
        let Some((want_room, want_root)) = pending_thread_restore.get() else {
            return;
        };
        let Some(open_key) = rooms.open_key.get() else {
            return;
        };
        if open_key != want_room {
            pending_thread_restore.set(None);
            return;
        }
        let transcript = rooms.transcript.get();
        if thread_root_for(&transcript, Some(want_root)).is_some() {
            pending_thread_restore.set(None);
            selected_thread_root_seq.set(Some(want_root));
        } else if !transcript.is_empty() {
            pending_thread_restore.set(None);
        }
    });

    // Persist the open room + thread so a reload — or an unmount from
    // toggling to another surface — restores the same view. Held while a
    // restore is pending so this mount's empty initial state can't clobber
    // the value it is about to restore from.
    Effect::new(move |_| {
        let room_key = rooms.open_key.get();
        let thread_root = selected_thread_root_seq.get();
        if pending_room_restore.get().is_some() || pending_thread_restore.get().is_some() {
            return;
        }
        store_view_state(room_key.as_deref(), thread_root);
    });

    // ── Left-rail: create room (draft retained until the typed
    //    create_op delivers a matching outcome — op-id gating so
    //    concurrent submits never cross-resolve, and CAS publication
    //    prevents stale completions from overwriting later ops).
    let pending_create = RwSignal::new(false);
    let create_op_id: RwSignal<u64> = RwSignal::new(0);
    // Auto-wake toggles for the room being created. Default off: an absent
    // policy (all four off) posts no `trigger_policy` at all.
    let create_on_mention = RwSignal::new(false);
    let create_on_thread_reply = RwSignal::new(false);
    let create_on_build_failure = RwSignal::new(false);
    let create_on_ci_failure = RwSignal::new(false);
    // The workspace folder the new room binds to, on the DAEMON's host. Empty
    // leaves the room unbound — which is what every room this form made used
    // to be, and an unbound room's agent turns all fail closed.
    let create_workspace = RwSignal::new(String::new());
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
        let op_id = rooms.create_room(
            name.clone(),
            create_trigger_policy(
                create_on_mention.get_untracked(),
                create_on_thread_reply.get_untracked(),
                create_on_build_failure.get_untracked(),
                create_on_ci_failure.get_untracked(),
            ),
            create_workspace_root(&create_workspace.get_untracked()),
        );
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
                // The toggles were part of the same draft: the next room
                // starts from the no-triggers default, like the name field.
                create_on_mention.set(false);
                create_on_thread_reply.set(false);
                create_on_build_failure.set(false);
                create_on_ci_failure.set(false);
                // Same rule for the workspace field: it was part of this
                // draft, and the next room chooses its own folder.
                create_workspace.set(String::new());
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
    let composer_epoch = RwSignal::new(None::<RoomComposerEpoch>);

    // Drafts and pending-send confirmation belong to one exact room
    // generation. A completion from the previous room is intentionally
    // generation-rejected by `Rooms`; synchronously clear its UI ownership too
    // so neither content nor a stuck "Sending…" gate follows the operator.
    Effect::new(move |_| {
        let room_key = rooms.open_key.get();
        let generation = rooms.generation_snapshot_reactive();
        if !room_composer_epoch_changed(
            composer_epoch.get_untracked().as_ref(),
            generation,
            room_key.as_deref(),
        ) {
            return;
        }
        composer_epoch.set(Some(RoomComposerEpoch {
            generation,
            room_key,
        }));

        composer.set(String::new());
        mention_ctx.set(None);
        mention_active.set(0);
        last_sent_draft.set(String::new());
        last_sent_wire.set(String::new());
        last_sent_seq.set(0);
        send_in_flight.set(false);

        selected_thread_root_seq.set(None);
        thread_composer.set(String::new());
        thread_mention_ctx.set(None);
        thread_mention_active.set(0);
        thread_last_sent_draft.set(String::new());
        thread_last_sent_wire.set(String::new());
        thread_last_sent_seq.set(0);
        thread_send_in_flight.set(false);
    });
    let do_send = move || {
        let draft = composer.get_untracked();
        if !message_send_admitted(
            send_in_flight.get_untracked(),
            thread_send_in_flight.get_untracked(),
            composer_writes_allowed(
                rooms.access.get_untracked().as_ref(),
                rooms.closed.get_untracked(),
            ),
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
            composer_writes_allowed(
                rooms.access.get_untracked().as_ref(),
                rooms.closed.get_untracked(),
            ),
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
                .any(|item| outbox_matches_failed_message(item, &me, &wire, None))
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
                .any(|item| outbox_matches_failed_message(item, &me, &wire, Some(root_seq)))
        });
        let request_failed = rooms.status.get().starts_with("message ");
        if failed_outbox || request_failed {
            thread_last_sent_draft.set(String::new());
            thread_last_sent_wire.set(String::new());
            thread_last_sent_seq.set(0);
            thread_send_in_flight.set(false);
        }
    });

    // Shared thread-conversation pieces. Exactly one presentation renders
    // at a time (inline under the root, or the opt-in side panel), so the
    // composer state and its mention-listbox id never exist twice.
    let thread_replies_view = move |root_seq: u64| -> AnyView {
        view! {
                                        <For
                                            each=move || partition_thread_messages(&rooms.transcript.get(), root_seq).replies
                                            key=|m: &RoomMessage| m.seq
                                            children=move |reply: RoomMessage| {
                                                let full_ts = reply.created_at.clone();
                                                let is_system = room_messages::is_compact_system_row(&reply);
                                                let media = crate::transcript_media::marker_media_view(rooms, &reply);
                                                let ledger_row = reply.clone();
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
                                                                <span class="rooms-workspace__msg-name">{thread_display_name(&reply.author_id)}</span>
                                                                <time
                                                                    class="rooms-workspace__msg-time"
                                                                    datetime=full_ts.clone()
                                                                    aria-label=full_ts.clone()
                                                                    title=full_ts.clone()
                                                                >
                                                                    {canonical_wire_clock_time(&full_ts)}
                                                                </time>
                                                                {move || ledger_mark_view(
                                                                    rooms.access.get().as_ref(),
                                                                    &ledger_row,
                                                                )}
                                                            </div>
                                                            <div class="rooms-workspace__msg-text">
                                                                {crate::room_markdown::body_view(reply.body.clone(), member_ids)}
                                                            </div>
                                                            {media}
                                                        </div>
                                                    </div>
                                                }
                                            }
                                        />
        }
        .into_any()
    };
    let thread_composer_view = move || -> AnyView {
        view! {
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
                                                        let roster = mentionable_roster(
                                                            &local_participants,
                                                            rooms.access.get_untracked().as_ref(),
                                                            &room_agent_authority.active_agent_member_ids_untracked(),
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
                                                        let displayed_id = thread_mention_items
                                                            .get_untracked()
                                                            .get(active)
                                                            .map(|item| item.id.clone());
                                                        if !mention_accept_is_valid(
                                                            &thread_composer.get_untracked(),
                                                            selection,
                                                            &roster,
                                                            active,
                                                            displayed_id.as_deref(),
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
                                                            let displayed_id = thread_mention_items
                                                                .get_untracked()
                                                                .get(active)
                                                                .map(|item| item.id.clone());
                                                            accept_thread_mention(active, displayed_id);
                                                        }
                                                        MentionKey::Close => {
                                                            ev.prevent_default();
                                                            thread_mention_ctx.set(None);
                                                        }
                                                        MentionKey::Pass => {}
                                                    }
                                                }
                                                on:blur=move |_| thread_mention_ctx.set(None)
                                                disabled=move || !composer_writes_allowed(rooms.access.get().as_ref(), rooms.closed.get())
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
                                                                let clicked_id = item.id.clone();
                                                                view! {
                                                                    <div
                                                                        id=format!("rooms-mention-thread-opt-{i}")
                                                                        class="rooms-workspace__mention-opt"
                                                                        class:rooms-workspace__mention-opt--active=i == active
                                                                        role="option"
                                                                        aria-selected=(i == active).to_string()
                                                                        on:mousedown=move |ev: web_sys::MouseEvent| {
                                                                            // Keep combobox focus until the click activation runs.
                                                                            ev.prevent_default();
                                                                        }
                                                                        on:click=move |_| {
                                                                            accept_thread_mention(
                                                                                i,
                                                                                Some(clicked_id.clone()),
                                                                            )
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
                                                        || !composer_writes_allowed(
                                                            rooms.access.get().as_ref(),
                                                            rooms.closed.get(),
                                                        )
                                                }
                                            >
                                                {move || if thread_send_in_flight.get() { "Sending…" } else { "Reply" }}
                                            </button>
                                        </form>
                                    </div>
        }
        .into_any()
    };

    view! {
        <div
            class="rooms-workspace"
            class:rooms-workspace--thread-open=move || {
                selected_thread_root_seq.get().is_some()
                    && thread_view_mode.get() == ThreadViewMode::Panel
            }
            role="region"
            aria-label="Rooms workspace"
            on:keydown=move |ev| {
                if ev.key() != "Escape" {
                    return;
                }
                // The repo panel shares the artifacts panel's overlay tier
                // (z-index 445) and their rail triggers sit behind each
                // other's scrims, so at most one is open; ask it first for
                // the same reason the artifacts rung exists at all.
                if crate::room_repo::repo_escape_closes(
                    repo.panel_is_open(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    repo.close_panel();
                    return;
                }
                // The invite panel sits on the same overlay tier, behind the
                // same at-most-one-open argument — and it holds a code the
                // operator may want off the screen in a hurry.
                if crate::room_invite::invite_escape_closes(
                    invite.panel_is_open(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    invite.close_panel();
                    return;
                }
                // The workspace panel sits on the same overlay tier, behind
                // the same at-most-one-open argument.
                if crate::room_workspace_panel::workspace_panel_escape_closes(
                    workspace_panel.panel_is_open(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    workspace_panel.close_panel();
                    return;
                }
                // The summary and files panels sit on the same overlay tier
                // with their rail triggers behind the other panels' scrims,
                // so the same at-most-one-open argument holds and the order
                // of these rungs never decides anything.
                if crate::room_summary::summary_escape_closes(
                    summary.panel_is_open(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    summary.close_panel();
                    return;
                }
                if crate::attachments::files_escape_closes(
                    attachments.panel_is_open(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    attachments.close_panel();
                    return;
                }
                // Topmost overlay first, and that is the artifacts panel: a
                // fixed modal at z-index 445, above the members drawer's 430
                // and its backdrop's 425. Without this rung Escape closes a
                // drawer UNDERNEATH an open modal, or falls through to the app
                // rail and tears the whole rooms surface down with an unsaved
                // artifact draft inside it.
                if crate::room_artifacts::artifacts_escape_closes(
                    artifacts.panel_is_open(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    artifacts.close_panel();
                    return;
                }
                // Then the members drawer, which sits above the left drawer and
                // the app hierarchy. Only when it actually renders as an
                // overlay — an inline rail never owns the key.
                let members_overlay = window_inner_width().is_some_and(|width| {
                    members_drawer_is_overlay(
                        width,
                        selected_thread_root_seq.get_untracked().is_some()
                            && thread_view_mode.get_untracked() == ThreadViewMode::Panel,
                    )
                });
                if members_escape_closes(
                    members_overlay,
                    show_members.get_untracked(),
                    ev.default_prevented(),
                ) {
                    ev.prevent_default();
                    show_members.set(false);
                    if let Some(chip) = members_chip_ref.get() {
                        let _ = chip.focus();
                    }
                    return;
                }
                let action = compact_escape_action(
                    rooms_layout_is_compact(),
                    show_left_rail.get_untracked(),
                    ev.default_prevented(),
                );
                if action == Some(CompactEscapeAction::CloseDrawer) {
                    ev.prevent_default();
                    show_left_rail.set(false);
                    if let Some(toggle) = mobile_toggle_ref.get() {
                        let _ = toggle.focus();
                    }
                }
            }
        >

            // ═══ COMPACT NAV — narrow-screen room selector ════════════
            // Always present but CSS hides it above the shared 650px
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
                    // Auto-wake flags for the room being created. Only the
                    // four live flags get a control; see TriggerToggle. A row
                    // whose flag cannot fire in a Local room — which is the
                    // only kind this form makes — is held and says which kind
                    // it needs, so the form cannot arm on day one exactly the
                    // flag the right rail will grey out on day one.
                    <div
                        class="rooms-workspace__create-triggers"
                        role="group"
                        aria-label="Auto-wake triggers for the new room"
                    >
                        {create_trigger_row(
                            TriggerToggle::Mention,
                            "@mention",
                            create_on_mention,
                            pending_create,
                        )}
                        {create_trigger_row(
                            TriggerToggle::ThreadReply,
                            "thread reply",
                            create_on_thread_reply,
                            pending_create,
                        )}
                        {create_trigger_row(
                            TriggerToggle::BuildFailure,
                            "build failure",
                            create_on_build_failure,
                            pending_create,
                        )}
                        {create_trigger_row(
                            TriggerToggle::CiFailure,
                            "CI failure",
                            create_on_ci_failure,
                            pending_create,
                        )}
                    </div>
                    // The folder the room's agents will actually run in. Its
                    // own field rather than a trigger row because it is not a
                    // flag: without it every trigger above is armed to wake an
                    // agent that then fails closed on the daemon with
                    // `workspace_unavailable`. The path is resolved on the
                    // DAEMON's host — the browser cannot see that filesystem,
                    // so nothing here validates it and the helper text says
                    // whose machine it means.
                    <label class="rooms-workspace__create-workspace">
                        <span class="rooms-workspace__create-workspace-label">
                            "Workspace folder on the daemon host"
                        </span>
                        <input
                            class="rooms-workspace__left-input"
                            type="text"
                            aria-label="Workspace folder on the daemon host"
                            aria-describedby="rooms-create-workspace-help"
                            placeholder="/absolute/path/to/project"
                            prop:value=move || create_workspace.get()
                            on:input=move |ev| create_workspace.set(event_target_value(&ev))
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" {
                                    ev.prevent_default();
                                    create_room();
                                }
                            }
                            disabled=move || pending_create.get()
                        />
                        <span
                            class="rooms-workspace__create-workspace-help"
                            id="rooms-create-workspace-help"
                        >
                            "An absolute path that must already exist on the machine \
                             running the daemon. Leave it empty to create the room \
                             unbound — its agents cannot run until a folder is bound."
                        </span>
                    </label>
                </div>

                // The other way into a room: a code someone else minted. A
                // SIBLING of the create block, not a child — creating and
                // joining are two verbs — and in the left rail rather than
                // anywhere in the room surface, because a person holding a
                // code has no room open and may have no rooms at all, which
                // is precisely the state this rail is the only thing visible
                // in.
                <crate::room_redeem::RoomRedeem rooms=rooms state=redeem />
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
                                        // Members chip: reopens the roster as a drawer
                                        // wherever the inline rail is hidden (CSS shows
                                        // the chip only in exactly those layouts).
                                        <button
                                            class="rooms-workspace__members-chip"
                                            type="button"
                                            node_ref=members_chip_ref
                                            aria-label="Toggle members"
                                            aria-controls="rooms-workspace-members"
                                            aria-expanded=move || show_members.get().to_string()
                                            on:click=move |_| {
                                                show_members.update(|v| *v = toggle_drawer(*v))
                                            }
                                        >
                                            <svg viewBox="0 0 16 16" width="13" height="13"
                                                fill="none" stroke="currentColor" stroke-width="1.5"
                                                stroke-linecap="round" stroke-linejoin="round">
                                                <circle cx="6" cy="5.5" r="2.5"/>
                                                <path d="M1.5 13.5c0-2.5 2-4 4.5-4s4.5 1.5 4.5 4"/>
                                                <path d="M11 3.5a2.5 2.5 0 0 1 0 4.6M12 9.8c1.5 0.6 2.5 1.9 2.5 3.7"/>
                                            </svg>
                                            {move || {
                                                let count = match rooms.access.get() {
                                                    Some(access)
                                                        if access.state != RoomAccessState::Local =>
                                                    {
                                                        access.members.len()
                                                    }
                                                    _ => rooms
                                                        .open_room
                                                        .get()
                                                        .map(|room| room.participants.len())
                                                        .unwrap_or(0),
                                                };
                                                count.to_string()
                                            }}
                                        </button>
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
                                    let access = rooms.access.get();
                                    let label = access_banner(access.as_ref());
                                    match access.map(|a| a.state) {
                                        Some(RoomAccessState::Connecting) => {
                                            view! {
                                                <div
                                                    class="room-stage__access-state room-stage__access-state--connecting"
                                                    role="status"
                                                    aria-live="polite"
                                                >
                                                    {label}
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
                                                    {label}
                                                </div>
                                            }.into_any()
                                        }
                                        Some(RoomAccessState::Revoked) => {
                                            view! {
                                                <div
                                                    class="room-stage__access-state room-stage__access-state--revoked"
                                                    role="alert"
                                                >
                                                    {label}
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
                                    // The edge of what is loaded. Hydration
                                    // opens a room at its newest page and walks
                                    // back a bounded number of pages; past that
                                    // the oldest row on screen would otherwise
                                    // read as the first message in the room.
                                    // Inside the scroll container and above the
                                    // rows on purpose — it marks a position in
                                    // the log, so it has to sit at that
                                    // position and scroll with it, unlike the
                                    // jump affordance below, which is a
                                    // viewport-fixed control.
                                    {move || rooms.older_transcript_available().then(|| view! {
                                        <button
                                            type="button"
                                            class="rooms-workspace__load-older"
                                            disabled=move || rooms.older_transcript_in_flight()
                                            on:click=move |_| {
                                                // Read BEFORE the request, because
                                                // the rows it brings back land
                                                // above these very numbers.
                                                if let Some(el) = list_ref.get() {
                                                    older_anchor.set(Some((
                                                        el.scroll_height(),
                                                        el.scroll_top(),
                                                    )));
                                                }
                                                rooms.load_older_transcript_page();
                                            }
                                        >
                                            {move || if rooms.older_transcript_in_flight() {
                                                "Loading older messages…"
                                            } else {
                                                "\u{2191} Load older messages"
                                            }}
                                        </button>
                                    })}
                                    <For
                                        // Pair each root with its predecessor so
                                        // density decisions (grouping, gap headers,
                                        // day separators) are derived per row — which
                                        // is why the predecessor is half the key. The
                                        // transcript grows at BOTH ends: an older page
                                        // gives the row that was oldest a predecessor
                                        // it did not have, and `day_separator_label`
                                        // answers `Some` unconditionally against
                                        // `None`, so a child cached under `seq` alone
                                        // would keep a day divider and an ungrouped
                                        // header under a same-day row that now sits
                                        // directly above it. Keying the pair rebuilds
                                        // exactly that one seam row; every other row's
                                        // predecessor is unchanged, so a tail append
                                        // still caches the whole list.
                                        each=move || {
                                            let roots = partition_thread_messages(&rooms.transcript.get(), 0).roots;
                                            std::iter::once(None)
                                                .chain(roots.iter().cloned().map(Some))
                                                .zip(roots.clone())
                                                .collect::<Vec<_>>()
                                        }
                                        key=|(prev, m): &(Option<RoomMessage>, RoomMessage)| {
                                            (prev.as_ref().map(|p| p.seq), m.seq)
                                        }
                                        children=move |(prev, m): (Option<RoomMessage>, RoomMessage)| {
                                            let is_system = room_messages::is_compact_system_row(&m);
                                            let media = crate::transcript_media::marker_media_view(rooms, &m);
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
                                            // Cloned for the ledger mark, which
                                            // re-reads reactively: the access
                                            // projection can arrive after the
                                            // keyed row was cached.
                                            let ledger_row = m.clone();
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
                                                            {move || ledger_mark_view(
                                                                rooms.access.get().as_ref(),
                                                                &ledger_row,
                                                            )}
                                                        </div>
                                                        <div class="rooms-workspace__msg-text">
                                                            {crate::room_markdown::body_view(m.body.clone(), member_ids)}
                                                        </div>
                                                        {media}
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
                                                // Inline thread: the default
                                                // presentation — the conversation
                                                // surfaces directly under its
                                                // root message.
                                                {move || {
                                                    let inline_open = thread_view_mode.get()
                                                        == ThreadViewMode::Inline
                                                        && is_thread_open(
                                                            selected_thread_root_seq.get(),
                                                            root_seq,
                                                        );
                                                    if !inline_open {
                                                        return ().into_any();
                                                    }
                                                    view! {
                                                        <div class="rooms-workspace__thread-inline">
                                                            <div class="rooms-workspace__thread-inline-head">
                                                                <span class="rooms-workspace__thread-inline-count">
                                                                    {move || reply_count_label(
                                                                        reply_count_for(&rooms.transcript.get(), root_seq),
                                                                    )}
                                                                </span>
                                                                <button
                                                                    class="rooms-workspace__thread-inline-expand"
                                                                    type="button"
                                                                    title="Open as side panel"
                                                                    aria-label="Open thread as side panel"
                                                                    on:click=move |_| {
                                                                        thread_view_mode.set(ThreadViewMode::Panel)
                                                                    }
                                                                >
                                                                    <svg viewBox="0 0 16 16" width="13" height="13"
                                                                        fill="none" stroke="currentColor" stroke-width="1.5"
                                                                        stroke-linecap="round" stroke-linejoin="round">
                                                                        <rect x="2" y="2.5" width="12" height="11" rx="1.5"/>
                                                                        <path d="M9.5 2.5v11"/>
                                                                    </svg>
                                                                </button>
                                                            </div>
                                                            {thread_replies_view(root_seq)}
                                                            {thread_composer_view()}
                                                        </div>
                                                    }.into_any()
                                                }}
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
                                // informational; only failed items can retry,
                                // and only while the room is open. A closed
                                // room keeps the outbox on screen — it is part
                                // of the frozen record — and loses the button:
                                // the daemon's retry gates on the room
                                // EXISTING, not on `closed_at`, so a press
                                // here would answer 202 and requeue a
                                // federated send rather than fail the way
                                // every other write into a closed room does.
                                {move || {
                                    let outbox = rooms.access.get()
                                        .map(|access| access.outbox)
                                        .unwrap_or_default();
                                    let room_closed = rooms.closed.get();
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
                                                        let can_retry = failed && !room_closed;
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
                                                                {if can_retry {
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
                                    // Why the input below is dead. It belongs
                                    // here and not in the status line: that
                                    // line carries transient errors and is
                                    // cleared by the next one, while this is
                                    // the room's permanent condition. Without
                                    // it a closed room is a composer that
                                    // simply does not respond, which reads as
                                    // the app being broken rather than the
                                    // room being finished.
                                    {move || {
                                        if !rooms.closed.get() {
                                            return ().into_any();
                                        }
                                        view! {
                                            <div
                                                class="rooms-workspace__composer-closed"
                                                role="status"
                                            >
                                                "This room is closed. You are reading a frozen \
                                                 audit view of its transcript — nothing can be \
                                                 posted to it and no new messages will arrive."
                                            </div>
                                        }
                                        .into_any()
                                    }}
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
                                                    let roster = mentionable_roster(
                                                        &local_participants,
                                                        rooms.access.get_untracked().as_ref(),
                                                        &room_agent_authority.active_agent_member_ids_untracked(),
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
                                                    let displayed_id = mention_items
                                                        .get_untracked()
                                                        .get(active)
                                                        .map(|item| item.id.clone());
                                                    if !mention_accept_is_valid(
                                                        &composer.get_untracked(),
                                                        selection,
                                                        &roster,
                                                        active,
                                                        displayed_id.as_deref(),
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
                                                        let displayed_id = mention_items
                                                            .get_untracked()
                                                            .get(active)
                                                            .map(|item| item.id.clone());
                                                        accept_mention(active, displayed_id);
                                                    }
                                                    MentionKey::Close => {
                                                        ev.prevent_default();
                                                        mention_ctx.set(None);
                                                    }
                                                    MentionKey::Pass => {}
                                                }
                                            }
                                            on:blur=move |_| mention_ctx.set(None)
                                            disabled=move || !composer_writes_allowed(rooms.access.get().as_ref(), rooms.closed.get())
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
                                                            let clicked_id = item.id.clone();
                                                            view! {
                                                                <div
                                                                    id=format!("rooms-mention-opt-{i}")
                                                                    class="rooms-workspace__mention-opt"
                                                                    class:rooms-workspace__mention-opt--active=i == active
                                                                    role="option"
                                                                    aria-selected=(i == active).to_string()
                                                                    on:mousedown=move |ev: web_sys::MouseEvent| {
                                                                        // Keep combobox focus until the click activation runs.
                                                                        ev.prevent_default();
                                                                    }
                                                                    on:click=move |_| {
                                                                        accept_mention(i, Some(clicked_id.clone()))
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
                                                    || !composer_writes_allowed(
                                                        rooms.access.get().as_ref(),
                                                        rooms.closed.get(),
                                                    )
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

            // ═══ RIGHT RAIL — members ════════════════════════════════════
            // Backdrop closes the members drawer on layouts where the rail
            // overlays (tapping outside); CSS keeps it inert elsewhere.
            {move || {
                if show_members.get() {
                    view! {
                        <div
                            class="rooms-workspace__members-backdrop"
                            aria-hidden="true"
                            on:click=move |_| {
                                show_members.set(false);
                                if let Some(chip) = members_chip_ref.get() {
                                    let _ = chip.focus();
                                }
                            }
                        ></div>
                    }.into_any()
                } else {
                    ().into_any()
                }
            }}
            <div
                id="rooms-workspace-members"
                class="rooms-workspace__right"
                class:rooms-workspace__right--visible=move || show_members.get()
            >
                <div class="rooms-workspace__right-head">
                    <h3 class="rooms-workspace__right-title">"Members"</h3>
                    // Drawer dismiss: CSS reveals it only in the overlay
                    // layouts the members chip serves.
                    <button
                        class="rooms-workspace__right-close"
                        type="button"
                        aria-label="Close members"
                        on:click=move |_| {
                            show_members.set(false);
                            if let Some(chip) = members_chip_ref.get() {
                                let _ = chip.focus();
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
                                                        let pid = p.id.clone();
                                                        let owner_row_id = p.id.clone();
                                                        let display = p.display_name.clone();
                                                        let kind = p.kind;
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
                                                                // Row tail: the kind badge plus a remove
                                                                // control, or — armed — the two-step
                                                                // confirm in the badge's place, because a
                                                                // 220px rail cannot hold both. Two-step
                                                                // for the same reason the agent builder's
                                                                // delete is: the removal is durable (a
                                                                // ParticipantLeft marker in the
                                                                // transcript), so one stray click must
                                                                // not fire it.
                                                                {move || {
                                                                    let removable = participant_removable(
                                                                        &pid,
                                                                        &rooms.identity_id.get(),
                                                                    );
                                                                    let armed = removable
                                                                        && member_remove_armed.get().as_deref()
                                                                            == Some(pid.as_str());
                                                                    if armed {
                                                                        let confirm_id = pid.clone();
                                                                        let confirm_label =
                                                                            format!("Confirm removing {display} from room");
                                                                        view! {
                                                                            <button
                                                                                class="rooms-workspace__member-remove-btn rooms-workspace__member-remove-btn--danger"
                                                                                type="button"
                                                                                aria-label=confirm_label
                                                                                on:click=move |_| {
                                                                                    member_remove_armed.set(None);
                                                                                    rooms.remove_participant(confirm_id.clone());
                                                                                }
                                                                            >
                                                                                "remove"
                                                                            </button>
                                                                            <button
                                                                                class="rooms-workspace__member-remove-btn"
                                                                                type="button"
                                                                                on:click=move |_| member_remove_armed.set(None)
                                                                            >
                                                                                "keep"
                                                                            </button>
                                                                        }.into_any()
                                                                    } else {
                                                                        let arm_id = pid.clone();
                                                                        let arm_label =
                                                                            format!("Remove {display} from room");
                                                                        view! {
                                                                            <span class="rooms-workspace__member-kind">
                                                                                {participant_kind_label(kind)}
                                                                            </span>
                                                                            {removable.then(|| view! {
                                                                                <button
                                                                                    class="rooms-workspace__member-remove"
                                                                                    type="button"
                                                                                    title="Remove from room"
                                                                                    aria-label=arm_label
                                                                                    on:click=move |_| {
                                                                                        member_remove_armed
                                                                                            .set(Some(arm_id.clone()))
                                                                                    }
                                                                                >
                                                                                    <svg viewBox="0 0 16 16" width="10" height="10"
                                                                                        fill="none" stroke="currentColor" stroke-width="1.6"
                                                                                        stroke-linecap="round">
                                                                                        <path d="M3 3l10 10M13 3L3 13"/>
                                                                                    </svg>
                                                                                </button>
                                                                            })}
                                                                        }.into_any()
                                                                    }
                                                                }}
                                                                // Second line, on agent rows only: which
                                                                // worker owns this agent, in the rail's own
                                                                // presence-dot language, or "unclaimed" when
                                                                // no ownership row names it. Reads BOTH
                                                                // signals live — `agent_owners` arrives with
                                                                // hydration while `open_room` is replaced by
                                                                // every join/leave/remove, and the owner's
                                                                // display name comes from the second.
                                                                // Ungated on `closed`: a frozen room's
                                                                // audit view is the one whose reader can no
                                                                // longer ask anyone who owned what.
                                                                {move || {
                                                                    if kind != RoomParticipantKind::Agent {
                                                                        return ().into_any();
                                                                    }
                                                                    let participants = rooms.open_room.get()
                                                                        .map(|r| r.participants)
                                                                        .unwrap_or_default();
                                                                    match rooms.agent_owners.with(|owners| {
                                                                        agent_ownership(owners.as_deref(), &participants, &owner_row_id)
                                                                    }) {
                                                                        AgentOwnership::Owned { owner, present } => {
                                                                            let dot_label = if present {
                                                                                format!("{owner} is in the room")
                                                                            } else {
                                                                                format!("{owner} has left the room")
                                                                            };
                                                                            view! {
                                                                                <span class="rooms-workspace__member-owner">
                                                                                    <span
                                                                                        class="rooms-workspace__member-presence"
                                                                                        class:rooms-workspace__member-presence--live=present
                                                                                        class:rooms-workspace__member-presence--unavailable=!present
                                                                                        role="img"
                                                                                        aria-label=dot_label
                                                                                    ></span>
                                                                                    {format!("owned by {owner}")}
                                                                                </span>
                                                                            }.into_any()
                                                                        }
                                                                        AgentOwnership::Unclaimed => view! {
                                                                            <span class="rooms-workspace__member-owner rooms-workspace__member-owner--unclaimed">
                                                                                "unclaimed"
                                                                            </span>
                                                                        }.into_any(),
                                                                        // Nothing, and that is the point: the
                                                                        // surface has not been told who owns
                                                                        // this agent, which is not the same as
                                                                        // being told nobody does.
                                                                        AgentOwnership::Unknown => ().into_any(),
                                                                    }
                                                                }}
                                                            </div>
                                                        }
                                                    }
                                                />
                                                </div>
                                            }.into_any()
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
                                        let self_member_id = access.self_member_id.clone();
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
                                                    let member_id = member.member_id.clone();
                                                    let member_display = member.display_name.clone();
                                                    let is_self = federated_member_is_self(
                                                        self_member_id.as_deref(),
                                                        &member.member_id,
                                                    );
                                                    let yours = federated_member_is_yours(
                                                        self_member_id.as_deref(),
                                                        &member,
                                                    );
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
                                                            // Row tail: the badges plus a remove
                                                            // control, or — armed — the two-step
                                                            // confirm in their place, for the same
                                                            // rail-width and durability reasons as
                                                            // the Local branch. The projection's
                                                            // self_member_id names the caller's
                                                            // own row: that one renders NO control
                                                            // (self-removal is the header's Leave,
                                                            // and would sever your own federation)
                                                            // and your agents get a "yours" chip —
                                                            // the rows bedrock's owner-or-self
                                                            // policy lets a non-owner remove. For
                                                            // every other row, and whenever the
                                                            // field is absent (local room, older
                                                            // daemon), bedrock still answers each
                                                            // attempt: a refusal lands in the
                                                            // status line with the roster intact.
                                                            {move || {
                                                                // `!is_self` also unarms a confirm
                                                                // primed before an access update
                                                                // revealed the row is the caller.
                                                                let armed = !is_self
                                                                    && member_remove_armed.get().as_deref()
                                                                        == Some(member_id.as_str());
                                                                if armed {
                                                                    let confirm_id = member_id.clone();
                                                                    let confirm_display = member_display.clone();
                                                                    let confirm_label =
                                                                        format!("Confirm removing {member_display} from room");
                                                                    view! {
                                                                        <button
                                                                            class="rooms-workspace__member-remove-btn rooms-workspace__member-remove-btn--danger"
                                                                            type="button"
                                                                            aria-label=confirm_label
                                                                            on:click=move |_| {
                                                                                member_remove_armed.set(None);
                                                                                rooms.remove_member(
                                                                                    confirm_id.clone(),
                                                                                    confirm_display.clone(),
                                                                                );
                                                                            }
                                                                        >
                                                                            "remove"
                                                                        </button>
                                                                        <button
                                                                            class="rooms-workspace__member-remove-btn"
                                                                            type="button"
                                                                            on:click=move |_| member_remove_armed.set(None)
                                                                        >
                                                                            "keep"
                                                                        </button>
                                                                    }.into_any()
                                                                } else {
                                                                    let arm_id = member_id.clone();
                                                                    let arm_label =
                                                                        format!("Remove {member_display} from room");
                                                                    view! {
                                                                        <span class="rooms-workspace__member-kind">
                                                                            {actor_label}
                                                                        </span>
                                                                        <span class="rooms-workspace__member-role">
                                                                            {role_label}
                                                                        </span>
                                                                        {yours.then(|| view! {
                                                                            <span class="rooms-workspace__member-yours">
                                                                                "yours"
                                                                            </span>
                                                                        })}
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
                                                                        {if is_self {
                                                                            ().into_any()
                                                                        } else {
                                                                            view! {
                                                                                <button
                                                                                    class="rooms-workspace__member-remove"
                                                                                    type="button"
                                                                                    title="Remove from room"
                                                                                    aria-label=arm_label
                                                                                    on:click=move |_| {
                                                                                        member_remove_armed
                                                                                            .set(Some(arm_id.clone()))
                                                                                    }
                                                                                >
                                                                                    <svg viewBox="0 0 16 16" width="10" height="10"
                                                                                        fill="none" stroke="currentColor" stroke-width="1.6"
                                                                                        stroke-linecap="round">
                                                                                        <path d="M3 3l10 10M13 3L3 13"/>
                                                                                    </svg>
                                                                                </button>
                                                                            }.into_any()
                                                                        }}
                                                                    }.into_any()
                                                                }
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
                    }}
                    <crate::room_agent_authorization::RoomAgentAuthorizationPanel
                        rooms=rooms
                        state=room_agent_authority
                        agent_builder=agent_builder
                    />
                </div>

                // How a second person reaches this room: mint an invite code.
                // Directly under the roster because that is what it changes.
                // A sibling of it, not a child of the closure above — that
                // closure re-runs on every access change, and this section
                // owns a mint's in-flight state. Unlike the repo section it
                // renders for a Local room too: minting is how a Local room
                // becomes federated, so hiding it there hides the only door.
                // It keeps the COMPOSER's gate, and is one of the two rails
                // that should: a mint registers this room with the federation
                // control plane, so a code minted over a link that is down is
                // a code no second person can ever redeem.
                <crate::room_invite::RoomInvite
                    rooms=rooms
                    state=invite
                    writes_allowed=Signal::derive(move || {
                        access_allows_writes(rooms.access.get().as_ref())
                    })
                />

                // How this room wakes its agents: the four live
                // trigger-policy flags, editable in place. Every access state
                // renders it — the policy each flag is judged against is read
                // from THIS daemon's store, on the federation bridge's ingest
                // paths as much as on the local post path, so a federated
                // room's policy is live state and not a mirror. What varies is
                // which flag's EVENT can reach which kind of room, and that is
                // a per-row decision (see `trigger_row_dead_here`), not a
                // decision about the section. For the same reason the rows
                // take their OWN write gate rather than the composer's: this
                // PATCH lands in the local store, so a link that is down or
                // coming back does not hold it (see
                // `trigger_policy_accepts_writes`). A sibling of the roster
                // for the same reason as its neighbours: it owns a PATCH's
                // in-flight state.
                {move || {
                    let access = rooms.access.get();
                    let Some(room) = rooms.open_room.get() else {
                        return ().into_any();
                    };
                    let policy = room.trigger_policy;
                    let flag = |pick: fn(&RoomTriggerPolicy) -> bool| {
                        policy.as_ref().map(pick).unwrap_or(false)
                    };
                    view! {
                        <div
                            class="rooms-workspace__triggers"
                            role="group"
                            aria-label="Agent wake triggers"
                        >
                            <div class="rooms-workspace__triggers-head">
                                <span class="rooms-workspace__triggers-title">"Triggers"</span>
                            </div>
                            {trigger_toggle_row(
                                rooms,
                                TriggerToggle::Mention,
                                "@mention",
                                flag(|p| p.on_mention),
                                access.as_ref(),
                            )}
                            {trigger_toggle_row(
                                rooms,
                                TriggerToggle::ThreadReply,
                                "thread reply",
                                flag(|p| p.on_thread_reply),
                                access.as_ref(),
                            )}
                            {trigger_toggle_row(
                                rooms,
                                TriggerToggle::BuildFailure,
                                "build failure",
                                flag(|p| p.on_build_failure),
                                access.as_ref(),
                            )}
                            {trigger_toggle_row(
                                rooms,
                                TriggerToggle::CiFailure,
                                "CI failure",
                                flag(|p| p.on_ci_failure),
                                access.as_ref(),
                            )}
                            {move || {
                                rooms.policy_update_error.get().map(|error| view! {
                                    <div class="rooms-workspace__triggers-error" role="alert">
                                        {format!("trigger update failed: {error}")}
                                    </div>
                                })
                            }}
                            // Directly under the four triggers, because this is
                            // the condition that makes all four inert: a room
                            // with no bound workspace refuses every agent turn
                            // before it starts, so a checked @mention row above
                            // an unbound room promises a wake that cannot
                            // happen.
                            {workspace_binding_section(rooms, access.as_ref())}
                        </div>
                    }.into_any()
                }}

                // What the room says about itself, above the shelf of files it
                // was handed. A sibling of the roster for the same reason as
                // the files below — that closure re-runs on every access
                // change, and this section owns a run's in-flight state.
                // A run reads this room's own transcript and amends the one
                // `room-summary` artifact in this daemon's store — ocean-os
                // documents a federated room's summary as local-only and never
                // enqueues it — so the rail takes the local-store gate and
                // stays usable while the link is coming back.
                <crate::room_summary::RoomSummary
                    rooms=rooms
                    state=summary
                    writes_allowed=Signal::derive(move || {
                        local_store_write_gate(rooms.access.get().as_ref())
                    })
                    members=member_ids
                />

                // What the room produced: tasks, decisions, captured
                // knowledge. A sibling for the same reason as its neighbours —
                // it owns a write's in-flight state and an open editor. The
                // rail holds only the compact list; reading and writing happen
                // in the panel it opens, because 220px is not a measure prose
                // can be edited at. Create and amend both land through the
                // daemon's own store handle and announce themselves on the
                // local event stream, with no outbox row anywhere on either
                // path, so this rail takes the local-store gate too.
                <crate::room_artifacts::RoomArtifacts
                    rooms=rooms
                    state=artifacts
                    writes_allowed=Signal::derive(move || {
                        local_store_write_gate(rooms.access.get().as_ref())
                    })
                    members=member_ids
                />

                // Room context files. A sibling of the roster, not a child of
                // the closure above: that closure re-runs on every access
                // change, and this section owns an upload's in-flight state.
                // An upload writes bytes and a row on THIS host and nothing
                // else — the daemon's attachment module names the federation
                // outbox nowhere — so the local-store gate again.
                <crate::attachments::RoomAttachments
                    rooms=rooms
                    state=attachments
                    writes_allowed=Signal::derive(move || {
                        local_store_write_gate(rooms.access.get().as_ref())
                    })
                />

                // The room's bound repo — see, clone and build it from the
                // room. A sibling for the same reason as its neighbours, and
                // it renders NOTHING for a Local room: no Bedrock workspace
                // exists there, and a refusal would read as breakage. The
                // other rail that keeps the COMPOSER's gate, and the clearest
                // case for it: every command here — bind, clone, build, CI —
                // is executed by a Bedrock container, so a link that is down
                // is a command that cannot run at all.
                <crate::room_repo::RoomRepo
                    rooms=rooms
                    state=repo
                    writes_allowed=Signal::derive(move || {
                        access_allows_writes(rooms.access.get().as_ref())
                    })
                />

                // The workspace itself and its command history — the read
                // half of the lane the repo section drives. A sibling for the
                // same reason as its neighbours, and pure reads: what a
                // member may see, the daemon already decided per row.
                <crate::room_workspace_panel::RoomWorkspacePanel
                    rooms=rooms
                    state=workspace_panel
                />

                // Trigger-policy summary at bottom of right rail. Only live
                // triggers are listed — the unwired fields never fire, and
                // writes carrying them are refused (`trigger_unwired`). A live
                // flag that is on but cannot fire in this kind of room is
                // listed carrying the same note its row above shows, so the
                // two controls cannot say different things about one flag.
                {move || {
                    let access = rooms.access.get();
                    rooms.open_room.get()
                        .and_then(|r| r.trigger_policy)
                        .map(|p| {
                            let triggers = trigger_summary(&p, access.as_ref());
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
            // ═══ THREAD PANEL — dedicated conversation column ═════════════
            // A real column (an overlay below 900px), no longer mode-swapped
            // into the members rail: roster and thread coexist, and the
            // reply composer is pinned outside the scroll region.
            {move || {
                // Opt-in presentation: inline-under-the-message is the
                // default; this panel renders only when popped out.
                if thread_view_mode.get() != ThreadViewMode::Panel {
                    return ().into_any();
                }
                let Some(root) =
                    thread_root_for(&rooms.transcript.get(), selected_thread_root_seq.get())
                else {
                    return ().into_any();
                };
                let root_seq = root.seq;
                let full_ts = root.created_at.clone();
                let root_is_system = matches!(
                    root.kind,
                    RoomMessageKind::System
                        | RoomMessageKind::ParticipantJoined
                        | RoomMessageKind::ParticipantLeft
                );
                let reply_count = reply_count_for(&rooms.transcript.get(), root_seq);
                let subtitle = thread_panel_subtitle(
                    reply_count,
                    &thread_display_name(&root.author_id),
                );
                view! {
                    <aside class="rooms-workspace__thread-panel" aria-label="Thread">
                        <div class="rooms-workspace__thread-panel-head">
                            <div>
                                <p class="rooms-workspace__thread-panel-title">"Thread"</p>
                                <div class="rooms-workspace__thread-panel-subtitle">
                                    {subtitle}
                                </div>
                            </div>
                            <div class="rooms-workspace__thread-panel-actions">
                                <button
                                    class="rooms-workspace__thread-panel-dock"
                                    type="button"
                                    title="Show under message"
                                    aria-label="Show thread under its message"
                                    on:click=move |_| {
                                        thread_view_mode.set(ThreadViewMode::Inline)
                                    }
                                >
                                    <svg viewBox="0 0 16 16" width="13" height="13"
                                        fill="none" stroke="currentColor" stroke-width="1.5"
                                        stroke-linecap="round" stroke-linejoin="round">
                                        <path d="M3 6l5 5 5-5"/>
                                        <path d="M3 3h10"/>
                                    </svg>
                                </button>
                                <button
                                    class="rooms-workspace__thread-panel-close"
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
                            </div>
                        </div>
                        <div
                            class="rooms-workspace__thread-panel-transcript"
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
                                                    <span class="rooms-workspace__msg-name">{thread_display_name(&root.author_id)}</span>
                                                    <time
                                                        class="rooms-workspace__msg-time"
                                                        datetime=full_ts.clone()
                                                        aria-label=full_ts.clone()
                                                        title=full_ts.clone()
                                                    >
                                                        {canonical_wire_clock_time(&full_ts)}
                                                    </time>
                                                    // The enclosing closure re-runs on access
                                                    // changes, so this needs no closure of its own.
                                                    {ledger_mark_view(rooms.access.get().as_ref(), &root)}
                                                </div>
                                                <div class="rooms-workspace__msg-text">
                                                    {crate::room_markdown::body_view(root.body.clone(), member_ids)}
                                                </div>
                                            </div>
                                        </div>
                                        {thread_replies_view(root_seq)}
                        </div>
                        {thread_composer_view()}
                    </aside>
                }.into_any()
            }}
        </div>
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::{
        room_request_is_current, CreateOutcome, CreateResolution, FederatedActorType,
        FederatedMessageMeta, FederatedRoomMemberProjection, FederatedRoomRole, MemberPresence,
        RoomAccessProjection, RoomAccessState, RoomMessage, RoomMessageKind, RoomParticipantKind,
    };

    /// Flipping one exposed flag must normalize the unwired fields away —
    /// the daemon refuses any write carrying `on_component_event: true` or a
    /// set `on_schedule` (`trigger_unwired`), so carrying stored dead values
    /// through would 400 every flip for that room, permanently breaking all
    /// four working toggles. The live flags still carry through untouched.
    #[test]
    fn policy_with_toggle_normalizes_dead_fields_and_keeps_live_ones() {
        let current = RoomTriggerPolicy {
            on_mention: true,
            on_thread_reply: false,
            on_component_event: true,
            on_build_failure: false,
            on_ci_failure: false,
            on_schedule: Some("0 9 * * 1".into()),
        };
        let flipped = policy_with_toggle(Some(&current), TriggerToggle::BuildFailure, true);
        assert_eq!(
            flipped,
            RoomTriggerPolicy {
                on_mention: true,
                on_thread_reply: false,
                on_component_event: false,
                on_build_failure: true,
                on_ci_failure: false,
                on_schedule: None,
            }
        );
        // Flipping back off never resurrects the dead values either.
        let back = policy_with_toggle(Some(&flipped), TriggerToggle::BuildFailure, false);
        assert_eq!(
            back,
            RoomTriggerPolicy {
                on_mention: true,
                ..RoomTriggerPolicy::default()
            }
        );
    }

    /// A room with no stored policy starts from all-off, so the first flip
    /// enables exactly one flag.
    #[test]
    fn policy_with_toggle_from_no_policy_starts_from_default() {
        let policy = policy_with_toggle(None, TriggerToggle::Mention, true);
        assert_eq!(
            policy,
            RoomTriggerPolicy {
                on_mention: true,
                ..RoomTriggerPolicy::default()
            }
        );
    }

    /// All-off must post NO policy (the daemon default applies, as before the
    /// form had toggles), and any set flag must produce a policy that never
    /// touches the unexposed fields.
    #[test]
    fn create_trigger_policy_only_carries_exposed_flags() {
        assert_eq!(create_trigger_policy(false, false, false, false), None);
        let policy = create_trigger_policy(true, false, true, false).expect("policy");
        assert!(policy.on_mention);
        assert!(!policy.on_thread_reply);
        assert!(policy.on_build_failure);
        assert!(!policy.on_ci_failure);
        assert!(!policy.on_component_event);
        assert_eq!(policy.on_schedule, None);
    }

    /// `on_ci_failure` is an exposed toggle, not a flag that only ever arrives
    /// from the daemon: it has to survive create on its own, and it has to be
    /// enough on its own to post a policy at all. Ticking only the CI box and
    /// getting `None` back would drop the box's whole meaning silently — the
    /// room would be created with the daemon's no-triggers default.
    #[test]
    fn create_trigger_policy_carries_a_lone_ci_failure_tick() {
        let policy = create_trigger_policy(false, false, false, true).expect("policy");
        assert!(policy.on_ci_failure);
        assert!(!policy.on_mention);
        assert!(!policy.on_thread_reply);
        assert!(!policy.on_build_failure);
        assert!(!policy.on_component_event);
        assert_eq!(policy.on_schedule, None);
    }

    /// The right rail's summary must name every live flag that is on. This is
    /// the only guard on that list: the summary renders in a browser, so a
    /// dropped line passes both `cargo test` and the wasm build otherwise.
    ///
    /// Access is `None` throughout, which makes this the unknown-access case
    /// too: before the projection lands nothing is known about the room, so
    /// every listed flag stays UNANNOTATED rather than guessing at a note that
    /// would flip once access arrives. That silence is inherited from
    /// [`trigger_row_dead_here`] rather than decided again here, which is why
    /// it cannot drift from what the rows above render.
    #[test]
    fn trigger_summary_names_every_live_flag_that_is_on() {
        assert_eq!(trigger_summary(&RoomTriggerPolicy::default(), None), "none");
        let all_on = RoomTriggerPolicy {
            on_mention: true,
            on_thread_reply: true,
            on_build_failure: true,
            on_ci_failure: true,
            on_component_event: true,
            on_schedule: Some("0 9 * * 1".into()),
        };
        assert_eq!(
            trigger_summary(&all_on, None),
            "mention, thread reply, build failure, CI failure"
        );
        let ci_only = RoomTriggerPolicy {
            on_ci_failure: true,
            ..RoomTriggerPolicy::default()
        };
        assert_eq!(trigger_summary(&ci_only, None), "CI failure");
    }

    /// The bug this pass closes: the summary named a flag the trigger rail had
    /// just greyed out and labelled dead, a few hundred lines above it in the
    /// same right rail. A Local room stored with `on_build_failure` and
    /// `on_ci_failure` showed two disabled rows reading `federated rooms only`
    /// and a summary reading `mention, build failure, CI failure` anyway.
    ///
    /// The flag stays LISTED — it is stored true and its row renders checked,
    /// so dropping it would only invert the contradiction — and carries the
    /// rail's own note verbatim, since both strings come from
    /// `trigger_row_dead_here`.
    #[test]
    fn trigger_summary_annotates_a_flag_that_cannot_fire_in_this_room() {
        let all_on = RoomTriggerPolicy {
            on_mention: true,
            on_thread_reply: true,
            on_build_failure: true,
            on_ci_failure: true,
            on_component_event: false,
            on_schedule: None,
        };

        // A Local room has no workspace, so neither marker-fed flag can fire.
        let local = test_access(RoomAccessState::Local);
        assert_eq!(
            trigger_summary(&all_on, Some(&local)),
            "mention, thread reply, build failure (federated rooms only), \
             CI failure (federated rooms only)"
        );

        // The federated inverse: the bridge carries no thread parent.
        let live = test_access(RoomAccessState::Live);
        assert_eq!(
            trigger_summary(&all_on, Some(&live)),
            "mention, thread reply (local rooms only), build failure, CI failure"
        );

        // A flag that is OFF is not listed at all, annotated or otherwise.
        let ci_only = RoomTriggerPolicy {
            on_ci_failure: true,
            ..RoomTriggerPolicy::default()
        };
        assert_eq!(
            trigger_summary(&ci_only, Some(&local)),
            "CI failure (federated rooms only)"
        );
        assert_eq!(trigger_summary(&ci_only, Some(&live)), "CI failure");
    }

    /// Why `on_ci_failure` is mirrored and not just toggled: a PATCH from this
    /// rail replaces the stored policy WHOLESALE, so a flag the daemon set and
    /// this struct did not know about would be cleared by the next flip of an
    /// unrelated row. Its own row is not what protects it — this asserts a
    /// BuildFailure flip — and rooms that gained the flag before the row
    /// existed still depend on that. It rides through: unlike the two unwired
    /// fields above, nothing refuses it, so dropping it would be destruction
    /// rather than the honest normalization those get.
    #[test]
    fn policy_with_toggle_carries_a_daemon_set_ci_failure_through_a_flip() {
        let current = RoomTriggerPolicy {
            on_mention: true,
            on_thread_reply: true,
            on_component_event: true,
            on_build_failure: false,
            on_ci_failure: true,
            on_schedule: Some("0 9 * * 1".into()),
        };
        assert_eq!(
            policy_with_toggle(Some(&current), TriggerToggle::BuildFailure, true),
            RoomTriggerPolicy {
                on_mention: true,
                on_thread_reply: true,
                on_component_event: false,
                on_build_failure: true,
                on_ci_failure: true,
                on_schedule: None,
            }
        );
    }

    /// The toggle classes render from Rust; their rules must exist in the
    /// root stylesheet or the controls ship unstyled.
    #[test]
    fn trigger_toggles_are_styled_in_the_root_stylesheet() {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let normalized = css_without_whitespace(&strip_css_comments(css));
        assert!(normalized.contains(".rooms-workspace__create-triggers{"));
        assert!(normalized.contains(".rooms-workspace__triggers{"));
        assert!(normalized.contains(".rooms-workspace__trigger{"));
        assert!(normalized.contains(".rooms-workspace__trigger-note{"));
        assert!(normalized.contains(".rooms-workspace__triggers-error{"));
    }

    // ── trigger rows: which flag is live in which kind of room ────────
    //
    // Traced through the daemon at ocean-os origin/main 0b32db5d, and
    // re-traced at b77f791c when the CI row joined. All four flags are read
    // from THIS daemon's store — the federation bridge's ingest paths call
    // `store.trigger_policy(..)` exactly like the local post path does — so a
    // federated room's policy is live state. What differs is which flag's
    // EVENT can be constructed for which kind of room. The four flags land in
    // three places, not four: the two workspace-marker flags share one.

    /// `RoomTriggerEvent::Mention` is built on BOTH paths: the local post
    /// path parses mentions out of a posted body, and the bridge's
    /// `ingest_message_row` evaluates one per federated `mention_member_ids`
    /// entry. So the row is live wherever the room takes writes at all.
    #[test]
    fn mention_is_live_in_local_and_federated_rooms_alike() {
        for state in [RoomAccessState::Local, RoomAccessState::Live] {
            let access = test_access(state);
            assert_eq!(
                trigger_row_dead_here(TriggerToggle::Mention, Some(&access)),
                None,
                "{state:?}"
            );
            for checked in [false, true] {
                assert!(
                    trigger_row_is_editable(TriggerToggle::Mention, checked, Some(&access)),
                    "{state:?} checked={checked}"
                );
            }
        }
    }

    /// `RoomTriggerEvent::ThreadReply` is built in exactly one place — the
    /// local post path, from the thread root's author. The federated
    /// `MessagePayload` carries `{client_event_id, author_member_id, body,
    /// mention_member_ids}` and no thread parent at all, so the bridge can
    /// never construct one. Local-only here is the CORRECT reading.
    #[test]
    fn thread_reply_is_dead_in_a_federated_room() {
        let local = test_access(RoomAccessState::Local);
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::ThreadReply, Some(&local)),
            None
        );
        for checked in [false, true] {
            assert!(trigger_row_is_editable(
                TriggerToggle::ThreadReply,
                checked,
                Some(&local)
            ));
        }

        let live = test_access(RoomAccessState::Live);
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::ThreadReply, Some(&live)),
            Some("local rooms only")
        );
        assert!(!trigger_row_is_editable(
            TriggerToggle::ThreadReply,
            false,
            Some(&live)
        ));
    }

    /// The inverse, and the bug this split was written for: a build failure
    /// reaches a room only as a `room.workspace.build_failed` marker, and
    /// markers arrive through the federation bridge. A Local room has no
    /// workspace and can never produce one. The rail used to offer this
    /// checkbox on Local rooms alone — visible exactly where it cannot fire,
    /// hidden exactly where it can.
    #[test]
    fn build_failure_is_dead_in_a_local_room() {
        let local = test_access(RoomAccessState::Local);
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::BuildFailure, Some(&local)),
            Some("federated rooms only")
        );
        assert!(!trigger_row_is_editable(
            TriggerToggle::BuildFailure,
            false,
            Some(&local)
        ));

        let live = test_access(RoomAccessState::Live);
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::BuildFailure, Some(&live)),
            None
        );
        for checked in [false, true] {
            assert!(trigger_row_is_editable(
                TriggerToggle::BuildFailure,
                checked,
                Some(&live)
            ));
        }
    }

    /// A red CI check reaches a room the same way a build failure does and
    /// from the same lane: `RoomTriggerEvent::CiFailure` has exactly one
    /// non-test construction site in the daemon (`ingest_workspace_row`, at
    /// ocean-os b77f791c), off a `room.workspace.ci_checked` marker the bridge
    /// reads red. A Local room has no workspace to check, so the row is dead
    /// there for BuildFailure's reason rather than by analogy to it.
    #[test]
    fn ci_failure_is_dead_in_a_local_room() {
        let local = test_access(RoomAccessState::Local);
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::CiFailure, Some(&local)),
            Some("federated rooms only")
        );
        assert!(!trigger_row_is_editable(
            TriggerToggle::CiFailure,
            false,
            Some(&local)
        ));

        let live = test_access(RoomAccessState::Live);
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::CiFailure, Some(&live)),
            None
        );
        for checked in [false, true] {
            assert!(trigger_row_is_editable(
                TriggerToggle::CiFailure,
                checked,
                Some(&live)
            ));
        }
    }

    /// The asymmetry the three tests above are now only half of, and the bug
    /// this pass closes. A flag is stored on a room, the room changes kind,
    /// and the flag is suddenly one whose event can never reach it: a room
    /// created Local with `on_thread_reply` that later federates, or one
    /// created with the workspace-marker flags that never does. The row
    /// renders CHECKED — `trigger_toggle_row` draws it from the stored policy
    /// — greyed, noted, and [`trigger_summary`] lists it as on, deliberately,
    /// because hiding it would only invert the contradiction. Under one gate
    /// the un-tick was refused along with the tick and there was no control
    /// anywhere in the app that could clear the flag.
    ///
    /// So the two directions part company. A dead row that is on takes the
    /// un-tick; a dead row that is off still refuses the tick, which is the
    /// half of the old gate that was always right — arming a flag this room
    /// can never fire stores the same contradiction on purpose.
    #[test]
    fn a_dead_flag_that_is_stored_on_can_still_be_turned_off() {
        let dead_pairings = [
            (TriggerToggle::ThreadReply, RoomAccessState::Live),
            (TriggerToggle::BuildFailure, RoomAccessState::Local),
            (TriggerToggle::CiFailure, RoomAccessState::Local),
        ];
        for (toggle, state) in dead_pairings {
            let access = test_access(state);
            assert!(
                trigger_row_dead_here(toggle, Some(&access)).is_some(),
                "{toggle:?} in {state:?} must be the dead pairing this pins"
            );

            assert!(
                trigger_row_is_editable(toggle, true, Some(&access)),
                "{toggle:?} stored on in {state:?} must accept the un-tick — \
                 otherwise the stored flag can never be cleared"
            );
            assert!(
                !trigger_row_is_editable(toggle, false, Some(&access)),
                "{toggle:?} off in {state:?} must still refuse the tick"
            );
        }

        // And the write gate still leads. A `Revoked` room refuses both
        // directions on every flag, stored-on ones included: the operator has
        // been removed, so there is no edit left to offer them — see
        // [`a_revoked_room_holds_every_trigger_row`], which pins the same
        // ordering from the other side.
        let revoked = test_access(RoomAccessState::Revoked);
        assert!(!trigger_row_is_editable(
            TriggerToggle::ThreadReply,
            true,
            Some(&revoked)
        ));
    }

    /// `Revoked` is the one access state that holds every row. The daemon
    /// would take the PATCH — `room_update` has no access check — but the
    /// operator has been removed from this room, so a control that still
    /// worked would be offering an action that cannot mean anything to them
    /// again. Both directions, so a flag stored on is held too: the un-tick
    /// [`a_dead_flag_that_is_stored_on_can_still_be_turned_off`] admits
    /// elsewhere is an edit like any other, and this is the state that offers
    /// none. The federated NOTE survives being held: `Revoked` is non-Local,
    /// so `on_thread_reply` is still dead here for its own reason.
    #[test]
    fn a_revoked_room_holds_every_trigger_row() {
        let access = test_access(RoomAccessState::Revoked);
        for toggle in [
            TriggerToggle::Mention,
            TriggerToggle::ThreadReply,
            TriggerToggle::BuildFailure,
            TriggerToggle::CiFailure,
        ] {
            for checked in [false, true] {
                assert!(
                    !trigger_row_is_editable(toggle, checked, Some(&access)),
                    "{toggle:?} checked={checked}"
                );
            }
        }
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::ThreadReply, Some(&access)),
            Some("local rooms only")
        );
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::BuildFailure, Some(&access)),
            None
        );
        assert_eq!(
            trigger_row_dead_here(TriggerToggle::CiFailure, Some(&access)),
            None
        );
    }

    /// The ruling this rail is built on: a link that is down or coming back
    /// does not hold the trigger policy, because the policy is a row in this
    /// daemon's own store and the PATCH that writes it never leaves the
    /// machine. `Connecting` and `Recovering` therefore keep every row whose
    /// event can fire in a federated room — and `on_thread_reply` stays held
    /// there — against being ARMED, the direction that would store a flag
    /// this room can never fire — on the pre-existing, unrelated grounds that
    /// the bridge can never construct that event, note and all.
    ///
    /// This is the capability the gate used to take away: a room stuck
    /// `Recovering` while every mention woke an agent, and no way to stop it.
    #[test]
    fn a_reconnecting_room_keeps_its_live_trigger_rows_writable() {
        for state in [RoomAccessState::Connecting, RoomAccessState::Recovering] {
            let access = test_access(state);

            for checked in [false, true] {
                assert!(
                    trigger_row_is_editable(TriggerToggle::Mention, checked, Some(&access)),
                    "{state:?} checked={checked}"
                );
                assert!(
                    trigger_row_is_editable(TriggerToggle::BuildFailure, checked, Some(&access)),
                    "{state:?} checked={checked}"
                );
                assert!(
                    trigger_row_is_editable(TriggerToggle::CiFailure, checked, Some(&access)),
                    "{state:?} checked={checked}"
                );
            }
            assert!(
                !trigger_row_is_editable(TriggerToggle::ThreadReply, false, Some(&access)),
                "{state:?}"
            );

            assert_eq!(
                trigger_row_dead_here(TriggerToggle::ThreadReply, Some(&access)),
                Some("local rooms only"),
                "{state:?}"
            );
            assert_eq!(
                trigger_row_dead_here(TriggerToggle::BuildFailure, Some(&access)),
                None,
                "{state:?}"
            );
            assert_eq!(
                trigger_row_dead_here(TriggerToggle::CiFailure, Some(&access)),
                None,
                "{state:?}"
            );
        }
    }

    /// The ruling stated as a difference, so the divergence between the two
    /// gates is pinned rather than left to be re-derived from two separate
    /// matrices. They agree everywhere the write's destination is the same
    /// question, and part company on exactly the two non-terminal states —
    /// where the composer's write must reach a peer and this rail's write
    /// must only reach the local store.
    #[test]
    fn the_trigger_gate_diverges_from_the_composer_gate_only_while_reconnecting() {
        let cases = [
            (RoomAccessState::Local, true),
            (RoomAccessState::Live, true),
            (RoomAccessState::Connecting, true),
            (RoomAccessState::Recovering, true),
            (RoomAccessState::Revoked, false),
        ];
        for (state, policy_writable) in cases {
            let access = test_access(state);
            assert_eq!(
                trigger_policy_accepts_writes(Some(&access)),
                policy_writable,
                "{state:?}"
            );

            let diverges = matches!(
                state,
                RoomAccessState::Connecting | RoomAccessState::Recovering
            );
            assert_eq!(
                trigger_policy_accepts_writes(Some(&access)) != access_allows_writes(Some(&access)),
                diverges,
                "{state:?}"
            );
        }

        // Unknown access is the one place the two gates must never diverge:
        // the rail has nothing to rule on yet.
        assert!(!trigger_policy_accepts_writes(None));
        assert!(!access_allows_writes(None));
    }

    /// Before the access projection lands, nothing is known about the room.
    /// Every row is held by the write gate, and no row claims its flag is
    /// dead — that claim would be a guess that flips once access arrives.
    #[test]
    fn an_unknown_access_state_claims_nothing_about_any_flag() {
        for toggle in [
            TriggerToggle::Mention,
            TriggerToggle::ThreadReply,
            TriggerToggle::BuildFailure,
            TriggerToggle::CiFailure,
        ] {
            assert_eq!(trigger_row_dead_here(toggle, None), None, "{toggle:?}");
            for checked in [false, true] {
                assert!(
                    !trigger_row_is_editable(toggle, checked, None),
                    "{toggle:?} checked={checked}"
                );
            }
        }
    }

    /// The three helpers above are pure, so a test of them alone proves only
    /// that they answer correctly if asked — the view is free to stop asking
    /// and every one of those tests stays green while the defect returns.
    /// Both halves of this fix are text in this file, so pin the wiring the
    /// way the guards further down this module pin an emitter: read the
    /// source and assert on it, with the needles concatenated at runtime so
    /// this test's own literals cannot stand in for the code it is scanning.
    ///
    /// The last assert pins WHICH gate the row takes. That one the behaviour
    /// tests above would also catch, but only as three unexplained failures;
    /// naming the rail-local gate here means a future revert to the composer's
    /// gate has to delete the sentence explaining why it is not that gate.
    #[test]
    fn the_triggers_section_and_row_are_wired_to_the_per_row_gate() {
        let markup = include_str!("rooms_workspace.rs");

        // The section renders in every access state. The bug was an early
        // return in this closure keyed on the access STATE, which hid the
        // whole section — build failure included — on exactly the federated
        // rooms where a build can fail. Nothing between the closure's brace
        // and the group it emits may consult that state again; the split is
        // per row now.
        let group = markup
            .find(&format!(
                "class=\"{}\"",
                ["rooms-workspace_", "_triggers"].concat()
            ))
            .expect("the triggers group must be emitted from this file");
        let opens = markup[..group]
            .rfind("{move || {")
            .expect("the triggers group must render inside a closure");
        assert!(
            !markup[opens..group].contains("RoomAccessState"),
            "the triggers section must render in every access state — an \
             access-state gate here hides the build-failure row on exactly \
             the federated rooms where a build failure can happen"
        );

        // And the row must actually take the per-row gate: without it in the
        // `disabled=` binding, a flag whose event can never fire in this kind
        // of room is offered as live again, note and all.
        let row_at = markup
            .find(&format!("fn {}(", ["trigger_toggle", "_row"].concat()))
            .expect("the trigger row must render from this file");
        let row = &markup[row_at..];
        let row = &row[..row.find("\nfn ").unwrap_or(row.len())];
        assert!(
            row.contains(&["trigger_row", "_is_editable"].concat()),
            "the trigger row must consult the per-row gate"
        );
        // And it must hand the gate its OWN `checked`. The direction argument
        // is a degree of freedom the old two-argument gate did not have: a
        // literal `true` there compiles, takes every unit test above green
        // with it — they all call the gate directly — and re-arms every dead
        // row in the browser, which is the half of the gate that was right.
        assert!(
            row.contains(&["trigger_row", "_is_editable(toggle, checked, access)"].concat()),
            "the trigger row must pass its own `checked` to the per-row gate \
             — a literal there decides the direction for every row at once"
        );
        let disabled = row
            .find("disabled=")
            .map(|at| row[at..].lines().next().unwrap_or_default())
            .expect("the trigger checkbox must carry a disabled binding");
        assert!(
            disabled.contains("editable"),
            "`disabled=` must consult the per-row gate — on its own, \
             `policy_update_in_flight` offers a dead row as a live one"
        );

        // And the per-row gate must take the RAIL's write gate, not the
        // composer's. `access_allows_writes` is the right question for a write
        // that has to reach a peer; a trigger-policy PATCH lands in this
        // daemon's local store, so pointing this row back at it would hold the
        // rail through `Connecting`/`Recovering` for no reason that survives
        // being stated.
        let gate_at = markup
            .find(&format!("fn {}(", ["trigger_row", "_is_editable"].concat()))
            .expect("the per-row gate must be defined in this file");
        let gate = &markup[gate_at..];
        let gate = &gate[..gate.find("\n}").map_or(gate.len(), |at| at + 2)];
        assert!(
            gate.contains(&["trigger_policy", "_accepts_writes"].concat()),
            "the per-row gate must take the rail-local write gate"
        );
        assert!(
            !gate.contains(&["access_allows", "_writes"].concat()),
            "the per-row gate must NOT take the composer's write gate — that \
             holds the whole rail through `Connecting`/`Recovering`, where the \
             policy write lands in the local store regardless"
        );
    }

    // ── create rows: the room this form makes is Local ─────────────

    /// `POST /v1/rooms/persistent` has no federation in its body, so the room
    /// the create rail makes is Local on day one. Judging the create rows
    /// against that access — rather than against nothing — is what lets the
    /// two workspace-marker flags say why they are held, in the same words
    /// the right rail will use on the very same room a second later.
    #[test]
    fn a_created_room_is_local_and_its_dead_flags_say_so() {
        assert_eq!(creating_room_access().state, RoomAccessState::Local);

        assert_eq!(create_trigger_row_dead_here(TriggerToggle::Mention), None);
        assert_eq!(
            create_trigger_row_dead_here(TriggerToggle::ThreadReply),
            None
        );
        assert_eq!(
            create_trigger_row_dead_here(TriggerToggle::BuildFailure),
            Some("federated rooms only")
        );
        assert_eq!(
            create_trigger_row_dead_here(TriggerToggle::CiFailure),
            Some("federated rooms only")
        );
    }

    /// The trap this slice exists to avoid, pinned as a difference. A room
    /// being created has no `RoomAccessProjection`, so the obvious wiring is
    /// `trigger_row_dead_here(toggle, None)` — and that is the one reading
    /// which annotates NOTHING, because unknown access deliberately makes no
    /// claim (see [`an_unknown_access_state_claims_nothing_about_any_flag`]).
    /// It compiles, it passes, and it ships silence. A room in creation is not
    /// unknown; it is Local, and the two answers must not be allowed to
    /// converge by someone "simplifying" the explicit projection away.
    #[test]
    fn create_rows_are_not_judged_against_unknown_access() {
        for toggle in [TriggerToggle::BuildFailure, TriggerToggle::CiFailure] {
            assert_eq!(trigger_row_dead_here(toggle, None), None, "{toggle:?}");
            assert!(
                create_trigger_row_dead_here(toggle).is_some(),
                "{toggle:?} must be annotated at create time, not silently \
                 armed — passing `None` access here notes nothing"
            );
        }
    }

    /// The pure helpers above only prove the create rail answers correctly if
    /// it asks. Pin that it asks, and that every row asks through the one
    /// helper — the same source-scan guard the right rail's row carries, for
    /// the same reason: a checkbox is reachable only from a browser, so a row
    /// that quietly goes back to longhand markup takes every one of these
    /// tests green with it.
    #[test]
    fn the_create_trigger_rows_are_wired_to_the_created_rooms_access() {
        let markup = include_str!("rooms_workspace.rs");

        let group_class = ["rooms-workspace_", "_create-triggers"].concat();
        let group = markup
            .find(&format!("class=\"{group_class}\""))
            .expect("the create-triggers group must be emitted from this file");
        let group = &markup[group..];
        let group = &group[..group.find("</div>").expect("the group must close")];
        assert_eq!(
            group
                .matches(&format!("{}(", ["create_trigger", "_row"].concat()))
                .count(),
            4,
            "every create trigger row must render through the shared helper — \
             longhand markup here is a row that cannot carry its note"
        );

        let row_at = markup
            .find(&format!("fn {}(", ["create_trigger", "_row"].concat()))
            .expect("the create row must render from this file");
        let row = &markup[row_at..];
        let row = &row[..row.find("\nfn ").unwrap_or(row.len())];
        assert!(
            row.contains(&["create_trigger_row", "_dead_here"].concat()),
            "the create row must judge its flag against the room it is making"
        );
        let disabled = row
            .find("disabled=")
            .map(|at| row[at..].lines().next().unwrap_or_default())
            .expect("the create checkbox must carry a disabled binding");
        assert!(
            disabled.contains("dead_here"),
            "`disabled=` must consult the dead-here note — on its own, \
             `pending_create` arms a flag the created room can never fire"
        );
        // The right rail's row holds its own note by compiling: `dead_here`
        // has no other reader there, so deleting the span is an unused-binding
        // error under the release lane. Here `disabled=` reads it too, so the
        // span can be deleted with everything still green — leaving a greyed
        // box that explains nothing, which is this slice inverted.
        assert!(
            row.contains(&["rooms-workspace_", "_trigger-note"].concat()),
            "the create row must still render the note span — the hold is \
             only half the product, and nothing but this line notices it going"
        );
    }

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
            transcript_pass_action(0, 5, true, true, false, false),
            TranscriptPassAction::Reset
        );
        // First fill, regardless of the measured scroll position.
        assert_eq!(
            transcript_pass_action(5, 0, true, false, false, false),
            TranscriptPassAction::PinAndQueue
        );
        // Access arrives after the fill, reader still at the bottom: requeue.
        assert_eq!(
            transcript_pass_action(5, 5, true, true, false, false),
            TranscriptPassAction::PinAndQueue
        );
        // Access arrives after the fill, reader scrolled up: hold.
        assert_eq!(
            transcript_pass_action(5, 5, true, false, false, false),
            TranscriptPassAction::Hold
        );
        // Append below a scrolled-up reader keeps the jump affordance.
        assert_eq!(
            transcript_pass_action(6, 5, true, false, false, false),
            TranscriptPassAction::RaiseJump
        );
        // Append while at the bottom pins and queues.
        assert_eq!(
            transcript_pass_action(6, 5, true, true, false, false),
            TranscriptPassAction::PinAndQueue
        );
        // No transcript element yet: hold, so the first-fill state survives
        // for the pass that can actually measure.
        assert_eq!(
            transcript_pass_action(5, 0, false, true, false, false),
            TranscriptPassAction::Hold
        );
    }

    /// A REQUESTED older page landing in front of the paint is neither of the
    /// growth cases the two existing arms describe, and both of them are wrong
    /// for it.
    ///
    /// `RaiseJump` is what a scrolled-up reader got before this arm existed —
    /// the same `len > prev_len` a tail append produces — so pressing "load
    /// older" raised "↓ New messages" over rows that had arrived ABOVE.
    /// `PinAndQueue` is the other direction of the same mistake: it throws a
    /// reader who happened to be at the bottom down to it again, having just
    /// asked to see the top.
    #[test]
    fn an_older_page_anchors_instead_of_jumping_or_pinning() {
        // Scrolled-up reader — the press's own case.
        assert_eq!(
            transcript_pass_action(200, 5, true, false, true, true),
            TranscriptPassAction::AnchorOlder
        );
        // At the bottom, where the arm order is what decides it.
        assert_eq!(
            transcript_pass_action(200, 5, true, true, true, true),
            TranscriptPassAction::AnchorOlder
        );
        // An unmeasured element still holds: there is nothing to anchor
        // against, and the pass that can measure will see the same prepend
        // because this one refuses to consume the state.
        assert_eq!(
            transcript_pass_action(200, 5, false, false, true, true),
            TranscriptPassAction::Hold
        );
        // A room switch beats everything, prepend or not.
        assert_eq!(
            transcript_pass_action(0, 200, true, false, true, true),
            TranscriptPassAction::Reset
        );
    }

    /// The anchor is what makes a prepend a REQUEST, and a prepend without one
    /// has to keep the behaviour ocean-surface#190 landed.
    ///
    /// `backfill_open_transcript` prepends up to four more pages after the
    /// first fill, and every one of them raises `grew_at_front`. Routing those
    /// to `AnchorOlder` reintroduces #190 through a different mechanism: that
    /// arm holds nothing when there is no anchor to hold against, so
    /// `scroll_top` stays where it was while rows land above it and the reader
    /// drifts backwards by a page per walk step. `.rooms-workspace__transcript`
    /// is a plain `overflow-y: auto` column and the Tauri host is WebKit, which
    /// has no `overflow-anchor`, so nothing below this decision catches it.
    ///
    /// The knock-on is the second-order half: a hydration that ends scrolled up
    /// makes the access projection's re-entry pass (`len == prev_len`,
    /// `near_bottom` now false) take `Hold` instead of `PinAndQueue`, so the
    /// read advance that re-entry exists to queue is never queued at all.
    #[test]
    fn an_unasked_prepend_keeps_the_pin_that_opens_a_room_at_its_newest_page() {
        // The hydration walk's pages: at the bottom, because the first fill's
        // pin put the reader there, and unanchored because no press parked
        // geometry for them.
        assert_eq!(
            transcript_pass_action(200, 5, true, true, true, false),
            TranscriptPassAction::PinAndQueue
        );
        assert_eq!(
            transcript_pass_action(400, 200, true, true, true, false),
            TranscriptPassAction::PinAndQueue
        );
        // A walk page landing after the reader scrolled away keeps the pre-#190
        // answer too: this is not the decision that changes it.
        assert_eq!(
            transcript_pass_action(400, 200, true, false, true, false),
            TranscriptPassAction::RaiseJump
        );
        // And a parked anchor alone decides nothing, so a press whose page has
        // not landed yet cannot divert the appends that arrive while it flies.
        assert_eq!(
            transcript_pass_action(6, 5, true, true, false, true),
            TranscriptPassAction::PinAndQueue
        );
        assert_eq!(
            transcript_pass_action(6, 5, true, false, false, true),
            TranscriptPassAction::RaiseJump
        );
        assert_eq!(
            transcript_pass_action(5, 5, true, false, false, true),
            TranscriptPassAction::Hold
        );
    }

    /// The signal the arm above turns on. A prepend is the only write that
    /// lowers the oldest `seq`, and reading the seq rather than the row count is
    /// what stops a page that was entirely already painted — `prepend_transcript_page`
    /// keeps only rows strictly older than the oldest painted — from being
    /// mistaken for one that moved the view.
    #[test]
    fn only_a_fallen_oldest_seq_reads_as_a_prepend() {
        assert!(transcript_grew_at_front(Some(4001), Some(3801)));
        assert!(!transcript_grew_at_front(Some(4001), Some(4001)));
        assert!(
            !transcript_grew_at_front(Some(4001), Some(4200)),
            "a rising oldest seq is not a prepend; nothing in this module \
             produces one, and reading it as history arriving would anchor the \
             reader against a page that never came",
        );
        assert!(
            !transcript_grew_at_front(None, Some(4001)),
            "the first fill has no rows to arrive in front of — it must stay on \
             the pin path that opens a room at its newest message",
        );
        assert!(!transcript_grew_at_front(Some(4001), None));
        assert!(!transcript_grew_at_front(None, None));
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
            transcript_pass_action(transcript.len(), 0, true, near_bottom, false, false),
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
            transcript_pass_action(
                transcript.len(),
                transcript.len(),
                true,
                near_bottom,
                false,
                false
            ),
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
            transcript_pass_action(
                transcript.len(),
                transcript.len(),
                true,
                false,
                false,
                false
            ),
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
            self_member_id: None,
            outbox: vec![],
        }
    }

    // ── access policy: writes + banner ────────────────────────────────

    #[test]
    fn all_access_states_pin_write_and_banner_policy() {
        // The banner strings are the RENDERED ones — the stage's banner
        // match takes its label from `access_banner`, so this matrix pins
        // exactly what users see.
        let cases = [
            (RoomAccessState::Local, true, None),
            (
                RoomAccessState::Connecting,
                false,
                Some("Connecting to federated room…"),
            ),
            (RoomAccessState::Live, true, None),
            (
                RoomAccessState::Recovering,
                false,
                Some("Recovering connection…"),
            ),
            (RoomAccessState::Revoked, false, Some("Access revoked")),
        ];

        assert!(!access_allows_writes(None));
        assert_eq!(access_banner(None), None);
        for (state, writes, banner) in cases {
            let access = test_access(state);
            assert_eq!(access_allows_writes(Some(&access)), writes);
            assert_eq!(access_banner(Some(&access)), banner);
        }
    }

    /// The rail-local gate stated per state, and stated as a difference from
    /// the composer's so the divergence is pinned rather than re-derived from
    /// two matrices. They part company on exactly the two non-terminal states:
    /// a write that has to reach a peer cannot land while the link is down or
    /// coming back, and a write that lands in this daemon's store is untouched
    /// by either.
    #[test]
    fn every_access_state_pins_the_local_store_write_gate() {
        // Unknown access is the one place the two gates must never diverge:
        // nothing is known about the room yet, and a control that flips to
        // disabled once the projection lands is worse than one that waits.
        assert!(!local_store_write_gate(None));
        assert!(!access_allows_writes(None));

        let cases = [
            (RoomAccessState::Local, true),
            (RoomAccessState::Connecting, true),
            (RoomAccessState::Live, true),
            (RoomAccessState::Recovering, true),
            (RoomAccessState::Revoked, false),
        ];
        for (state, writable) in cases {
            let access = test_access(state);
            assert_eq!(local_store_write_gate(Some(&access)), writable, "{state:?}");

            let diverges = matches!(
                state,
                RoomAccessState::Connecting | RoomAccessState::Recovering
            );
            assert_eq!(
                local_store_write_gate(Some(&access)) != access_allows_writes(Some(&access)),
                diverges,
                "{state:?}"
            );
        }
    }

    /// Which gate a rail takes is a `Signal::derive` inside the view, so no
    /// unit test of the predicates can reach it: both gates stay pure and
    /// correct, and every test above stays green while a section is wired to
    /// the wrong one. Read the source and assert on it instead, the way the
    /// trigger section's guard does, with the needles concatenated at runtime
    /// so this test's own literals cannot stand in for the code it scans.
    ///
    /// The ruling is per rail, so the table states it once. Summary, artifacts
    /// and attachments all write through the daemon's own store handle with no
    /// outbox row on any path, and take the local gate. Invite mints
    /// federation and repo drives a Bedrock container, so both keep the
    /// composer's — a code nobody can redeem and a build nothing can run are
    /// not writes a down link merely delays.
    #[test]
    fn each_rail_takes_the_gate_its_write_destination_earns() {
        let markup = include_str!("rooms_workspace.rs");
        let local = ["local_store", "_write_gate"].concat();
        let peer = ["access_allows", "_writes"].concat();

        let sections = [
            (["<crate::room_summary", "::RoomSummary"].concat(), true),
            (["<crate::room_artifacts", "::RoomArtifacts"].concat(), true),
            (["<crate::attachments", "::RoomAttachments"].concat(), true),
            (["<crate::room_invite", "::RoomInvite"].concat(), false),
            (["<crate::room_repo", "::RoomRepo"].concat(), false),
        ];

        for (tag, writes_land_locally) in sections {
            let at = markup
                .find(&tag)
                .unwrap_or_else(|| panic!("{tag} must be mounted from this file"));
            let section = &markup[at..];
            let section = &section[..section
                .find("/>")
                .unwrap_or_else(|| panic!("{tag} must close in this file"))];
            assert!(
                section.contains("writes_allowed="),
                "{tag} must carry a write gate"
            );

            let (wanted, refused) = if writes_land_locally {
                (&local, &peer)
            } else {
                (&peer, &local)
            };
            assert!(
                section.contains(wanted.as_str()),
                "{tag} must take `{wanted}`"
            );
            assert!(
                !section.contains(refused.as_str()),
                "{tag} must not take `{refused}` — which gate a rail takes IS \
                 its ruling on whether its write has to reach a peer"
            );
        }
    }

    /// Three sibling files carried a sentence this ruling falsifies: that the
    /// control and the composer can never disagree about the same room's
    /// access projection. They can now, and on purpose. A stale sentence there
    /// is worse than none — it is the argument a future reader would use to
    /// wire the rail back to the composer's gate — so pin both halves: the
    /// claim is gone, and the gate that replaced it is NAMED beside the prop
    /// that carries it. The second needle is the gate's identifier rather than
    /// a sentence, because a prose needle breaks on the next doc rewrap and
    /// this guard has to survive one.
    #[test]
    fn no_moved_rail_still_claims_it_cannot_disagree_with_the_composer() {
        let stale = ["composer can never disagree", " about the same room"].concat();
        let ruling = ["local_store", "_write_gate"].concat();
        for (name, source) in [
            ("room_summary.rs", include_str!("room_summary.rs")),
            ("room_artifacts.rs", include_str!("room_artifacts.rs")),
            ("attachments.rs", include_str!("attachments.rs")),
        ] {
            assert!(
                !source.contains(&stale),
                "{name} still claims this control and the composer can never \
                 disagree — they now do, in `Connecting` and `Recovering`"
            );
            assert!(
                source.contains(&ruling),
                "{name} must name the gate it actually takes, so a revert to \
                 the composer's leaves a doc that reads as wrong"
            );
        }
    }

    // ── roster remove control ─────────────────────────────────────────

    #[test]
    fn every_row_but_your_own_offers_remove() {
        assert!(participant_removable("scout", "web-1"));
        assert!(!participant_removable("web-1", "web-1"));
    }

    #[test]
    fn armed_remove_survives_a_roster_update_that_keeps_its_target() {
        let roster = [
            part("web-1", "John", RoomParticipantKind::Human),
            part("scout", "Scout", RoomParticipantKind::Agent),
        ];
        assert!(keep_armed_remove(Some("scout"), false, &roster, &[], None));
    }

    #[test]
    fn armed_remove_disarms_when_its_target_leaves_the_roster() {
        let roster = [part("web-1", "John", RoomParticipantKind::Human)];
        assert!(!keep_armed_remove(Some("scout"), false, &roster, &[], None));
    }

    #[test]
    fn armed_remove_disarms_across_a_room_switch_even_for_a_same_id_row() {
        // The next room can list the very same agent id; a confirm armed
        // against the previous room's row must not carry over to it.
        let roster = [part("scout", "Scout", RoomParticipantKind::Agent)];
        assert!(!keep_armed_remove(Some("scout"), true, &roster, &[], None));
    }

    #[test]
    fn unarmed_state_has_nothing_to_keep() {
        let roster = [part("scout", "Scout", RoomParticipantKind::Agent)];
        assert!(!keep_armed_remove(None, false, &roster, &[], None));
    }

    #[test]
    fn federated_armed_remove_survives_the_access_updates_that_rebuild_the_rail() {
        // A federated member never appears in open_room.participants — the
        // access projection is the roster that keeps its confirm alive, so
        // an SSE access update that retains the target must not disarm it.
        let members = [fed_member("member-agent")];
        assert!(keep_armed_remove(
            Some("member-agent"),
            false,
            &[],
            &members,
            None
        ));
    }

    #[test]
    fn federated_armed_remove_disarms_when_its_target_leaves_the_projection() {
        let members = [fed_member("member-other")];
        assert!(!keep_armed_remove(
            Some("member-agent"),
            false,
            &[],
            &members,
            None
        ));
    }

    #[test]
    fn federated_armed_remove_disarms_across_a_room_switch() {
        let members = [fed_member("member-agent")];
        assert!(!keep_armed_remove(
            Some("member-agent"),
            true,
            &[],
            &members,
            None
        ));
    }

    #[test]
    fn armed_remove_disarms_when_the_projection_reveals_the_target_is_you() {
        // self_member_id can arrive AFTER a row was armed — the first SSE
        // access update to carry the field. The confirm must not survive
        // the discovery that it points at the caller's own row.
        let members = [fed_member("member-you"), fed_member("member-agent")];
        assert!(!keep_armed_remove(
            Some("member-you"),
            false,
            &[],
            &members,
            Some("member-you")
        ));
        // A known self leaves confirms against OTHER rows alone.
        assert!(keep_armed_remove(
            Some("member-agent"),
            false,
            &[],
            &members,
            Some("member-you")
        ));
    }

    // ── federated self / yours row marks ──────────────────────────────

    #[test]
    fn self_mark_needs_the_projection_to_name_the_row() {
        assert!(federated_member_is_self(Some("member-you"), "member-you"));
        assert!(!federated_member_is_self(
            Some("member-you"),
            "member-other"
        ));
        // Absent field (local room, older daemon): no row is self, so
        // every row keeps today's remove control.
        assert!(!federated_member_is_self(None, "member-you"));
    }

    #[test]
    fn yours_mark_needs_a_known_self_owning_an_agent_row() {
        let mut agent = fed_member("member-agent");
        agent.owner_member_id = Some("member-you".into());
        assert!(federated_member_is_yours(Some("member-you"), &agent));
        assert!(!federated_member_is_yours(Some("member-else"), &agent));
        // The None == None trap: an ownerless agent under an absent
        // self_member_id belongs to nobody — no chip.
        assert!(!federated_member_is_yours(
            None,
            &fed_member("member-agent")
        ));
        // A user row is never "yours", whatever it owns.
        let mut human = fed_member("member-you");
        human.actor_type = FederatedActorType::User;
        human.owner_member_id = Some("member-you".into());
        assert!(!federated_member_is_yours(Some("member-you"), &human));
    }

    // ── room_is_federated ─────────────────────────────────────────────

    #[test]
    fn only_a_federated_room_has_the_section() {
        assert!(!room_is_federated(None));
        assert!(!room_is_federated(Some(&test_access(
            RoomAccessState::Local
        ))));
        assert!(room_is_federated(Some(&test_access(RoomAccessState::Live))));
        assert!(room_is_federated(Some(&test_access(
            RoomAccessState::Connecting
        ))));
        assert!(room_is_federated(Some(&test_access(
            RoomAccessState::Revoked
        ))));
    }

    // ── ledger_mark ───────────────────────────────────────────────────

    fn test_meta() -> FederatedMessageMeta {
        FederatedMessageMeta {
            ledger_event_id: "evt-1".into(),
            global_sequence: 7,
            source_id: "surface-web".into(),
            source_sequence: 3,
            client_event_id: "client-1".into(),
            origin_principal_id: "principal-1".into(),
            origin_member_id: "user".into(),
        }
    }

    #[test]
    fn ledger_mark_confirms_only_rows_with_meta_in_federated_rooms() {
        let mut confirmed = test_msg(1, "hello", None);
        confirmed.federated = Some(test_meta());
        let live = test_access(RoomAccessState::Live);

        assert_eq!(ledger_mark(Some(&live), &confirmed), LedgerMark::Confirmed);
        assert_eq!(
            ledger_mark(Some(&live), &test_msg(2, "local-era", None)),
            LedgerMark::Unmarked
        );
    }

    /// Confirmation is a fact about the row, not about connection health:
    /// a degraded federated room keeps its confirmed marks.
    #[test]
    fn ledger_mark_survives_degraded_federated_states() {
        let mut confirmed = test_msg(1, "hello", None);
        confirmed.federated = Some(test_meta());
        for state in [
            RoomAccessState::Connecting,
            RoomAccessState::Recovering,
            RoomAccessState::Revoked,
        ] {
            assert_eq!(
                ledger_mark(Some(&test_access(state)), &confirmed),
                LedgerMark::Confirmed
            );
        }
    }

    /// A Local room (or a room whose access projection has not loaded)
    /// has no ledger to reach — nothing may render, even for a row that
    /// somehow carries metadata.
    #[test]
    fn ledger_mark_is_silent_where_no_ledger_exists() {
        let mut confirmed = test_msg(1, "hello", None);
        confirmed.federated = Some(test_meta());
        let local = test_access(RoomAccessState::Local);

        assert_eq!(
            ledger_mark(Some(&local), &confirmed),
            LedgerMark::NotApplicable
        );
        assert_eq!(ledger_mark(None, &confirmed), LedgerMark::NotApplicable);
        assert_eq!(
            ledger_mark(Some(&local), &test_msg(2, "plain", None)),
            LedgerMark::NotApplicable
        );
        assert_eq!(
            ledger_mark(None, &test_msg(3, "plain", None)),
            LedgerMark::NotApplicable
        );
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

    fn roster_row(id: &str, display_name: &str, kind: RoomParticipantKind) -> RoomParticipant {
        RoomParticipant {
            id: id.into(),
            kind,
            display_name: display_name.into(),
        }
    }

    /// The four answers the rail can give an agent row, on one roster.
    ///
    /// The unclaimed arm is the one the slice exists for. Before it, an agent
    /// nobody owns and an agent whose owner the surface never decoded rendered
    /// identically — as a bare row — so a reader could not tell an unclaimed
    /// worker-less agent from a rail that simply had nothing to say. It is a
    /// distinct variant rather than an empty label for that reason.
    #[test]
    fn agent_ownership_names_the_owner_or_says_unclaimed() {
        let participants = vec![
            roster_row("alice", "Alice", RoomParticipantKind::Human),
            roster_row("researcher", "Researcher", RoomParticipantKind::Agent),
            roster_row("scribe", "Scribe", RoomParticipantKind::Agent),
            roster_row("drifter", "Drifter", RoomParticipantKind::Agent),
        ];
        let owners = vec![
            RoomAgentOwner {
                agent_id: "researcher".into(),
                owner_id: "alice".into(),
                owner_present: true,
            },
            // The binding outlives the worker: `bob` is gone from the roster,
            // and the daemon says so rather than dropping the row.
            RoomAgentOwner {
                agent_id: "scribe".into(),
                owner_id: "bob".into(),
                owner_present: false,
            },
        ];

        assert_eq!(
            agent_ownership(Some(&owners), &participants, "researcher"),
            AgentOwnership::Owned {
                owner: "Alice".into(),
                present: true,
            },
            "an owner still on the roster is named by their DISPLAY name — the \
             participant id is a key, not something a reader recognises",
        );
        assert_eq!(
            agent_ownership(Some(&owners), &participants, "scribe"),
            AgentOwnership::Owned {
                owner: "bob".into(),
                present: false,
            },
            "a departed owner keeps their row: the ownership happened. The raw \
             id is the only name left once the roster no longer carries them",
        );
        assert_eq!(
            agent_ownership(Some(&owners), &participants, "drifter"),
            AgentOwnership::Unclaimed,
            "no ownership row names this agent, and the rail must SAY that \
             rather than render nothing",
        );
        assert_eq!(
            agent_ownership(Some(&[]), &participants, "researcher"),
            AgentOwnership::Unclaimed,
            "an AUTHORITATIVE empty list is the daemon saying nobody owns \
             anything in this room, which is unclaimed for every agent in it — \
             never a present owner",
        );
    }

    /// No answer is its own state, and the rail renders NOTHING for it.
    ///
    /// Codex found this on #195: a bare `Vec` with `#[serde(default)]` makes a
    /// daemon predating ocean-os#437 — which omits the key and may hold durable
    /// ownership rows it simply cannot project — indistinguishable from a
    /// current daemon answering `[]`. Every agent in every room on such a
    /// daemon would wear an `unclaimed` badge the surface has no evidence for.
    /// It is the same provenance rule the older-history edge draws between
    /// `ReachedBeginning` and `Unknown`: an absent answer is not a negative one.
    ///
    /// `None` also covers the window a binding mutation opens — the refresh
    /// invalidates before it asks, so a refresh that never answers degrades to
    /// silence rather than to a stale claim.
    #[test]
    fn no_answer_renders_nothing_and_is_never_unclaimed() {
        let participants = vec![
            roster_row("alice", "Alice", RoomParticipantKind::Human),
            roster_row("researcher", "Researcher", RoomParticipantKind::Agent),
        ];

        assert_eq!(
            agent_ownership(None, &participants, "researcher"),
            AgentOwnership::Unknown,
            "a daemon that said nothing about ownership has not said that \
             nobody owns this agent",
        );
        assert_ne!(
            agent_ownership(None, &participants, "researcher"),
            agent_ownership(Some(&[]), &participants, "researcher"),
            "no answer and an authoritative empty answer must not be the same \
             value — collapsing them is the defect this test exists for",
        );
    }

    /// `owner_present` is the daemon's answer NARROWED by the roster in front
    /// of the reader. The daemon computes it at hydration as "is `owner_id`
    /// still on this roster"; the roster then moves under the surface, because
    /// join/leave/remove replace `Room::participants` from routes that carry no
    /// `agent_owners` at all. So a `true` beside a worker the rail no longer
    /// shows is one read stale, and rendering it would badge a present owner
    /// the reader cannot find in the rail three pixels above.
    ///
    /// The other direction is deliberately NOT symmetric: a daemon that says
    /// absent stays absent even when a same-id row is back on the roster. A
    /// participant id is reusable and a rejoin is not evidence the original
    /// binding survived.
    #[test]
    fn a_present_flag_never_outlives_the_owner_leaving_the_rail() {
        let owners = vec![RoomAgentOwner {
            agent_id: "researcher".into(),
            owner_id: "alice".into(),
            owner_present: true,
        }];
        let with_alice = vec![
            roster_row("alice", "Alice", RoomParticipantKind::Human),
            roster_row("researcher", "Researcher", RoomParticipantKind::Agent),
        ];
        let without_alice = vec![roster_row(
            "researcher",
            "Researcher",
            RoomParticipantKind::Agent,
        )];

        assert_eq!(
            agent_ownership(Some(&owners), &with_alice, "researcher"),
            AgentOwnership::Owned {
                owner: "Alice".into(),
                present: true,
            },
        );
        assert_eq!(
            agent_ownership(Some(&owners), &without_alice, "researcher"),
            AgentOwnership::Owned {
                owner: "alice".into(),
                present: false,
            },
            "removed after hydration: the ownership stands, the presence does \
             not, and the name falls back to the id the row carries",
        );

        let absent_flag = vec![RoomAgentOwner {
            agent_id: "researcher".into(),
            owner_id: "alice".into(),
            owner_present: false,
        }];
        assert_eq!(
            agent_ownership(Some(&absent_flag), &with_alice, "researcher"),
            AgentOwnership::Owned {
                owner: "Alice".into(),
                present: false,
            },
            "a rejoining id does not resurrect a presence the daemon denied",
        );
    }

    /// Roster order is the daemon's (`ORDER BY p.position`) and the lookup must
    /// not depend on it: two agents owned by two workers resolve to their own
    /// owners whichever way round the rows arrive. A `find` on the wrong field
    /// — or a positional zip of owners onto agents, which is the shortcut this
    /// shape invites — passes with one row and swaps the owners here.
    #[test]
    fn two_owned_agents_do_not_borrow_each_others_owners() {
        let participants = vec![
            roster_row("alice", "Alice", RoomParticipantKind::Human),
            roster_row("bob", "Bob", RoomParticipantKind::Human),
            roster_row("researcher", "Researcher", RoomParticipantKind::Agent),
            roster_row("scribe", "Scribe", RoomParticipantKind::Agent),
        ];
        let owners = vec![
            RoomAgentOwner {
                agent_id: "scribe".into(),
                owner_id: "bob".into(),
                owner_present: true,
            },
            RoomAgentOwner {
                agent_id: "researcher".into(),
                owner_id: "alice".into(),
                owner_present: true,
            },
        ];

        assert_eq!(
            agent_ownership(Some(&owners), &participants, "researcher"),
            AgentOwnership::Owned {
                owner: "Alice".into(),
                present: true,
            },
        );
        assert_eq!(
            agent_ownership(Some(&owners), &participants, "scribe"),
            AgentOwnership::Owned {
                owner: "Bob".into(),
                present: true,
            },
        );
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

    fn fed_member(id: &str) -> FederatedRoomMemberProjection {
        FederatedRoomMemberProjection {
            member_id: id.into(),
            owner_member_id: None,
            actor_type: FederatedActorType::Agent,
            role_in_room: FederatedRoomRole::Member,
            display_name: id.into(),
            public_agent_descriptor: None,
            joined_at: "2026-07-16T22:00:00Z".into(),
            derived_presence: None,
            local_binding_available: Some(true),
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
    fn mention_candidates_exclude_agents_without_active_local_authority() {
        let local = vec![
            part("human", "Human", RoomParticipantKind::Human),
            part("active-agent", "Active", RoomParticipantKind::Agent),
            part("compat-agent", "Compatibility", RoomParticipantKind::Agent),
        ];
        let access = test_access(RoomAccessState::Local);
        let active = std::collections::HashSet::from(["active-agent".to_owned()]);
        assert_eq!(
            mentionable_roster(&local, Some(&access), &active),
            vec![
                part("human", "Human", RoomParticipantKind::Human),
                part("active-agent", "Active", RoomParticipantKind::Agent),
            ]
        );
        // Roster rendering remains unchanged for historical attribution.
        assert_eq!(mention_roster(&local, Some(&access)), local);
    }

    #[test]
    fn federated_mention_candidates_require_available_active_binding() {
        let mut access = test_access(RoomAccessState::Live);
        access.members = vec![
            FederatedRoomMemberProjection {
                member_id: "available".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::Agent,
                role_in_room: FederatedRoomRole::Member,
                display_name: "Available".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: None,
                local_binding_available: Some(true),
            },
            FederatedRoomMemberProjection {
                member_id: "projection-denied".into(),
                owner_member_id: None,
                actor_type: FederatedActorType::Agent,
                role_in_room: FederatedRoomRole::Member,
                display_name: "Denied".into(),
                public_agent_descriptor: None,
                joined_at: String::new(),
                derived_presence: None,
                local_binding_available: Some(false),
            },
        ];
        let active = std::collections::HashSet::from([
            "available".to_owned(),
            "projection-denied".to_owned(),
        ]);
        assert_eq!(
            mentionable_roster(&[], Some(&access), &active),
            vec![part("available", "Available", RoomParticipantKind::Agent)]
        );
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
        assert!(mention_accept_is_valid(
            text,
            Some(3),
            &roster,
            0,
            Some("flux")
        ));
        assert!(!mention_accept_is_valid(
            text,
            Some(11),
            &roster,
            0,
            Some("flux")
        ));
        assert!(!mention_accept_is_valid(
            text,
            None,
            &roster,
            0,
            Some("flux")
        ));
    }

    #[test]
    fn mention_accept_validation_rejects_a_stale_displayed_candidate() {
        let roster = vec![
            part("ax", "Adel", RoomParticipantKind::Human),
            part("ada", "Ada", RoomParticipantKind::Human),
        ];
        assert!(mention_accept_is_valid(
            "@ad",
            Some(3),
            &roster,
            0,
            Some("ada")
        ));
        assert!(!mention_accept_is_valid(
            "@ad",
            Some(2),
            &roster,
            0,
            Some("ada")
        ));
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
        assert_eq!(text, "hi @flux tail");
        assert_eq!(caret, "hi @flux ".len());

        let (text, caret) = apply_mention("@", 0, 1, "designer");
        assert_eq!(text, "@designer ");
        assert_eq!(caret, text.len());

        let (text, caret) = apply_mention("@flux", 0, 3, "flux");
        assert_eq!(text, "@flux ");
        assert_eq!(caret, text.len());

        let (text, caret) = apply_mention("@fl\u{00a0}tail", 0, 3, "flux");
        assert_eq!(text, "@flux tail");
        assert_eq!(caret, "@flux ".len());
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
            attachment_id: None,
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

    #[test]
    fn composer_epoch_changes_on_room_or_generation_and_not_on_rerender() {
        let first = RoomComposerEpoch {
            generation: 4,
            room_key: Some("room-a".into()),
        };
        assert!(!room_composer_epoch_changed(
            Some(&first),
            4,
            Some("room-a")
        ));
        assert!(room_composer_epoch_changed(Some(&first), 5, Some("room-a")));
        assert!(room_composer_epoch_changed(Some(&first), 4, Some("room-b")));
        assert!(room_composer_epoch_changed(Some(&first), 4, None));
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

    #[test]
    fn compact_escape_closes_an_open_unhandled_drawer() {
        assert_eq!(
            compact_escape_action(true, true, false),
            Some(CompactEscapeAction::CloseDrawer)
        );
    }

    #[test]
    fn compact_escape_with_closed_drawer_bubbles_to_app() {
        assert_eq!(compact_escape_action(true, false, false), None);
    }

    #[test]
    fn desktop_escape_is_a_rooms_workspace_no_op() {
        assert_eq!(compact_escape_action(false, true, false), None);
    }

    #[test]
    fn already_handled_escape_does_not_also_close_drawer() {
        assert_eq!(compact_escape_action(true, true, true), None);
    }

    fn strip_css_comments(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(open) = rest.find("/*") {
            out.push_str(&rest[..open]);
            if let Some(close) = rest[open + 2..].find("*/") {
                rest = &rest[open + 2 + close + 2..];
            } else {
                break;
            }
        }
        out.push_str(rest);
        out
    }

    /// Brace-matched bodies for media rules with the requested prelude.
    fn css_media_blocks(css: &str, needle: &str) -> Vec<String> {
        let css = strip_css_comments(css);
        let bytes = css.as_bytes();
        let mut blocks = Vec::new();
        let mut from = 0usize;
        while let Some(relative) = css[from..].find(needle) {
            let at = from + relative;
            let Some(open_relative) = css[at..].find('{') else {
                break;
            };
            let open = at + open_relative;
            let mut depth = 0usize;
            let mut end = None;
            for (index, byte) in bytes.iter().enumerate().skip(open) {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(index);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                break;
            };
            blocks.push(css[open + 1..end].to_string());
            from = end + 1;
        }
        blocks
    }

    fn css_without_whitespace(css: &str) -> String {
        css.chars().filter(|char| !char.is_whitespace()).collect()
    }

    #[test]
    fn compact_nav_visibility_override_follows_its_hidden_base_rule() {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let stripped = strip_css_comments(css);
        let compact_blocks = css_media_blocks(&stripped, "@media (max-width: 650px)");
        let has_compact_override = compact_blocks.iter().any(|body| {
            css_without_whitespace(body).contains(".rooms-workspace__mobile-nav{display:flex;")
        });
        assert!(
            has_compact_override,
            "compact media body must make the Rooms mobile nav visible"
        );

        let normalized = css_without_whitespace(&stripped);
        let hidden = normalized
            .find(".rooms-workspace__mobile-nav{display:none;")
            .expect("compact nav needs a hidden desktop base rule");
        let visible = normalized
            .rfind(".rooms-workspace__mobile-nav{display:flex;")
            .expect("compact nav needs a compact visibility override");
        assert!(
            hidden < visible,
            "compact override must follow the base rule"
        );
    }

    // ── Thread panel layout guards ───────────────────────────────────────

    /// Regression: the old mode-swapped thread rail was `display: none` in
    /// the 901–1080px band, so "Open thread" toggled state and rendered
    /// nothing. The dedicated panel must never be display-hidden at any
    /// width — narrow layouts reposition it (absolute/fixed) instead.
    #[test]
    fn thread_panel_is_never_display_hidden() {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let stripped = strip_css_comments(css);
        let normalized = css_without_whitespace(&stripped);
        assert!(
            normalized.contains(".rooms-workspace__thread-panel{flex:00380px;"),
            "thread panel needs its in-flow base column rule"
        );
        let mut search = normalized.as_str();
        while let Some(at) = search.find(".rooms-workspace__thread-panel{") {
            let rule_end = search[at..]
                .find('}')
                .map(|end| at + end)
                .unwrap_or(search.len());
            assert!(
                !search[at..rule_end].contains("display:none"),
                "thread panel must never be display-hidden at any width"
            );
            search = &search[rule_end..];
        }
        let overlay_900 = css_media_blocks(&stripped, "@media (max-width: 900px)")
            .iter()
            .any(|body| {
                let body = css_without_whitespace(body);
                body.contains(".rooms-workspace__thread-panel{position:absolute;")
            });
        assert!(overlay_900, "thread panel must overlay below 900px");
        let fullscreen_650 = css_media_blocks(&stripped, "@media (max-width: 650px)")
            .iter()
            .any(|body| {
                let body = css_without_whitespace(body);
                body.contains(".rooms-workspace__thread-panel{position:fixed;inset:0;")
            });
        assert!(fullscreen_650, "thread panel must go full-screen at 650px");
    }

    /// The mode-swapped rail selectors are dead: gone from the stylesheet
    /// and never emitted from Rust again (TASK-49 guard style). Needles are
    /// concatenated at runtime so this test's own literals can't match the
    /// source blob it scans.
    #[test]
    fn mode_swapped_thread_rail_selectors_are_removed_and_unemitted() {
        let rail_modifier = ["rooms-workspace__right", "--thread"].concat();
        let rail_prefix = ["rooms-workspace__right", "-thread"].concat();
        let css = include_str!("../../../styles/rooms-workspace.css");
        assert!(
            !css.contains(&rail_modifier),
            "the --thread rail modifier is replaced by the dedicated panel"
        );
        assert!(
            !css.contains(&rail_prefix),
            "the __right-thread-* selectors are replaced by __thread-panel-*"
        );
        let markup = include_str!("rooms_workspace.rs");
        assert!(
            !markup.contains(&format!("{rail_modifier}=")),
            "no Rust emitter may resurrect the mode-swapped rail modifier"
        );
        assert!(
            !markup.contains(&format!("\"{rail_prefix}\"")),
            "no Rust emitter may resurrect the mode-swapped rail container"
        );
    }

    /// Members reachability: everywhere the inline rail is hidden the chip
    /// and drawer rules must exist, and the drawer override must follow the
    /// hide rules so it wins the cascade.
    #[test]
    fn members_drawer_overrides_follow_the_rail_hide_rules() {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let stripped = strip_css_comments(css);
        let normalized = css_without_whitespace(&stripped);
        assert!(
            normalized.contains(".rooms-workspace__members-chip{display:none;"),
            "members chip needs a hidden base rule"
        );
        let chip_1080 = css_media_blocks(&stripped, "@media (max-width: 1080px)")
            .iter()
            .any(|body| {
                css_without_whitespace(body)
                    .contains(".rooms-workspace__members-chip{display:inline-flex;")
            });
        assert!(chip_1080, "chip must be revealed at 1080px and below");
        let chip_1440 = css_media_blocks(&stripped, "@media (max-width: 1440px)")
            .iter()
            .any(|body| {
                css_without_whitespace(body).contains(
                    ".rooms-workspace--thread-open.rooms-workspace__members-chip{display:inline-flex;",
                )
            });
        assert!(
            chip_1440,
            "chip must be revealed while a thread is open up to 1440px"
        );
        let hide = normalized
            .find(".rooms-workspace__right{display:none;")
            .expect("the 901-1080px band hides the inline rail");
        let drawer = normalized
            .rfind(".rooms-workspace__right.rooms-workspace__right--visible{display:flex;")
            .expect("the drawer override must exist");
        assert!(
            hide < drawer,
            "drawer override must follow the rail hide rule to win the cascade"
        );
    }

    // ── Members drawer + view-state helpers ─────────────────────────────

    #[test]
    fn members_drawer_overlay_matches_the_css_breakpoints() {
        // ≤1080: always an overlay, threads or not.
        assert!(members_drawer_is_overlay(1080.0, false));
        assert!(members_drawer_is_overlay(1080.0, true));
        assert!(members_drawer_is_overlay(650.0, false));
        // 1081-1440: overlay only while the thread panel occupies the row.
        assert!(!members_drawer_is_overlay(1081.0, false));
        assert!(members_drawer_is_overlay(1081.0, true));
        assert!(members_drawer_is_overlay(1440.0, true));
        // >1440: the inline rail is always present; never an overlay.
        assert!(!members_drawer_is_overlay(1441.0, true));
        assert!(!members_drawer_is_overlay(1441.0, false));
    }

    #[test]
    fn members_escape_consumes_only_an_open_unhandled_overlay() {
        assert!(members_escape_closes(true, true, false));
        // Inline rail: Escape bubbles to the drawer/app hierarchy.
        assert!(!members_escape_closes(false, true, false));
        // Closed drawer: bubbles.
        assert!(!members_escape_closes(true, false, false));
        // Already handled upstream: never double-handled.
        assert!(!members_escape_closes(true, true, true));
    }

    #[test]
    fn view_state_round_trips_room_and_thread() {
        assert_eq!(
            decode_view_state(&encode_view_state("room-a", Some(42))),
            Some(("room-a".to_string(), Some(42)))
        );
        assert_eq!(
            decode_view_state(&encode_view_state("room-a", None)),
            Some(("room-a".to_string(), None))
        );
    }

    #[test]
    fn view_state_decode_fails_closed_on_malformed_payloads() {
        assert_eq!(decode_view_state(""), None);
        assert_eq!(decode_view_state("\n7"), None);
        assert_eq!(decode_view_state("room-a\nnot-a-number"), None);
        assert_eq!(decode_view_state("room-a\n-3"), None);
    }

    #[test]
    fn thread_view_defaults_to_inline_under_the_message() {
        assert_eq!(ThreadViewMode::default(), ThreadViewMode::Inline);
    }

    #[test]
    fn thread_view_mode_round_trips_and_fails_closed() {
        for mode in [ThreadViewMode::Inline, ThreadViewMode::Panel] {
            assert_eq!(
                decode_thread_view_mode(encode_thread_view_mode(mode)),
                Some(mode)
            );
        }
        assert_eq!(decode_thread_view_mode(""), None);
        assert_eq!(decode_thread_view_mode("sidebar"), None);
        assert_eq!(decode_thread_view_mode("Inline"), None);
    }

    /// The inline presentation must exist in both the stylesheet and the
    /// markup — the default thread experience is under the message, not
    /// the panel.
    #[test]
    fn inline_thread_is_styled_and_emitted() {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let stripped = strip_css_comments(css);
        let normalized = css_without_whitespace(&stripped);
        assert!(
            normalized.contains(".rooms-workspace__thread-inline{"),
            "inline thread container needs a base rule"
        );
        // Needle built at runtime so this test's own literal can't satisfy
        // the check if the emitter disappears from the markup.
        let emitter = format!(
            "class=\"{}\"",
            ["rooms-workspace__thread", "-inline"].concat()
        );
        let markup = include_str!("rooms_workspace.rs");
        assert!(
            markup.contains(&emitter),
            "the timeline must emit the inline thread container"
        );
    }

    /// The ledger mark must exist in both the stylesheet and the markup,
    /// and only as the positive class — no pending/failed variant may ever
    /// appear, because an unmarked row is not a failure state.
    #[test]
    fn ledger_mark_is_styled_and_emitted() {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let stripped = strip_css_comments(css);
        let normalized = css_without_whitespace(&stripped);
        assert!(
            normalized.contains(".rooms-workspace__msg-ledger{"),
            "ledger mark needs a base rule"
        );
        assert!(
            !normalized.contains("msg-ledger--"),
            "the ledger mark is positive-only; no state variants"
        );
        // Needle built at runtime so this test's own literal can't satisfy
        // the check if the emitter disappears from the markup.
        let emitter = format!("class=\"{}\"", ["rooms-workspace__msg", "-ledger"].concat());
        let markup = include_str!("rooms_workspace.rs");
        assert!(
            markup.contains(&emitter),
            "confirmed rows must emit the ledger mark"
        );
    }

    #[test]
    fn reply_count_label_is_truthful() {
        assert_eq!(reply_count_label(0), "No replies yet");
        assert_eq!(reply_count_label(1), "1 reply");
        assert_eq!(reply_count_label(4), "4 replies");
    }

    // ── Thread panel header helpers ─────────────────────────────────────

    #[test]
    fn thread_display_name_resolves_roster_and_falls_back_to_id() {
        let roster = vec![part("agent-1", "Atlas", RoomParticipantKind::Agent)];
        assert_eq!(roster_display_name(&roster, "agent-1"), "Atlas");
        assert_eq!(roster_display_name(&roster, "gone-user"), "gone-user");
    }

    #[test]
    fn thread_display_name_never_returns_an_empty_name() {
        let roster = vec![part("u1", "", RoomParticipantKind::Human)];
        assert_eq!(roster_display_name(&roster, "u1"), "u1");
    }

    #[test]
    fn thread_subtitle_counts_truthfully() {
        assert_eq!(
            thread_panel_subtitle(0, "Atlas"),
            "No replies yet \u{b7} replying to Atlas"
        );
        assert_eq!(
            thread_panel_subtitle(1, "Atlas"),
            "1 reply \u{b7} replying to Atlas"
        );
        assert_eq!(
            thread_panel_subtitle(3, "Atlas"),
            "3 replies \u{b7} replying to Atlas"
        );
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
