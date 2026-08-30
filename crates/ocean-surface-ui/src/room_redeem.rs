//! Redeeming a room invite — the browser half of the daemon's redeem route.
//!
//! One call, and it is the only way this browser joins a room it was not
//! already in:
//!
//!   POST /v1/rooms/persistent/invites/redeem  {code}  → 200 RoomAccessProjection + room_key
//!
//! It is the mirror of [`crate::room_invite`], and four properties of its wire
//! contract make it a different animal from minting:
//!
//! 1. **The 200 names the room, on a daemon new enough to say so.** ocean-os
//!    #407 put `room_key` beside the flattened `RoomAccessProjection` and made
//!    it REQUIRED daemon-side, so a successful redemption finally answers
//!    which room it joined and this module opens exactly that one. It is
//!    decoded OPTIONAL here, and the asymmetry is the point: this bundle and
//!    the daemon it talks to roll forward independently, so requiring the key
//!    would turn a redemption that ALREADY SUCCEEDED — room created,
//!    credential installed, no un-redeem — into an unreadable reply on every
//!    daemon predating #407. Absent the key, the pre-#407 fallback still
//!    stands: snapshot the room list before the request and diff it after —
//!    [`newly_joined_key`], pure and provable — and open the room only when
//!    exactly one appeared. The key comes off the wire or off the list;
//!    either way a code is opaque and none can be read out of it.
//! 2. **The refusal set is not mint's.** `room_not_found` is UNREACHABLE: no
//!    path through `redeem_invite`/`recover_pending` returns
//!    `IntentError::NotFound`, so a 404 from this route is always the ROUTER,
//!    never the room, and a daemon predating the route is the only thing that
//!    produces one. And 403 `invite_forbidden` — Bedrock refusing the code, or
//!    the self-join being denied — is the COMMON failure here, the one a wrong
//!    or expired or already-used code earns. Mint has no equivalent.
//! 3. **The one distinction worth making is whether the CODE IS SPENT.**
//!    `recover_pending` runs `remove_pending` on exactly one class of refusal
//!    (403, on either leg); every other refusal RETAINS the pending redemption
//!    on purpose, and `get_or_insert_pending_redemption` keys that record on
//!    the code itself, so re-sending the SAME code resumes the redemption
//!    already open rather than starting a second. Retrying is therefore safe
//!    by construction, and the 409 in particular is the daemon asking to be
//!    resumed — a sentence that reads it as a dead end is backwards. Hence
//!    [`RedeemOutcome::Retry`] and [`RedeemOutcome::Refused`] are separate
//!    arms rather than one "error".
//! 4. **A redemption is two network legs and it is slow.** Bedrock's redeem,
//!    then a self-join against Bedrock, then a local store write. The
//!    in-flight flag has to cover all of it, because success creates a room
//!    row and installs a credential and there is no un-redeem.
//!
//! The invite code is a bearer grant to someone else's room. It goes in the
//! request body and nowhere else — never a log line, never an error sentence,
//! never a test fixture. Everything that turns a reply into what the operator
//! sees is a free function below, unit-testable natively.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::rooms::{Room, RoomAccessState, Rooms};

// ---- Wire types -------------------------------------------------------------

