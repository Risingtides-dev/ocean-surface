//! Renders live UI components that the agent emits via `component_render`.
//!
//! Each component kind maps to a Leptos view that reads the `props` JSON
//! and renders interactively. User interactions (button clicks, form submits,
//! card drags) are sent back to the daemon via `Daemon::send_component_event`,
//! which the agent's `component_wait` tool picks up.

use leptos::prelude::*;
use serde_json::{json, Value};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

use crate::daemon::{Daemon, PinnedWidget};

mod interactive_plot;
use interactive_plot::InteractivePlotView;

#[wasm_bindgen]
extern "C" {
    /// Defined in index.html. Loads the Google Maps JS API + Places UI Kit
    /// (once, idempotent) using `key`, then renders/updates the map for
    /// component `container_id` from `props_json`. `map_id` selects the visual
    /// style. `on_event` is invoked with (event_name, json_payload) for
    /// marker/place selections, to relay back to the agent.
    #[wasm_bindgen(js_name = oceanRenderMap)]
    fn ocean_render_map(
        container_id: &str,
        key: &str,
        map_id: &str,
        props_json: &str,
        on_event: &JsValue,
    );

    /// Defined in index.html. Injects a TikTok/Instagram embed blockquote into
    /// `container_id` and loads/refreshes the platform embed script so it
    /// renders. `platform` is "tiktok" | "instagram".
    #[wasm_bindgen(js_name = oceanRenderSocialVideo)]
    fn ocean_render_social_video(container_id: &str, platform: &str, url: &str);
}

// ---------------------------------------------------------------------------
// Path resolver — resolves file paths relative to cwd and workspace root.
// ---------------------------------------------------------------------------

/// Map a file extension to an icon label. Used by the deck file-tree rows.
/// Returns `"code"` for most extensions, `"folder"` for directories, and
/// `"git"` for git-related names.
pub fn file_icon_label(name: &str) -> &'static str {
    // Check for git-related files.
    if name == ".gitignore" || name == ".gitattributes" || name == ".gitmodules" {
        return "git";
    }
    // Code extensions.
    if let Some(dot) = name.rfind('.') {
        let ext = &name[dot + 1..];
        match ext {
            "rs" | "toml" | "lock" | "md" | "json" | "yaml" | "yml" | "css" | "html" | "js"
            | "ts" | "jsx" | "tsx" | "py" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt"
            | "swift" | "rb" | "pl" | "sh" | "bash" | "zsh" | "fish" | "Dockerfile" | "sql"
            | "graphql" | "proto" | "xml" | "svg" | "txt" => "code",
            _ => "code",
        }
    } else {
        "code"
    }
}

/// Resolve a relative or absolute file path. Rules:
///   1. Absolute path → returned as-is.
///   2. Path starting with `~` → home-relative, returned as-is (daemon resolves `~`).
///   3. Relative path: join with `cwd` first; if that doesn't start with
///      `workspace_root`, join with `workspace_root` instead.
///   4. Fallback: join with `workspace_root`.
pub fn resolve_file_path(workspace_root: &str, cwd: Option<&str>, file_path: &str) -> String {
    // Rule 1: absolute.
    if file_path.starts_with('/') {
        return file_path.to_string();
    }
    // Rule 2: home-relative.
    if file_path.starts_with('~') {
        return file_path.to_string();
    }
    // Rule 3: cwd-first for relative paths (authoritative; no starts_with guard).
    if let Some(cwd) = cwd {
        return join_path(cwd.trim_end_matches('/'), file_path);
    }
    // Rule 4: fallback to workspace_root when cwd is absent.
    join_path(workspace_root.trim_end_matches('/'), file_path)
}

/// Join a base path and a relative segment, normalizing `..` and `.`.
/// Does not resolve symlinks — purely syntactic.
pub fn join_path(base: &str, segment: &str) -> String {
    if base.is_empty() {
        return normalize_path(segment);
    }
    if segment.is_empty() {
        return normalize_path(base);
    }
    let combined = if base.ends_with('/') {
        format!("{}{}", base, segment)
    } else {
        format!("{}/{}", base, segment)
    };
    normalize_path(&combined)
}

/// Normalize a path by collapsing `..` and `.` segments syntactically.
fn normalize_path(path: &str) -> String {
    let is_absolute = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                if is_absolute {
                    // For absolute paths, don't pop past root.
                    if !components.is_empty() {
                        components.pop();
                    }
                } else {
                    components.pop();
                }
            }
            _ => components.push(part),
        }
    }
    if is_absolute {
        format!("/{}", components.join("/"))
    } else if components.is_empty() {
        ".".to_string()
    } else {
        components.join("/")
    }
}

/// Unified production resolver for agent-emitted file-tree entry paths.
///
/// Branches on whether the agent provided an explicit `path` field:
/// - **Explicit absolute or home-relative** → passthrough unchanged.
/// - **Explicit relative** → resolved against the session cwd (authoritative);
///   falls back to workspace_root when cwd is absent.
/// - **Absent path** (only `name`) → assembled as
///   `resolve_root(workspace_root, cwd) + "/" + ancestor_prefix + "/" + name`,
///   where `resolve_root` makes workspace_root absolute via cwd when root is
///   relative.
///
/// This is the pure function backing the FileTreeNode on-click callback.
/// It replaces the split `ancestor_path` + `resolve_file_path` so the
/// explicit-vs-absent provenance is not lost before resolution.
pub fn resolve_file_tree_path(
    entry_path: Option<&str>,
    ancestor_prefix: &str,
    name: &str,
    workspace_root: &str,
    cwd: Option<&str>,
) -> String {
    let explicit = entry_path.filter(|p| !p.is_empty());

    if let Some(path) = explicit {
        // Agent-provided explicit path.
        if path.starts_with('/') || path.starts_with('~') {
            return path.to_string();
        }
        // Relative explicit: resolve against cwd (authoritative).
        if let Some(cwd) = cwd {
            return join_path(cwd.trim_end_matches('/'), path);
        }
        // No cwd: fall back to workspace_root.
        return join_path(workspace_root.trim_end_matches('/'), path);
    }

    // Absent explicit path — assemble from ancestor chain + name, then
    // prepend the resolved workspace_root.
    let rel = if ancestor_prefix.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", ancestor_prefix, name)
    };
    let root = if workspace_root.starts_with('/') || workspace_root.starts_with('~') {
        workspace_root.trim_end_matches('/').to_string()
    } else if let Some(cwd) = cwd {
        join_path(
            cwd.trim_end_matches('/'),
            workspace_root.trim_end_matches('/'),
        )
    } else {
        workspace_root.trim_end_matches('/').to_string()
    };
    join_path(&root, &rel)
}

/// Dispatch to the right component renderer based on `kind`.
#[component]
pub fn ComponentView(
    component_id: String,
    kind: String,
    kind_props: Value,
    daemon: Daemon,
) -> impl IntoView {
    match kind.as_str() {
        "kanban" => view! {
            <KanbanView component_id kind_props daemon />
        }
        .into_any(),
        "form" => view! {
            <FormView component_id kind_props daemon />
        }
        .into_any(),
        "table" => view! {
            <TableView component_id kind_props daemon />
        }
        .into_any(),
        "progress" => view! {
            <ProgressView kind_props />
        }
        .into_any(),
        "markdown" => view! {
            <MarkdownView kind_props />
        }
        .into_any(),
        "dashboard" => view! {
            <DashboardView kind_props daemon />
        }
        .into_any(),
        "chart" => view! {
            <ChartView kind_props />
        }
        .into_any(),
        "interactive_plot" => view! {
            <InteractivePlotView component_id kind_props daemon />
        }
        .into_any(),
        "timeline" => view! {
            <TimelineView kind_props />
        }
        .into_any(),
        "stat" => view! {
            <StatView kind_props />
        }
        .into_any(),
        "file_tree" => view! {
            <FileTreeView component_id kind_props daemon />
        }
        .into_any(),
        "diff" => view! {
            <DiffView kind_props />
        }
        .into_any(),
        "code" => view! {
            <CodeView kind_props />
        }
        .into_any(),
        "callout" => view! {
            <CalloutView kind_props />
        }
        .into_any(),
        "gallery" => view! {
            <GalleryView kind_props />
        }
        .into_any(),
        "confirm" => view! {
            <ConfirmView component_id kind_props daemon />
        }
        .into_any(),
        "map" => view! {
            <MapView component_id kind_props daemon />
        }
        .into_any(),
        "video" => view! {
            <VideoView component_id kind_props />
        }
        .into_any(),
        other => view! {
            <div class="block block--component-unknown">
                <span class="component-fallback">
                    {format!("unknown component kind: {other}")}
                </span>
            </div>
        }
        .into_any(),
    }
}

