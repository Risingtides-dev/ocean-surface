//! Browser cockpit — v1 renders the agent-driven browser session as a pure
//! reducer over browser_* tool calls already present in the session's turns
//! (action timeline, latest PageRead, latest screenshot). Zero daemon changes.
//! Owned by the Cockpit workstream (feat/desktop-cockpit).
//!
//! # Screenshot gap
//! `Block::ToolCall::output` is a plain `String` with no image/binary storage.
//! `browser_screenshot` results that carry a base64 data-URL arrive as raw text
//! and are not structurally distinguishable from any other tool output. Until
//! `Block` gains an image variant (or `ToolCall` gains an `images: Vec<TurnImage>`
//! field), screenshots are skipped. The `CockpitState::screenshot` field is
//! always `None`.

use leptos::prelude::*;
use serde_json::Value;

use crate::daemon::Daemon;
use crate::model::{Block, ToolStatus, Turn};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// The operation a browser tool performed, derived from its tool name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserOp {
    Navigate,
    Click,
    Type,
    Scroll,
    Eval,
    Read,
    Screenshot,
    Other,
}

/// One browser action the agent performed in a single turn.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserAction {
    pub op: BrowserOp,
    /// Human-readable summary derived from the tool's key arguments.
    pub summary: String,
    /// Zero-based turn index in the session's turn list.
    pub turn_ordinal: usize,
}

/// The current page as last reported by `browser_read_page`.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSnapshot {
    pub url: String,
    pub title: String,
    pub element_count: usize,
    /// e.g. "above-the-fold" or "full-page"
    pub visual_hint: Option<String>,
}

/// The folded state for the entire session.
#[derive(Debug, Clone, PartialEq)]
pub struct CockpitState {
    pub actions: Vec<BrowserAction>,
    pub page: Option<PageSnapshot>,
    /// Always `None` in the current model — see module-level "Screenshot gap".
    pub screenshot: Option<String>,
    /// True while any `browser_*` tool call is `Running` in the newest turn.
    pub active: bool,
}

// ---------------------------------------------------------------------------
// Reducer
// ---------------------------------------------------------------------------

/// Map a `browser_*` tool name to its operation.
fn op_from_name(name: &str) -> BrowserOp {
    if name.contains("navigate") {
        BrowserOp::Navigate
    } else if name.contains("click") {
        BrowserOp::Click
    } else if name.contains("type") {
        BrowserOp::Type
    } else if name.contains("scroll") {
        BrowserOp::Scroll
    } else if name.contains("eval") {
        BrowserOp::Eval
    } else if name.contains("read") {
        BrowserOp::Read
    } else if name.contains("screenshot") {
        BrowserOp::Screenshot
    } else {
        BrowserOp::Other
    }
}

/// Derive a human-readable one-line summary from the tool's JSON args.
/// `args_preview` is the daemon's truncated (≤60-char) serialisation of
/// `call.args_json`.  We parse it as JSON to extract the key fields;
/// if parsing fails we fall back to a stripped tool name.
fn summary_from_args(name: &str, args_preview: &str) -> String {
    let args: Value = serde_json::from_str(args_preview).unwrap_or(Value::Null);
    let op = op_from_name(name);
    match op {
        BrowserOp::Navigate => args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        BrowserOp::Click => args
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        BrowserOp::Type => {
            let sel = args.get("selector").and_then(|v| v.as_str()).unwrap_or("?");
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("?");
            format!("{} → \"{}\"", sel, text)
        }
        BrowserOp::Scroll => args
            .get("direction")
            .or_else(|| args.get("selector"))
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
        BrowserOp::Read => args
            .get("selector")
            .and_then(|v| v.as_str())
            .unwrap_or("full page")
            .to_string(),
        BrowserOp::Eval | BrowserOp::Screenshot | BrowserOp::Other => {
            name.strip_prefix("browser_").unwrap_or(name).to_string()
        }
    }
}

