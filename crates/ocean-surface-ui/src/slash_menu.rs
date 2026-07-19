//! Slash command popover — the presentational composer `/` surface.
//!
//! Owns NO state and mounts NO key listeners. There is exactly ONE ordering of
//! rows — [`project_rows`] groups the filtered rows by first-group-appearance
//! and flattens them, and that flat vector's index space IS the selection
//! space. The composer projects its filtered rows through [`project_rows`]
//! once, then feeds the same projected vector to both keyboard navigation
//! (`selected` indexes it directly, moved via [`next_selection`] /
//! [`prev_selection`] and clamped via [`clamp_selection`]) and this component.
//! Because render order equals projection order, the visually highlighted row
//! and the row `selected` dispatches are always the same row — keyboard order
//! can never diverge from visual order.
//!
//! This component only renders the already-projected rows, inserting a group
//! header before the first row of each group ([`render_rows`]), and fires
//! `on_pick(id)` on click of an enabled row. Disabled rows render greyed and
//! do not fire. The data contract between the registry (Agent A) and this
//! popover (Agent B) is [`SlashRow`]; rows are built upstream from
//! `CommandRegistry::slash_filter` so there is exactly one command registry.

use leptos::prelude::*;

/// CSS class for a group header. Pinned to the styled selector in
/// `styles/composer.css` (`.ocean-slash-menu__group`); the
/// `css_group_class_matches_emitted` test asserts the stylesheet actually
/// styles this exact class so the emitted DOM and the stylesheet cannot drift.
pub const GROUP_CLASS: &str = "ocean-slash-menu__group";

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

// ── Projection ─────────────────────────────────────────────────────────────

/// The single deterministic row projection: group rows by their `group` label
/// in first-appearance order, then flatten. Rows keep their original order
/// within a group.
///
/// The returned vector's index space is the one true selection space. The
/// composer projects its filtered rows through this once and uses the result
/// for rendering, selection movement, Enter/Tab dispatch, and clamping — so
/// there is never a second, independently ordered vector. It is idempotent:
/// projecting an already-projected vector is a no-op (proved by
/// `projection_is_idempotent`).
pub fn project_rows(items: Vec<SlashRow>) -> Vec<SlashRow> {
    let mut groups: Vec<(String, Vec<SlashRow>)> = Vec::new();
    for row in items {
        match groups.iter_mut().find(|(g, _)| *g == row.group) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((row.group.clone(), vec![row])),
        }
    }
    groups.into_iter().flat_map(|(_, rows)| rows).collect()
}

/// Next selection index with wrap. Used by the composer's ArrowDown handler so
/// the wrap arithmetic is testable in isolation over the projected length.
pub fn next_selection(selected: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (selected + 1) % len
    }
}

/// Previous selection index with wrap. Used by the composer's ArrowUp handler.
pub fn prev_selection(selected: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else if selected == 0 {
        len - 1
    } else {
        selected - 1
    }
}

/// Clamp a selection index into a projection of length `len` (0 when empty).
/// Used both at render time and before dispatch so a shrinking filtered list
/// can never index out of bounds.
pub fn clamp_selection(selected: usize, len: usize) -> usize {
    selected.min(len.saturating_sub(1))
}

// ── Rendering ──────────────────────────────────────────────────────────────

/// One rendered element of the projected menu: a group header, or a row at a
/// specific flat index into the projection (which equals the value `selected`
/// takes when that row is highlighted).
enum RenderRow {
    Header(String),
    Item { flat_idx: usize, row: SlashRow },
}

/// Walk the already-projected rows in order and insert a group header before
/// the first row of each group. Because the input is projected (groups are
/// contiguous), each group yields exactly one header, and `flat_idx` — the
/// row's index in the projection — equals its selection index. This does NOT
/// reorder: render order is projection order, so it cannot diverge from the
/// order the composer navigates and dispatches.
fn render_rows(items: Vec<SlashRow>) -> Vec<RenderRow> {
    let mut out = Vec::with_capacity(items.len() + 4);
    let mut current: Option<String> = None;
    for (flat_idx, row) in items.into_iter().enumerate() {
        if current.as_deref() != Some(row.group.as_str()) {
            current = Some(row.group.clone());
            out.push(RenderRow::Header(row.group.clone()));
        }
        out.push(RenderRow::Item { flat_idx, row });
    }
    out
}

