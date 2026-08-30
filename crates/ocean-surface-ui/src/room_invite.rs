//! Minting a room invite — the browser half of the daemon's invite route.
//!
//! One call, and it is the only way a second person reaches a room from here:
//!
//!   POST /v1/rooms/persistent/{key}/invites  → 201 `{code, expires_at, …}`
//!
//! Five properties of that wire contract shape everything below:
//!
//! 1. **The 201 answers the invite RAW.** `room_create_invite` serializes
//!    `InviteResponse` straight into the body — no `{ok:true}` envelope, no
//!    `{invite: …}` key, unlike artifacts, attachments and the workspace lane.
//!    A decoder copied from any of those neighbours reads a successful mint as
//!    a malformed reply.
//! 2. **`code` means two different things.** On the 201 it is the invite —
//!    the bearer grant itself. On a refusal it is absent: the daemon's
//!    `intent_error_response` writes `{ok:false, error:"<code>"}`, where the
//!    top-level `error` IS the machine code. So success is settled FIRST, on
//!    the status and a present code, and only a reply that is not a success is
//!    asked what its `error` means. Reading `code` as a refusal code the way
//!    the workspace lane does would classify a minted invite as whatever its
//!    characters happened to spell.
//! 3. **Federation being unconfigured is a STATE.** A daemon with no Bedrock
//!    client or no owner token answers 503 `federation_unavailable` — the
//!    deployment saying what it is, not a fault — and it reads as a plain
//!    sentence. So does a daemon predating the route, which answers a bare 404
//!    with no code at all; a 404 that DOES carry `room_not_found` is the room
//!    being gone, a different thing, and says so.
//! 4. **On a Local room this call bootstraps federation.** `create_invite`
//!    registers a credential-less Local room with Bedrock under the daemon's
//!    own owner token and installs the credential it gets back; the room is
//!    federated from that moment on, permanently. (A non-Local room without a
//!    credential is refused `federation_conflict` instead of published.) That
//!    is irreversible from this surface, so a Local room's first click only
//!    ARMS the control and states what firing it will do — the warning has to
//!    reach the operator before the request, not after.
//! 5. **Nothing here asserts an actor.** The route takes no `?actor_id=`:
//!    authority is the room credential, or the daemon's owner token, inside
//!    the daemon. So there is no bootstrap to wait for, and no identity for
//!    this side to guess at.
//!
//! The minted code is a bearer grant to the room. It lives in one signal and
//! the open panel's DOM and nowhere else — never a log line, never the rail
//! (which is on screen for as long as the room is), and never past the room it
//! was minted for. Everything that turns a reply into what the operator sees
//! is a free function below, unit-testable natively.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::rooms::{encode, RoomAccessProjection, RoomAccessState, Rooms};

/// The daemon's own default when `ttl_minutes` is omitted (24 hours) and the
/// ceiling its route validates against (7 days). Mirrored here so the field
/// can name the default it will fall back to, and so a value the route would
/// reject costs a sentence rather than a round trip.
const DEFAULT_TTL_MINUTES: u32 = 1440;
const MAX_TTL_MINUTES: u32 = 10080;

// ---- Wire types -------------------------------------------------------------

/// The invite as the 201 carries it. `room_key` and `room_name` ride along on
/// the wire and are deliberately not decoded: this panel only ever renders an
/// invite for the room it is open on, and echoing the daemon's idea of which
/// room that is would only let the two disagree.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Invite {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub expires_at: String,
}

/// The one lenient envelope both replies fit into. `code` is the invite on a
/// success and absent on a refusal; `error` is the machine code on a refusal
/// and absent on a success — see property 2. Neither may be read without first
/// knowing which of the two arrived.
#[derive(Debug, Default, Deserialize)]
struct InviteBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl InviteBody {
    /// The invite, if this body is one. A code that is present but blank is
    /// not an invite — nobody can redeem an empty grant.
    fn invite(self) -> Option<Invite> {
        let code = self.code?;
        if code.trim().is_empty() {
            return None;
        }
        Some(Invite {
            expires_at: self.expires_at.unwrap_or_default(),
            code,
        })
    }
}

// ---- Pure helpers -----------------------------------------------------------

/// No `?actor_id=`: authority on this route is the room credential inside the
/// daemon, never an id this side asserts.
fn invite_url(base: &str, key: &str) -> String {
    format!("{base}/v1/rooms/persistent/{}/invites", encode(key))
}

/// What a mint reply means for the panel.
#[derive(Debug, PartialEq, Eq)]
enum MintOutcome {
    /// The invite exists. The only arm that ever holds a code.
    Minted(Invite),
    /// The deployment answering honestly about itself: federation is not
    /// configured, or this daemon predates the route. A sentence, never a
    /// failure — nothing is broken and nothing the operator did is wrong.
    State(String),
    /// A refusal or fault, in words an operator can act on.
    Failure(String),
}

