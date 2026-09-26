//! The daemon's reset frame on `GET /v1/agent/events`, told apart from a
//! transport error.
//!
//! ocean-os publishes it as `agent_events_error` in
//! `docs/contracts/session-wire.json`: an SSE frame named `event: error` whose
//! `data:` is a typed `AgentReplayGap` (`ocean-core`) —
//! `{code, requested_event_id?, oldest_available_event_id?,
//! newest_available_event_id?, reset_required}` with `code` one of
//! `anchor_unavailable`, `live_lag`, `malformed_anchor`. Every one of them
//! means the same thing to a client: the live tail has a hole the daemon will
//! not fill, so re-fetch the session from a fresh anchor. The daemon keeps the
//! stream open after a `live_lag` frame; it is the client's job to act.
//!
//! The SSE event name collides with the DOM's own connection-error event: an
//! `event: error` frame is dispatched to every `"error"` listener on the
//! `EventSource`, exactly like a dropped connection. The two differ in shape —
//! the reset frame is a `MessageEvent` carrying `.data`, the transport error is
//! a plain `Event` with none — and in nothing else gloo exposes. gloo-net's
//! per-subscription `"error"` listener sees both as
//! `EventSourceError::ConnectionError`, so the only way to hold the reset
//! frame is to read the raw event and check its type ([`reset_frame_data`]),
//! then decide on the text alone ([`decide_agent_stream_error`]).

use serde::Deserialize;
use wasm_bindgen::JsCast;

/// The SSE `event:` name the daemon writes its reset frame under
/// (`Event::default().event("error")` in the `agent_events` handler).
pub(crate) const AGENT_EVENTS_RESET_EVENT: &str = "error";

/// `AgentReplayGapCode`, as published. An unrecognized code decodes to
/// [`AgentResetCode::Unknown`] rather than failing, and is resynced like the
/// rest: a code the daemon grows later still arrived under the reset frame's
/// name, so the safe reading is "the tail has a hole".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentResetCode {
    AnchorUnavailable,
    LiveLag,
    MalformedAnchor,
    #[serde(other)]
    Unknown,
}

/// The published `agent_events_error` body. The id bounds are diagnostic
/// opaque ids (the daemon says so); they are logged, never used as an anchor.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct AgentResetFrame {
    pub code: AgentResetCode,
    #[serde(default)]
    pub requested_event_id: Option<String>,
    #[serde(default)]
    pub oldest_available_event_id: Option<String>,
    #[serde(default)]
    pub newest_available_event_id: Option<String>,
    // Carried for the log line. The daemon only ever sends `true`; a `false`
    // is still resynced, because it came in under the reset frame's name.
    #[serde(default)]
    pub reset_required: bool,
}

/// Why the agent stream is being resynced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentResetCause {
    /// A frame that decoded against the published shape (its code may still
    /// be [`AgentResetCode::Unknown`]).
    Frame(AgentResetFrame),
    /// A data-bearing `error` frame that did not decode at all.
    Malformed(String),
}

/// What the agent stream loop does with one `"error"` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AgentStreamErrorAction {
    /// A daemon reset frame: leave this connection and take the reconnect
    /// path, which re-commits the session projection and opens a fresh
    /// `EventSource` (so no stale `Last-Event-ID` rides along).
    Resync(AgentResetCause),
    /// A plain transport error: not this module's business. The existing
    /// gloo `ConnectionError` handling stays in charge of it.
    Transport,
}

/// One item of the agent stream loop's merged stream: the reset-frame
/// subscription's, or one of the `AGENT_EVENT_NAMES` subscriptions'.
pub(crate) enum AgentStreamItem<T> {
    Reset(T),
    Event(T),
}

/// Decide what one `"error"` event means, from its data alone: `None` is a
/// plain transport `Event`, anything else came from the daemon.
pub(crate) fn decide_agent_stream_error(data: Option<&str>) -> AgentStreamErrorAction {
    let Some(data) = data else {
        return AgentStreamErrorAction::Transport;
    };
    AgentStreamErrorAction::Resync(match serde_json::from_str::<AgentResetFrame>(data) {
        Ok(frame) => AgentResetCause::Frame(frame),
        Err(_) => AgentResetCause::Malformed(data.to_string()),
    })
}

