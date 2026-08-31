//! Source guards for the Phase 1 Room-agent authority boundary.
//!
//! The legacy bare participant picker performed `rooms.add_agent(package)`
//! directly from the package catalogue. That skips the server-derived preview,
//! digest confirmation, owner proof, policy selection, and durable binding
//! ceremony. The compiler does not hold its removal: both paths can coexist and
//! compile, leaving the unsafe shortcut one innocent refactor away from return.

mod common;

use common::{read, view_source, without_whitespace};

#[test]
fn the_legacy_bare_agent_picker_stays_removed() {
    let view = without_whitespace(&view_source("rooms_workspace.rs"));
    for legacy in [
        "rooms-workspace-agent-picker",
        "show_add_agent",
        "rooms.add_agent(",
    ] {
        assert!(
            !view.contains(legacy),
            "legacy bare Room-agent picker path `{legacy}` must not return; all local agent authority goes through the reviewed binding ceremony",
        );
    }
    assert!(
        view.contains("<crate::room_agent_authorization::RoomAgentAuthorizationPanel"),
        "the Room members rail must retain the reviewed authorization ceremony after the bare picker removal",
    );
    assert!(
        !read("styles/rooms-workspace.css").contains("authority-memory-note"),
        "the obsolete unavailable-room-memory notice has no emitter and its dead selector must not return",
    );

    let authority = read("crates/ocean-surface-ui/src/room_agent_authorization.rs");
    assert!(
        !authority.contains("/{}/participants"),
        "first-agent setup must never restore the unauthenticated participant POST; local bootstrap belongs to the operator-authenticated daemon route",
    );
    assert!(
        authority.contains("/{}/agents/bootstrap"),
        "the reviewed server-proven first-agent bootstrap route must remain wired",
    );
}
