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
/// Empty roots are omitted because they cannot be a usable directory choice.
pub(crate) fn working_directory_options(projects: &[ProjectInfo]) -> Vec<WorkingDirectoryOption> {
    let mut options = Vec::new();
    for project in projects {
        let project_name = if project.name.trim().is_empty() {
            project.id.as_str()
        } else {
            project.name.trim()
        };
        if !project.workspace_root.trim().is_empty() {
            options.push(WorkingDirectoryOption {
                path: project.workspace_root.clone(),
                label: format!("{project_name} — {}", project.workspace_root),
            });
        }
        for worktree in &project.worktrees {
            if worktree.path.trim().is_empty() {
                continue;
            }
            let worktree_name = worktree.branch.as_deref().unwrap_or("worktree");
            options.push(WorkingDirectoryOption {
                path: worktree.path.clone(),
                label: format!("{project_name} · {worktree_name} — {}", worktree.path),
            });
        }
    }
    options
}

pub(crate) fn directory_is_registered(projects: &[ProjectInfo], value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && working_directory_options(projects)
            .iter()
            .any(|option| option.path == value)
}

/// Directory selection shared by the create form and the open-room details.
///
/// `value` is the exact path eventually sent through the existing room create or
/// PATCH route. Selecting a catalogue row writes its daemon-provided absolute
/// path. Choosing "Other absolute path" reveals a text field without touching
/// the previous value, so a binding absent from the current catalogue survives.
#[component]
pub(crate) fn WorkingDirectoryPicker(
    projects: RwSignal<Vec<ProjectInfo>>,
    value: RwSignal<String>,
    #[prop(into)] disabled: Signal<bool>,
    label: &'static str,
    help: &'static str,
    allow_empty: bool,
    /// Pressed Enter in the manual path field (the create form submits).
    #[prop(optional)]
    on_enter: Option<Callback<()>>,
) -> impl IntoView {
    let manual_requested = RwSignal::new(false);
    let options = Memo::new(move |_| working_directory_options(&projects.get()));
    let show_manual = Memo::new(move |_| {
        manual_requested.get()
            || (!value.get().trim().is_empty()
                && !directory_is_registered(&projects.get(), &value.get()))
    });
    let selected = move || {
        let current = value.get();
        if show_manual.get() {
            MANUAL_CHOICE.to_string()
        } else {
            current
        }
    };

    view! {
        <div class="working-directory">
            <label class="working-directory__label">
                <span class="working-directory__label-text">{label}</span>
                <select
                    class="working-directory__select"
                    aria-label=label
                    prop:value=selected
                    disabled=move || disabled.get()
                    on:change=move |ev| {
                        let choice = event_target_value(&ev);
                        if choice == MANUAL_CHOICE {
                            manual_requested.set(true);
                        } else {
                            manual_requested.set(false);
                            value.set(choice);
                        }
                    }
                >
                    {allow_empty.then(|| view! {
                        <option value="">"Unbound — choose a folder later"</option>
                    })}
                    <option value=MANUAL_CHOICE>"Other absolute path…"</option>
                    <For
                        each=move || options.get()
                        key=|option| option.path.clone()
                        children=move |option| view! {
                            <option value=option.path>{option.label}</option>
                        }
                    />
                </select>
            </label>
            <Show when=move || show_manual.get()>
                <label class="working-directory__manual">
                    <span class="working-directory__manual-label">"Absolute path"</span>
                    <input
                        class="working-directory__input"
                        type="text"
                        aria-label="Other working directory on the connected machine"
                        placeholder="/absolute/path/to/project"
                        prop:value=move || value.get()
                        on:input=move |ev| value.set(event_target_value(&ev))
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
            <span class="working-directory__help">{help}</span>
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
    }
}