/// The one lenient envelope both replies fit into.
///
/// `state` is `RoomAccessProjection`'s single required field, so its presence
/// on a 2xx settles success without decoding the projection a second time —
/// `crate::rooms` already owns that decoder and this module needs only the
/// landing state. `room_key` is #407's addition, `Option` for the reason
/// property 1 gives. `error` is the machine code `intent_error_response`
/// writes into `{ok:false, error:"<code>"}`. Neither `state` nor `error` may
/// be read without first knowing which of the two replies arrived.
#[derive(Debug, Default, Deserialize)]
struct RedeemBody {
    #[serde(default)]
    state: Option<RoomAccessState>,
    #[serde(default)]
    room_key: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The room list, read for its keys alone, and read only on the pre-#407
/// fallback path property 1 describes. Its own envelope rather than
/// `crate::rooms`'s, which is private and decodes read-state summaries this
/// probe has no use for; `Room` itself is shared, so the two cannot drift on
/// the field that matters.
#[derive(Debug, Default, Deserialize)]
struct RoomsProbe {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    rooms: Vec<Room>,
}

// ---- Pure helpers -----------------------------------------------------------

/// No key and no `?actor_id=`: the code IS the request, and authority is the
/// bearer token the daemon mints for the redemption, inside the daemon.
fn redeem_url(base: &str) -> String {
    format!("{base}/v1/rooms/persistent/invites/redeem")
}

/// What a redeem reply means for the rail.
#[derive(Debug, PartialEq, Eq)]
enum RedeemOutcome {
    /// The room is on this daemon now. Carries the access state it landed in
    /// and the key the reply named — `None` from a daemon predating #407,
    /// which sends [`RoomRedeemState::redeem`] to the room-list diff instead.
    Joined(RoomAccessState, Option<String>),
    /// The deployment answering honestly about itself: this daemon predates
    /// the redeem route. A sentence, never a failure.
    State(String),
    /// Refused, and the code was NOT consumed — property 3. The same code
    /// resumes the redemption already open, so the sentence invites a retry.
    Retry(String),
    /// Refused, and no retry will change it: the code is spent, barred, or the
    /// far side is answering a shape this daemon will not accept.
    Refused(String),
}

/// The one room that appeared while the redemption ran — the fallback for a
/// reply carrying no `room_key`, and untouched by any daemon that sends one.
///
/// `None` when the answer is not unambiguous — nothing new (redeeming into a
/// room already held takes the daemon's `(Some(_), _) => {}` arm and creates
/// no row), or more than one (something else created a room concurrently).
/// Neither is provably the room redeemed, and opening the wrong one is worse
/// than saying "it's in your list". That second case is what `room_key` was
/// added to answer, and why this is now a fallback rather than the mechanism.
fn newly_joined_key(before: &[String], after: &[Room]) -> Option<String> {
    let mut fresh = after
        .iter()
        .map(|room| room.id.as_str())
        .filter(|key| !key.is_empty() && !before.iter().any(|seen| seen == key));
    let first = fresh.next()?;
    fresh.next().is_none().then(|| first.to_string())
}

/// The sentence a refusal that did NOT consume the code earns — property 3.
/// Each one ends by sending the operator back at the same code, because that
/// is what resumes the redemption the daemon is still holding.
fn retry_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        // Bedrock or the self-join answered 409. The pending redemption is
        // retained deliberately (pinned by the daemon's own
        // `p2c_redeem_and_self_join_conflicts_are_409_class_and_retain_pending`)
        // and re-sending resumes it. This is the arm a copied decoder reads as
        // a dead end.
        "federation_conflict" => {
            "That redemption is still settling. Nothing was consumed \u{2014} run the same code \
             again to resume it."
        }
        // Two causes, and unlike mint's 503 they are not both the deployment
        // describing itself: no Bedrock client configured, OR Bedrock answered
        // 429/5xx. The sentence has to hold both, and what to do next is the
        // same either way.
        "federation_unavailable" => {
            "The federation service didn't answer \u{2014} it is either not configured on this \
             daemon or unreachable right now. Nothing was consumed; the same code still works."
        }
        "internal_error" => {
            "The daemon couldn't record the redemption. Nothing was consumed \u{2014} the same \
             code will resume it."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The sentence a refusal that is over earns. Nothing here invites a retry.
fn refused_sentence(code: &str) -> Option<String> {
    let sentence = match code {
        // Near-unreachable: a blank code is refused before it leaves this
        // module, and the route's body type requires `code`. Kept because the
        // daemon checks for blank twice and a silent screen would be worse
        // than a redundant sentence.
        "invalid_request" => "The daemon read no invite code in that request.",
        // The common one. Bedrock refuses a wrong, expired and already-spent
        // code identically, and a denied self-join lands here too; all of them
        // run `remove_pending`, so this code is finished on this daemon.
        "invite_forbidden" => {
            "That invite was refused \u{2014} it is wrong, expired, or already used. This code \
             will not work again; ask for a new one."
        }
        // The `REVOKED_STORE_SENTINEL` arm: the room is already here and its
        // membership was revoked locally. An invite cannot undo that.
        "federation_forbidden" => {
            "Your membership of that room was revoked on this daemon, so an invite can't \
             restore it."
        }
        // A deterministic shape check, not a transient — re-sending earns the
        // same answer. Nothing was consumed, and saying so keeps the operator
        // from hunting for a replacement code that would fail the same way.
        "federation_protocol" => {
            "The federation service answered something this daemon refuses. Nothing was \
             consumed, but the same code will be refused the same way."
        }
        _ => return None,
    };
    Some(sentence.to_string())
}

/// The sentence any 5xx earns when no known code named it — property 3 read
/// off the STATUS rather than off a code.
///
/// `recover_pending` runs `remove_pending` on a 403 it produced itself and on
/// nothing else, so nothing at or above 500 spends the invite: a proxy 502
/// whose HTML body never decodes, a daemon 500 carrying no code, and a
/// refusal code this bundle predates all leave the pending redemption open
/// for the same code to resume. Reading any of them as spent sends the
/// operator hunting for a replacement invite they do not need.
fn server_fault_sentence(status: u16, code: Option<&str>) -> String {
    let what = match code {
        Some(code) => format!("The redemption failed on the daemon ({code}, {status})"),
        None => format!("The redemption got no readable answer ({status})"),
    };
    format!("{what}. Nothing was consumed \u{2014} run the same code again to resume it.")
}

/// The sentence a deployment without the route earns.
fn route_absent_sentence() -> String {
    "Joining by invite code isn't available on this deployment yet.".to_string()
}

/// Map a redeem reply onto what the rail should show. `body` is `None` when
/// the reply did not decode, which a route-less deployment produces (an empty
/// 404) and so does a proxy in front of the daemon (a 502/504 whose body is
/// HTML), so that case is an ANSWER here rather than a transport fault.
///
/// Success is settled on the status and a present `state` before any refusal
/// code is consulted, for the same reason mint settles on a present `code`:
/// the two replies share one envelope and only one of them holds each field.
///
/// Anything at or above 500 that no known code named is a
/// [`RedeemOutcome::Retry`] — see [`server_fault_sentence`]. Only a 403 spends
/// the invite, so answering a gateway fault with `Refused` would tell the
/// operator their code is dead while the daemon still holds it open.
fn classify_redeem(status: u16, body: Option<RedeemBody>) -> RedeemOutcome {
    let Some(body) = body else {
        if status == 404 {
            return RedeemOutcome::State(route_absent_sentence());
        }
        // A proxy 502/504 answers HTML, which never decodes into `RedeemBody`.
        // The daemon never produced a refusal to run `remove_pending` on, so
        // the pending redemption — and the code that resumes it — survives.
        if status >= 500 {
            return RedeemOutcome::Retry(server_fault_sentence(status, None));
        }
        return RedeemOutcome::Refused(format!("The redeem reply could not be read ({status})."));
    };
    if (200..300).contains(&status) {
        if let Some(state) = body.state {
            // A blank key is not a room and must never reach `open_room` —
            // the same bar [`newly_joined_key`] holds the diff to.
            let key = body
                .room_key
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty());
            return RedeemOutcome::Joined(state, key);
        }
    }
    let code = body
        .error
        .as_deref()
        .map(str::trim)
        .filter(|code| !code.is_empty());
    match code {
        Some(code) => {
            if let Some(sentence) = retry_sentence(code) {
                return RedeemOutcome::Retry(sentence);
            }
            if let Some(sentence) = refused_sentence(code) {
                return RedeemOutcome::Refused(sentence);
            }
            // A code this bundle has never seen is still bounded by its
            // status, and only a 403 spends the invite.
            match status >= 500 {
                true => RedeemOutcome::Retry(server_fault_sentence(status, Some(code))),
                false => RedeemOutcome::Refused(format!("The invite was refused: {code}")),
            }
        }
        // A 404 with no code is the daemon's unknown-route answer. Property 2:
        // the route itself has no 404, so this is the only 404 it can produce
        // and there is no room-missing reading to confuse it with.
        None if status == 404 => RedeemOutcome::State(route_absent_sentence()),
        None if status >= 500 => RedeemOutcome::Retry(server_fault_sentence(status, None)),
        None => RedeemOutcome::Refused(format!("The redemption failed ({status}).")),
    }
}

/// What a redeemed room reads as. `key` is `None` only on the pre-#407 path,
/// where the reply named no room and the diff could not either — see
/// [`newly_joined_key`] — and the sentence then points at the list instead of
/// at a key it does not have.
///
/// Every successful redemption lands in `Connecting`: `recover_pending` ends
/// on `update_room_access_safe(…, Some(Connecting), …)` and the bridge
/// promotes it from there. So the sentence says the room is joined and still
/// catching up rather than promising a transcript that has not arrived.
fn joined_sentence(state: RoomAccessState, key: Option<&str>) -> String {
    let room = match key {
        Some(key) => format!("Joined {key}"),
        None => "You're in \u{2014} the room is in your list".to_string(),
    };
    match state {
        RoomAccessState::Connecting => {
            format!("{room}. Connecting to the room's federation service\u{2026}")
        }
        _ => format!("{room}."),
    }
}

// ---- State ------------------------------------------------------------------

/// Reactive handle for the rail's redeem control.
///
/// Constructed at `RoomsWorkspace` component scope, never inside a rail
/// closure: those closures re-run on every `rooms.access` SSE update, and an
/// in-flight flag rebuilt mid-request would re-enable the control during its
/// own redemption — property 4, against a call that creates a room row and
/// installs a credential with no way back.
#[derive(Clone, Copy)]
pub struct RoomRedeemState {
    /// Daemon base URL, shared with `Daemon::url` through `Rooms::url`.
    pub url: RwSignal<String>,
    /// The code as typed. Cleared on success only — every refusal keeps it,
    /// because retrying is done with the SAME code.
    code: RwSignal<String>,
    /// A calm sentence: joined, or the route is absent.
    note: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
    /// A redemption is in flight across BOTH legs. Blocks re-submit and drives
    /// the label.
    redeeming: RwSignal<bool>,
}

impl RoomRedeemState {
    pub fn new(rooms: &Rooms) -> Self {
        Self {
            url: rooms.url,
            code: RwSignal::new(String::new()),
            note: RwSignal::new(None),
            error: RwSignal::new(None),
            redeeming: RwSignal::new(false),
        }
    }