/// Walk every tool-call block whose name starts with `browser_` and fold into
/// a `CockpitState`.  Pure — no side effects, no daemon access.
pub fn reduce(turns: &[Turn]) -> CockpitState {
    let mut actions: Vec<BrowserAction> = Vec::new();
    let mut page: Option<PageSnapshot> = None;
    let mut active = false;

    for (turn_idx, turn) in turns.iter().enumerate() {
        for block in &turn.blocks {
            let Block::ToolCall {
                name,
                args_preview,
                output,
                status,
                ..
            } = block
            else {
                continue;
            };

            if !name.starts_with("browser_") {
                continue;
            }

            let op = op_from_name(name);
            let summary = summary_from_args(name, args_preview);
            actions.push(BrowserAction {
                op,
                summary,
                turn_ordinal: turn_idx,
            });

            // Parse browser_read_page result
            if name == "browser_read_page" && matches!(status, ToolStatus::Ok) && !output.is_empty()
            {
                // Malformed JSON is skipped — no panic.
                if let Ok(val) = serde_json::from_str::<Value>(output) {
                    let url = val
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let title = val
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let element_count = val
                        .get("elements")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    let visual_hint = val
                        .get("visual_hint")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    page = Some(PageSnapshot {
                        url,
                        title,
                        element_count,
                        visual_hint,
                    });
                }
            }

            // Check for in-progress browser tool calls
            if *status == ToolStatus::Running {
                active = true;
            }
        }
    }

    CockpitState {
        actions,
        page,
        screenshot: None,
        active,
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// A stateless action icon for an operation. Returns a single Unicode glyph
/// that fits in a tight inline chip.
fn op_glyph(op: BrowserOp) -> &'static str {
    match op {
        BrowserOp::Navigate => "→",
        BrowserOp::Click => "◎",
        BrowserOp::Type => "⌨",
        BrowserOp::Scroll => "↕",
        BrowserOp::Eval => "⟐",
        BrowserOp::Read => "☰",
        BrowserOp::Screenshot => "◻",
        BrowserOp::Other => "•",
    }
}

#[component]
pub fn BrowserCockpit(daemon: Daemon) -> impl IntoView {
    let turns = daemon.turns;

    // Pure derived signal: rebuild CockpitState whenever turns change.
    let state = Signal::derive(move || reduce(&turns.get()));

    view! {
        <div class="deck-browser-cockpit">
            {move || {
                let s = state.get();
                if s.actions.is_empty() {
                    view! {
                        <div class="deck-browser-empty">
                            <p class="deck-browser-empty-text">
                                "Agent hasn't driven the browser this session"
                            </p>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <div class="deck-browser-content">
                            {page_card(&s.page)}
                            {action_timeline(&s.actions, s.active)}
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
    .into_any()
}

fn page_card(page: &Option<PageSnapshot>) -> impl IntoView {
    let (title, url, element_count, visual_hint) = match page {
        Some(p) => (
            p.title.clone(),
            p.url.clone(),
            p.element_count,
            p.visual_hint.clone(),
        ),
        None => (String::new(), String::new(), 0, None),
    };

    view! {
        <div class="deck-browser-page">
            {{
                if title.is_empty() && url.is_empty() {
                    view! { <div class="deck-browser-page-empty">"No page read yet"</div> }
                        .into_any()
                } else {
                    let hint_view = visual_hint.clone().map(|h| {
                        view! { <span class="deck-browser-page-hint">{h}</span> }
                    });
                    view! {
                        <div class="deck-browser-page-card">
                            <div class="deck-browser-page-title">{title.clone()}</div>
                            <div class="deck-browser-page-url">{url.clone()}</div>
                            <div class="deck-browser-page-meta">
                                <span class="deck-browser-page-elements">
                                    {format!("{} elements", element_count)}
                                </span>
                                {hint_view}
                            </div>
                        </div>
                    }
                    .into_any()
                }
            }}
        </div>
    }
}

fn action_timeline(actions: &[BrowserAction], active: bool) -> impl IntoView {
    // Most recent first.
    let mut items: Vec<&BrowserAction> = actions.iter().collect();
    items.reverse();

    view! {
        <div class="deck-browser-timeline">
            <div class="deck-browser-timeline-header">
                {format!("Actions ({})", actions.len())}
                {if active {
                    view! { <span class="deck-browser-active-dot">"●"</span> }.into_any()
                } else {
                    view! { <span></span> }.into_any()
                }}
            </div>
            {items.into_iter().map(|action| {
                let op = action.op;
                let summary = action.summary.clone();
                let ordinal = action.turn_ordinal;
                view! {
                    <div class="deck-browser-action">
                        <span class="deck-browser-action-glyph">{op_glyph(op)}</span>
                        <span class="deck-browser-action-summary">{summary}</span>
                        <span class="deck-browser-action-turn">{"#"}{ordinal}</span>
                    </div>
                }
            }).collect::<Vec<_>>()}
        </div>
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Block, Role, ToolStatus, Turn};

    fn tool_block(
        call_id: &str,
        name: &str,
        args_preview: &str,
        output: &str,
        status: ToolStatus,
    ) -> Block {
        Block::ToolCall {
            call_id: call_id.into(),
            name: name.into(),
            args_preview: args_preview.into(),
            output: output.into(),
            status,
            expanded: false,
        }
    }

    fn assistant_turn(blocks: Vec<Block>) -> Turn {
        Turn {
            turn_id: Some("t1".into()),
            role: Role::Assistant,
            blocks,
        }
    }

    // ------------------------------------------------------------------
    // op_from_name
    // ------------------------------------------------------------------

    #[test]
    fn op_from_name_navigate() {
        assert_eq!(op_from_name("browser_navigate"), BrowserOp::Navigate);
    }

    #[test]
    fn op_from_name_click() {
        assert_eq!(op_from_name("browser_click"), BrowserOp::Click);
    }

    #[test]
    fn op_from_name_type() {
        assert_eq!(op_from_name("browser_type"), BrowserOp::Type);
    }

    #[test]
    fn op_from_name_scroll() {
        assert_eq!(op_from_name("browser_scroll"), BrowserOp::Scroll);
    }

    #[test]
    fn op_from_name_eval() {
        assert_eq!(op_from_name("browser_eval"), BrowserOp::Eval);
    }

    #[test]
    fn op_from_name_read() {
        assert_eq!(op_from_name("browser_read_page"), BrowserOp::Read);
    }

    #[test]
    fn op_from_name_screenshot() {
        assert_eq!(op_from_name("browser_screenshot"), BrowserOp::Screenshot);
    }

    #[test]
    fn op_from_name_unknown() {
        assert_eq!(op_from_name("browser_magic"), BrowserOp::Other);
    }

    // ------------------------------------------------------------------
    // summary_from_args
    // ------------------------------------------------------------------

    #[test]
    fn summary_from_navigate_args() {
        let s = summary_from_args("browser_navigate", r#"{"url":"https://example.com"}"#);
        assert_eq!(s, "https://example.com");
    }

    #[test]
    fn summary_from_click_args() {
        let s = summary_from_args("browser_click", r##"{"selector":"#submit-btn"}"##);
        assert_eq!(s, "#submit-btn");
    }

    #[test]
    fn summary_from_type_args() {
        let s = summary_from_args(
            "browser_type",
            r#"{"selector":"input[name=q]","text":"hello"}"#,
        );
        assert_eq!(s, "input[name=q] → \"hello\"");
    }

    #[test]
    fn summary_from_scroll_args() {
        let s = summary_from_args("browser_scroll", r#"{"direction":"down"}"#);
        assert_eq!(s, "down");
    }

    #[test]
    fn summary_from_read_args() {
        let s = summary_from_args("browser_read_page", r##"{"selector":"#main"}"##);
        assert_eq!(s, "#main");
    }

    #[test]
    fn summary_from_read_args_defaults_to_full_page() {
        let s = summary_from_args("browser_read_page", r#"{}"#);
        assert_eq!(s, "full page");
    }

    #[test]
    fn summary_malformed_json_falls_back() {
        let s = summary_from_args("browser_navigate", "not-json");
        assert_eq!(s, "?"); // url absent → "?"
    }

    // ------------------------------------------------------------------
    // reduce — empty session
    // ------------------------------------------------------------------

    #[test]
    fn reduce_empty_session() {
        let state = reduce(&[]);
        assert!(state.actions.is_empty());
        assert!(state.page.is_none());
        assert!(!state.active);
    }

    // ------------------------------------------------------------------
    // reduce — navigate + click + read sequence
    // ------------------------------------------------------------------

    #[test]
    fn reduce_nav_click_read_sequence() {
        let turns = vec![
            // Turn 0: navigate to example.com
            assistant_turn(vec![tool_block(
                "c0",
                "browser_navigate",
                r#"{"url":"https://example.com"}"#,
                "",
                ToolStatus::Ok,
            )]),
            // Turn 1: click a button + read the page
            assistant_turn(vec![
                tool_block(
                    "c1",
                    "browser_click",
                    r##"{"selector":"#btn"}"##,
                    "",
                    ToolStatus::Ok,
                ),
                tool_block(
                    "c2",
                    "browser_read_page",
                    r#"{}"#,
                    r#"{"url":"https://example.com","title":"Example Domain","elements":[{"role":"heading","name":"Example"}],"visual_hint":"above-the-fold"}"#,
                    ToolStatus::Ok,
                ),
            ]),
        ];

        let state = reduce(&turns);

        // Actions
        assert_eq!(state.actions.len(), 3);
        assert_eq!(state.actions[0].op, BrowserOp::Navigate);
        assert_eq!(state.actions[0].summary, "https://example.com");
        assert_eq!(state.actions[0].turn_ordinal, 0);
        assert_eq!(state.actions[1].op, BrowserOp::Click);
        assert_eq!(state.actions[1].summary, "#btn");
        assert_eq!(state.actions[1].turn_ordinal, 1);
        assert_eq!(state.actions[2].op, BrowserOp::Read);
        assert_eq!(state.actions[2].turn_ordinal, 1);

        // Page snapshot
        let page = state.page.unwrap();
        assert_eq!(page.url, "https://example.com");
        assert_eq!(page.title, "Example Domain");
        assert_eq!(page.element_count, 1);
        assert_eq!(page.visual_hint.as_deref(), Some("above-the-fold"));

        assert!(!state.active);
    }

    // ------------------------------------------------------------------
    // reduce — malformed read_page JSON is skipped
    // ------------------------------------------------------------------

    #[test]
    fn reduce_malformed_read_page_skipped() {
        let turns = vec![assistant_turn(vec![tool_block(
            "c0",
            "browser_read_page",
            r#"{}"#,
            "not-json-at-all",
            ToolStatus::Ok,
        )])];

        let state = reduce(&turns);
        assert_eq!(state.actions.len(), 1);
        assert!(
            state.page.is_none(),
            "malformed result JSON should be skipped"
        );
    }

    // ------------------------------------------------------------------
    // reduce — active flag when a browser tool is Running
    // ------------------------------------------------------------------

    #[test]
    fn reduce_active_when_running() {
        let turns = vec![assistant_turn(vec![tool_block(
            "c0",
            "browser_navigate",
            r#"{"url":"https://x.com"}"#,
            "",
            ToolStatus::Running,
        )])];

        let state = reduce(&turns);
        assert!(state.active);
    }

    // ------------------------------------------------------------------
    // reduce — browser_read_page with Err status does not update page
    // ------------------------------------------------------------------

    #[test]
    fn reduce_read_page_err_skipped() {
        let turns = vec![assistant_turn(vec![tool_block(
            "c0",
            "browser_read_page",
            r#"{}"#,
            r#"{"url":"https://x.com","title":"X"}"#,
            ToolStatus::Err,
        )])];

        let state = reduce(&turns);
        assert_eq!(state.actions.len(), 1);
        assert!(state.page.is_none(), "Err read_page should not set page");
    }

    // ------------------------------------------------------------------
    // reduce — only latest read_page wins
    // ------------------------------------------------------------------

    #[test]
    fn reduce_latest_read_page_wins() {
        let turns = vec![assistant_turn(vec![
            tool_block(
                "c0",
                "browser_read_page",
                r#"{}"#,
                r#"{"url":"https://first.com","title":"First"}"#,
                ToolStatus::Ok,
            ),
            tool_block(
                "c1",
                "browser_read_page",
                r#"{}"#,
                r#"{"url":"https://second.com","title":"Second"}"#,
                ToolStatus::Ok,
            ),
        ])];

        let state = reduce(&turns);
        let page = state.page.unwrap();
        assert_eq!(page.title, "Second");
    }

    // ------------------------------------------------------------------
    // reduce — empty elements array yields zero count
    // ------------------------------------------------------------------

    #[test]
    fn reduce_empty_elements() {
        let turns = vec![assistant_turn(vec![tool_block(
            "c0",
            "browser_read_page",
            r#"{}"#,
            r#"{"url":"https://x.com","title":"X","elements":[]}"#,
            ToolStatus::Ok,
        )])];

        let state = reduce(&turns);
        assert_eq!(state.page.unwrap().element_count, 0);
    }

    // ------------------------------------------------------------------
    // reduce — missing visual_hint is None
    // ------------------------------------------------------------------

    #[test]
    fn reduce_missing_visual_hint() {
        let turns = vec![assistant_turn(vec![tool_block(
            "c0",
            "browser_read_page",
            r#"{}"#,
            r#"{"url":"https://x.com","title":"X"}"#,
            ToolStatus::Ok,
        )])];

        let state = reduce(&turns);
        assert!(state.page.unwrap().visual_hint.is_none());
    }

    // ------------------------------------------------------------------
    // reduce — non-browser tool calls are ignored
    // ------------------------------------------------------------------

    #[test]
    fn reduce_ignores_non_browser_tools() {
        let turns = vec![assistant_turn(vec![
            tool_block("c0", "read", "{}", "some file content", ToolStatus::Ok),
            tool_block(
                "c1",
                "browser_navigate",
                r#"{"url":"https://example.com"}"#,
                "",
                ToolStatus::Ok,
            ),
            tool_block("c2", "bash", "{}", "ls output", ToolStatus::Ok),
        ])];

        let state = reduce(&turns);
        assert_eq!(state.actions.len(), 1);
        assert_eq!(state.actions[0].op, BrowserOp::Navigate);
    }
}