// ── Component ──────────────────────────────────────────────────────────────

/// Presentational popover for the composer `/` command menu.
///
/// Owns NO state. The composer passes the already-[`project_rows`]-projected
/// `Vec<SlashRow>` (rebuilt every time the `/` query or registry changes), the
/// currently highlighted `selected` index into that same projection, and an
/// `on_pick` callback. This component:
///
/// - renders rows in projection order, inserting a header before the first row
///   of each group (so visual order equals the navigated/dispatched order),
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
    let rendered = render_rows(items)
        .into_iter()
        .map(|entry| match entry {
            RenderRow::Header(group) => view! { <div class=GROUP_CLASS>{group}</div> }.into_any(),
            RenderRow::Item { flat_idx, row } => {
                let id = row.id.clone();
                let alias = row.alias.clone();
                let title = row.title.clone();
                let hint = row.hint.clone();
                let enabled = row.enabled;
                let is_selected = flat_idx == selected;
                let is_disabled = !enabled;
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
                .into_any()
            }
        })
        .collect::<Vec<_>>();

    view! { <div class="ocean-slash-menu">{rendered}</div> }
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

    fn ids(rows: &[SlashRow]) -> Vec<&str> {
        rows.iter().map(|r| r.id.as_str()).collect()
    }

    /// The live interleaved registry order from the audit verdict: two App rows
    /// straddle four other scopes. This is the fixture that used to make
    /// keyboard order diverge from visual order.
    fn interleaved_five_scope() -> Vec<SlashRow> {
        vec![
            row("floor", "App", true),
            row("new", "Session", true),
            row("files", "Files", true),
            row("repo", "Repo", true),
            row("browser", "Browser", true),
            row("sessions", "App", true),
        ]
    }

    /// Extract the rendered rows in visual order, ignoring headers, as
    /// (flat_idx, id). This is the exact order and index a user sees.
    fn visual_rows(items: Vec<SlashRow>) -> Vec<(usize, String)> {
        render_rows(items)
            .into_iter()
            .filter_map(|e| match e {
                RenderRow::Item { flat_idx, row } => Some((flat_idx, row.id)),
                RenderRow::Header(_) => None,
            })
            .collect()
    }

    fn headers(items: Vec<SlashRow>) -> Vec<String> {
        render_rows(items)
            .into_iter()
            .filter_map(|e| match e {
                RenderRow::Header(g) => Some(g),
                RenderRow::Item { .. } => None,
            })
            .collect()
    }

    #[test]
    fn projection_groups_and_flattens_in_first_appearance_order() {
        let projected = project_rows(interleaved_five_scope());
        // App's two rows sit adjacent and first; remaining groups follow in the
        // order their first row appeared.
        assert_eq!(
            ids(&projected),
            ["floor", "sessions", "new", "files", "repo", "browser"]
        );
    }

    #[test]
    fn projection_is_idempotent() {
        let once = project_rows(interleaved_five_scope());
        let twice = project_rows(once.clone());
        assert_eq!(ids(&once), ids(&twice));
    }

    #[test]
    fn render_order_equals_projection_order() {
        // Visual (rendered) row order must match the projected flat order 1:1,
        // and flat indices must be contiguous 0..n in visual order — this is
        // the invariant that keeps keyboard order == visual order.
        let projected = project_rows(interleaved_five_scope());
        let visual = visual_rows(projected.clone());
        let visual_ids: Vec<&str> = visual.iter().map(|(_, id)| id.as_str()).collect();
        assert_eq!(visual_ids, ids(&projected));
        let visual_idx: Vec<usize> = visual.iter().map(|(i, _)| *i).collect();
        assert_eq!(visual_idx, (0..projected.len()).collect::<Vec<_>>());
    }

    #[test]
    fn one_header_per_group_in_first_appearance_order() {
        let projected = project_rows(interleaved_five_scope());
        assert_eq!(
            headers(projected),
            ["App", "Session", "Files", "Repo", "Browser"]
        );
    }

    #[test]
    fn arrow_down_wraps_over_projected_length() {
        let len = project_rows(interleaved_five_scope()).len(); // 6
        assert_eq!(next_selection(0, len), 1);
        assert_eq!(next_selection(4, len), 5);
        assert_eq!(next_selection(len - 1, len), 0); // wrap to top
    }

    #[test]
    fn arrow_up_wraps_over_projected_length() {
        let len = project_rows(interleaved_five_scope()).len(); // 6
        assert_eq!(prev_selection(1, len), 0);
        assert_eq!(prev_selection(0, len), len - 1); // wrap to bottom
    }

    #[test]
    fn selection_helpers_are_safe_when_empty() {
        assert_eq!(next_selection(0, 0), 0);
        assert_eq!(prev_selection(0, 0), 0);
        assert_eq!(clamp_selection(9, 0), 0);
    }

    #[test]
    fn clamp_holds_selection_in_bounds_as_filter_shrinks() {
        // Selection was on the 6th row; a narrower query leaves 2 rows.
        let filtered = project_rows(vec![row("floor", "App", true), row("files", "Files", true)]);
        assert_eq!(clamp_selection(5, filtered.len()), 1);
    }

    #[test]
    fn disabled_rows_keep_their_slot_in_projection_and_render() {
        let projected = project_rows(vec![
            row("new", "Session", true),
            row("resume", "Session", false), // disabled signpost
            row("files", "Files", true),
        ]);
        // The disabled row still occupies an index and still renders.
        assert_eq!(ids(&projected), ["new", "resume", "files"]);
        let visual = visual_rows(projected.clone());
        assert!(visual.iter().any(|(_, id)| id == "resume"));
        assert!(!projected[1].enabled);
    }

    #[test]
    fn selected_index_dispatches_the_visually_highlighted_row() {
        // Dispatch identity: the composer runs `projected[selected]`; the user
        // sees the row at the same visual index. They must be the same row for
        // every selectable index — including across a group boundary and onto a
        // disabled row (which the composer refuses, but must still be the row
        // under the highlight).
        let projected = project_rows(vec![
            row("floor", "App", true),
            row("new", "Session", true),
            row("resume", "Session", false),
            row("files", "Files", true),
            row("sessions", "App", true),
        ]);
        let visual = visual_rows(projected.clone());
        for (visual_pos, (flat_idx, visual_id)) in visual.iter().enumerate() {
            // The nth visible row carries flat index n...
            assert_eq!(*flat_idx, visual_pos);
            // ...and the row the composer would dispatch at that selection is
            // exactly the row rendered there.
            assert_eq!(&projected[*flat_idx].id, visual_id);
        }
    }

    #[test]
    fn filtered_results_project_without_reordering_within_group() {
        // A query narrowing to one group keeps that group's internal order.
        let projected = project_rows(vec![
            row("new", "Session", true),
            row("model", "Session", true),
            row("thinking", "Session", true),
        ]);
        assert_eq!(ids(&projected), ["new", "model", "thinking"]);
        assert_eq!(headers(projected), ["Session"]);
    }

    #[test]
    fn empty_input_yields_no_rows() {
        assert!(project_rows(Vec::new()).is_empty());
        assert!(render_rows(Vec::new()).is_empty());
    }

    /// Source/DOM assertion: the class this component emits for a group header
    /// (`GROUP_CLASS`) must be the class the stylesheet actually styles, and the
    /// dead `-label` selector the audit flagged must be gone. Reads the real
    /// stylesheet at repo-root `styles/composer.css`.
    #[test]
    fn css_group_class_matches_emitted() {
        let css_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../styles/composer.css");
        let css =
            std::fs::read_to_string(css_path).unwrap_or_else(|e| panic!("read {css_path}: {e}"));

        // The emitted header class is styled as its own rule.
        let styled = format!(".{GROUP_CLASS} {{");
        assert!(
            css.contains(&styled),
            "composer.css must style the emitted header class `{styled}`"
        );
        // Group spacing is based on the real rendered structure: a header that
        // follows a row (i.e. every group after the first), not the old
        // header-adjacent-header selector that rows always sit between.
        assert!(
            css.contains(&format!(".ocean-slash-menu__row + .{GROUP_CLASS}")),
            "group spacing must key off a header following a row"
        );
        // The dead selector the audit flagged is removed.
        assert!(
            !css.contains("ocean-slash-menu__group-label"),
            "dead `.ocean-slash-menu__group-label` selector must be removed"
        );
        assert!(
            !css.contains(".ocean-slash-menu__group + .ocean-slash-menu__group"),
            "dead header-adjacent-header spacing rule must be removed"
        );
    }
}