// ---------------------------------------------------------------------------
// Kanban
// ---------------------------------------------------------------------------

/// A kanban board. Props shape:
/// ```json
/// { "columns": [{ "id": "todo", "title": "To Do" }],
///   "cards": [{ "id": "card-1", "column": "todo", "title": "Fix bug" }] }
/// ```
#[component]
fn KanbanView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let columns = kind_props
        .get("columns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cards = kind_props
        .get("cards")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let on_card_click = {
        let component_id = component_id.clone();
        let daemon = daemon.clone();
        move |card_id: &str| {
            let payload = serde_json::json!({
                "type": "card_clicked",
                "payload": { "card_id": card_id }
            });
            daemon.send_component_event(component_id.clone(), payload);
        }
    };

    view! {
        <div class="component-kanban">
            <div class="kanban-columns">
                {columns.into_iter().map(|col| {
                    let col_id = col.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let col_title = col.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let col_cards: Vec<&Value> = cards.iter().filter(|c| {
                        c.get("column").and_then(|v| v.as_str()) == Some(&col_id)
                    }).collect();
                    let on_click = on_card_click.clone();

                    view! {
                        <div class="kanban-column">
                            <div class="kanban-column__header">{col_title.clone()}</div>
                            <div class="kanban-column__cards">
                                {col_cards.into_iter().map(move |card| {
                                    let card_id = card.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let card_title = card.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let card_desc = card.get("description").and_then(|v| v.as_str()).unwrap_or("");
                                    let oc = on_click.clone();
                                    let cid = card_id.clone();
                                    view! {
                                        <button
                                            class="kanban-card"
                                            type="button"
                                            on:click=move |_| oc(&cid)
                                        >
                                            <div class="kanban-card__title">{card_title.clone()}</div>
                                            {if !card_desc.is_empty() {
                                                view! { <div class="kanban-card__desc">{card_desc.to_string()}</div> }.into_any()
                                            } else {
                                                ().into_any()
                                            }}
                                        </button>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    }
                }).collect::<Vec<_>>()}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Form
// ---------------------------------------------------------------------------

/// A simple input form. Props shape:
/// ```json
/// { "title": "Report a bug",
///   "fields": [{ "name": "title", "label": "Title", "type": "text", "required": true }],
///   "submit_label": "Submit" }
/// ```
#[component]
fn FormView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let title = kind_props
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Form");
    let fields = kind_props
        .get("fields")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let submit_label = kind_props
        .get("submit_label")
        .and_then(|v| v.as_str())
        .unwrap_or("Submit");

    // Store form values reactively by field name.
    let values: Vec<(String, RwSignal<String>)> = fields
        .iter()
        .filter_map(|f| {
            let name = f.get("name").and_then(|v| v.as_str())?.to_string();
            Some((name, RwSignal::new(String::new())))
        })
        .collect();

    let on_submit = {
        let component_id = component_id.clone();
        let daemon = daemon.clone();
        let values = values.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            let mut payload = serde_json::Map::new();
            for (name, signal) in &values {
                payload.insert(name.clone(), Value::String(signal.get_untracked()));
            }
            daemon.send_component_event(
                component_id.clone(),
                serde_json::json!({
                    "type": "form_submit",
                    "payload": Value::Object(payload),
                }),
            );
        }
    };

    view! {
        <div class="component-form">
            <form on:submit=on_submit>
                <h4 class="component-form__title">{title.to_string()}</h4>
                {fields.clone().into_iter().map(|field| {
                    let name = field.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let label = field.get("label").and_then(|v| v.as_str()).unwrap_or(&name).to_string();
                    let field_type = field.get("type").and_then(|v| v.as_str()).unwrap_or("text").to_string();
                    let required = field.get("required").and_then(|v| v.as_bool()).unwrap_or(false);
                    let signal = values.iter().find(|(n, _)| *n == name).map(|(_, s)| *s);

                    match field_type.as_str() {
                        "textarea" => {
                            let label_c = label.clone();
                            view! {
                                <label class="form-field">
                                    <span class="form-field__label">{label_c}{if required { "*" } else { "" }}</span>
                                    <textarea
                                        class="form-field__input"
                                        prop:value=move || signal.map(|s| s.get()).unwrap_or_default()
                                        on:input=move |ev| { if let Some(s) = signal { s.set(event_target_value(&ev)) } }
                                        rows="3"
                                    />
                                </label>
                            }.into_any()
                        }
                        "select" => {
                            let options = field.get("options").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                            let label_c = label.clone();
                            view! {
                                <label class="form-field">
                                    <span class="form-field__label">{label_c}{if required { "*" } else { "" }}</span>
                                    <select
                                        class="form-field__input"
                                        on:change=move |ev| { if let Some(s) = signal { s.set(event_target_value(&ev)) } }
                                    >
                                        <option value="" disabled selected>"—"</option>
                                        {options.into_iter().map(|opt| {
                                            let val = opt.as_str().unwrap_or("").to_string();
                                            let val2 = val.clone();
                                            view! { <option value=val2>{val}</option> }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </label>
                            }.into_any()
                        }
                        _ => {
                            let label_c = label.clone();
                            view! {
                                <label class="form-field">
                                    <span class="form-field__label">{label_c}{if required { "*" } else { "" }}</span>
                                    <input
                                        class="form-field__input"
                                        type=field_type
                                        prop:value=move || signal.map(|s| s.get()).unwrap_or_default()
                                        on:input=move |ev| { if let Some(s) = signal { s.set(event_target_value(&ev)) } }
                                    />
                                </label>
                            }.into_any()
                        }
                    }
                }).collect::<Vec<_>>()}
                <button class="form-submit" type="submit">{submit_label.to_string()}</button>
            </form>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Table
// ---------------------------------------------------------------------------

/// A simple data table. Props shape:
/// ```json
/// { "columns": ["Name", "Status"],
///   "rows": [["Fix bug", "open"], ["Add tests", "done"]] }
/// ```
#[component]
fn TableView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let columns = kind_props
        .get("columns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let rows = kind_props
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let on_row_click = {
        let component_id = component_id.clone();
        let daemon = daemon.clone();
        move |row_index: usize| {
            daemon.send_component_event(
                component_id.clone(),
                serde_json::json!({
                    "type": "row_clicked",
                    "payload": { "row_index": row_index },
                }),
            );
        }
    };

    let col_count = columns.len().max(1);
    let row_count = rows.len();
    let is_empty = rows.is_empty();

    view! {
        <div class="component-table">
            <table class="data-table">
                <thead>
                    <tr>
                        {columns.iter().map(|col| {
                            view! { <th>{cell_text(col)}</th> }
                        }).collect::<Vec<_>>()}
                    </tr>
                </thead>
                <tbody>
                    {rows.iter().enumerate().map(|(i, row)| {
                        let cells = row.as_array().cloned().unwrap_or_default();
                        let oc = on_row_click.clone();
                        view! {
                            <tr on:click=move |_| oc(i) class="data-table__row">
                                {cells.iter().enumerate().map(|(j, cell)| {
                                    let col_label = columns.get(j).map(cell_text).unwrap_or_default();
                                    view! { <td data-label=col_label>{cell_text(cell)}</td> }
                                }).collect::<Vec<_>>()}
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                    {is_empty.then(|| view! {
                        <tr><td class="data-table__empty" colspan=col_count.to_string()>"no rows"</td></tr>
                    })}
                </tbody>
            </table>
            {(!is_empty).then(|| view! {
                <div class="data-table__footer">{format!("{row_count} row{}", if row_count == 1 { "" } else { "s" })}</div>
            })}
        </div>
    }
}

/// Coerce any JSON value to display text. Strings render as-is; numbers and
/// bools render their literal form (a numeric cell like `73` must not vanish);
/// null/objects render empty.
fn cell_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// A progress bar. Props shape:
/// ```json
/// { "label": "Building...", "value": 0.6, "max": 1.0, "indeterminate": false }
/// ```
#[component]
fn ProgressView(kind_props: Value) -> impl IntoView {
    let label = kind_props
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let value = kind_props
        .get("value")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let max = kind_props
        .get("max")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0);
    let indeterminate = kind_props
        .get("indeterminate")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let pct = if max > 0.0 {
        (value / max * 100.0).round().clamp(0.0, 100.0) as i32
    } else {
        0
    };

    view! {
        <div class="component-progress">
            {if !label.is_empty() {
                view! { <div class="progress-label">{label.to_string()}</div> }.into_any()
            } else {
                ().into_any()
            }}
            <div class="progress-bar">
                <div
                    class:progress-bar__fill=true
                    class:is-indeterminate=indeterminate
                    style=format!("width: {pct}%")
                ></div>
            </div>
            // Outside .progress-bar: the bar clips (overflow:hidden) so the
            // sliding indeterminate fill never paints past the pill — a label
            // inside the 4px fill could never render. CSS right-aligns this
            // under the bar.
            {if !indeterminate {
                view! { <span class="progress-pct">{format!("{pct}%")}</span> }.into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// Renders markdown content as embedded HTML.
/// Props: `{ "content": "## Heading\n\nParagraph." }`
#[component]
fn MarkdownView(kind_props: Value) -> impl IntoView {
    let content = kind_props
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    view! {
        <div
            class="component-markdown"
            inner_html=crate::markdown::render(content)
            on:click=crate::host::open_external_link_click
        ></div>
    }
}

// ---------------------------------------------------------------------------
// Dashboard (layout container)
// ---------------------------------------------------------------------------

/// A grid container for child components. Props shape:
/// ```json
/// { "children": [{ "id": "kanban-1", "width": 2 }, { "id": "progress-1", "width": 1 }] }
/// ```
/// Children are referenced by their component_id (rendered elsewhere in the
/// turn's block list). This view places them in a CSS grid.
#[component]
fn DashboardView(kind_props: Value, daemon: Daemon) -> impl IntoView {
    let children = kind_props
        .get("children")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Column track widths come from each child's `width` (CSS grid `fr` units,
    // per the render protocol). Default 1fr when omitted. Each track uses
    // minmax(260px, Xfr) so cells never collapse below a readable width; the
    // container gets overflow-x: auto when all minimums exceed the viewport.
    let columns = children
        .iter()
        .map(|c| {
            let w = c
                .get("width")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0)
                .max(1.0);
            format!("minmax(260px, {w}fr)")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let columns = if columns.is_empty() {
        "minmax(260px, 1fr)".to_string()
    } else {
        columns
    };

    view! {
        <div
            class="component-dashboard"
            style=format!(
                "display: grid; grid-template-columns: {columns}; gap: 12px;"
            )
        >
            {children.into_iter().map(|child| {
                let child_id = child.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                // A child may carry its own component spec inline (kind + props),
                // in which case we mount it directly. Otherwise it's a bare layout
                // placeholder referenced by id and rendered elsewhere.
                match child.get("kind").and_then(|v| v.as_str()) {
                    Some(kind) => {
                        let kind = kind.to_string();
                        let props = child.get("props").cloned().unwrap_or(Value::Null);
                        view! {
                            <div class="dashboard-cell dashboard-cell--filled">
                                <ComponentView
                                    component_id=child_id
                                    kind=kind
                                    kind_props=props
                                    daemon=daemon.clone()
                                />
                            </div>
                        }
                        .into_any()
                    }
                    None => view! {
                        <div class="dashboard-cell">{child_id}</div>
                    }
                    .into_any(),
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Chart — bar / line / sparkline from numeric series
// ---------------------------------------------------------------------------

/// Compact, locale-free formatting for chart values: at most two decimals
/// (trailing zeros stripped) and thousands separators once the integer part
/// reaches four digits. Examples: `29.8`, `1.03`, `0.5`, `12`, `12,400`.
fn compact_format(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    let negative = value < 0.0;
    // Round to two decimals, then drop trailing zeros and a dangling dot so
    // 29.80 -> 29.8, 1.00 -> 1, 0.50 -> 0.5. Always '.' — never locale-aware.
    let rounded = format!("{:.2}", value.abs());
    let trimmed = rounded.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "0" {
        return "0".to_string();
    }
    let (int_part, frac_part) = match trimmed.split_once('.') {
        Some((i, f)) => (i, f),
        None => (trimmed, ""),
    };
    let mut out = insert_commas(int_part);
    if !frac_part.is_empty() {
        out.push('.');
        out.push_str(frac_part);
    }
    if negative {
        out.insert(0, '-');
    }
    out
}

/// Groups an unsigned integer's digits into comma-separated thousands triplets.
fn insert_commas(digits: &str) -> String {
    let len = digits.len();
    if len <= 3 {
        return digits.to_string();
    }
    let mut out = String::with_capacity(len + len / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// A lightweight inline chart. Props shape:
/// ```json
/// { "title": "Plays", "type": "bar",
///   "series": [{ "label": "Mon", "value": 12 }, { "label": "Tue", "value": 30 }] }
/// ```
/// `type` is "bar" (horizontal rows) | "line" (SVG line + area fill). Pure
/// CSS/SVG, no deps. Long category labels ellipsize with the full text in a
/// `title` tooltip; values are compact-formatted and mono-set.
#[component]
fn ChartView(kind_props: Value) -> impl IntoView {
    let title = kind_props
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let chart_type = kind_props
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("bar")
        .to_string();
    let series = kind_props
        .get("series")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let points: Vec<(String, f64)> = series
        .iter()
        .map(|p| {
            let label = p
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let value = p.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            (label, value)
        })
        .collect();
    // Floor at 1.0 so all-zero (or all-negative) series still divide safely
    // and render zero-width fills rather than NaNs.
    let max = points
        .iter()
        .map(|(_, v)| *v)
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let body = if points.is_empty() {
        view! {
            <div class="chart-empty">"No data"</div>
        }
        .into_any()
    } else if chart_type == "line" {
        let n = points.len();
        // Uniform-scaling 120x30 viewBox with interior padding so dots/labels
        // never clip; height:auto keeps this aspect ratio responsively, and
        // uniform scaling keeps dots circular (no preserveAspectRatio="none").
        let (vb_w, vb_h) = (120.0_f64, 30.0);
        let (pad_x, pad_top, pad_bottom) = (6.0_f64, 4.0, 5.0);
        let plot_w = vb_w - 2.0 * pad_x;
        let baseline = vb_h - pad_bottom;
        let plot_h = vb_h - pad_top - pad_bottom;

        let coords: Vec<(f64, f64)> = points
            .iter()
            .enumerate()
            .map(|(i, (_, v))| {
                let x = if n > 1 {
                    pad_x + (i as f64 / (n - 1) as f64) * plot_w
                } else {
                    vb_w / 2.0
                };
                // Clamp negatives to the baseline for geometry; the true value
                // still shows in the per-dot tooltip and the end labels.
                let y = baseline - (v.max(0.0) / max) * plot_h;
                (x, y)
            })
            .collect();
        let line_pts = coords
            .iter()
            .map(|(x, y)| format!("{x:.2},{y:.2}"))
            .collect::<Vec<_>>()
            .join(" ");
        let (first_x, last_x) = (coords[0].0, coords[n - 1].0);
        let area_pts = format!("{line_pts} {last_x:.2},{baseline:.2} {first_x:.2},{baseline:.2}");
        let dots = points
            .iter()
            .zip(coords.iter())
            .map(|((label, v), (x, y))| {
                let tip = format!("{}: {}", label, compact_format(*v));
                view! {
                    <circle class="chart-line__dot" cx=format!("{x:.2}") cy=format!("{y:.2}") r="0.9">
                        <title>{tip}</title>
                    </circle>
                }
            })
            .collect::<Vec<_>>();
        // End labels live in HTML (not SVG <text>) so 11px mono holds at any width.
        let first_val = compact_format(points[0].1);
        let last_val = compact_format(points[n - 1].1);

        view! {
            <div class="chart-line__vals">
                <span>{first_val}</span>
                <span>{last_val}</span>
            </div>
            <svg class="chart-line" viewBox="0 0 120 30" preserveAspectRatio="xMidYMid meet">
                <polygon class="chart-line__area" points=area_pts></polygon>
                <polyline class="chart-line__line" points=line_pts></polyline>
                {dots}
            </svg>
        }
        .into_any()
    } else {
        view! {
            <div class="chart-rows">
                {points
                    .iter()
                    .enumerate()
                    .map(|(i, (label, v))| {
                        let label = label.clone();
                        let val_str = compact_format(*v);
                        // Width tracks the share of max; negatives clamp to a
                        // zero-width fill but keep their true formatted value.
                        let pct = (v.max(0.0) / max * 100.0).min(100.0);
                        let delay = (i * 30).min(300);
                        let fill_style = if *v > 0.0 {
                            format!("width: {pct:.2}%; animation-delay: {delay}ms")
                        } else {
                            // Zero/negative: override the 2px min-width so the
                            // fill is truly absent while the value still shows.
                            format!("width: 0%; min-width: 0; animation-delay: {delay}ms")
                        };
                        view! {
                            <div class="chart-row">
                                <span class="chart-row__label" title=label.clone()>
                                    {label.clone()}
                                </span>
                                <div class="chart-row__track">
                                    <div class="chart-row__fill" style=fill_style></div>
                                </div>
                                <span class="chart-row__val">{val_str}</span>
                            </div>
                        }
                    })
                    .collect::<Vec<_>>()}
            </div>
        }
        .into_any()
    };

    view! {
        <div class="component-chart">
            {(!title.is_empty())
                .then(|| view! { <div class="component-chart__title">{title.clone()}</div> })}
            {body}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Timeline — ordered steps with status
// ---------------------------------------------------------------------------

/// A vertical timeline of steps. Props shape:
/// ```json
/// { "steps": [{ "label": "Plan", "status": "done", "detail": "approved" },
///             { "label": "Build", "status": "active" },
///             { "label": "Ship", "status": "pending" }] }
/// ```
/// status is "done" | "active" | "pending" | "error".
#[component]
fn TimelineView(kind_props: Value) -> impl IntoView {
    let steps = kind_props
        .get("steps")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    view! {
        <div class="component-timeline">
            {steps.into_iter().map(|step| {
                let label = step.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let detail = step.get("detail").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let status = step.get("status").and_then(|v| v.as_str()).unwrap_or("pending").to_string();
                let dot = match status.as_str() {
                    "done" => "✓",
                    "error" => "✗",
                    // active/pending markers are drawn geometrically in CSS
                    // (::before circle) — text glyphs like ◉ sit wherever the
                    // font's metrics put them, never optical center.
                    _ => "",
                };
                view! {
                    <div class=format!("timeline-step timeline-step--{status}")>
                        <div class="timeline-step__rail">
                            <span class="timeline-step__dot">{dot}</span>
                        </div>
                        <div class="timeline-step__body">
                            <div class="timeline-step__label">{label}</div>
                            {(!detail.is_empty()).then(|| view! { <div class="timeline-step__detail">{detail.clone()}</div> })}
                        </div>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Stat — row of KPI cards
// ---------------------------------------------------------------------------

/// A row of stat / KPI cards. Props shape:
/// ```json
/// { "stats": [{ "label": "Views", "value": "1.2M", "delta": "+12%", "trend": "up" }] }
/// ```
/// trend is "up" | "down" | "flat" (colors the delta).
#[component]
fn StatView(kind_props: Value) -> impl IntoView {
    let stats = kind_props
        .get("stats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    view! {
        <div class="component-stats">
            {stats.into_iter().map(|s| {
                let label = s.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let value = cell_text(s.get("value").unwrap_or(&Value::Null));
                let delta = s.get("delta").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let trend = s.get("trend").and_then(|v| v.as_str()).unwrap_or("flat").to_string();
                view! {
                    <div class="stat-card">
                        <div class="stat-card__value">{value}</div>
                        <div class="stat-card__label">{label}</div>
                        {(!delta.is_empty()).then(|| view! {
                            <div class=format!("stat-card__delta stat-card__delta--{trend}")>{delta.clone()}</div>
                        })}
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

// ---------------------------------------------------------------------------
// File tree — collapsible directory tree, files emit clicks
// ---------------------------------------------------------------------------

/// A collapsible file/dir tree. Props shape:
/// ```json
/// { "root": "src/", "entries": [
///     { "name": "main.rs", "type": "file" },
///     { "name": "tools", "type": "dir", "children": [{ "name": "mod.rs", "type": "file" }] } ] }
/// ```
/// Clicking a file emits file_clicked { path }.
#[component]
fn FileTreeView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let root = kind_props
        .get("root")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let entries = kind_props
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // When a file is clicked in the transcript, resolve the relative path
    // against the workspace root and set preview_file_intent so the deck
    // FilesPanel opens a preview (or Tauri opens the file externally).
    let on_file_click: Arc<dyn Fn(Option<String>, String, String) + Send + Sync> = {
        let daemon = daemon.clone();
        let workspace_root = root.clone();
        let cwd = daemon.cwd.get_untracked();
        Arc::new(
            move |entry_path: Option<String>, ancestor_prefix: String, name: String| {
                let resolved = resolve_file_tree_path(
                    entry_path.as_deref(),
                    &ancestor_prefix,
                    &name,
                    &workspace_root,
                    if cwd.is_empty() { None } else { Some(&cwd) },
                );
                daemon.preview_file_intent.set(Some((resolved, 0)));
            },
        )
    };

    view! {
        <div class="component-filetree">
            {(!root.is_empty()).then(|| view! { <div class="filetree__root">{root.clone()}</div> })}
            <ul class="filetree__list">
                {entries.into_iter().map(|e| {
                    view! { <FileTreeNode entry=e depth=0 component_id=component_id.clone() daemon=daemon.clone() on_file_click=Arc::clone(&on_file_click) ancestor_prefix=String::new() /> }
                }).collect::<Vec<_>>()}
            </ul>
        </div>
    }
}

#[component]
fn FileTreeNode(
    entry: Value,
    depth: usize,
    component_id: String,
    daemon: Daemon,
    on_file_click: Arc<dyn Fn(Option<String>, String, String) + Send + Sync>,
    /// Accumulated relative path from the file_tree root down to this node's
    /// parent directory (e.g. "src/lib"). Empty at root level. Threaded through
    /// recursion so every node can reconstruct the full relative path even when
    /// the agent only emits `name` without a `path` field.
    #[prop(default=String::new())]
    ancestor_prefix: String,
) -> impl IntoView {
    let name = entry
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let is_dir = entry.get("type").and_then(|v| v.as_str()) == Some("dir");
    // Secret-bearing filenames (.env, .env.*, *.pem, *.key, id_rsa*) are
    // hidden by default, the way editors treat dotenv files (QA-004).
    if !is_dir && crate::deck::files::is_secret_file(&name) {
        return ().into_any();
    }
    let children = entry
        .get("children")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let indent = format!("padding-left: {}px", depth * 14 + 4);

    if is_dir {
        let open = RwSignal::new(depth == 0);
        let name_c = name.clone();
        // Inline rotation (not a stylesheet rule): this component's styles
        // live in the transcript sheet another workstream owns; keep the
        // icon swap self-contained.
        let arrow_style = move || {
            if open.get() {
                ""
            } else {
                "transform: rotate(-90deg)"
            }
        };
        let child_prefix = if ancestor_prefix.is_empty() {
            name_c.clone()
        } else {
            format!("{}/{}", ancestor_prefix, name_c.clone())
        };
        view! {
            <li class="filetree__node filetree__node--dir">
                <button class="filetree__row filetree__row--dir" type="button" style=indent
                    on:click=move |_| open.update(|v| *v = !*v)>
                    <span class="filetree__arrow" style=arrow_style><crate::icons::ChevronDown /></span>
                    <span class="filetree__icon"><crate::icons::Folder /></span>
                    <span class="filetree__name">{name_c}</span>
                </button>
                <Show when=move || open.get()>
                    <ul class="filetree__list">
                        {children.clone().into_iter().map({
                            let cp = child_prefix.clone();
                            let cid = component_id.clone();
                            let d = daemon.clone();
                            let ofc = Arc::clone(&on_file_click);
                            move |c| {
                                view! { <FileTreeNode entry=c depth=depth+1 component_id=cid.clone() daemon=d.clone() on_file_click=Arc::clone(&ofc) ancestor_prefix=cp.clone() /> }
                            }
                        }).collect::<Vec<_>>()}
                    </ul>
                </Show>
            </li>
        }
        .into_any()
    } else {
        let entry_path_opt = entry
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let on_click = {
            let component_id = component_id.clone();
            let daemon = daemon.clone();
            let on_file_click = Arc::clone(&on_file_click);
            let ep = entry_path_opt.clone();
            let ap = ancestor_prefix.clone();
            let nm = name.clone();
            let path_for_event = ep.clone().unwrap_or_else(|| {
                if ap.is_empty() {
                    nm.clone()
                } else {
                    format!("{}/{}", ap, nm)
                }
            });
            move |_| {
                daemon.send_component_event(
                    component_id.clone(),
                    serde_json::json!({ "type": "file_clicked", "payload": { "path": path_for_event } }),
                );
                // Deep-link: pass raw provenance to the unified resolver via
                // the callback so the resolver can branch on explicit vs absent.
                on_file_click(ep.clone(), ap.clone(), nm.clone());
            }
        };
        view! {
            <li class="filetree__node">
                <button class="filetree__row" type="button" style=indent on:click=on_click>
                    <span class="filetree__icon"><crate::icons::Code /></span>
                    <span class="filetree__name">{name}</span>
                </button>
            </li>
        }
        .into_any()
    }
}

// ---------------------------------------------------------------------------
// Diff — unified diff with +/- line coloring
// ---------------------------------------------------------------------------

/// A unified diff view. Props shape:
/// ```json
/// { "filename": "src/lib.rs",
///   "lines": [{ "kind": "ctx", "text": "fn main() {" },
///             { "kind": "del", "text": "  old();" },
///             { "kind": "add", "text": "  new();" }] }
/// ```
/// kind is "add" | "del" | "ctx". Alternatively pass `unified: "@@ ...\n+foo\n-bar"`.
#[component]
fn DiffView(kind_props: Value) -> impl IntoView {
    let filename = kind_props
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Either structured `lines`, or a raw `unified` string we parse by prefix.
    let lines: Vec<(String, String)> =
        if let Some(arr) = kind_props.get("lines").and_then(|v| v.as_array()) {
            arr.iter()
                .map(|l| {
                    let kind = l
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("ctx")
                        .to_string();
                    let text = l
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (kind, text)
                })
                .collect()
        } else if let Some(raw) = kind_props.get("unified").and_then(|v| v.as_str()) {
            raw.lines()
                .map(|l| {
                    let kind = match l.chars().next() {
                        Some('+') => "add",
                        Some('-') => "del",
                        Some('@') => "hunk",
                        _ => "ctx",
                    };
                    (kind.to_string(), l.to_string())
                })
                .collect()
        } else {
            Vec::new()
        };

    view! {
        <div class="component-diff">
            {(!filename.is_empty()).then(|| view! { <div class="diff__filename">{filename.clone()}</div> })}
            <pre class="diff__body">
                {lines.into_iter().map(|(kind, text)| {
                    let sym = match kind.as_str() { "add" => "+", "del" => "-", _ => " " };
                    view! {
                        <div class=format!("diff__line diff__line--{kind}")>
                            <span class="diff__gutter">{sym}</span>
                            <span class="diff__text">{text}</span>
                        </div>
                    }
                }).collect::<Vec<_>>()}
            </pre>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Code — syntax block with header + copy button
// ---------------------------------------------------------------------------

/// A code block with a language tag and copy-to-clipboard. Props shape:
/// ```json
/// { "language": "rust", "filename": "main.rs", "code": "fn main() {}" }
/// ```
#[component]
fn CodeView(kind_props: Value) -> impl IntoView {
    let language = kind_props
        .get("language")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let filename = kind_props
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let code = kind_props
        .get("code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let header = if !filename.is_empty() {
        filename.clone()
    } else {
        language.clone()
    };
    let copied = RwSignal::new(false);
    let code_for_copy = code.clone();
    let on_copy = move |_| {
        if let Some(win) = web_sys::window() {
            let clip = win.navigator().clipboard();
            let _ = clip.write_text(&code_for_copy);
            copied.set(true);
        }
    };
    let copy_label = move || if copied.get() { "copied" } else { "copy" };

    view! {
        <div class="component-code">
            <div class="code__head">
                <span class="code__lang">{header}</span>
                <button class="code__copy" type="button" on:click=on_copy>{copy_label}</button>
            </div>
            <pre class="code__body"><code>{code}</code></pre>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Callout — colored info/warn/success/error banner
// ---------------------------------------------------------------------------

/// A colored callout banner. Props shape:
/// ```json
/// { "variant": "warn", "title": "Heads up", "body": "This is destructive." }
/// ```
/// variant is "info" | "success" | "warn" | "error".
#[component]
fn CalloutView(kind_props: Value) -> impl IntoView {
    let variant = kind_props
        .get("variant")
        .and_then(|v| v.as_str())
        .unwrap_or("info")
        .to_string();
    let title = kind_props
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let body = kind_props
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let icon = match variant.as_str() {
        "success" => "✓",
        "warn" => "⚠",
        "error" => "✗",
        _ => "ℹ",
    };

    view! {
        <div class=format!("component-callout component-callout--{variant}")>
            <span class="callout__icon">{icon}</span>
            <div class="callout__body">
                {(!title.is_empty()).then(|| view! { <div class="callout__title">{title.clone()}</div> })}
                {(!body.is_empty()).then(|| view! {
                    <div
                        class="callout__text"
                        inner_html=crate::markdown::render(&body)
                        on:click=crate::host::open_external_link_click
                    ></div>
                })}
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Gallery — image grid
// ---------------------------------------------------------------------------

/// An image gallery grid. Props shape:
/// ```json
/// { "images": [{ "src": "https://... or data:image/png;base64,..", "caption": "before" }] }
/// ```
#[component]
fn GalleryView(kind_props: Value) -> impl IntoView {
    let images = kind_props
        .get("images")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    view! {
        <div class="component-gallery">
            {images.into_iter().map(|img| {
                let src = img.get("src").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let caption = img.get("caption").and_then(|v| v.as_str()).unwrap_or("").to_string();
                view! {
                    <figure class="gallery__item">
                        <img class="gallery__img" src=src loading="lazy" />
                        {(!caption.is_empty()).then(|| view! { <figcaption class="gallery__cap">{caption.clone()}</figcaption> })}
                    </figure>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Confirm — yes/no prompt, emits the choice
// ---------------------------------------------------------------------------

/// A confirm prompt with two buttons. Props shape:
/// ```json
/// { "title": "Delete 10 files?", "body": "This cannot be undone.",
///   "confirm_label": "Delete", "cancel_label": "Cancel", "variant": "error" }
/// ```
/// Emits confirm_response { confirmed: bool }. variant colors the confirm button.
#[component]
fn ConfirmView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let title = kind_props
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Confirm")
        .to_string();
    let body = kind_props
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let confirm_label = kind_props
        .get("confirm_label")
        .and_then(|v| v.as_str())
        .unwrap_or("Confirm")
        .to_string();
    let cancel_label = kind_props
        .get("cancel_label")
        .and_then(|v| v.as_str())
        .unwrap_or("Cancel")
        .to_string();
    let variant = kind_props
        .get("variant")
        .and_then(|v| v.as_str())
        .unwrap_or("info")
        .to_string();

    let answered = RwSignal::new(false);
    let send = {
        let component_id = component_id.clone();
        let daemon = daemon.clone();
        move |confirmed: bool| {
            answered.set(true);
            daemon.send_component_event(
                component_id.clone(),
                serde_json::json!({ "type": "confirm_response", "payload": { "confirmed": confirmed } }),
            );
        }
    };
    let send_yes = send.clone();
    let send_no = send.clone();

    view! {
        <div class="component-confirm">
            <div class="confirm__title">{title}</div>
            {(!body.is_empty()).then(|| view! { <div class="confirm__body">{body.clone()}</div> })}
            <div class="confirm__actions">
                <button class="confirm__btn confirm__btn--cancel" type="button"
                    prop:disabled=move || answered.get()
                    on:click=move |_| send_no(false)>{cancel_label}</button>
                <button class=format!("confirm__btn confirm__btn--{variant}") type="button"
                    prop:disabled=move || answered.get()
                    on:click=move |_| send_yes(true)>{confirm_label}</button>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Map — a live Google Map / Places UI Kit surface. Props:
//   { mode?: "markers"|"place"|"search",        // default inferred from fields
//     center?: {lat,lng}, zoom?,
//     markers?: [{lat,lng,title?}],              // markers mode
//     place_id?: "ChIJ...",                      // place mode (details card)
//     query?: "coffee in Austin",                // search mode (text search)
//     nearby?: {lat,lng,radius?,type?},          // search mode (nearby)
//     fit_markers? }
// Relays marker_clicked / place_selected back to the agent.
// ---------------------------------------------------------------------------
#[component]
fn MapView(component_id: String, kind_props: Value, daemon: Daemon) -> impl IntoView {
    let dom_id = format!("ocean-map-{}", sanitize_id(&component_id));
    let maps_key = daemon.maps_key;
    let maps_map_id = daemon.maps_map_id;

    // Selection callback → component event back to the agent. JS calls it with
    // (event_name: String, payload_json: String).
    let cid = component_id.clone();
    let daemon_cb = daemon.clone();
    let on_event =
        Closure::<dyn FnMut(String, String)>::new(move |event: String, payload: String| {
            let data = serde_json::from_str::<Value>(&payload).unwrap_or_else(|_| json!({}));
            daemon_cb.send_component_event(cid.clone(), json!({ "event": event, "data": data }));
        });
    // Leak so it stays callable from JS for the life of the map (maps are few
    // and long-lived; a small per-render leak is acceptable here).
    let on_event_js: JsValue = on_event.into_js_value();

    let props_str = kind_props.to_string();
    let dom_id_eff = dom_id.clone();
    Effect::new(move |_| {
        let key = maps_key.get();
        let map_id = maps_map_id.get();
        if key.trim().is_empty() {
            return; // config not loaded yet — effect re-runs when the key lands
        }
        let id = dom_id_eff.clone();
        let props = props_str.clone();
        let cb = on_event_js.clone();
        let mid = if map_id.trim().is_empty() {
            "DEMO_MAP_ID".to_string()
        } else {
            map_id
        };
        // Defer a frame so the container div exists in the DOM.
        request_animation_frame(move || {
            ocean_render_map(&id, &key, &mid, &props, &cb);
        });
    });

    view! {
        <div class="block block--map">
            <Show
                when=move || !maps_key.get().trim().is_empty()
                fallback=move || view! {
                    <div class="component-fallback">"map unavailable — Maps API key not configured"</div>
                }
            >
                <div id={dom_id.clone()} class="ocean-map">
                    <div class="ocean-map__loading">"loading map…"</div>
                </div>
            </Show>
        </div>
    }
}

/// Keep only chars safe for a DOM id.
fn sanitize_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Video — embed a clip inline. Props:
//   { url, title?, autoplay?, start? }
// `url` may be a TikTok / Instagram Reel / YouTube / Vimeo link, or a direct
// .mp4/.webm/.m3u8 file. The right embed is chosen from the URL.
// ---------------------------------------------------------------------------
#[derive(Clone)]
enum VideoKind {
    /// Plain iframe embed (YouTube, Vimeo) at this src.
    Iframe(String),
    /// Direct media file → <video> element.
    File(String),
    /// Social embed (TikTok / Instagram) needing the platform embed script.
    /// Carries (platform, canonical_url).
    Social(&'static str, String),
    /// Couldn't classify — show the raw link.
    Unknown(String),
}

fn classify_video(url: &str, start: i64) -> VideoKind {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();

    // Direct media files.
    if lower.ends_with(".mp4")
        || lower.ends_with(".webm")
        || lower.ends_with(".mov")
        || lower.ends_with(".m3u8")
        || lower.ends_with(".ogg")
    {
        return VideoKind::File(u.to_string());
    }

    // YouTube → privacy-friendly nocookie embed.
    if let Some(id) = youtube_id(&lower, u) {
        let mut src = format!("https://www.youtube-nocookie.com/embed/{id}");
        if start > 0 {
            src.push_str(&format!("?start={start}"));
        }
        return VideoKind::Iframe(src);
    }

    // Vimeo → player.vimeo.com/video/<id>.
    if lower.contains("vimeo.com") {
        if let Some(id) = u
            .rsplit('/')
            .find(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())
        {
            return VideoKind::Iframe(format!("https://player.vimeo.com/video/{id}"));
        }
    }

    // TikTok / Instagram → social embed via their script.
    if lower.contains("tiktok.com") {
        return VideoKind::Social("tiktok", u.to_string());
    }
    if lower.contains("instagram.com") {
        return VideoKind::Social("instagram", u.to_string());
    }

    VideoKind::Unknown(u.to_string())
}

/// Pull a YouTube video id from common URL shapes.
fn youtube_id(lower: &str, raw: &str) -> Option<String> {
    if lower.contains("youtu.be/") {
        return raw
            .split("youtu.be/")
            .nth(1)
            .map(|s| s.split(['?', '&', '/']).next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty());
    }
    if lower.contains("youtube.com") {
        // watch?v=ID
        if let Some(rest) = raw.split("v=").nth(1) {
            let id = rest.split('&').next().unwrap_or("").to_string();
            if !id.is_empty() {
                return Some(id);
            }
        }
        // /embed/ID or /shorts/ID
        for marker in ["/embed/", "/shorts/"] {
            if let Some(rest) = raw.split(marker).nth(1) {
                let id = rest.split(['?', '&', '/']).next().unwrap_or("").to_string();
                if !id.is_empty() {
                    return Some(id);
                }
            }
        }
    }
    None
}

#[component]
fn VideoView(component_id: String, kind_props: Value) -> impl IntoView {
    let url = kind_props
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = kind_props
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let autoplay = kind_props
        .get("autoplay")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let start = kind_props
        .get("start")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if url.trim().is_empty() {
        return view! { <div class="block block--video"><div class="video-empty">"(no video url)"</div></div> }.into_any();
    }

    let kind = classify_video(&url, start);

    // Social embeds (TikTok/IG) are injected + processed by their script via JS glue.
    if let VideoKind::Social(platform, canon) = &kind {
        let dom_id = format!("ocean-video-{}", sanitize_id(&component_id));
        let platform = *platform;
        let canon = canon.clone();
        let dom_id_eff = dom_id.clone();
        Effect::new(move |_| {
            let id = dom_id_eff.clone();
            let p = platform.to_string();
            let c = canon.clone();
            request_animation_frame(move || {
                ocean_render_social_video(&id, &p, &c);
            });
        });
        return view! {
            <div class="block block--video">
                {(!title.is_empty()).then(|| view!{ <div class="video__title">{title.clone()}</div> })}
                <div id=dom_id class="video-embed video-embed--social">
                    <div class="video-embed__loading">"loading video…"</div>
                </div>
            </div>
        }.into_any();
    }

    let body = match kind {
        VideoKind::Iframe(src) => {
            // Build the iframe as raw HTML to sidestep macro attr limitations
            // (frameborder/allowfullscreen/allow). src is provider-derived, not
            // user free-text, but escape quotes defensively.
            let safe = src.replace('"', "%22");
            let html = format!(
                "<iframe src=\"{safe}\" frameborder=\"0\" allowfullscreen \
                 allow=\"accelerometer; autoplay; clipboard-write; encrypted-media; \
                 gyroscope; picture-in-picture; web-share\"></iframe>"
            );
            view! { <div class="video-embed video-embed--16x9" inner_html=html></div> }.into_any()
        }
        VideoKind::File(src) => view! {
            <div class="video-embed">
                <video
                    src=src
                    controls=true
                    autoplay=autoplay
                    playsinline=true
                    class="video-file"
                ></video>
            </div>
        }
        .into_any(),
        VideoKind::Unknown(u) => {
            let href = u.clone();
            view! {
                <div class="video-embed video-embed--unknown">
                    <a
                        href=href
                        target="_blank"
                        rel="noopener"
                        on:click=crate::host::open_external_link_click
                    >{u}</a>
                </div>
            }
            .into_any()
        }
        VideoKind::Social(_, _) => {
            unreachable!("Social embeds are handled by the early return above")
        }
    };

    view! {
        <div class="block block--video">
            {(!title.is_empty()).then(|| view!{ <div class="video__title">{title.clone()}</div> })}
            {body}
        </div>
    }
    .into_any()
}

/// Permission-approval overlay (OCEAN-64).
///
/// When the daemon runs with permission-gating on, a mutating tool call
/// (write / edit / bash) BLOCKS until the operator posts a decision. The daemon
/// emits a `permission_request` on the control stream; `Daemon` collects them in
/// `pending_permissions`. This renders one prominent card per pending request —
/// stacked, oldest first — each with Approve / Deny. Clicking POSTs the decision
/// and clears the card; a decision made elsewhere (e.g. the TUI) clears it via
/// the `permission_decision` frame. A pending request blocks the turn, so the
/// stack is fixed at the bottom of the viewport above the composer and can't be
/// scrolled away.
#[component]
pub fn PermissionPrompts(daemon: Daemon) -> impl IntoView {
    let pending = daemon.pending_permissions;
    let daemon = StoredValue::new(daemon);

    view! {
        <Show when=move || !pending.get().is_empty()>
            <div class="ocean-perms" role="region" aria-label="permission requests">
                <For
                    each=move || pending.get()
                    key=|p| p.permission_id.clone()
                    children=move |p| {
                        let allow_id = p.permission_id.clone();
                        let deny_id = p.permission_id.clone();
                        let deciding = p.deciding;
                        // A card raised by another surface (no local decision
                        // token bound to its request) is read-only here: this
                        // surface can't produce the token the daemon's gate
                        // requires, so the buttons disable and `decide_permission`
                        // refuses regardless (TASK-44, finding 4).
                        let actionable = p.actionable;
                        let has_args = !p.args_summary.trim().is_empty();
                        let tool = p.tool.clone();
                        let reason = p.reason.clone();
                        let args_summary = p.args_summary.clone();
                        view! {
                            <div
                                class="ocean-perm"
                                class:is-deciding=move || deciding
                                class:is-readonly=move || !actionable
                            >
                                <div class="ocean-perm__head">
                                    <span class="ocean-perm__badge">"permission"</span>
                                    <span class="ocean-perm__tool">{tool}</span>
                                </div>
                                <div class="ocean-perm__reason">{reason}</div>
                                {has_args.then(|| view! {
                                    <pre class="ocean-perm__args">{args_summary.clone()}</pre>
                                })}
                                {(!actionable).then(|| view! {
                                    <p class="ocean-perm__note">
                                        "Awaiting a decision from the surface that started this turn."
                                    </p>
                                })}
                                <Show when=move || actionable>
                                    <div class="ocean-perm__actions">
                                        <button
                                            class="ocean-perm__deny"
                                            type="button"
                                            disabled=deciding
                                            on:click={
                                                let deny_id = deny_id.clone();
                                                move |_| daemon.with_value(|d| {
                                                    d.decide_permission(deny_id.clone(), false)
                                                })
                                            }
                                        >
                                            "Deny"
                                        </button>
                                        <button
                                            class="ocean-perm__approve"
                                            type="button"
                                            disabled=deciding
                                            on:click={
                                                let allow_id = allow_id.clone();
                                                move |_| daemon.with_value(|d| {
                                                    d.decide_permission(allow_id.clone(), true)
                                                })
                                            }
                                        >
                                            {move || if deciding { "…" } else { "Approve" }}
                                        </button>
                                    </div>
                                </Show>
                            </div>
                        }
                    }
                />
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Pinned rail — docked widgets that persist across turns
// ---------------------------------------------------------------------------

/// The persistent pinned rail: widgets the agent docked with
/// `props.placement == "pinned"` (map/player/metrics that stay visible across
/// turns, outside the chat scroll). Collapses to nothing when empty (absence,
/// not chrome); each card reuses [`ComponentView`] and carries a ghost unpin
/// affordance. Desktop lays out as a side rail; compact/mobile collapses to a
/// stacked dock (see `panels.css`). Session-scoped via the daemon registry.
#[component]
pub fn PinnedRail(daemon: Daemon) -> impl IntoView {
    let pinned = daemon.pinned_widgets;
    // StoredValue (Copy): the <Show> children closure must be `Fn`, so it
    // can't move a plain `Daemon` clone into the <For> children below.
    let daemon = StoredValue::new(daemon);
    view! {
        <Show when=move || !pinned.with(Vec::is_empty)>
            <aside class="pinned-rail" aria-label="Pinned widgets">
                <For
                    each=move || pinned.get()
                    key=|w| w.component_id.clone()
                    children=move |widget| {
                        view! {
                            <PinnedCard widget daemon=daemon.get_value() />
                        }
                    }
                />
            </aside>
        </Show>
    }
}

/// One docked widget: the component render plus a ghost unpin (×) affordance.
#[component]
fn PinnedCard(widget: PinnedWidget, daemon: Daemon) -> impl IntoView {
    let component_id = widget.component_id.clone();
    let kind = widget.kind.clone();
    let props = widget.props.clone();
    let unpin_id = widget.component_id.clone();
    let daemon_unpin = daemon.clone();
    view! {
        <section class="pinned-card">
            <button
                class="pinned-card__unpin"
                type="button"
                aria-label="unpin widget"
                title="Unpin from rail"
                on:click=move |_| daemon_unpin.unpin_widget(&unpin_id)
            >
                "×"
            </button>
            <div class="pinned-card__body">
                <ComponentView component_id kind kind_props=props daemon />
            </div>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn extracted_youtube_id(raw: &str) -> Option<String> {
        youtube_id(&raw.to_ascii_lowercase(), raw)
    }

    #[test]
    fn cell_text_renders_json_scalars_and_current_compound_values() {
        assert_eq!(cell_text(&json!("ocean")), "ocean");
        assert_eq!(cell_text(&json!(73)), "73");
        assert_eq!(cell_text(&json!(true)), "true");
        assert_eq!(cell_text(&json!(false)), "false");
        assert_eq!(cell_text(&Value::Null), "");
        assert_eq!(cell_text(&json!({"a": 1})), r#"{"a":1}"#);
        assert_eq!(cell_text(&json!([1, "two"])), r#"[1,"two"]"#);
    }

    #[test]
    fn sanitize_id_keeps_ascii_id_chars_and_replaces_everything_else() {
        assert_eq!(sanitize_id("Az-09_ok"), "Az-09_ok");

        let sanitized = sanitize_id("a b<\"/é_x-9");

        assert_eq!(sanitized, "a-b----_x-9");
        assert!(sanitized
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(![' ', '<', '"', '/', 'é']
            .iter()
            .any(|&unsafe_char| sanitized.contains(unsafe_char)));
    }

    #[test]
    fn classify_video_returns_youtube_nocookie_iframe_with_start() {
        match classify_video(
            "https://www.youtube.com/watch?v=AbC-123_xy&feature=share",
            42,
        ) {
            VideoKind::Iframe(src) => {
                assert_eq!(
                    src,
                    "https://www.youtube-nocookie.com/embed/AbC-123_xy?start=42"
                );
            }
            _ => panic!("expected YouTube iframe"),
        }
    }

    #[test]
    fn classify_video_returns_youtu_be_short_link_as_nocookie_iframe() {
        match classify_video("https://youtu.be/Short_Id-42?si=share", 0) {
            VideoKind::Iframe(src) => {
                assert_eq!(src, "https://www.youtube-nocookie.com/embed/Short_Id-42");
            }
            _ => panic!("expected youtu.be iframe"),
        }
    }

    #[test]
    fn classify_video_returns_vimeo_player_iframe() {
        match classify_video("https://vimeo.com/channels/staffpicks/123456789", 0) {
            VideoKind::Iframe(src) => {
                assert_eq!(src, "https://player.vimeo.com/video/123456789");
            }
            _ => panic!("expected Vimeo iframe"),
        }
    }

    #[test]
    fn classify_video_preserves_trimmed_direct_media_file_url() {
        match classify_video("  https://cdn.example.com/video/demo.MP4  ", 0) {
            VideoKind::File(src) => {
                assert_eq!(src, "https://cdn.example.com/video/demo.MP4");
            }
            _ => panic!("expected direct media file"),
        }
    }

    #[test]
    fn classify_video_returns_social_platform_and_canonical_url() {
        match classify_video(" https://www.tiktok.com/@ocean/video/123 ", 0) {
            VideoKind::Social(platform, canon) => {
                assert_eq!(platform, "tiktok");
                assert_eq!(canon, "https://www.tiktok.com/@ocean/video/123");
            }
            _ => panic!("expected TikTok social embed"),
        }

        match classify_video("https://www.instagram.com/reel/ABC123/", 0) {
            VideoKind::Social(platform, canon) => {
                assert_eq!(platform, "instagram");
                assert_eq!(canon, "https://www.instagram.com/reel/ABC123/");
            }
            _ => panic!("expected Instagram social embed"),
        }
    }

    #[test]
    fn classify_video_returns_unknown_for_non_video_url() {
        match classify_video(" https://example.com/post ", 0) {
            VideoKind::Unknown(url) => {
                assert_eq!(url, "https://example.com/post");
            }
            _ => panic!("expected unknown video kind"),
        }
    }

    #[test]
    fn youtube_id_extracts_common_url_shapes() {
        let cases = [
            ("https://youtu.be/ShortId_1-2?si=share", Some("ShortId_1-2")),
            ("https://www.youtube.com/watch?v=WatchId", Some("WatchId")),
            (
                "https://www.youtube.com/watch?v=Watch-Id_42&feature=share",
                Some("Watch-Id_42"),
            ),
            (
                "https://www.youtube.com/embed/Embed_123-xy",
                Some("Embed_123-xy"),
            ),
            (
                "https://www.youtube.com/shorts/Shorts_123-xy?feature=share",
                Some("Shorts_123-xy"),
            ),
            ("https://example.com/watch?v=not-youtube", None),
        ];

        for (raw, expected) in cases {
            assert_eq!(extracted_youtube_id(raw).as_deref(), expected, "{raw}");
        }
    }

    #[test]
    fn compact_format_strips_trailing_zeros_and_caps_at_two_decimals() {
        // Spec examples: 29.8, 1.03, 0.5, 12.
        assert_eq!(compact_format(29.8), "29.8");
        assert_eq!(compact_format(1.03), "1.03");
        assert_eq!(compact_format(0.5), "0.5");
        assert_eq!(compact_format(12.0), "12");
        assert_eq!(compact_format(0.0), "0");
    }

    #[test]
    fn compact_format_inserts_thousands_separators_at_four_digits() {
        assert_eq!(compact_format(12400.0), "12,400");
        assert_eq!(compact_format(1000.0), "1,000");
        assert_eq!(compact_format(1_234_567.89), "1,234,567.89");
        // Rounding carries into the next thousand before separators apply.
        assert_eq!(compact_format(999.999), "1,000");
        assert_eq!(compact_format(1234.5), "1,234.5");
    }

    #[test]
    fn compact_format_keeps_true_value_for_negatives() {
        assert_eq!(compact_format(-3.5), "-3.5");
        assert_eq!(compact_format(-12400.0), "-12,400");
        // Rounds to zero magnitude -> plain "0", never "-0".
        assert_eq!(compact_format(-0.001), "0");
        assert_eq!(compact_format(-0.0), "0");
    }

    // ── Path resolver tests ───────────────────────────────────────────

    #[test]
    fn resolve_absolute_path_returns_as_is() {
        assert_eq!(
            resolve_file_path("/workspace", Some("/workspace"), "/home/user/file.rs"),
            "/home/user/file.rs"
        );
    }

    #[test]
    fn resolve_home_relative_returns_as_is() {
        assert_eq!(
            resolve_file_path("/workspace", Some("/workspace"), "~/file.rs"),
            "~/file.rs"
        );
    }

    #[test]
    fn resolve_relative_with_cwd_match() {
        assert_eq!(
            resolve_file_path("/workspace", Some("/workspace/src"), "main.rs"),
            "/workspace/src/main.rs"
        );
    }

    #[test]
    fn resolve_relative_cwd_authoritative_even_outside_root() {
        // cwd is somewhere outside workspace_root — cwd is still authoritative
        // for relative paths per the frozen contract.
        assert_eq!(
            resolve_file_path("/workspace", Some("/other/project"), "lib.rs"),
            "/other/project/lib.rs"
        );
    }

    #[test]
    fn resolve_relative_no_cwd_uses_workspace_root() {
        assert_eq!(
            resolve_file_path("/workspace", None, "lib.rs"),
            "/workspace/lib.rs"
        );
    }

    #[test]
    fn normalize_path_collapses_dot_segments() {
        assert_eq!(normalize_path("/a/b/./c"), "/a/b/c");
        assert_eq!(normalize_path("/a/./b/./c"), "/a/b/c");
    }

    #[test]
    fn normalize_path_collapses_parent_segments() {
        assert_eq!(normalize_path("/a/b/../c"), "/a/c");
        assert_eq!(normalize_path("/a/b/c/../../d"), "/a/d");
    }

    #[test]
    fn normalize_path_parent_past_root_stays_at_root() {
        assert_eq!(normalize_path("/.."), "/");
        assert_eq!(normalize_path("/a/../../.."), "/");
    }

    #[test]
    fn normalize_relative_path() {
        assert_eq!(normalize_path("a/b/c"), "a/b/c");
        assert_eq!(normalize_path("a/b/../c"), "a/c");
        assert_eq!(normalize_path("a/../.."), ".");
        assert_eq!(normalize_path("."), ".");
    }

    #[test]
    fn join_path_simple() {
        assert_eq!(join_path("/a/b", "c"), "/a/b/c");
        assert_eq!(join_path("/a/b/", "c"), "/a/b/c");
        assert_eq!(join_path("/a", "b/../c"), "/a/c");
    }

    #[test]
    fn file_icon_labels() {
        assert_eq!(file_icon_label("main.rs"), "code");
        assert_eq!(file_icon_label("Cargo.toml"), "code");
        assert_eq!(file_icon_label("README.md"), "code");
        assert_eq!(file_icon_label(".gitignore"), "git");
        assert_eq!(file_icon_label("Dockerfile"), "code");
        assert_eq!(file_icon_label("unknown.xyz"), "code");
    }

    // -- Production routing: resolve_file_tree_path (unified) -------------------

    // Vector A (Codex): absent path — root=src, cwd=/proj, no entry.path,
    // ancestor=tools, name=mod.rs → /proj/src/tools/mod.rs
    #[test]
    fn resolve_absent_path_assemble_chain_with_relative_root() {
        let resolved = resolve_file_tree_path(
            None,     // no entry.path
            "tools",  // ancestor_prefix (dir chain from root)
            "mod.rs", // name
            "src",    // workspace_root (relative)
            Some("/proj"),
        );
        assert_eq!(resolved, "/proj/src/tools/mod.rs");
    }

    // Vector B (Codex): explicit relative — entry.path=main.rs, root=src,
    // cwd=/proj → /proj/main.rs (cwd authoritative, NOT src/main.rs)
    #[test]
    fn resolve_explicit_relative_cwd_authoritative() {
        let resolved = resolve_file_tree_path(
            Some("main.rs"), // explicit agent-provided path
            "",              // ancestor_prefix (root level)
            "mod.rs",        // name
            "src",           // workspace_root (relative)
            Some("/proj"),
        );
        assert_eq!(resolved, "/proj/main.rs");
    }

    // Explicit absolute → passthrough regardless of root/cwd/ancestor.
    #[test]
    fn resolve_explicit_absolute_passthrough() {
        let resolved =
            resolve_file_tree_path(Some("/etc/hosts"), "unused", "unused", "src", Some("/proj"));
        assert_eq!(resolved, "/etc/hosts");
    }

    // Explicit home-relative → passthrough.
    #[test]
    fn resolve_explicit_home_relative_passthrough() {
        let resolved = resolve_file_tree_path(Some("~/.ssh/config"), "", "", "src", Some("/proj"));
        assert_eq!(resolved, "~/.ssh/config");
    }

    // Absent path, absolute root — no cwd resolution needed.
    #[test]
    fn resolve_absent_path_absolute_root() {
        let resolved = resolve_file_tree_path(None, "lib", "mod.rs", "/proj/src", Some("/proj"));
        assert_eq!(resolved, "/proj/src/lib/mod.rs");
    }

    // Explicit relative, no cwd — falls back to workspace_root.
    #[test]
    fn resolve_explicit_relative_no_cwd_fallback() {
        let resolved = resolve_file_tree_path(Some("main.rs"), "", "", "src", None);
        assert_eq!(resolved, "src/main.rs");
    }

    // Absent path, relative root, no cwd — joins literally.
    #[test]
    fn resolve_absent_path_relative_root_no_cwd() {
        let resolved = resolve_file_tree_path(None, "tools", "mod.rs", "src", None);
        assert_eq!(resolved, "src/tools/mod.rs");
    }

    // Explicit relative with absolute root — cwd authoritative.
    #[test]
    fn resolve_explicit_relative_absolute_root() {
        let resolved = resolve_file_tree_path(
            Some("lib.rs"),
            "",
            "",
            "/home/project",
            Some("/home/project/src"),
        );
        assert_eq!(resolved, "/home/project/src/lib.rs");
    }
}