    fn base(&self) -> String {
        self.url.get_untracked().trim_end_matches('/').to_string()
    }

    /// Redeem one invite code.
    ///
    /// Nothing here is scoped to the open room — a redeemer typically has no
    /// rooms at all — so unlike the mint lane there is no generation to guard
    /// against. The only staleness that could matter is the control going
    /// away, and these signals are owned at workspace scope, so it cannot.
    fn redeem(&self, rooms: Rooms) {
        if self.redeeming.get_untracked() {
            return;
        }
        let code = self.code.get_untracked().trim().to_string();
        if code.is_empty() {
            self.error
                .set(Some("Paste an invite code first.".to_string()));
            // Every other path clears BOTH slots before it starts. Without
            // this the empty submit stacks "Paste an invite code first." on
            // top of a stale "Joined warroom. Connecting…" and the rail
            // paints two contradictory answers at once.
            self.note.set(None);
            return;
        }
        let base = self.base();
        let me = *self;
        // Snapshotted BEFORE the request goes out: the pre-#407 diff is only
        // sound against the list as it stood when the redemption started.
        // Unread when the reply names the room itself.
        let before: Vec<String> = rooms
            .list
            .get_untracked()
            .iter()
            .map(|room| room.id.clone())
            .collect();
        self.redeeming.set(true);
        self.error.set(None);
        self.note.set(None);
        spawn_local(async move {
            let outcome = post_redeem(&redeem_url(&base), &code).await;
            let key = match &outcome {
                // Property 1: from #407 on, the reply says which room this
                // was and no second request is owed.
                RedeemOutcome::Joined(_, Some(key)) => Some(key.clone()),
                // An older daemon, so ask the list. Its own request because
                // `Rooms::fetch_rooms` is fire-and-forget and a diff needs an
                // await point.
                RedeemOutcome::Joined(_, None) => {
                    newly_joined_key(&before, &fetch_room_list(&base).await)
                }
                _ => None,
            };
            let joined = matches!(outcome, RedeemOutcome::Joined(..));
            me.publish(outcome, key.as_deref());
            if joined {
                // The canonical refresh, which the fallback probe deliberately
                // is not: this is what merges read summaries and drives the
                // rail's own loading state.
                rooms.fetch_rooms();
                if let Some(key) = key {
                    rooms.open_room(key);
                }
            }
        });
    }

