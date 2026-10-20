//! Guards the room working-directory chooser as a daemon-catalogue projection.
//!
//! This crate is binary-only, so integration guards inspect release source. Pure
//! option flattening is covered beside the production helper in
//! `working_directory.rs`.

mod common;

use common::{view_source, without_whitespace};

#[test]
fn rooms_share_the_daemon_project_signal_without_fetching_again() {
    let rooms = without_whitespace(&view_source("rooms.rs"));
    assert!(
        rooms.contains("pubprojects:RwSignal<Vec<crate::daemon::ProjectInfo>>"),
        "Rooms must expose the daemon-owned project catalogue to room UI",
    );
    assert!(
        rooms.contains("projects:daemon.projects"),
        "Rooms must share Daemon::projects rather than snapshotting or refetching it",
    );

    let picker = view_source("working_directory.rs");
    assert!(
        !picker.contains("Request::get") && !picker.contains("/v1/projects"),
        "the working-directory control must not issue a second project request",
    );
}

#[test]
fn create_and_details_use_the_same_picker_and_existing_room_routes() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));
    assert_eq!(
        workspace.matches("<WorkingDirectoryPicker").count(),
        2,
        "room creation and room details must use one shared directory control",
    );
    assert!(
        workspace.contains("projects=rooms.projectsvalue=create_workspace"),
        "create must consume the shared catalogue and retain its create draft",
    );
    assert!(
        workspace.contains("projects=rooms.projectsvalue=draft"),
        "details must consume the shared catalogue and retained binding draft",
    );
    assert!(
        workspace.contains("create_workspace_root(&create_workspace.get_untracked()),"),
        "create must still send the existing optional workspace_root field",
    );
    assert!(
        workspace.contains(
            "rooms.set_open_room_workspace(create_workspace_root(&draft.get_untracked()),);"
        ),
        "details must still use the guarded workspace PATCH dispatcher",
    );
}

#[test]
fn picker_offers_registered_roots_and_an_explicit_manual_path() {
    let picker = without_whitespace(&view_source("working_directory.rs"));
    assert!(
        picker.contains("forprojectinprojects")
            && picker.contains("forworktreein&project.worktrees"),
        "the picker must include both registered project and worktree roots",
    );
    assert!(
        picker.contains("Otherabsolutepath…")
            && picker.contains("value.set(event_target_value(&ev))"),
        "a manual absolute-path fallback must remain available without a browser filesystem API",
    );
    assert!(
        picker.contains("!directory_is_registered(&projects.get(),&value.get())"),
        "a stored room binding absent from the catalogue must remain visible and editable",
    );
    assert!(
        !picker.contains("showDirectoryPicker") && !picker.contains("webkitdirectory"),
        "browser filesystem APIs are not authority for the daemon host",
    );
}

/// The binding contract is unchanged: an unbound room cannot execute agent
/// turns, and the folder lives on the DAEMON's host. The picker must not soften
/// either statement, and must not pre-validate a path in the browser.
#[test]
fn unbound_copy_stays_truthful_about_refusal_and_the_daemon_host() {
    let workspace = view_source("rooms_workspace.rs");
    // String continuations (`\\` + newline) leave a backslash once whitespace is
    // stripped; drop it so the copy is compared as the operator reads it.
    let flat = without_whitespace(&workspace).replace('\\', "");
    assert!(
        flat.contains(&without_whitespace(
            "No workspace folder is bound. Agents in this room cannot run until one is"
        )) && flat.contains(&without_whitespace(
            "every turn is refused before it starts"
        )),
        "the details notice must state that an unbound room's agent turns are refused",
    );
    assert!(
        flat.contains(&without_whitespace(
            "its agents cannot run until a folder is bound"
        )),
        "the create form must still say an unbound room's agents cannot run",
    );
    assert!(
        flat.contains(&without_whitespace(
            "resolved on the machine running the daemon, not in this browser"
        )),
        "the daemon-host explanation must remain on the binding section",
    );
    assert!(
        flat.contains(&without_whitespace("registered on the daemon's machine")),
        "picker help must name the daemon's machine as the owner of the path",
    );
    for banned in [
        "resource grants may still supply",
        "resource grants may supply",
        "No room default",
        "Use as room default",
        "Clear default",
    ] {
        assert!(
            !workspace.contains(banned),
            "`{banned}` softens or rewrites the unbound-room contract",
        );
    }

    let picker = view_source("working_directory.rs");
    assert!(
        !picker.contains("No room default") && !picker.contains("resource grants"),
        "the picker must not describe an unbound room as having a default or a grant fallback",
    );
}

/// Bind/unbind keep their wording and their guarded dispatchers.
#[test]
fn bind_and_unbind_controls_keep_their_names() {
    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));
    assert!(workspace.contains("class=\"rooms-workspace__workspace-bind\""));
    assert!(workspace.contains("class=\"rooms-workspace__workspace-unbind\""));
    assert!(workspace.contains("\"Bind\"</button>") && workspace.contains("\"Unbind\"</button>"));
}