/// The sentence a typed state earns. `None` means the code is not a state and
/// the caller falls through to the failure arm.
fn state_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        "federation_unavailable" => {
            "Federation isn't configured on this daemon, so it can't mint invites."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The failure sentence for a coded refusal that is not a state. Every code
/// `intent_error_response` can write for this route is answered here.
fn failure_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        // NOT the expiry: `parse_ttl` already holds it to the route's own
        // `1..=10080`, so a request that leaves here cannot earn the ttl 400.
        // The cause left is the bootstrap's key check — a Local room whose key
        // Bedrock will not register, which room creation never rejected.
        "invalid_request" => {
            "This room's key can't be registered for federation. It has to start with a \
             lowercase letter or digit and hold only lowercase letters, digits, '.', '_' \
             or '-', within 128 characters."
        }
        "room_not_found" => "This room isn't on this daemon.",
        "federation_forbidden" => "This room's federation access was refused.",
        "invite_forbidden" => "This room's federation service wouldn't mint an invite.",
        "federation_conflict" => {
            "This room federates elsewhere and has no credential here, so it can't mint invites."
        }
        "federation_protocol" => {
            "The federation service answered something this surface can't read."
        }
        "internal_error" => "The daemon couldn't record the invite.",
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The sentence a deployment without the route earns.
fn route_absent_sentence() -> String {
    "Room invites aren't available on this deployment yet.".to_string()
}

/// Map a mint reply onto what the panel should show. `body` is `None` when the
/// reply did not decode, which a route-less deployment produces (an empty
/// 404), so that case is an ANSWER here rather than a transport fault.
///
/// Success is settled before any refusal code is consulted — property 2.
fn classify_mint(status: u16, body: Option<InviteBody>) -> MintOutcome {
    let Some(body) = body else {
        if status == 404 {
            return MintOutcome::State(route_absent_sentence());
        }
        return MintOutcome::Failure(format!("The invite reply could not be read ({status})."));
    };
    let error = body.error.clone();
    if (200..300).contains(&status) {
        if let Some(invite) = body.invite() {
            return MintOutcome::Minted(invite);
        }
    }
    let code = error
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty());
    match code {
        Some(code) => {
            if let Some(sentence) = state_sentence(code) {
                return MintOutcome::State(sentence);
            }
            MintOutcome::Failure(
                failure_sentence(code).unwrap_or_else(|| format!("The invite was refused: {code}")),
            )
        }
        // A 404 with no code is the daemon's unknown-route answer: this
        // deployment predates the invite lane. An answer, not a fault.
        None if status == 404 => MintOutcome::State(route_absent_sentence()),
        None => MintOutcome::Failure(format!("The invite failed ({status}).")),
    }
}

/// Read the expiry field. Empty means "let the daemon default" — 24 hours,
/// which the placeholder names — so a cleared field is a value, not a fault.
/// The range is the route's own (`1..=10080`), checked here so an impossible
/// value is refused in words instead of as a 400.
fn parse_ttl(raw: &str) -> Result<Option<u32>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let Ok(minutes) = raw.parse::<u32>() else {
        return Err("Expiry must be a whole number of minutes.".to_string());
    };
    if !(1..=MAX_TTL_MINUTES).contains(&minutes) {
        return Err(format!(
            "Expiry must be between 1 and {MAX_TTL_MINUTES} minutes (7 days)."
        ));
    }
    Ok(Some(minutes))
}

/// The request body, carrying ONLY keys the route admits. `CreateInviteBody`
/// is `deny_unknown_fields` and both its fields are optional, so an omitted
/// key is the honest way to say "no opinion" — a `null` would be a value this
/// side never meant to hold.
fn mint_payload(recipient: &str, ttl_minutes: Option<u32>) -> serde_json::Value {
    let mut payload = serde_json::Map::new();
    let recipient = recipient.trim();
    if !recipient.is_empty() {
        payload.insert(
            "recipient_name".to_string(),
            serde_json::Value::String(recipient.to_string()),
        );
    }
    if let Some(minutes) = ttl_minutes {
        payload.insert(
            "ttl_minutes".to_string(),
            serde_json::Value::Number(minutes.into()),
        );
    }
    serde_json::Value::Object(payload)
}

/// Whether minting from here would BOOTSTRAP federation — property 4. Only a
/// Local room; every other state either already holds a credential or is
/// refused `federation_conflict` rather than published.
fn mint_federates(access: Option<&RoomAccessProjection>) -> bool {
    matches!(access.map(|a| a.state), Some(RoomAccessState::Local))
}

/// What one click on the mint control does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MintClick {
    /// A Local room's first click: state the consequence, send nothing.
    Arm,
    Fire,
    /// A mint is already in flight. The control is disabled too, but a
    /// disabled attribute is paint — this is the guard that makes a double
    /// click cost one invite rather than two.
    Ignore,
}

fn mint_click(federates: bool, armed: bool, minting: bool) -> MintClick {
    if minting {
        return MintClick::Ignore;
    }
    if federates && !armed {
        return MintClick::Arm;
    }
    MintClick::Fire
}

/// The rail's one line. Deliberately CODE-FREE: the rail is on screen for as
/// long as the room is, and a bearer grant should not sit there for an hour
/// because someone minted one once. The code lives in the panel, where it was
/// asked for.
fn rail_line(invite: Option<&Invite>) -> Option<String> {
    let invite = invite?;
    Some(match invite.expires_at.trim() {
        "" => "Invite ready \u{2014} open to copy it.".to_string(),
        at => format!("Invite ready \u{2014} expires {at}."),
    })
}