    /// Publish a finished redemption.
    fn publish(&self, outcome: RedeemOutcome, key: Option<&str>) {
        self.redeeming.set(false);
        match outcome {
            // `key` is the RESOLVED one — the reply's where it carried one,
            // the diff's where it did not — so the arm's own key, which is
            // only the input to that choice, is spent by here.
            RedeemOutcome::Joined(state, _) => {
                // Spent, and the only arm that clears the field. Every refusal
                // keeps the code where the operator can run it again.
                self.code.set(String::new());
                self.note.set(Some(joined_sentence(state, key)));
            }
            RedeemOutcome::State(sentence) => self.note.set(Some(sentence)),
            RedeemOutcome::Retry(sentence) | RedeemOutcome::Refused(sentence) => {
                self.error.set(Some(sentence))
            }
        }
    }
}

/// One redeem POST: transport, decode, classify.
async fn post_redeem(url: &str, code: &str) -> RedeemOutcome {
    let payload = serde_json::json!({ "code": code });
    match Request::post(url)
        .header("content-type", "application/json")
        .json(&payload)
    {
        Ok(request) => match request.send().await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.json::<RedeemBody>().await.ok();
                classify_redeem(status, body)
            }
            // The daemon records the pending redemption before either leg goes
            // out, so a lost response may already have joined the room. The
            // same code resumes that record rather than opening a second, so
            // the sentence sends the operator back at it.
            Err(err) => RedeemOutcome::Retry(format!(
                "The request was cut ({err}) \u{2014} run the same code again; it resumes the \
                 redemption rather than starting another."
            )),
        },
        Err(err) => RedeemOutcome::Refused(format!("Redeem request encode error: {err}")),
    }
}

/// The daemon's room list, for the pre-#407 diff alone, and not fetched at all
/// when the reply named the room. Every failure answers an empty list, which
/// [`newly_joined_key`] reads as "cannot say" — the redemption has already
/// succeeded by the time this runs, and a probe that did not land must not
/// turn that into an error.
async fn fetch_room_list(base: &str) -> Vec<Room> {
    let url = format!("{base}/v1/rooms/persistent");
    let Ok(resp) = Request::get(&url).send().await else {
        return Vec::new();
    };
    match resp.json::<RoomsProbe>().await {
        Ok(probe) if probe.ok => probe.rooms,
        _ => Vec::new(),
    }
}

// ---- Component --------------------------------------------------------------

