//! Shared daemon-catalogue working-directory control for room creation and binding.
//!
//! The chosen path is a folder on the DAEMON's host. An unbound room cannot run
//! agent turns (the daemon refuses `503 workspace_unavailable`), so this control
//! only ever feeds the existing `workspace_root` create/PATCH fields.
//!
//! The browser never inspects a filesystem. Choices come from the daemon's live
//! project catalogue, while the explicit path field remains available for a
//! directory that has not been registered yet. The daemon remains the authority
//! that canonicalizes and accepts or rejects the submitted path.

use leptos::prelude::*;

use crate::daemon::ProjectInfo;

const MANUAL_CHOICE: &str = "__ocean_manual_directory__";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkingDirectoryOption {
    pub path: String,
    pub label: String,
}

/// Flatten project roots and their attached worktrees in daemon order.
/// Empty roots are omitted because they cannot be a usable directory choice,
/// and a path the catalogue lists twice (a project's main worktree is usually
/// also its root) is offered once — the picker keys its rows by path, and a
/// duplicate key would misrender the list.
pub(crate) fn working_directory_options(projects: &[ProjectInfo]) -> Vec<WorkingDirectoryOption> {
    let mut options: Vec<WorkingDirectoryOption> = Vec::new();
    let mut push = |option: WorkingDirectoryOption| {
        if !options.iter().any(|seen| seen.path == option.path) {
            options.push(option);
        }
    };
    for project in projects {
        let project_name = if project.name.trim().is_empty() {
            project.id.as_str()
        } else {
            project.name.trim()
        };
        if !project.workspace_root.trim().is_empty() {
            push(WorkingDirectoryOption {
                path: project.workspace_root.clone(),
                label: format!("{project_name} — {}", project.workspace_root),
            });
        }
        for worktree in &project.worktrees {
            if worktree.path.trim().is_empty() {
                continue;
            }
            let worktree_name = worktree.branch.as_deref().unwrap_or("worktree");
            push(WorkingDirectoryOption {
                path: worktree.path.clone(),
                label: format!("{project_name} · {worktree_name} — {}", worktree.path),
            });
        }
    }
    options
}

/// Is `value` EXACTLY one of the catalogue's paths? Exact, not trimmed: the
/// `<option>` rows carry the daemon's strings verbatim, so only an exact match
/// can be rendered as the selected row.
pub(crate) fn directory_is_registered(projects: &[ProjectInfo], value: &str) -> bool {
    !value.trim().is_empty()
        && working_directory_options(projects)
            .iter()
            .any(|option| option.path == value)
}

/// What the select shows as chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DirectoryChoice {
    /// The explicit "unbound" row (create form only).
    Unbound,
    /// A catalogue row, by its exact path.
    Registered(String),
    /// "Other absolute path…" with its text field showing.
    Manual,
}

/// Decide the selection from state alone, so first render, a reseed and a
/// catalogue arriving late all agree.
///
/// - `manual_requested` wins: once the operator chose "Other…" or typed into
///   the field, the field stays mounted even if what they typed happens to
///   equal a catalogue row (unmounting it would drop focus mid-keystroke).
/// - An empty value with `allow_empty == false` (the open room's bind panel)
///   must expose the text field: nothing is selected, and a lone catalogue row
///   showing as "selected" could never be picked because choosing it fires no
///   change event.
/// - A value the catalogue does not list (a stored binding, or one typed
///   before the catalogue loaded) is manual, so it survives and stays editable.
pub(crate) fn directory_choice(
    projects: &[ProjectInfo],
    value: &str,
    manual_requested: bool,
    allow_empty: bool,
) -> DirectoryChoice {
    if manual_requested {
        DirectoryChoice::Manual
    } else if value.trim().is_empty() {
        if allow_empty {
            DirectoryChoice::Unbound
        } else {
            DirectoryChoice::Manual
        }
    } else if directory_is_registered(projects, value) {
        DirectoryChoice::Registered(value.to_string())
    } else {
        DirectoryChoice::Manual
    }
}