/// The sentence under a minted code. The daemon always sends `expires_at`;
/// the empty arm is honesty about a reply that somehow didn't.
fn expiry_line(invite: &Invite) -> String {
    match invite.expires_at.trim() {
        "" => "The daemon didn't say when this expires.".to_string(),
        at => format!("Expires {at}."),
    }
}

/// Escape owned by the invite panel. Same contract as `repo_escape_closes`:
/// the panel is a fixed modal at the top of the rooms surface, so it consumes
/// the key before the drawers under it.
pub fn invite_escape_closes(panel_open: bool, default_prevented: bool) -> bool {
    panel_open && !default_prevented
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for one room's invite control.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail
/// closure: those closures re-run on every `rooms.access` SSE update, and a
/// mint's in-flight flag rebuilt mid-request would re-enable the control
/// during its own mint — a second invite nobody asked for, against a call that
/// publishes a Local room the first time it runs.
#[derive(Clone, Copy)]
pub struct RoomInviteState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url`.
    pub url: RwSignal<String>,
    /// The last invite minted in THIS room. Cleared by `reset`, so a code
    /// never outlives the room it grants.
    invite: RwSignal<Option<Invite>>,
    /// Who the invite is for. Optional on the wire; the daemon records it
    /// beside the code so the far side can say who was expected.
    recipient: RwSignal<String>,
    /// The expiry field, as typed. A string rather than a number so an
    /// unparseable value is refused in words instead of silently coerced.
    ttl: RwSignal<String>,
    /// A typed state worth a sentence: federation unconfigured, route absent.
    note: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
    /// A mint is in flight — blocks re-submit and drives the label.
    minting: RwSignal<bool>,
    /// Whether a Local room's mint is one click from firing.
    confirm: RwSignal<bool>,
    /// Whether the code has been copied, for the control's own label.
    copied: RwSignal<bool>,
    /// Whether the panel is open.
    panel: RwSignal<bool>,
    /// The rail control that opens the panel, so closing hands focus back.
    open_ref: NodeRef<leptos::html::Button>,
}

impl RoomInviteState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            invite: RwSignal::new(None),
            recipient: RwSignal::new(String::new()),
            ttl: RwSignal::new(String::new()),
            note: RwSignal::new(None),
            error: RwSignal::new(None),
            minting: RwSignal::new(false),
            confirm: RwSignal::new(false),
            copied: RwSignal::new(false),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
        }
    }

    /// Whether the panel is on screen. Public because the Escape ladder that
    /// owns the key lives in `rooms_workspace`, not here.
    pub fn panel_is_open(&self) -> bool {
        self.panel.get_untracked()
    }

    /// Close the panel and hand focus back to the control that opened it. A
    /// reopened panel must not resume a primed federation confirm.
    pub fn close_panel(&self) {
        self.panel.set(false);
        self.confirm.set(false);
        if let Some(open) = self.open_ref.get_untracked() {
            let _ = open.focus();
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Retire everything this room's section holds. The invite goes with it: a
    /// code grants ONE room, and leaving it on screen while another room's
    /// panel renders would offer the wrong door.
    fn reset(&self) {
        self.invite.set(None);
        self.recipient.set(String::new());
        self.ttl.set(String::new());
        self.note.set(None);
        self.error.set(None);
        self.minting.set(false);
        self.confirm.set(false);
        self.copied.set(false);
        self.panel.set(false);
    }

    /// Read the expiry field, refusing an impossible one in words. `None`
    /// means the mint must not go out — the sentence is already on screen.
    fn ttl_or_refuse(&self) -> Option<Option<u32>> {
        match parse_ttl(&self.ttl.get_untracked()) {
            Ok(ttl) => Some(ttl),
            Err(sentence) => {
                self.error.set(Some(sentence));
                None
            }
        }
    }

    /// Mint one invite.
    fn mint(&self, rooms: Rooms, key: String) {
        if self.minting.get_untracked() {
            return;
        }
        let Some(ttl) = self.ttl_or_refuse() else {
            return;
        };
        let payload = mint_payload(&self.recipient.get_untracked(), ttl);
        let base = self.base();
        let me = *self;
        let generation = rooms.generation_snapshot();
        self.minting.set(true);
        self.error.set(None);
        self.note.set(None);
        self.copied.set(false);
        spawn_local(async move {
            let url = invite_url(&base, &key);
            let outcome = post_invite(&url, &payload).await;
            me.publish(outcome, rooms.room_is_current(generation, &key));
        });
    }

    /// Publish a finished mint — but only into the room it was fired from. A
    /// reply that arrives after the operator moved on is dropped whole; the
    /// `reset` that moved them has already cleared the flag it would release.
    fn publish(&self, outcome: MintOutcome, room_is_current: bool) {
        if !room_is_current {
            return;
        }
        self.minting.set(false);
        match outcome {
            MintOutcome::Minted(invite) => {
                self.invite.set(Some(invite));
                self.copied.set(false);
            }
            MintOutcome::State(sentence) => self.note.set(Some(sentence)),
            MintOutcome::Failure(error) => self.error.set(Some(error)),
        }
    }
}

/// One mint POST: transport, decode, classify.
async fn post_invite(url: &str, payload: &serde_json::Value) -> MintOutcome {
    match Request::post(url)
        .header("content-type", "application/json")
        .json(payload)
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<InviteBody>().await.ok();
                classify_mint(status, body)
            }
            // The daemon records an invite before it answers, so a lost
            // response can still have minted one. The sentence says so rather
            // than implying nothing happened — whoever retries deserves to
            // know the first code may be live.
            Err(err) => MintOutcome::Failure(format!(
                "The request was cut ({err}) \u{2014} an invite may have been minted without \
                 reaching this screen."
            )),
        },
        Err(err) => MintOutcome::Failure(format!("Invite request encode error: {err}")),
    }
}

// ---- Component --------------------------------------------------------------

/// The open room's invite control: a compact rail row, and a panel where an
/// invite is actually minted and read.
///
/// Renders for a LOCAL room too — unlike `RoomRepo`, which needs a Bedrock
/// workspace a Local room has not got. Minting is exactly how a Local room
/// gets one, so hiding this control there would hide the only door.
/// `writes_allowed` is supplied by the workspace so this control and the
/// composer can never disagree about the same room's access projection.
#[component]
pub fn RoomInvite(
    rooms: Rooms,
    state: RoomInviteState,
    writes_allowed: Signal<bool>,
) -> impl IntoView {
    // Follow the open room. Both arms clear: opening a room must not inherit
    // the previous room's code, and closing one must not leave it standing.
    Effect::new(move |_| {
        let _ = rooms.open_key.get();
        state.reset();
    });

    // A Memo, not a raw read in the section closure: `access` notifies on
    // every roster SSE update, and a section rebuilt by one would tear down
    // the open panel mid-mint. This flips only when a room opens or closes.
    let visible = Memo::new(move |_| rooms.open_key.get().is_some_and(|key| !key.is_empty()));

    // Same reasoning, and load-bearing for the warning: whether the mint
    // publishes the room must not flicker with roster traffic.
    let federates = Memo::new(move |_| mint_federates(rooms.access.get().as_ref()));

    let can_mint = move || {
        writes_allowed.get()
            && !state.minting.get()
            && rooms.open_key.get().is_some_and(|key| !key.is_empty())
    };

    let fire = move || {
        let Some(key) = rooms.open_key.get_untracked().filter(|key| !key.is_empty()) else {
            return;
        };
        match mint_click(
            federates.get_untracked(),
            state.confirm.get_untracked(),
            state.minting.get_untracked(),
        ) {
            MintClick::Ignore => {}
            MintClick::Arm => state.confirm.set(true),
            MintClick::Fire => {
                state.confirm.set(false);
                state.mint(rooms, key);
            }
        }
    };

    view! {
        {move || {
            if !visible.get() {
                return ().into_any();
            }
            view! {
                <div class="rooms-workspace__invite">
                    <div class="rooms-workspace__invite-head">
                        <span class="rooms-workspace__invite-title">"Invite"</span>
                        <button
                            class="rooms-workspace__invite-open"
                            type="button"
                            node_ref=state.open_ref
                            title="Mint an invite to this room"
                            on:click=move |_| {
                                state.error.set(None);
                                state.panel.set(true);
                            }
                        >
                            "open"
                        </button>
                    </div>

                    // Both rendered in the rail AND the panel. Closing the
                    // panel does not cancel an in-flight mint, so whichever
                    // slot the answer lands in must survive the panel going
                    // away — a 503 or a route-absent 404 that only the panel
                    // could show would leave the operator with no answer at
                    // all.
                    {move || {
                        state.error.get().map(|error| view! {
                            <div class="rooms-workspace__invite-error" role="alert">{error}</div>
                        })
                    }}

                    {move || {
                        state.note.get().map(|note| view! {
                            <div class="rooms-workspace__invite-note">{note}</div>
                        })
                    }}

                    {move || {
                        rail_line(state.invite.get().as_ref()).map(|line| view! {
                            <div class="rooms-workspace__invite-line">{line}</div>
                        })
                    }}

                    {move || {
                        if !state.panel.get() {
                            return ().into_any();
                        }
                        view! {
                            <div
                                class="rooms-workspace__invite-scrim"
                                on:click=move |_| state.close_panel()
                            ></div>
                            <div
                                class="rooms-workspace__invite-panel"
                                role="dialog"
                                aria-modal="true"
                                aria-label="Room invite"
                            >
                                <div class="rooms-workspace__invite-panel-head">
                                    <span class="rooms-workspace__invite-panel-title">
                                        "Invite"
                                    </span>
                                    <button
                                        class="rooms-workspace__invite-close"
                                        type="button"
                                        aria-label="Close invite"
                                        on:click=move |_| state.close_panel()
                                    >
                                        "\u{d7}"
                                    </button>
                                </div>
                                <div class="rooms-workspace__invite-panel-body">
                                    {move || {
                                        state.error.get().map(|error| view! {
                                            <div
                                                class="rooms-workspace__invite-error"
                                                role="alert"
                                            >
                                                {error}
                                            </div>
                                        })
                                    }}
                                    {move || {
                                        state.note.get().map(|note| view! {
                                            <div class="rooms-workspace__invite-note">{note}</div>
                                        })
                                    }}

                                    // The consequence, stated before the
                                    // request exists — not after it has
                                    // published the room.
                                    {move || {
                                        federates.get().then(|| view! {
                                            <div class="rooms-workspace__invite-warn">
                                                "This room is local. Minting an invite registers \
                                                 it with Bedrock and it stays federated \u{2014} \
                                                 there is no undo from here."
                                            </div>
                                        })
                                    }}

                                    <div class="rooms-workspace__invite-form">
                                        <input
                                            class="rooms-workspace__invite-input"
                                            type="text"
                                            aria-label="Who the invite is for (optional)"
                                            placeholder="who it's for (optional)"
                                            prop:value=move || state.recipient.get()
                                            on:input=move |ev| {
                                                state.recipient.set(event_target_value(&ev))
                                            }
                                        />
                                        // Empty means the daemon's own default,
                                        // so the placeholder names it rather
                                        // than this side pre-filling a value it
                                        // would then have to send.
                                        <input
                                            class="rooms-workspace__invite-input"
                                            type="text"
                                            inputmode="numeric"
                                            aria-label="Expires in minutes (defaults to 1440)"
                                            placeholder=format!(
                                                "expires in minutes ({DEFAULT_TTL_MINUTES})",
                                            )
                                            prop:value=move || state.ttl.get()
                                            on:input=move |ev| {
                                                state.ttl.set(event_target_value(&ev))
                                            }
                                        />
                                        <div class="rooms-workspace__invite-actions">
                                            <button
                                                class="rooms-workspace__invite-run"
                                                class:rooms-workspace__invite-run--danger=move || {
                                                    federates.get()
                                                }
                                                type="button"
                                                title="Mint an invite code for this room"
                                                disabled=move || !can_mint()
                                                on:click=move |_| fire()
                                            >
                                                {move || {
                                                    if state.minting.get() {
                                                        "minting\u{2026}"
                                                    } else if state.confirm.get() {
                                                        "federate & mint"
                                                    } else if federates.get() {
                                                        "mint invite\u{2026}"
                                                    } else {
                                                        "mint invite"
                                                    }
                                                }}
                                            </button>
                                            {move || {
                                                state.confirm.get().then(|| view! {
                                                    <button
                                                        class="rooms-workspace__invite-run"
                                                        type="button"
                                                        on:click=move |_| state.confirm.set(false)
                                                    >
                                                        "keep local"
                                                    </button>
                                                })
                                            }}
                                        </div>
                                    </div>

                                    {move || {
                                        let Some(invite) = state.invite.get() else {
                                            return ().into_any();
                                        };
                                        let code = invite.code.clone();
                                        view! {
                                            <div class="rooms-workspace__invite-code">
                                                <code class="rooms-workspace__invite-code-value">
                                                    {invite.code.clone()}
                                                </code>
                                                <button
                                                    class="rooms-workspace__invite-copy"
                                                    type="button"
                                                    on:click=move |_| {
                                                        if let Some(window) = web_sys::window() {
                                                            let _ = window
                                                                .navigator()
                                                                .clipboard()
                                                                .write_text(&code);
                                                            state.copied.set(true);
                                                        }
                                                    }
                                                >
                                                    {move || {
                                                        if state.copied.get() {
                                                            "copied"
                                                        } else {
                                                            "copy"
                                                        }
                                                    }}
                                                </button>
                                            </div>
                                            <div class="rooms-workspace__invite-note">
                                                {expiry_line(&invite)}
                                            </div>
                                        }.into_any()
                                    }}

                                    <div class="rooms-workspace__invite-footnote">
                                        "Anyone holding the code can join this room until it \
                                         expires. Send it the way you would a password."
                                    </div>
                                </div>
                            </div>
                        }.into_any()
                    }}
                </div>
            }.into_any()
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state as `new` leaves it, for the tests that drive one directly.
    fn fresh_state() -> RoomInviteState {
        RoomInviteState {
            url: RwSignal::new("http://d".to_string()),
            invite: RwSignal::new(None),
            recipient: RwSignal::new(String::new()),
            ttl: RwSignal::new(String::new()),
            note: RwSignal::new(None),
            error: RwSignal::new(None),
            minting: RwSignal::new(false),
            confirm: RwSignal::new(false),
            copied: RwSignal::new(false),
            panel: RwSignal::new(false),
            open_ref: NodeRef::new(),
        }
    }

    fn body(json: &str) -> InviteBody {
        serde_json::from_str(json).unwrap()
    }

    /// Obviously not a grant. No fixture in this repo may carry a real one.
    const FAKE_CODE: &str = "not-a-real-invite-code";

    fn minted_json() -> String {
        format!(
            r#"{{"code": "{FAKE_CODE}",
                "expires_at": "2026-08-31T09:00:00Z",
                "room_key": "warroom",
                "room_name": "War Room"}}"#
        )
    }

    // ---- classification -----------------------------------------------------

    /// The 201 is the invite itself — no envelope, no `invite` key. A decoder
    /// copied from the artifacts or workspace lanes fails exactly here.
    #[test]
    fn the_created_reply_is_the_raw_invite() {
        let outcome = classify_mint(201, Some(body(&minted_json())));
        let MintOutcome::Minted(invite) = outcome else {
            panic!("expected Minted, got {outcome:?}");
        };
        assert_eq!(invite.code, FAKE_CODE);
        assert_eq!(invite.expires_at, "2026-08-31T09:00:00Z");
    }

    /// Property 2, pinned: on a success `code` is the grant, never a refusal
    /// code. A minted code that happens to spell one is still an invite.
    #[test]
    fn a_success_code_is_never_read_as_a_refusal_code() {
        let outcome = classify_mint(
            201,
            Some(body(
                r#"{"code": "federation_unavailable", "expires_at": "2026-08-31T09:00:00Z"}"#,
            )),
        );
        let MintOutcome::Minted(invite) = outcome else {
            panic!("expected Minted, got {outcome:?}");
        };
        assert_eq!(invite.code, "federation_unavailable");
    }

    /// A 2xx whose code is blank grants nothing, so it is not a mint.
    #[test]
    fn a_blank_code_is_not_an_invite() {
        assert!(matches!(
            classify_mint(201, Some(body(r#"{"code": "   "}"#))),
            MintOutcome::Failure(_)
        ));
    }

    /// A daemon with no Bedrock client or owner token is a deployment saying
    /// what it is. It reads as a state, in a calm voice, never as a failure.
    #[test]
    fn federation_being_unconfigured_is_a_state() {
        let outcome = classify_mint(
            503,
            Some(body(r#"{"ok": false, "error": "federation_unavailable"}"#)),
        );
        let MintOutcome::State(sentence) = outcome else {
            panic!("expected State, got {outcome:?}");
        };
        assert!(sentence.contains("isn't configured"), "got: {sentence}");
    }

    /// Every code `intent_error_response` can write earns a sentence of its
    /// own — never the bare "refused: <code>" fallback.
    #[test]
    fn every_typed_refusal_reads_as_a_sentence() {
        let cases = [
            (400, "invalid_request"),
            (404, "room_not_found"),
            (403, "federation_forbidden"),
            (403, "invite_forbidden"),
            (409, "federation_conflict"),
            (502, "federation_protocol"),
            (500, "internal_error"),
        ];
        for (status, code) in cases {
            let json = format!(r#"{{"ok": false, "error": "{code}"}}"#);
            let outcome = classify_mint(status, Some(body(&json)));
            let MintOutcome::Failure(sentence) = outcome else {
                panic!("{code}: expected Failure, got {outcome:?}");
            };
            assert!(
                !sentence.contains(code),
                "{code} must read as prose, got: {sentence}"
            );
            assert!(sentence.ends_with('.'), "{code} got: {sentence}");
        }
    }

    /// `parse_ttl` refuses an out-of-range expiry before the request exists,
    /// so the ttl 400 is unreachable from here and the bootstrap's key check
    /// is the only `invalid_request` left. The sentence has to send the
    /// operator at the key rather than at the one input that cannot be wrong.
    #[test]
    fn a_rejected_request_names_the_key_not_the_expiry() {
        let outcome = classify_mint(
            400,
            Some(body(r#"{"ok": false, "error": "invalid_request"}"#)),
        );
        let MintOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(sentence.contains("key"), "got: {sentence}");
        assert!(!sentence.contains("expir"), "got: {sentence}");
    }

    /// A deployment predating the route answers a bare 404 — undecodable, or
    /// decodable but codeless. Both read as "not available yet".
    #[test]
    fn a_daemon_without_the_route_reads_as_not_available_yet() {
        let expected = MintOutcome::State(route_absent_sentence());
        assert_eq!(classify_mint(404, None), expected);
        assert_eq!(classify_mint(404, Some(body("{}"))), expected);
    }

    /// But a 404 that names its code is the ROOM being gone, a different
    /// answer, and must not be mistaken for a missing route.
    #[test]
    fn a_coded_404_is_the_room_missing_not_the_route() {
        let outcome = classify_mint(
            404,
            Some(body(r#"{"ok": false, "error": "room_not_found"}"#)),
        );
        let MintOutcome::Failure(sentence) = outcome else {
            panic!("expected Failure, got {outcome:?}");
        };
        assert!(sentence.contains("isn't on this daemon"), "got: {sentence}");
    }

    /// An unreadable reply that is not a 404 is a fault, and says so with the
    /// status the operator can hand to whoever runs the daemon.
    #[test]
    fn an_unreadable_reply_is_a_failure_with_its_status() {
        let MintOutcome::Failure(sentence) = classify_mint(500, None) else {
            panic!("expected Failure");
        };
        assert!(sentence.contains("500"), "got: {sentence}");
    }

    /// A code this surface has never heard of still reaches the operator
    /// rather than being swallowed into a generic failure.
    #[test]
    fn an_unknown_code_is_relayed_verbatim() {
        let MintOutcome::Failure(sentence) = classify_mint(
            400,
            Some(body(r#"{"ok": false, "error": "some_new_gate"}"#)),
        ) else {
            panic!("expected Failure");
        };
        assert!(sentence.contains("some_new_gate"), "got: {sentence}");
    }

    // ---- the request --------------------------------------------------------

    /// The route takes no actor, and the key is a path segment that may hold
    /// anything a room key may hold.
    #[test]
    fn the_url_encodes_the_key_and_asserts_no_actor() {
        assert_eq!(
            invite_url("http://d", "team room"),
            "http://d/v1/rooms/persistent/team%20room/invites"
        );
        assert!(!invite_url("http://d", "r").contains("actor_id"));
    }

    /// `CreateInviteBody` is deny-extra and both its fields are optional, so
    /// the payload carries a key only when the operator gave a value.
    #[test]
    fn the_payload_carries_only_the_keys_the_strict_lane_admits() {
        assert_eq!(mint_payload("   ", None), serde_json::json!({}));
        assert_eq!(
            mint_payload(" Ada ", Some(60)),
            serde_json::json!({"recipient_name": "Ada", "ttl_minutes": 60})
        );
        assert_eq!(
            mint_payload("", Some(MAX_TTL_MINUTES)),
            serde_json::json!({"ttl_minutes": MAX_TTL_MINUTES})
        );
    }

    /// The route's own range, checked here so an impossible value costs a
    /// sentence instead of a 400. Empty is a value: the daemon's default.
    #[test]
    fn the_expiry_field_holds_the_routes_own_range() {
        assert_eq!(parse_ttl(""), Ok(None));
        assert_eq!(parse_ttl("  "), Ok(None));
        assert_eq!(parse_ttl(" 60 "), Ok(Some(60)));
        assert_eq!(parse_ttl("1"), Ok(Some(1)));
        assert_eq!(parse_ttl("10080"), Ok(Some(MAX_TTL_MINUTES)));
        assert_eq!(parse_ttl(&DEFAULT_TTL_MINUTES.to_string()), Ok(Some(1440)));
        assert!(parse_ttl("0").is_err());
        assert!(parse_ttl("10081").is_err());
        assert!(parse_ttl("-1").is_err());
        assert!(parse_ttl("soon").is_err());
    }

    // ---- gates --------------------------------------------------------------

    /// Only a Local room's mint publishes it. Everything else either holds a
    /// credential already or is refused by the daemon.
    #[test]
    fn only_a_local_room_is_federated_by_minting() {
        let projection = |state| RoomAccessProjection {
            state,
            last_confirmed_global_sequence: None,
            members: Vec::new(),
            self_member_id: None,
            outbox: Vec::new(),
        };
        assert!(mint_federates(Some(&projection(RoomAccessState::Local))));
        for state in [
            RoomAccessState::Connecting,
            RoomAccessState::Live,
            RoomAccessState::Recovering,
            RoomAccessState::Revoked,
        ] {
            assert!(!mint_federates(Some(&projection(state))), "{state:?}");
        }
        assert!(!mint_federates(None));
    }

    /// A Local room's first click only states the consequence; a federated
    /// room's fires straight away; and neither fires twice.
    #[test]
    fn a_local_mint_arms_before_it_fires_and_never_fires_twice() {
        assert_eq!(mint_click(true, false, false), MintClick::Arm);
        assert_eq!(mint_click(true, true, false), MintClick::Fire);
        assert_eq!(mint_click(false, false, false), MintClick::Fire);
        // The in-flight guard wins over both, armed or not.
        assert_eq!(mint_click(true, true, true), MintClick::Ignore);
        assert_eq!(mint_click(false, false, true), MintClick::Ignore);
    }

    #[test]
    fn escape_closes_only_an_open_unclaimed_panel() {
        assert!(invite_escape_closes(true, false));
        assert!(!invite_escape_closes(false, false));
        assert!(!invite_escape_closes(true, true));
    }

    // ---- what reaches the screen --------------------------------------------

    /// The declarations of one rule, whitespace stripped, so a reformat of the
    /// stylesheet cannot fail these guards.
    fn css_rule(selector: &str) -> String {
        let css = include_str!("../../../styles/rooms-workspace.css");
        let normalized: String = css.chars().filter(|char| !char.is_whitespace()).collect();
        let needle = format!("{selector}{{");
        let at = normalized
            .find(&needle)
            .unwrap_or_else(|| panic!("{selector} is missing from the stylesheet"));
        let start = at + needle.len();
        let end = start + normalized[start..].find('}').expect("unterminated rule");
        normalized[start..end].to_string()
    }

    /// The panel shares the repo panel's overlay tier. That is what makes the
    /// Escape ladder's at-most-one-open argument true for this rung too: the
    /// rail trigger under any of these panels sits behind their scrims.
    #[test]
    fn the_panel_sits_on_the_shared_overlay_tier() {
        assert!(css_rule(".rooms-workspace__invite-scrim").contains("z-index:440;"));
        assert!(css_rule(".rooms-workspace__invite-panel").contains("z-index:445;"));
    }

    /// The Local-room warning must actually be legible: a rule that exists but
    /// paints nothing would leave the consequence unsaid on screen while this
    /// module believed it had said it.
    #[test]
    fn the_federation_warning_is_painted() {
        let rule = css_rule(".rooms-workspace__invite-warn");
        assert!(rule.contains("color:var(--err)"), "got: {rule}");
    }

    /// The grant never reaches a log. Guarded from source in the house style,
    /// with the needle concatenated so this test's own literal cannot match
    /// the blob it scans.
    #[test]
    fn the_code_never_reaches_a_log() {
        let source = include_str!("room_invite.rs");
        let needle = ["log", "::"].concat();
        assert!(
            !source.contains(&needle),
            "an invite code is a bearer grant and must never be logged"
        );
    }

    /// Closing the panel cancels nothing — the mint is still in flight and
    /// `publish` still writes — so BOTH answer slots have to render in the
    /// rail. A `State` only the panel could show, a 503 or a daemon without
    /// the route, would otherwise land where nobody is looking.
    #[test]
    fn both_answer_slots_render_outside_the_panel() {
        let source = include_str!("room_invite.rs");
        let panel_at = source
            .find("if !state.panel.get()")
            .expect("the panel gate moved");
        let rail = &source[..panel_at];
        assert!(
            rail.contains("state.error.get()"),
            "a failure while the panel is closed would be invisible"
        );
        assert!(
            rail.contains("state.note.get()"),
            "a state while the panel is closed would be invisible"
        );
    }

    /// The rail is on screen for as long as the room is, so its line says an
    /// invite EXISTS and never what it is.
    #[test]
    fn the_rail_line_never_carries_the_code() {
        assert_eq!(rail_line(None), None);
        let invite = Invite {
            code: FAKE_CODE.to_string(),
            expires_at: "2026-08-31T09:00:00Z".to_string(),
        };
        let line = rail_line(Some(&invite)).unwrap();
        assert!(!line.contains(FAKE_CODE), "got: {line}");
        assert!(line.contains("2026-08-31T09:00:00Z"), "got: {line}");
        let undated = Invite {
            code: FAKE_CODE.to_string(),
            expires_at: String::new(),
        };
        let line = rail_line(Some(&undated)).unwrap();
        assert!(!line.contains(FAKE_CODE), "got: {line}");
    }

    #[test]
    fn the_expiry_line_is_honest_about_a_reply_without_one() {
        let invite = Invite {
            code: FAKE_CODE.to_string(),
            expires_at: String::new(),
        };
        assert!(expiry_line(&invite).contains("didn't say"));
        let dated = Invite {
            code: FAKE_CODE.to_string(),
            expires_at: "2026-08-31T09:00:00Z".to_string(),
        };
        assert_eq!(expiry_line(&dated), "Expires 2026-08-31T09:00:00Z.");
    }

    // ---- the state ----------------------------------------------------------

    /// A code grants ONE room. Switching rooms takes it off the screen with
    /// everything else the section was holding.
    #[test]
    fn a_room_switch_clears_the_minted_code() {
        let state = fresh_state();
        state.publish(
            MintOutcome::Minted(Invite {
                code: FAKE_CODE.to_string(),
                expires_at: "2026-08-31T09:00:00Z".to_string(),
            }),
            true,
        );
        assert!(state.invite.get_untracked().is_some());
        state.panel.set(true);
        state.reset();
        assert_eq!(state.invite.get_untracked(), None);
        assert!(!state.minting.get_untracked());
        assert!(!state.panel.get_untracked());
    }

    /// A reply that lands after the operator left is dropped whole — it must
    /// not put the previous room's code in front of this room's panel.
    #[test]
    fn a_reply_for_a_room_left_behind_is_dropped() {
        let state = fresh_state();
        state.minting.set(true);
        state.publish(
            MintOutcome::Minted(Invite {
                code: FAKE_CODE.to_string(),
                expires_at: String::new(),
            }),
            false,
        );
        assert_eq!(state.invite.get_untracked(), None);
    }

    /// A state answers in the calm voice and never trips the error slot; a
    /// failure does the opposite. Neither invents an invite.
    #[test]
    fn a_state_notes_and_a_failure_errs() {
        let state = fresh_state();
        state.publish(MintOutcome::State(route_absent_sentence()), true);
        assert_eq!(state.note.get_untracked(), Some(route_absent_sentence()));
        assert_eq!(state.error.get_untracked(), None);
        assert_eq!(state.invite.get_untracked(), None);

        let state = fresh_state();
        state.publish(MintOutcome::Failure("nope.".to_string()), true);
        assert_eq!(state.error.get_untracked(), Some("nope.".to_string()));
        assert_eq!(state.note.get_untracked(), None);
    }

    /// An unparseable expiry never leaves the browser: it is refused in words
    /// and the mint is told not to go out.
    #[test]
    fn an_impossible_expiry_is_refused_before_any_request() {
        let state = fresh_state();
        state.ttl.set("tomorrow".to_string());
        assert_eq!(state.ttl_or_refuse(), None);
        assert!(state
            .error
            .get_untracked()
            .is_some_and(|error| error.contains("whole number")));
        assert!(!state.minting.get_untracked());

        state.ttl.set("90".to_string());
        assert_eq!(state.ttl_or_refuse(), Some(Some(90)));
    }
}