/// The rail's redeem control: paste a code, join the room it grants.
///
/// A sibling of the create block rather than anything hung off the open room.
/// You redeem to GET a room, so there is no open room to attach it to, and the
/// left rail is the only surface visible to someone holding no rooms at all —
/// which is exactly the state a redeemer is in.
#[component]
pub fn RoomRedeem(rooms: Rooms, state: RoomRedeemState) -> impl IntoView {
    let fire = move || state.redeem(rooms);

    view! {
        <div class="rooms-workspace__redeem">
            <div class="rooms-workspace__redeem-row">
                <input
                    class="rooms-workspace__left-input"
                    type="text"
                    aria-label="Invite code"
                    aria-busy=move || state.redeeming.get().to_string()
                    placeholder="Invite code\u{2026}"
                    prop:value=move || state.code.get()
                    on:input=move |ev| state.code.set(event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Enter" {
                            ev.prevent_default();
                            fire();
                        }
                    }
                    disabled=move || state.redeeming.get()
                />
                <button
                    class="rooms-workspace__redeem-run"
                    type="button"
                    title="Join a room with an invite code"
                    disabled=move || state.redeeming.get()
                    on:click=move |_| fire()
                >
                    {move || if state.redeeming.get() { "joining\u{2026}" } else { "join" }}
                </button>
            </div>

            {move || {
                state.error.get().map(|error| view! {
                    <div class="rooms-workspace__redeem-error" role="alert">{error}</div>
                })
            }}

            {move || {
                state.note.get().map(|note| view! {
                    <div class="rooms-workspace__redeem-note" role="status" aria-live="polite">
                        {note}
                    </div>
                })
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: &str) -> RedeemBody {
        serde_json::from_str(json).unwrap()
    }

    fn refusal(code: &str) -> RedeemBody {
        body(&format!(r#"{{"ok": false, "error": "{code}"}}"#))
    }

    fn room(id: &str) -> Room {
        Room {
            id: id.to_string(),
            name: id.to_string(),
            participants: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
            trigger_policy: None,
        }
    }

    /// Obviously not a grant. No fixture in this repo may carry a real one.
    const FAKE_CODE: &str = "not-a-real-invite-code";

    /// The 200 is a bare `RoomAccessProjection` plus #407's `room_key` — no
    /// `{ok:true}` envelope, no `invite` key, and the projection FLATTENED
    /// rather than nested under `access`, which is what keeps a top-level
    /// `state` where success detection looks for it. A decoder copied from the
    /// artifacts or workspace lanes fails exactly here.
    #[test]
    fn the_ok_reply_is_a_bare_access_projection_plus_the_key() {
        let outcome = classify_redeem(
            200,
            Some(body(
                r#"{"state": "connecting", "members": [], "outbox": [], "room_key": "warroom"}"#,
            )),
        );
        assert_eq!(
            outcome,
            RedeemOutcome::Joined(RoomAccessState::Connecting, Some("warroom".to_string()))
        );
    }

    /// Property 1's whole reason for `Option`: a daemon predating #407 sends
    /// no `room_key`, and by the time this decodes, its redemption has ALREADY
    /// SUCCEEDED — room created, credential installed, no un-redeem. It must
    /// still read as a join that falls back to the diff for a name, never a
    /// hard failure over a reply that cannot be un-sent.
    #[test]
    fn a_reply_without_the_key_still_joins() {
        let outcome = classify_redeem(
            200,
            Some(body(
                r#"{"state": "connecting", "members": [], "outbox": []}"#,
            )),
        );
        assert_eq!(
            outcome,
            RedeemOutcome::Joined(RoomAccessState::Connecting, None)
        );
    }

    /// A blank key is not a room. `newly_joined_key` already refuses one off
    /// the list, and the wire has to clear the same bar: this key goes
    /// straight to `open_room`.
    #[test]
    fn a_blank_room_key_reads_as_no_key() {
        let outcome = classify_redeem(
            200,
            Some(body(r#"{"state": "connecting", "room_key": " "}"#)),
        );
        assert_eq!(
            outcome,
            RedeemOutcome::Joined(RoomAccessState::Connecting, None)
        );
    }

    /// A 2xx with no `state` is not a projection, so it is not a redemption —
    /// and with no `error` either it has nothing left to say but its status.
    #[test]
    fn a_success_without_a_state_is_not_a_join() {
        assert!(matches!(
            classify_redeem(200, Some(body("{}"))),
            RedeemOutcome::Refused(_)
        ));
    }

    /// Property 3, the trap: 409 RETAINS the pending redemption and the same
    /// code resumes it. A sentence that reads it as a dead end is backwards.
    #[test]
    fn a_conflict_invites_the_same_code_again() {
        let outcome = classify_redeem(409, Some(refusal("federation_conflict")));
        let RedeemOutcome::Retry(sentence) = outcome else {
            panic!("expected Retry, got {outcome:?}");
        };
        assert!(sentence.contains("same code"), "got: {sentence}");
        assert!(sentence.contains("Nothing was consumed"), "got: {sentence}");
    }

    /// The common failure, and the one class where `remove_pending` runs. It
    /// must not invite a retry with a code that is already gone.
    #[test]
    fn a_forbidden_invite_is_terminal_and_says_so() {
        let outcome = classify_redeem(403, Some(refusal("invite_forbidden")));
        let RedeemOutcome::Refused(sentence) = outcome else {
            panic!("expected Refused, got {outcome:?}");
        };
        assert!(sentence.contains("will not work again"), "got: {sentence}");
        assert!(!sentence.contains("same code"), "got: {sentence}");
    }

    /// The other 403 is a LOCAL revocation, not a bad code — sending the
    /// operator after a replacement invite would waste their time.
    #[test]
    fn a_revoked_membership_reads_as_local_not_as_a_bad_code() {
        let outcome = classify_redeem(403, Some(refusal("federation_forbidden")));
        let RedeemOutcome::Refused(sentence) = outcome else {
            panic!("expected Refused, got {outcome:?}");
        };
        assert!(sentence.contains("revoked"), "got: {sentence}");
    }

    /// Unlike mint's 503, this one is not purely the deployment describing
    /// itself: Bedrock answering 429/5xx lands here too. Both readings have to
    /// survive, and so does the fact that nothing was consumed.
    #[test]
    fn unavailable_holds_both_of_its_causes() {
        let outcome = classify_redeem(503, Some(refusal("federation_unavailable")));
        let RedeemOutcome::Retry(sentence) = outcome else {
            panic!("expected Retry, got {outcome:?}");
        };
        assert!(sentence.contains("not configured"), "got: {sentence}");
        assert!(sentence.contains("unreachable"), "got: {sentence}");
        assert!(sentence.contains("Nothing was consumed"), "got: {sentence}");
    }

    /// Every code this route can write earns a sentence of its own — never the
    /// bare "refused: <code>" fallback — and each lands in the arm that
    /// matches whether the code survived. `room_not_found` is deliberately
    /// absent: no path through `redeem_invite`/`recover_pending` returns
    /// `IntentError::NotFound`, so writing a sentence for it would be fiction.
    #[test]
    fn every_reachable_refusal_reads_as_a_sentence_in_the_right_arm() {
        let retryable = [
            (409, "federation_conflict"),
            (503, "federation_unavailable"),
            (500, "internal_error"),
        ];
        let terminal = [
            (400, "invalid_request"),
            (403, "invite_forbidden"),
            (403, "federation_forbidden"),
            (502, "federation_protocol"),
        ];
        for (status, code) in retryable {
            let outcome = classify_redeem(status, Some(refusal(code)));
            let RedeemOutcome::Retry(sentence) = outcome else {
                panic!("{code}: expected Retry, got {outcome:?}");
            };
            assert!(!sentence.contains(code), "{code} got: {sentence}");
            assert!(sentence.ends_with('.'), "{code} got: {sentence}");
        }
        for (status, code) in terminal {
            let outcome = classify_redeem(status, Some(refusal(code)));
            let RedeemOutcome::Refused(sentence) = outcome else {
                panic!("{code}: expected Refused, got {outcome:?}");
            };
            assert!(!sentence.contains(code), "{code} got: {sentence}");
            assert!(sentence.ends_with('.'), "{code} got: {sentence}");
        }
        assert_eq!(refused_sentence("room_not_found"), None);
        assert_eq!(retry_sentence("room_not_found"), None);
    }

    /// Property 2: this route has no room-missing 404, so a bare one is always
    /// a daemon predating the route. Undecodable or decodable-but-codeless,
    /// both read the same calm way.
    #[test]
    fn a_daemon_without_the_route_reads_as_not_available_yet() {
        let expected = RedeemOutcome::State(route_absent_sentence());
        assert_eq!(classify_redeem(404, None), expected);
        assert_eq!(classify_redeem(404, Some(body("{}"))), expected);
    }

    /// A gateway in front of the daemon answers HTML on a 502/504, so the
    /// reply never decodes and the old reading of that was `Refused` — which
    /// tells the operator their code is spent. It is not: `remove_pending`
    /// runs on a 403 the daemon produced itself and on nothing else, and a
    /// gateway fault means the daemon produced no refusal at all. The pending
    /// redemption is still open and the same code resumes it.
    #[test]
    fn a_gateway_that_ate_the_reply_invites_the_same_code_again() {
        for status in [500, 502, 503, 504] {
            let outcome = classify_redeem(status, None);
            let RedeemOutcome::Retry(sentence) = outcome else {
                panic!("{status}: expected Retry, got {outcome:?}");
            };
            assert!(sentence.contains("Nothing was consumed"), "got: {sentence}");
            assert!(sentence.contains("same code"), "got: {sentence}");
        }
    }

    /// Below 500 an unreadable reply is still just unreadable — there is no
    /// retained-redemption argument to make about it, and only the 5xx
    /// reading moved.
    #[test]
    fn an_unreadable_reply_below_500_is_still_refused() {
        assert!(matches!(
            classify_redeem(400, None),
            RedeemOutcome::Refused(_)
        ));
    }

    /// A 5xx coded with something this bundle predates and a 5xx coded with
    /// nothing at all retain the redemption for the same reason. Neither may
    /// read as a spent code just because no sentence was written for it.
    #[test]
    fn a_server_error_retains_the_code_coded_or_not() {
        let outcome = classify_redeem(500, Some(refusal("brand_new_code")));
        let RedeemOutcome::Retry(coded) = outcome else {
            panic!("a coded 5xx must retain the code, got {outcome:?}");
        };
        assert!(coded.contains("brand_new_code"), "got: {coded}");
        assert!(coded.contains("Nothing was consumed"), "got: {coded}");

        let outcome = classify_redeem(500, Some(body("{}")));
        let RedeemOutcome::Retry(codeless) = outcome else {
            panic!("a codeless 5xx must retain the code, got {outcome:?}");
        };
        assert!(codeless.contains("Nothing was consumed"), "got: {codeless}");
    }

    /// An unknown code still reaches the operator rather than vanishing.
    #[test]
    fn an_unknown_code_still_says_something() {
        let outcome = classify_redeem(418, Some(refusal("brand_new_code")));
        let RedeemOutcome::Refused(sentence) = outcome else {
            panic!("expected Refused, got {outcome:?}");
        };
        assert!(sentence.contains("brand_new_code"), "got: {sentence}");
    }

    // ---- naming the room that was joined ------------------------------------

    /// Property 1: the room key is not on the wire, so it is the DIFF that
    /// names the room. Exactly one new key is the only provable answer.
    #[test]
    fn one_new_key_names_the_joined_room() {
        let before = vec!["alpha".to_string()];
        let after = [room("alpha"), room("warroom")];
        assert_eq!(
            newly_joined_key(&before, &after),
            Some("warroom".to_string())
        );
    }

    /// Redeeming into a room already held creates no row — the daemon takes
    /// its `(Some(_), _) => {}` arm. Nothing new, so nothing to open.
    #[test]
    fn redeeming_a_room_already_held_names_nothing() {
        let before = vec!["alpha".to_string(), "warroom".to_string()];
        let after = [room("alpha"), room("warroom")];
        assert_eq!(newly_joined_key(&before, &after), None);
    }

    /// Two new keys means something else created a room while the redemption
    /// ran. Neither is provably the one redeemed, and opening the wrong room
    /// is worse than not opening one.
    #[test]
    fn a_concurrent_create_makes_the_diff_refuse_to_guess() {
        let before = vec!["alpha".to_string()];
        let after = [room("alpha"), room("warroom"), room("standup")];
        assert_eq!(newly_joined_key(&before, &after), None);
    }

    /// A probe that did not land answers an empty list. It must read as
    /// "cannot say", never as "you joined nothing" — the redemption already
    /// succeeded before the probe ran.
    #[test]
    fn a_failed_probe_names_nothing_rather_than_lying() {
        assert_eq!(newly_joined_key(&["alpha".to_string()], &[]), None);
    }

    /// A blank key is not a room. It must not be handed to `open_room`.
    #[test]
    fn a_blank_key_is_never_the_joined_room() {
        assert_eq!(newly_joined_key(&[], &[room("")]), None);
    }

    // ---- sentences ----------------------------------------------------------

    /// Every redemption lands in `Connecting`, so the success sentence says
    /// the room is joined AND still catching up rather than promising a
    /// transcript that has not arrived.
    #[test]
    fn a_named_join_says_the_room_and_that_it_is_connecting() {
        let sentence = joined_sentence(RoomAccessState::Connecting, Some("warroom"));
        assert!(sentence.contains("warroom"), "got: {sentence}");
        assert!(sentence.contains("Connecting"), "got: {sentence}");
    }

    /// When the diff cannot name the room the sentence points at the list. It
    /// must never invent a key — nothing in a code or a reply carries one.
    #[test]
    fn an_unnamed_join_points_at_the_list() {
        let sentence = joined_sentence(RoomAccessState::Connecting, None);
        assert!(sentence.contains("in your list"), "got: {sentence}");
    }

    /// A state other than `Connecting` is unreachable today, but if the daemon
    /// ever lands one the sentence must not claim a connection is pending.
    #[test]
    fn a_settled_join_does_not_claim_to_be_connecting() {
        let sentence = joined_sentence(RoomAccessState::Live, Some("warroom"));
        assert!(!sentence.contains("Connecting"), "got: {sentence}");
    }

    // ---- request ------------------------------------------------------------

    /// No key in the path — the code is the whole request, and it rides in the
    /// body where it belongs. A code in a URL lands in every access log
    /// between here and the daemon.
    #[test]
    fn the_url_carries_no_code() {
        let url = redeem_url("http://d");
        assert_eq!(url, "http://d/v1/rooms/persistent/invites/redeem");
        assert!(!url.contains(FAKE_CODE));
    }

    // ---- the state ----------------------------------------------------------

    /// A state as `new` leaves it, for the tests that drive one directly.
    /// `Rooms::new` needs a `Daemon` and reads the host through `web_sys`, so
    /// the reactive half is exercised through `publish` rather than `redeem`.
    fn fresh_state() -> RoomRedeemState {
        RoomRedeemState {
            url: RwSignal::new("http://d".to_string()),
            code: RwSignal::new(String::new()),
            note: RwSignal::new(None),
            error: RwSignal::new(None),
            redeeming: RwSignal::new(false),
        }
    }

    /// Property 3 as the operator meets it: a spent code goes off the screen,
    /// and a code the daemon is still holding stays exactly where they can run
    /// it again. Clearing on a retryable refusal would send them hunting for a
    /// code that was never consumed.
    #[test]
    fn a_join_clears_the_code_and_a_retry_keeps_it() {
        let state = fresh_state();
        state.code.set(FAKE_CODE.to_string());
        state.redeeming.set(true);
        state.publish(
            RedeemOutcome::Joined(RoomAccessState::Connecting, None),
            None,
        );
        assert_eq!(state.code.get_untracked(), "");
        assert!(!state.redeeming.get_untracked());

        for outcome in [
            RedeemOutcome::Retry("still settling.".to_string()),
            RedeemOutcome::Refused("spent.".to_string()),
            RedeemOutcome::State(route_absent_sentence()),
        ] {
            let state = fresh_state();
            state.code.set(FAKE_CODE.to_string());
            state.publish(outcome, None);
            assert_eq!(state.code.get_untracked(), FAKE_CODE);
        }
    }

    /// The calm voice and the alarmed one are different slots. A route this
    /// deployment lacks is not the operator's fault and must not shout; a
    /// refusal they have to act on must not hide in a note.
    #[test]
    fn a_state_notes_and_both_refusals_err() {
        let state = fresh_state();
        state.publish(RedeemOutcome::State(route_absent_sentence()), None);
        assert_eq!(state.note.get_untracked(), Some(route_absent_sentence()));
        assert_eq!(state.error.get_untracked(), None);

        for outcome in [
            RedeemOutcome::Retry("still settling.".to_string()),
            RedeemOutcome::Refused("spent.".to_string()),
        ] {
            let state = fresh_state();
            state.publish(outcome, None);
            assert!(state.error.get_untracked().is_some());
            assert_eq!(state.note.get_untracked(), None);
        }
    }

    /// The key off the reply is the one that gets USED, and the diff is only
    /// what answers when there is not one. Pinned from source because
    /// `redeem` needs a `Rooms`: without this, `redeem` could decode
    /// `room_key` and then ignore it in favour of the probe — state written
    /// and never read — and every classify test above would still pass.
    #[test]
    fn the_replys_key_is_used_and_the_diff_is_only_the_fallback() {
        let source = include_str!("room_redeem.rs");
        let at = source
            .find("fn redeem(")
            .expect("the redeem entry point moved");
        let span = &source[at..];
        let end = span
            .find("fn publish(")
            .expect("the redeem body no longer ends at publish");
        let span = &span[..end];
        let names = span
            .find(&["Joined(_, ", "Some(key)) =>"].concat())
            .expect("redeem no longer reads the key off the reply");
        let falls_back = span
            .find(&["Joined(_, ", "None) =>"].concat())
            .expect("the pre-#407 fallback arm is gone");
        let diff = span
            .find(&["newly_joined", "_key(&before"].concat())
            .expect("the fallback no longer diffs the room list");
        assert!(names < falls_back, "the reply's key must be read first");
        assert!(falls_back < diff, "the diff must run only when no key came");
    }

    /// An empty submit must not paint two answers at once. The guard sets
    /// `error` and returns BEFORE the `note.set(None)` every other path runs,
    /// so without a clear of its own it stacks "Paste an invite code first."
    /// on top of a stale "Joined warroom. Connecting…". `redeem` needs a
    /// `Rooms`, which needs a `Daemon` and reads the host through `web_sys`,
    /// so the guard is pinned from source the way the mount guard below is,
    /// sliced to end where the request starts being built so the main path's
    /// own clear cannot answer for it.
    #[test]
    fn an_empty_submit_clears_the_stale_note() {
        let source = include_str!("room_redeem.rs");
        let at = source
            .find("fn redeem(")
            .expect("the redeem entry point moved");
        let guard = &source[at..];
        let end = guard
            .find("let base = self.base();")
            .expect("the guard no longer ends before the request is built");
        let clear = ["self.note", ".set(None)"].concat();
        assert!(
            guard[..end].contains(&clear),
            "an empty submit would leave a stale note under the error"
        );
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

    /// Every refusal here is one the operator has to read — the common one
    /// says their code is dead. A rule that exists but paints nothing would
    /// leave that unsaid while this module believed it had said it.
    #[test]
    fn a_refusal_is_painted() {
        assert!(
            css_rule(".rooms-workspace__redeem-error").contains("color:var(--err)"),
            "the refusal sentence must be legible"
        );
    }

    /// The control has to live in the LEFT rail. Someone holding a code has no
    /// room open and may hold no rooms at all; anywhere in the room surface it
    /// would be invisible to exactly the people it exists for. The needle is
    /// the ELEMENT form: the bare type name is a prefix of the `RoomRedeemState`
    /// binding at component scope, which sits above every rail marker and would
    /// satisfy this on its own with nothing mounted at all.
    #[test]
    fn the_control_is_mounted_in_the_left_rail() {
        let source = include_str!("rooms_workspace.rs");
        let mount = source
            .find("<crate::room_redeem::RoomRedeem ")
            .expect("the redeem control is not mounted anywhere");
        let center = source
            .find("CENTER RAIL")
            .expect("the center-rail marker moved");
        assert!(mount < center, "the redeem control left the left rail");
    }

    /// The mount test proves the element is THERE; this proves the element is
    /// something. Nothing else in the suite reads the rendered half — the
    /// stylesheet tests read only the stylesheet — so a view collapsed to a
    /// bare div would leave every rule written for it styling nothing, and
    /// every one of these tests would still pass. Scoped to the component so
    /// this test's own literals, which live below it, cannot answer for it.
    #[test]
    fn the_view_renders_the_control_the_stylesheet_dresses() {
        let source = include_str!("room_redeem.rs");
        let view_at = source
            .find("pub fn RoomRedeem(")
            .expect("the component moved");
        let tests_at = source.find("mod tests").expect("the test module moved");
        let view = &source[view_at..tests_at];
        for class in [
            "\"rooms-workspace__redeem\"",
            "\"rooms-workspace__redeem-row\"",
            "\"rooms-workspace__left-input\"",
            "\"rooms-workspace__redeem-run\"",
            "\"rooms-workspace__redeem-error\"",
            "\"rooms-workspace__redeem-note\"",
        ] {
            assert!(view.contains(class), "the view stopped emitting {class}");
        }
        assert!(
            view.contains("state.error.get()"),
            "a refusal would render nowhere"
        );
        assert!(
            view.contains("state.note.get()"),
            "a join in flight would say nothing"
        );
    }

    /// The code is a bearer grant to someone else's room and must never reach
    /// a log. Guarded from source in the house style, with the needle
    /// concatenated so this test's own literal cannot match the blob it scans.
    #[test]
    fn the_code_never_reaches_a_log() {
        let source = include_str!("room_redeem.rs");
        let needle = ["log", "::"].concat();
        assert!(
            !source.contains(&needle),
            "an invite code is a bearer grant and must never be logged"
        );
    }

    /// No sentence this module puts on the screen may echo the code back. It
    /// is a bearer grant and the screen is not where it belongs twice.
    ///
    /// Driven through `publish` with the code loaded in the state, rather than
    /// over the four sentence builders directly. None of them takes the invite
    /// code as an argument, so handing one a code proves only that a `format!`
    /// which could never exist does not exist. `publish` is the one place
    /// where a finished sentence and the signal holding the code are BOTH in
    /// scope — a "joined with <code>" confirmation would be written there, and
    /// that is what this catches.
    #[test]
    fn no_sentence_reaching_the_screen_can_carry_the_code() {
        let replies = [
            (
                200,
                Some(r#"{"state": "connecting", "room_key": "warroom"}"#),
            ),
            (200, Some(r#"{"state": "connecting"}"#)),
            (200, Some("{}")),
            (404, None),
            (404, Some("{}")),
            (409, Some(r#"{"error": "federation_conflict"}"#)),
            (503, Some(r#"{"error": "federation_unavailable"}"#)),
            (500, Some(r#"{"error": "internal_error"}"#)),
            (400, Some(r#"{"error": "invalid_request"}"#)),
            (403, Some(r#"{"error": "invite_forbidden"}"#)),
            (403, Some(r#"{"error": "federation_forbidden"}"#)),
            (502, Some(r#"{"error": "federation_protocol"}"#)),
            (418, Some(r#"{"error": "brand_new_code"}"#)),
            (502, None),
        ];
        for (status, json) in replies {
            let outcome = classify_redeem(status, json.map(body));
            let key = match &outcome {
                RedeemOutcome::Joined(_, key) => key.clone(),
                _ => None,
            };
            let state = fresh_state();
            state.code.set(FAKE_CODE.to_string());
            state.publish(outcome, key.as_deref());
            for slot in [state.note.get_untracked(), state.error.get_untracked()] {
                let Some(sentence) = slot else { continue };
                assert!(!sentence.contains(FAKE_CODE), "{status} got: {sentence}");
            }
        }
    }
}