/// The reset frame's text, or `None` for a transport error.
///
/// gloo hands every `"error"` listener's argument over as a `MessageEvent`
/// without checking, so the type is checked here: only a real `MessageEvent`
/// (the SSE frame) has data. A transport error, and the synthetic `Event`
/// gloo dispatches when an `EventSource` is dropped, are plain `Event`s.
pub(crate) fn reset_frame_data(event: &web_sys::MessageEvent) -> Option<String> {
    let raw: &wasm_bindgen::JsValue = event.as_ref();
    if !raw.is_instance_of::<web_sys::MessageEvent>() {
        return None;
    }
    Some(event.data().as_string().unwrap_or_default())
}

/// A one-line description of a resync for the log.
pub(crate) fn describe_reset(cause: &AgentResetCause) -> String {
    match cause {
        AgentResetCause::Frame(frame) => format!(
            "{:?} (reset_required={}, requested={:?}, oldest={:?}, newest={:?})",
            frame.code,
            frame.reset_required,
            frame.requested_event_id,
            frame.oldest_available_event_id,
            frame.newest_available_event_id,
        ),
        AgentResetCause::Malformed(data) => format!("malformed reset frame: {data}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(code: AgentResetCode) -> AgentStreamErrorAction {
        AgentStreamErrorAction::Resync(AgentResetCause::Frame(AgentResetFrame {
            code,
            requested_event_id: None,
            oldest_available_event_id: None,
            newest_available_event_id: None,
            reset_required: true,
        }))
    }

    #[test]
    fn a_transport_error_is_left_to_the_existing_path() {
        assert_eq!(
            decide_agent_stream_error(None),
            AgentStreamErrorAction::Transport
        );
    }

    #[test]
    fn every_published_reset_code_resyncs() {
        // The exact bodies the daemon writes (`serde_json::to_string` of
        // `AgentReplayGap`, and the hand-written fallback for `live_lag`).
        assert_eq!(
            decide_agent_stream_error(Some(r#"{"code":"live_lag","reset_required":true}"#)),
            frame(AgentResetCode::LiveLag)
        );
        assert_eq!(
            decide_agent_stream_error(Some(
                r#"{"code":"malformed_anchor","requested_event_id":"not-a-uuid","reset_required":true}"#
            )),
            AgentStreamErrorAction::Resync(AgentResetCause::Frame(AgentResetFrame {
                code: AgentResetCode::MalformedAnchor,
                requested_event_id: Some("not-a-uuid".into()),
                oldest_available_event_id: None,
                newest_available_event_id: None,
                reset_required: true,
            }))
        );
        let oldest = "0199a1b2-0000-7000-8000-000000000001";
        let newest = "0199a1b2-0000-7000-8000-000000000002";
        let requested = "0199a1b2-0000-7000-8000-000000000000";
        let body = format!(
            r#"{{"code":"anchor_unavailable","requested_event_id":"{requested}","oldest_available_event_id":"{oldest}","newest_available_event_id":"{newest}","reset_required":true}}"#
        );
        assert_eq!(
            decide_agent_stream_error(Some(&body)),
            AgentStreamErrorAction::Resync(AgentResetCause::Frame(AgentResetFrame {
                code: AgentResetCode::AnchorUnavailable,
                requested_event_id: Some(requested.into()),
                oldest_available_event_id: Some(oldest.into()),
                newest_available_event_id: Some(newest.into()),
                reset_required: true,
            }))
        );
    }

    #[test]
    fn an_unknown_code_fails_safe_by_resyncing() {
        // Includes the daemon's own serialize-failure fallback body.
        assert_eq!(
            decide_agent_stream_error(Some(r#"{"code":"replay_gap","reset_required":true}"#)),
            frame(AgentResetCode::Unknown)
        );
    }

    #[test]
    fn malformed_data_fails_safe_by_resyncing() {
        for data in ["", "not json", "{}", r#"{"code":7}"#, "null"] {
            assert_eq!(
                decide_agent_stream_error(Some(data)),
                AgentStreamErrorAction::Resync(AgentResetCause::Malformed(data.into())),
                "{data:?}"
            );
        }
    }
}
