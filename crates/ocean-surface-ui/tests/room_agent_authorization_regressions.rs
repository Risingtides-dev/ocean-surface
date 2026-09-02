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

/// Every privileged mutation leaves the ceremony through ONE transport seam.
///
/// The compiler holds half of this: `AuthorityRoute` is constructed only by the
/// four route builders and `send_authority_mutation` is its only consumer, so a
/// mutation cannot be addressed by a path the transport did not choose. What
/// the compiler does NOT hold is a call site that stops using the route type —
/// re-inlining a bare `Request::post(&format!("{base}/v1/rooms/…"))` compiles
/// perfectly, sends no credential, and on the desktop shell fails as an opaque
/// daemon 401 that reads exactly like a permissions problem. That is the
/// regression this scan is for.
///
/// Three verbs, three call sites: bootstrap, authorize/reauthorize, and the
/// status mutations (suspend/resume/revoke). The single remaining
/// `Request::post` is the federated membership registration, which is on no
/// allowlist and deliberately credential-free.
#[test]
fn every_privileged_mutation_goes_through_the_one_transport_seam() {
    let view = view_source("room_agent_authorization.rs");

    // Everything below the seam's own body is call-site territory: the
    // ceremony's methods and its view. Scanning it rather than the whole
    // module is what keeps the seam's two `Request` builders — the browser
    // half of the transport — from satisfying an assertion about call sites.
    let seam_start = view
        .find("async fn send_authority_mutation(")
        .expect("the ceremony must define its one transport seam");
    let seam_end = seam_start
        + view[seam_start..]
            .find("\n}\n")
            .expect("the seam is a function and functions end")
        + 3;
    let (seam, call_sites) = (&view[seam_start..seam_end], &view[seam_end..]);

    assert!(
        seam.contains("room_authority_native_transport()")
            && seam.contains("crate::host::daemon_operator_request("),
        "the seam is where the native transport is chosen",
    );
    assert_eq!(
        view.matches("room_authority_native_transport()").count(),
        1,
        "the native/browser transport branch belongs to the seam alone, so a \
         reader has one place to look for what changes on the desktop",
    );
    assert_eq!(
        call_sites.matches("send_authority_mutation(").count(),
        3,
        "expected exactly three privileged call sites (bootstrap, \
         authorize/reauthorize, status); a new mutation must route through the \
         seam, not around it",
    );
    assert!(
        !call_sites.contains("Request::delete("),
        "revoke is the authority allowlist's only DELETE and it belongs to the \
         seam; a bare Request::delete here sends no operator credential",
    );
    assert_eq!(
        call_sites.matches("Request::post(").count(),
        1,
        "the only credential-free POST left in the ceremony is the federated \
         membership registration; every other POST is a privileged mutation and \
         must go through send_authority_mutation",
    );
    assert!(
        call_sites.contains("/members/agents"),
        "that one permitted Request::post is the federated membership route — if \
         it is gone, the count above is pinning the wrong call",
    );
}

/// The Tauri shell is what makes the desktop ceremony writable: the command,
/// its registration, its allowlist, and the capability/scheme wiring the deep
/// link and notification halves also stand on. None of it is reachable from
/// this crate's compiler, so it is scanned.
#[test]
fn the_native_shell_carries_the_privileged_transport() {
    let shell = read("crates/ocean-tauri/src/lib.rs");
    assert!(
        shell.contains("async fn daemon_operator_request("),
        "the shell must define the privileged room-authority forwarder",
    );
    assert!(
        shell.contains("daemon_operator_request\n        ]")
            || without_whitespace(&shell).contains("daemon_operator_request]"),
        "the forwarder must be registered in generate_handler! or the webview \
         cannot invoke it",
    );
    assert!(
        shell.contains("fn room_agent_authority_mutation("),
        "the shell must re-check the six-route allowlist itself: Tauri 2 \
         capabilities do not gate generate_handler! commands, so the allowlist \
         IS the boundary",
    );
    assert!(
        shell.contains("fn read_room_operator_key("),
        "the shell must read the operator credential under its own custody check",
    );
    let dense = without_whitespace(&shell);
    // Dot segments must be judged on the DECODED segment. `%2e%2e` is not
    // `..` to a string comparison but is to the URL parser, which collapses
    // the segment and would carry the credential to a route the allowlist
    // never approved. Two independent holds, because this is the boundary:
    // the decode-aware pre-check, and a post-parse equality check that makes
    // the whole normalisation class inert rather than just this instance.
    // `crates/ocean-tauri` is ungated by CI, so this scan is the only place
    // the repo's own gate sees either of them.
    assert!(
        dense.contains("fnis_dot_segment(") && dense.contains(".any(is_dot_segment)"),
        "the shell must test dot segments after percent-decoding them",
    );
    assert!(
        dense.contains("ifparsed.path()!=path{"),
        "the shell must refuse a path that does not survive URL parsing, so a \
         future normalisation rule cannot rewrite an allowlisted route",
    );
    assert!(
        dense.contains("metadata.mode()&0o777!=0o600")
            && dense.contains("metadata.nlink()!=1")
            && dense.contains("metadata.uid()!=owner")
            && dense.contains("custom_flags(libc::O_NOFOLLOW)"),
        "the custody check must keep all four conditions the proxy enforces: \
         mode 0600, a single link, the calling owner, and no symlink follow",
    );
    // The reply type is the whole surface of what comes back from the command.
    // Two fields, both from the daemon's answer — never the credential.
    assert!(
        dense.contains("structOperatorReplyDto{status:u16,body:String,}"),
        "the command reply must carry the daemon's status and body and nothing \
         else; the operator credential never crosses back into the webview",
    );
}
