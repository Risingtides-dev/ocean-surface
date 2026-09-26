//! The agent stream's reset-frame handling, which nothing else holds.
//!
//! ocean-os publishes an `event: error` reset frame on `GET /v1/agent/events`
//! (`agent_events_error`: `live_lag`, `anchor_unavailable`, `malformed_anchor`)
//! and a client must resync on every one of them. The frame shares its name
//! with the DOM's connection-error event, so gloo reports it as a transport
//! `ConnectionError`; `crate::agent_events_reset` tells the two apart and
//! `Daemon::connect` leaves the connection on a reset. Its pure decision is
//! unit-tested there. What those tests cannot see is the wiring: whether the
//! stream loop subscribes to the frame at all, reads it before gloo's errors,
//! and acts on the decision. Same lane and discipline as
//! `unheld_room_controls.rs`: each mutation below was applied for real on this
//! branch with the full UI gate (`cargo test`, both clippies) run against it.
//!
//! ## Measured
//!
//! | Mutation | Result |
//! |---|---|
//! | delete the reset subscription + its arm in `connect` | RED — compiler-held |
//! | `Resync(..) => break` becomes `continue` | GREEN — pinned |
//! | `PollNext::Left` becomes `PollNext::Right` | GREEN — pinned |
//! | `AGENT_EVENTS_RESET_EVENT` renamed off `"error"` | GREEN — pinned |
//! | `Transport => continue` becomes `break` | GREEN — pinned |
//! | `reset_frame_data` loses its `MessageEvent` type check * | GREEN — pinned |
//!
//! \* Deleting the check alone is RED only by accident: it takes the file's
//! one `JsCast` use with it, so the import warns. Measured again with the
//! import removed too, as a reviewer would, it is GREEN.
//!
//! The held one is held only because deleting it takes the module's helpers
//! dead under the wasm clippy's `-D warnings`. Every silent one leaves all of
//! that referenced and changes what a reset DOES: `continue` reads the frame
//! and drops it, `Right` lets gloo's `ConnectionError` win the race so the
//! frame is handled as a transport error and never decoded, a rename listens
//! for a frame the daemon never sends, `break` on `Transport` tears the
//! connection down on every browser auto-reconnect, and without the type check
//! every transport error decodes as a malformed reset.
//!
//! Needles run over production source only, so the modules' own tests cannot
//! satisfy them, with whitespace stripped so rustfmt may rewrap freely.

mod common;

use common::{src, view_source, without_whitespace};

/// `daemon.rs` minus its bottom test module. `view_source` cannot be used
/// here: the file has an earlier item-level `#[cfg(test)]` (line ~1056), and
/// splitting on that would cut `connect` off entirely.
fn connect_loop() -> String {
    let source = src("daemon.rs");
    let (production, _) = source
        .split_once("\n#[cfg(test)]\nmod tests {")
        .expect("daemon.rs carries its unit tests at the bottom");
    without_whitespace(production)
}

#[test]
fn the_agent_stream_subscribes_to_the_daemon_reset_frame() {
    assert!(
        connect_loop().contains("es.subscribe(AGENT_EVENTS_RESET_EVENT)"),
        "`connect` must subscribe to the daemon's reset frame; without it a \
         `live_lag` / `anchor_unavailable` reset is never decoded",
    );
    assert!(
        without_whitespace(&view_source("agent_events_reset.rs"))
            .contains("pub(crate)constAGENT_EVENTS_RESET_EVENT:&str=\"error\";"),
        "the daemon writes its reset frame as `event: error`; any other name \
         subscribes to a frame that never arrives",
    );
}

#[test]
fn the_reset_frame_is_read_before_gloo_connection_errors() {
    assert!(
        connect_loop().contains(
            "select_with_strategy(reset_sub.map(AgentStreamItem::Reset),\
             futures_util::stream::select_all(subs).map(AgentStreamItem::Event),\
             |_:&mut()|futures_util::stream::PollNext::Left,)"
        ),
        "one reset frame also queues a gloo `ConnectionError` on every \
         subscription; the reset stream must be polled first or the frame \
         is never decoded",
    );
}

#[test]
fn a_reset_frame_leaves_the_connection_and_a_transport_error_does_not() {
    let view = connect_loop();
    assert!(
        view.contains("decide_agent_stream_error(reset_frame_data(&frame).as_deref())"),
        "the reset listener must decide on the event it received",
    );
    assert!(
        view.contains(
            "AgentStreamErrorAction::Resync(cause)=>{log::warn!(\
             \"agentstreamresetbydaemon,resyncing:{}\",describe_reset(&cause));break;}"
        ),
        "a reset must LEAVE the connection so the reconnect path re-commits \
         the projection from a fresh anchor; reading it and staying is the bug",
    );
    assert!(
        view.contains("AgentStreamErrorAction::Transport=>continue,"),
        "a plain transport error must fall through to gloo's existing \
         handling; breaking on it drops the connection on every browser \
         auto-reconnect",
    );
}

#[test]
fn only_a_message_event_counts_as_a_reset_frame() {
    assert!(
        without_whitespace(&view_source("agent_events_reset.rs"))
            .contains("if!raw.is_instance_of::<web_sys::MessageEvent>(){returnNone;}"),
        "gloo passes a plain transport `Event` to the same listener; without \
         the type check every connection error decodes as a malformed reset",
    );
}
