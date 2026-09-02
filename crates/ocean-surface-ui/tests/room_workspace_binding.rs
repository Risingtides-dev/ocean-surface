//! A room the surface creates must be able to wake its agents.
//!
//! The defect this guards against is a field that is simply not there. The
//! daemon has accepted `workspace_root` on `POST /v1/rooms/persistent` since
//! OCEAN-260, and a room without one is unbound: the daemon resolves a
//! room-bound agent turn's project and `cwd` from that binding, and with none
//! stored it refuses every turn with `workspace_unavailable` before the agent
//! sees the message. The surface sent `key`, `name` and `trigger_policy` only,
//! so every room this product created was unbound and every agent mention in
//! one did nothing at all.
//!
//! Nothing in the compiler holds a field's PRESENCE in a serialized body: drop
//! `workspace_root` from either struct and the create still posts, the PATCH
//! still 200s, the room still opens, and the whole feature is silently back to
//! doing nothing. `cargo test` would stay green — which is exactly the failure
//! this directory exists for, and why these two are pinned by source scan.
//!
//! Both needles run over `view_source`, so a unit test quoting the same field
//! name cannot satisfy them, and both name the struct that carries the field
//! rather than the bare word: `workspace_root` appears all over this crate
//! meaning the unrelated SESSION workspace root.

mod common;

use common::{view_source, without_whitespace};

/// The create body must carry the field. Named on `CreateRoomBody` itself, so
/// a rename of the struct or a deletion of the field both trip it.
#[test]
fn the_create_body_carries_a_workspace_root() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    assert!(
        rooms.contains("structCreateRoomBody<'a>{"),
        "`CreateRoomBody` is the create wire body; if it was renamed, repoint \
         this guard rather than deleting it",
    );
    let body = rooms
        .split_once("structCreateRoomBody<'a>{")
        .expect("checked above")
        .1
        .split_once('}')
        .expect("the struct is brace-delimited")
        .0;
    assert!(
        body.contains("workspace_root:Option<&'astr>"),
        "`CreateRoomBody` must carry `workspace_root`, or every room this \
         surface creates is unbound and its agents can never run (the field \
         was missing entirely until Rooms 1.4)",
    );
    assert!(
        body.contains("skip_serializing_if=\"Option::is_none\""),
        "an absent binding must be an absent KEY on the wire, not a null",
    );
}

/// The PATCH body must carry it too, and must NOT skip `None` — an omitted
/// field is what the daemon reads as "leave the binding alone", so a skipped
/// `None` turns the unbind control into a no-op that reports success.
#[test]
fn the_workspace_patch_body_carries_a_nullable_workspace_root() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    assert!(
        rooms.contains("structRoomWorkspacePatchBody<'a>{"),
        "`RoomWorkspacePatchBody` is the bind/unbind wire body; if it was \
         renamed, repoint this guard rather than deleting it",
    );
    let body = rooms
        .split_once("structRoomWorkspacePatchBody<'a>{")
        .expect("checked above")
        .1
        .split_once('}')
        .expect("the struct is brace-delimited")
        .0;
    assert!(
        body.contains("workspace_root:Option<&'astr>"),
        "the PATCH body must carry `workspace_root`",
    );
    assert!(
        !body.contains("skip_serializing_if"),
        "`None` here MUST serialize as an explicit null: the daemon leaves an \
         absent field unchanged, so skipping it would make every unbind a \
         request that changes nothing and still answers 200",
    );
}

/// The route the two controls call. A bind that never dispatches is the same
/// defect in a later place.
#[test]
fn the_binding_controls_patch_the_room_route() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    assert!(
        rooms.contains("fnset_open_room_workspace(&self,workspace_root:Option<String>)"),
        "`Rooms::set_open_room_workspace` is what the bind/unbind buttons call",
    );
    assert!(
        rooms.contains("RoomWorkspacePatchBody{workspace_root:workspace_root.as_deref(),}"),
        "the binding PATCH must send `RoomWorkspacePatchBody`, so an unbind \
         carries its explicit null",
    );

    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));
    assert!(
        workspace.contains("on:click=move|_|rooms.set_open_room_workspace(None)"),
        "the unbind button's click must dispatch the unbind; the arming half \
         of a control is the shape that goes missing without warning anything",
    );
    assert!(
        workspace.contains(
            "rooms.set_open_room_workspace(create_workspace_root(&draft.get_untracked()),);"
        ),
        "the bind button's click must dispatch the trimmed draft",
    );
    assert!(
        workspace.contains("create_workspace_root(&create_workspace.get_untracked()),"),
        "the create form must pass its workspace field to `create_room`, or a \
         room created through this form is unbound however the field is filled",
    );
}

/// The unbound notice must say what an operator can act on: that agents cannot
/// run, and that the folder lives on the daemon's machine rather than theirs.
#[test]
fn the_unbound_notice_names_the_consequence_and_the_host() {
    let workspace = view_source("rooms_workspace.rs");
    let compact = without_whitespace(&workspace);
    assert!(
        compact.contains("Noworkspacefolderisbound."),
        "the unbound room notice must survive; without it an operator sees \
         four armed triggers over a room that refuses every turn",
    );
    assert!(
        compact.contains("Agentsinthisroomcannotrun"),
        "the notice must name the consequence, not just the missing value",
    );
    assert!(
        compact.contains("machinerunningthedaemon"),
        "both the create field's helper text and the binding section must say \
         whose filesystem the path is resolved on — the browser cannot see it",
    );
}
