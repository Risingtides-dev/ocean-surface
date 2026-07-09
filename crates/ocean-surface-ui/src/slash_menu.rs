//! Slash command popover — the presentational composer `/` surface.
//!
//! Owns NO state and mounts NO key listeners. The composer drives `selected`
//! (a flat index into the filtered row order — i.e. the same order `items`
//! arrives in) and all keyboard handling; this component only renders the
//! filtered rows grouped by `group` (first-appearance order) and fires
//! `on_pick(id)` on click of an enabled row. Disabled rows render greyed and
//! do not fire. The data contract between the registry (Agent A) and this
//! popover (Agent B) is [`SlashRow`]; rows are built upstream from
//! `CommandRegistry::slash_filter` so there is exactly one command registry.

use leptos::prelude::*;

// ── Row contract ───────────────────────────────────────────────────────────

/// A single row in the composer `/` popover.
///
/// The composer builds a `Vec<SlashRow>` from `CommandRegistry::slash_filter`
/// results; this struct is the wire format between the registry and the
/// presentational popover. Field names are pinned by the shared Agent A/B/C
/// contract — do not rename.
#[derive(Clone)]
pub struct SlashRow {
    /// Registry command id, e.g. "new-session". Fired back via `on_pick`.
    pub id: String,
    /// Display title, e.g. "New session".
    pub title: String,
    /// Slash alias WITH the leading slash, e.g. "/model". Primary row text.
    pub alias: String,
    /// Optional trailing hint, e.g. a keyboard shortcut. Rendered dimmed.
    pub hint: Option<String>,
    /// Scope label used to group rows, e.g. "Session". Comes from the
    /// existing `CommandScope::label()` upstream — this component does not
    /// know about scopes, it just renders the string.
    pub group: String,
    /// `false` marks a roadmap / not-yet-wired command: it still appears (so
    /// every TUI command is reachable by typing) but is greyed, non-selectable,
    /// and its click does NOT fire `on_pick`.
    pub enabled: bool,
}

// ── Grouping ───────────────────────────────────────────────────────────────

/// Group rows by their `group` label, preserving the first-appearance order of
/// groups and the original order of rows within a group.
///
/// Each row carries its FLAT index — its position in the input `items` vector
/// — because [`SlashMenu`]'s `selected` argument is a flat index across the
/// same filtered order, not a per-group index. Keeping the flat index on the
/// grouped row lets the render check selection without a second pass.
fn group_by_first_appearance(
    items: Vec<SlashRow>,
) -> Vec<(String, Vec<(usize, SlashRow)>)> {
    let mut groups: Vec<(String, Vec<(usize, SlashRow)>)> = Vec::new();
    for (idx, row) in items.into_iter().enumerate() {
        match groups.iter_mut().find(|(g, _)| *g == row.group) {
            Some((_, rows)) => rows.push((idx, row)),
            None => {
                let label = row.group.clone();
                groups.push((label, vec![(idx, row)]));
            }
        }
    }
    groups
}

// ── Component ──────────────────────────────────────────────────────────────

/// Presentational popover for the composer `/` command menu.
///
/// Owns NO state. The composer passes a freshly filtered `Vec<SlashRow>`
/// (every time the `/` query or registry changes), the currently highlighted
/// `selected` index across that same flat order, and an `on_pick` callback.
/// This component:
///
/// - groups rows by `group` (first-appearance order) with a header per group,
/// - highlights the row whose flat index equals `selected` with `.is-selected`,
/// - greys disabled rows with `.is-disabled` and never fires `on_pick` for them,
/// - fires `on_pick(row.id)` on click of an enabled row.
///
/// It does NOT listen for keys — the composer owns Escape/↑/↓/Enter. It is
/// positioned via CSS (`.ocean-slash-menu`: `position: absolute; bottom: 100%`)
/// so it floats above its relatively-positioned composer parent.
#[component]
pub fn SlashMenu(
    items: Vec<SlashRow>,
    selected: usize,
    on_pick: Callback<String>,
) -> impl IntoView {
    let groups = group_by_first_appearance(items);

    view! {
        <div class="ocean-slash-menu">
            <For
                each=move || groups.clone()
                key=|(group, _)| group.clone()
                children=move |(group, rows)| {
                    view! {
                        <div class="ocean-slash-menu__group">{group}</div>
                        {rows.into_iter().map(|(flat_idx, row)| {
                            let id = row.id.clone();
                            let alias = row.alias.clone();
                            let title = row.title.clone();
                            let hint = row.hint.clone();
                            let enabled = row.enabled;
                            let is_selected = flat_idx == selected;
                            let is_disabled = !enabled;
                            // Clone the callback per row; Callback is cheaply
                            // cloneable (Arc-backed) and each row needs its own.
                            let on_pick = on_pick.clone();
                            view! {
                                <div
                                    class="ocean-slash-menu__row"
                                    class:is-selected=is_selected
                                    class:is-disabled=is_disabled
                                    on:click=move |_| {
                                        if enabled {
                                            on_pick.run(id.clone());
                                        }
                                    }
                                >
                                    <span class="ocean-slash-menu__alias">
                                        {alias}
                                    </span>
                                    <span class="ocean-slash-menu__title">
                                        {title}
                                    </span>
                                    {if let Some(h) = hint {
                                        view! {
                                            <span class="ocean-slash-menu__hint">
                                                {h}
                                            </span>
                                        }.into_any()
                                    } else {
                                        ().into_any()
                                    }}
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    }
                }
            />
        </div>
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, group: &str, enabled: bool) -> SlashRow {
        SlashRow {
            id: id.into(),
            title: id.into(),
            alias: format!("/{}", id),
            hint: None,
            group: group.into(),
            enabled,
        }
    }

    #[test]
    fn grouping_preserves_first_appearance_order_of_groups() {
        let items = vec![
            row("new", "Session", true),
            row("files", "Files", true),
            row("sessions", "Session", true),
            row("repo", "Repo", true),
            row("model", "Session", true),
        ];
        let groups = group_by_first_appearance(items);

        // Group order follows first appearance: Session, Files, Repo.
        let labels: Vec<&str> = groups.iter().map(|(g, _)| g.as_str()).collect();
        assert_eq!(labels, ["Session", "Files", "Repo"]);
    }

    #[test]
    fn flat_index_survives_grouping() {
        let items = vec![
            row("a", "G1", true), // 0
            row("b", "G2", true), // 1
            row("c", "G1", true), // 2
        ];
        let groups = group_by_first_appearance(items);

        // G1 holds the original flat indices 0 and 2 (not 0 and 1).
        let g1: Vec<usize> = groups[0].1.iter().map(|(i, _)| *i).collect();
        assert_eq!(g1, [0, 2]);
        let g2: Vec<usize> = groups[1].1.iter().map(|(i, _)| *i).collect();
        assert_eq!(g2, [1]);
    }

    #[test]
    fn empty_input_yields_no_groups() {
        assert!(group_by_first_appearance(Vec::new()).is_empty());
    }
}