/// Directory selection shared by the create form and the open-room details.
///
/// `value` is the exact path eventually sent through the existing room create or
/// PATCH route; the path is a folder on the DAEMON's host. Selecting a
/// catalogue row writes its daemon-provided absolute path. Choosing "Other
/// absolute path" reveals a text field without touching the previous value, so
/// a binding absent from the current catalogue survives. `reset` is bumped by
/// the owner when the draft is reseeded or cleared (room switch, successful
/// create) so manual mode does not outlive the draft it belonged to.
/// `id_base` must be unique per mounted picker; it anchors the help text for
/// `aria-describedby`.
#[component]
pub(crate) fn WorkingDirectoryPicker(
    projects: RwSignal<Vec<ProjectInfo>>,
    value: RwSignal<String>,
    #[prop(into)] disabled: Signal<bool>,
    #[prop(into)] reset: Signal<u32>,
    id_base: &'static str,
    label: &'static str,
    help: &'static str,
    allow_empty: bool,
    /// Pressed Enter in the manual path field (the create form submits).
    #[prop(optional)]
    on_enter: Option<Callback<()>>,
) -> impl IntoView {
    let manual_requested = RwSignal::new(false);
    Effect::new(move |_: Option<()>| {
        reset.track();
        manual_requested.set(false);
    });
    let options = Memo::new(move |_| working_directory_options(&projects.get()));
    let choice = Memo::new(move |_| {
        directory_choice(
            &projects.get(),
            &value.get(),
            manual_requested.get(),
            allow_empty,
        )
    });
    let show_manual = move || choice.get() == DirectoryChoice::Manual;
    let help_id = format!("{id_base}-help");
    let help_id_input = help_id.clone();

    view! {
        <div class="working-directory">
            <label class="working-directory__label">
                <span class="working-directory__label-text">{label}</span>
                <select
                    class="working-directory__select"
                    aria-label=label
                    aria-describedby=help_id.clone()
                    disabled=move || disabled.get()
                    on:change=move |ev| {
                        let picked = event_target_value(&ev);
                        if picked == MANUAL_CHOICE {
                            manual_requested.set(true);
                        } else {
                            manual_requested.set(false);
                            value.set(picked);
                        }
                    }
                >
                    {allow_empty.then(|| view! {
                        <option
                            value=""
                            prop:selected=move || choice.get() == DirectoryChoice::Unbound
                        >
                            "Unbound — choose a folder later"
                        </option>
                    })}
                    <option
                        value=MANUAL_CHOICE
                        prop:selected=move || choice.get() == DirectoryChoice::Manual
                    >
                        "Other absolute path…"
                    </option>
                    <For
                        each=move || options.get()
                        key=|option| option.path.clone()
                        children=move |option| {
                            let path = option.path.clone();
                            view! {
                                <option
                                    value=option.path
                                    prop:selected=move || {
                                        choice.get() == DirectoryChoice::Registered(path.clone())
                                    }
                                >
                                    {option.label}
                                </option>
                            }
                        }
                    />
                </select>
            </label>
            <Show when=show_manual>
                <label class="working-directory__manual">
                    <span class="working-directory__manual-label">"Absolute path"</span>
                    <input
                        class="working-directory__input"
                        type="text"
                        aria-label="Other absolute path on the daemon host"
                        aria-describedby=help_id_input.clone()
                        placeholder="/absolute/path/to/project"
                        prop:value=move || value.get()
                        on:input=move |ev| {
                            // Typing keeps manual mode even if the text comes to
                            // equal a catalogue row; only the select leaves it.
                            manual_requested.set(true);
                            value.set(event_target_value(&ev));
                        }
                        on:keydown=move |ev| {
                            if ev.key() == "Enter" {
                                if let Some(submit) = on_enter {
                                    ev.prevent_default();
                                    submit.run(());
                                }
                            }
                        }
                        disabled=move || disabled.get()
                    />
                </label>
            </Show>
            <span class="working-directory__help" id=help_id>{help}</span>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::WorktreeInfo;

    fn project(name: &str, root: &str, worktrees: &[(&str, Option<&str>)]) -> ProjectInfo {
        ProjectInfo {
            id: name.to_lowercase(),
            name: name.into(),
            workspace_root: root.into(),
            git_branch: None,
            git_dirty: None,
            worktrees: worktrees
                .iter()
                .map(|(path, branch)| WorktreeInfo {
                    path: (*path).into(),
                    branch: branch.map(str::to_string),
                })
                .collect(),
        }
    }

    #[test]
    fn catalogue_flattens_project_and_worktree_roots_with_human_labels() {
        let projects = vec![project(
            "Ocean Surface",
            "/dev/ocean-surface",
            &[("/dev/worktrees/picker", Some("ship/picker"))],
        )];
        assert_eq!(
            working_directory_options(&projects),
            vec![
                WorkingDirectoryOption {
                    path: "/dev/ocean-surface".into(),
                    label: "Ocean Surface — /dev/ocean-surface".into(),
                },
                WorkingDirectoryOption {
                    path: "/dev/worktrees/picker".into(),
                    label: "Ocean Surface · ship/picker — /dev/worktrees/picker".into(),
                },
            ]
        );
    }

    #[test]
    fn an_existing_manual_binding_is_not_misrepresented_as_registered() {
        let projects = vec![project("Ocean", "/dev/ocean", &[])];
        assert!(directory_is_registered(&projects, "/dev/ocean"));
        assert!(!directory_is_registered(&projects, "/srv/private-room"));
        assert!(!directory_is_registered(&projects, "  "));
        // Exact: a padded copy is not a row the select can show as chosen.
        assert!(!directory_is_registered(&projects, " /dev/ocean "));
    }

    #[test]
    fn a_path_listed_twice_is_offered_once_in_first_seen_order() {
        let projects = vec![
            project("Ocean", "/dev/ocean", &[("/dev/ocean", Some("main"))]),
            project(
                "Again",
                "/dev/ocean",
                &[("/dev/wt", None), ("/dev/wt", None)],
            ),
        ];
        let paths: Vec<_> = working_directory_options(&projects)
            .into_iter()
            .map(|option| option.path)
            .collect();
        assert_eq!(paths, vec!["/dev/ocean", "/dev/wt"]);
    }

    #[test]
    fn bind_panel_with_nothing_chosen_exposes_the_manual_field() {
        // allow_empty=false and an empty value: whether the catalogue is empty
        // or has rows, the operator needs a usable field, not a phantom row.
        let none: Vec<ProjectInfo> = Vec::new();
        let some = vec![project("Ocean", "/dev/ocean", &[])];
        for projects in [&none, &some] {
            assert_eq!(
                directory_choice(projects, "", false, false),
                DirectoryChoice::Manual
            );
            assert_eq!(
                directory_choice(projects, "   ", false, false),
                DirectoryChoice::Manual
            );
        }
    }

    #[test]
    fn create_form_empty_value_is_the_explicit_unbound_row() {
        let some = vec![project("Ocean", "/dev/ocean", &[])];
        assert_eq!(
            directory_choice(&some, "", false, true),
            DirectoryChoice::Unbound
        );
        // Choosing "Other…" on an empty draft must stay manual.
        assert_eq!(
            directory_choice(&some, "", true, true),
            DirectoryChoice::Manual
        );
    }

    #[test]
    fn a_bound_registered_path_renders_as_that_row() {
        let projects = vec![project("Ocean", "/dev/ocean", &[("/dev/wt", Some("x"))])];
        assert_eq!(
            directory_choice(&projects, "/dev/wt", false, false),
            DirectoryChoice::Registered("/dev/wt".into())
        );
    }

    #[test]
    fn an_unlisted_binding_is_manual_and_typing_keeps_manual_mode() {
        let projects = vec![project("Ocean", "/dev/ocean", &[])];
        assert_eq!(
            directory_choice(&projects, "/srv/elsewhere", false, false),
            DirectoryChoice::Manual
        );
        // Once the operator is typing, a value that comes to equal a row must
        // not unmount the field they are typing in.
        assert_eq!(
            directory_choice(&projects, "/dev/ocean", true, false),
            DirectoryChoice::Manual
        );
    }
}
