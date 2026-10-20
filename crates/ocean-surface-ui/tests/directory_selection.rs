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
        picker.contains("DirectoryChoice::Manual")
            && picker.contains("elseifdirectory_is_registered(projects,value){"),
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
    // Normalised on both sides — lowercase, whitespace and string-continuation
    // backslashes gone — so a banned phrase cannot hide behind a line wrap, a
    // `\` continuation or different capitalisation.
    let sources = [
        ("rooms_workspace.rs", normalize(&workspace)),
        (
            "working_directory.rs",
            normalize(&view_source("working_directory.rs")),
        ),
        ("rooms.rs", normalize(&view_source("rooms.rs"))),
    ];
    for banned in [
        "resource grants may still supply",
        "resource grants may supply",
        "grants may still supply",
        "may still supply a working directory",
        "No room default",
        "Use as room default",
        "Clear default",
        "explicit room default",
    ] {
        let needle = normalize(banned);
        for (file, text) in &sources {
            assert!(
                !text.contains(&needle),
                "`{banned}` in {file} softens or rewrites the unbound-room contract",
            );
        }
    }
    assert!(
        !sources[1].1.contains(&normalize("resource grants")),
        "the picker must not describe a resource-grant fallback for an unbound room",
    );
}

fn normalize(source: &str) -> String {
    without_whitespace(source).replace('\\', "").to_lowercase()
}

/// The browser cannot see the daemon's filesystem, so nothing in the picker
/// judges a path: no absoluteness, existence or canonical-form check. The
/// daemon's `400 invalid_workspace_root` is the only verdict.
#[test]
fn the_picker_never_prevalidates_a_path_in_the_browser() {
    // Code only: the module docs may NAME the daemon's canonicalisation.
    let picker: String = view_source("working_directory.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "starts_with(",
        "is_absolute",
        "Path::",
        "PathBuf",
        "canonicalize",
        "exists(",
        "invalid_workspace_root",
        "type=\"file\"",
    ] {
        assert!(
            !picker.contains(forbidden),
            "`{forbidden}` in working_directory.rs would judge a daemon-host path \
             in the browser; only the daemon validates it",
        );
    }
}

/// Help text is linked to both controls by a stable id, and the two mounted
/// pickers never share one; manual mode is reset with the draft it belongs to.
#[test]
fn picker_links_help_and_resets_with_its_draft() {
    let picker = without_whitespace(&view_source("working_directory.rs"));
    assert!(picker.contains("aria-describedby=help_id.clone()"));
    assert!(picker.contains("aria-describedby=help_id_input.clone()"));
    assert!(picker.contains("id=help_id>"));
    assert!(
        picker.contains("aria-label=\"Otherabsolutepathonthedaemonhost\"")
            || picker.contains("aria-label=\"Other absolute path on the daemon host\""),
        "the manual field's accessible name must say the path is on the daemon host",
    );
    assert!(
        !picker.contains("connectedmachine"),
        "the manual field must not call the daemon host a vague connected machine",
    );

    let workspace = without_whitespace(&view_source("rooms_workspace.rs"));
    assert!(workspace.contains("id_base=\"rooms-create-workspace\""));
    assert!(workspace.contains("id_base=\"rooms-details-workspace\""));
    assert!(workspace.contains("reset=create_workspace_reset"));
    assert!(workspace.contains("reset=picker_reset"));
    assert!(
        workspace.contains("create_workspace.set(String::new());create_workspace_reset.update("),
        "a successful create must clear the draft AND reset the picker's manual mode",
    );
    assert!(
        workspace.contains("seeded_for.set(id);picker_reset.update("),
        "reseeding the bind draft for another room must reset the picker's manual mode",
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
