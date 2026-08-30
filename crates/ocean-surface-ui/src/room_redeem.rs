//! Redeeming a room invite — the browser half of the daemon's redeem route.
//!
//! One call, and it is the only way this browser joins a room it was not
//! already in:
//!
//!   POST /v1/rooms/persistent/invites/redeem  {code}  → 200 RoomAccessProjection
//!
//! It is the mirror of [`crate::room_invite`], and four properties of its wire
//! contract make it a different animal from minting:
//!
//! 1. **The 200 carries NO room key.** The body is a serialized
//!    `RoomAccessProjection` — `{state, last_confirmed_global_sequence?,
//!    members?, outbox?}` — and nothing else. The daemon knows the key (it
//!    derives it from the invite's scope and calls `store.create(key, …)`) and
//!    drops it on the way out. So a successful redemption cannot say which
//!    room it joined, and this module must not invent one: a code is opaque
//!    and no key can be read out of it. What it does instead is snapshot the
//!    room list before the request and diff it after — [`newly_joined_key`],
//!    pure and provable — and open the room only when exactly one appeared.
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
/// landing state. `error` is the machine code `intent_error_response` writes
/// into `{ok:false, error:"<code>"}`. Neither may be read without first
/// knowing which of the two replies arrived.
#[derive(Debug, Default, Deserialize)]
struct RedeemBody {
    #[serde(default)]
    state: Option<RoomAccessState>,
    #[serde(default)]
    error: Option<String>,
}

/// The room list, read for its keys alone. Its own envelope rather than
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
    /// The room is on this daemon now. Carries the access state it landed in.
    Joined(RoomAccessState),
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

/// The one room that appeared while the redemption ran.
///
/// `None` when the answer is not unambiguous — nothing new (redeeming into a
/// room already held takes the daemon's `(Some(_), _) => {}` arm and creates
/// no row), or more than one (something else created a room concurrently).
/// Neither is provably the room redeemed, and opening the wrong one is worse
/// than saying "it's in your list".
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

/// The sentence a deployment without the route earns.
fn route_absent_sentence() -> String {
    "Joining by invite code isn't available on this deployment yet.".to_string()
}

/// Map a redeem reply onto what the rail should show. `body` is `None` when
/// the reply did not decode, which a route-less deployment produces (an empty
/// 404), so that case is an ANSWER here rather than a transport fault.
///
/// Success is settled on the status and a present `state` before any refusal
/// code is consulted, for the same reason mint settles on a present `code`:
/// the two replies share one envelope and only one of them holds each field.
fn classify_redeem(status: u16, body: Option<RedeemBody>) -> RedeemOutcome {
    let Some(body) = body else {
        if status == 404 {
            return RedeemOutcome::State(route_absent_sentence());
        }
        return RedeemOutcome::Refused(format!("The redeem reply could not be read ({status})."));
    };
    if (200..300).contains(&status) {
        if let Some(state) = body.state {
            return RedeemOutcome::Joined(state);
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
            RedeemOutcome::Refused(
                refused_sentence(code).unwrap_or_else(|| format!("The invite was refused: {code}")),
            )
        }
        // A 404 with no code is the daemon's unknown-route answer. Property 2:
        // the route itself has no 404, so this is the only 404 it can produce
        // and there is no room-missing reading to confuse it with.
        None if status == 404 => RedeemOutcome::State(route_absent_sentence()),
        None => RedeemOutcome::Refused(format!("The redemption failed ({status}).")),
    }
}

/// What a redeemed room reads as. `key` is `None` when the diff could not name
/// the room — see [`newly_joined_key`] — and the sentence then points at the
/// list instead of at a key it does not have.
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
            return;
        }
        let base = self.base();
        let me = *self;
        // Snapshotted BEFORE the request goes out: the diff that names the
        // joined room is only sound against the list as it stood when the
        // redemption started.
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
            let joined = matches!(outcome, RedeemOutcome::Joined(_));
            // Property 1: the reply cannot say which room this was, so ask the
            // list. Its own request because `Rooms::fetch_rooms` is
            // fire-and-forget and a diff needs an await point.
            let key = match joined {
                true => newly_joined_key(&before, &fetch_room_list(&base).await),
                false => None,
            };
            me.publish(outcome, key.as_deref());
            if joined {
                // The canonical refresh, which the probe above deliberately is
                // not: this is what merges read summaries and drives the
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
            RedeemOutcome::Joined(state) => {
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

/// The daemon's room list, for the diff alone. Every failure answers an empty
/// list, which [`newly_joined_key`] reads as "cannot say" — the redemption has
/// already succeeded by the time this runs, and a probe that did not land must
/// not turn that into an error.
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

    /// The 200 is a bare `RoomAccessProjection` — no `{ok:true}` envelope and
    /// no `invite` key. A decoder copied from the artifacts or workspace lanes
    /// fails exactly here.
    #[test]
    fn the_ok_reply_is_a_bare_access_projection() {
        let outcome = classify_redeem(
            200,
            Some(body(
                r#"{"state": "connecting", "members": [], "outbox": []}"#,
            )),
        );
        assert_eq!(outcome, RedeemOutcome::Joined(RoomAccessState::Connecting));
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
        state.publish(RedeemOutcome::Joined(RoomAccessState::Connecting), None);
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

    /// No sentence this module can produce may echo the code back. It is a
    /// bearer grant and the screen is not where it belongs twice.
    #[test]
    fn no_sentence_can_carry_the_code() {
        let sentences = [
            joined_sentence(RoomAccessState::Connecting, Some("warroom")),
            joined_sentence(RoomAccessState::Connecting, None),
            route_absent_sentence(),
        ]
        .into_iter()
        .chain(
            [
                "federation_conflict",
                "federation_unavailable",
                "internal_error",
            ]
            .into_iter()
            .filter_map(retry_sentence),
        )
        .chain(
            [
                "invalid_request",
                "invite_forbidden",
                "federation_forbidden",
                "federation_protocol",
            ]
            .into_iter()
            .filter_map(refused_sentence),
        );
        for sentence in sentences {
            assert!(!sentence.contains(FAKE_CODE), "got: {sentence}");
        }
    }
}
