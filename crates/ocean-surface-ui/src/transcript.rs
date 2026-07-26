//! Renders the conversation. No visible turn role headers; assistant turns
//! use unified `transcript-disclosure` classes for tool groups, thinking
//! groups, and individual tool rows — each with explicit `aria-expanded`.
//! Collapsed Thinking pills and tool chips with status color.
//!
//! Everything derives from the `turns` signal so streaming deltas reflect
//! live. Turns are keyed by index for stable DOM; within a turn the block
//! list is rebuilt on each change (cheap for chat-sized content, and avoids
//! stale snapshots that would freeze streaming text).
//!
//! LiveActivity — a pure reducer-driven row at transcript tail (outside the
//! turn For loop) replaces the heavy SoundingsThinking WebGL plate and the
//! per-assistant ocean-status-row. Contract: TASK-22 v1.1 freeze.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::ComponentView;
use crate::daemon::Daemon;
use crate::icons::WaveBadge;
use crate::model::{Block, Role, ToolStatus};

/// A single live-activity descriptor from the reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveActivity {
    /// No activity to report (not streaming).
    Hidden,
    /// Waiting for the operator to approve a permission request.
    WaitingForApproval,
    /// User turn submitted, daemon hasn't returned a first token yet.
    GettingStarted,
    /// The agent is reasoning (Thinking block).
    Reasoning,
    /// A tool is running — the descriptor comes from the allowlist mapper.
    Tool(&'static str),
    /// The agent is composing a text response.
    Responding,
    /// A tool completed or errored; next block hasn't arrived yet.
    Working,
}

/// Tool-name → descriptor mapper.
///
/// Input: normalised lowercase tool name (ASCII, `-`/`.` → `_`). Falls back to
/// `"Using a tool…"` for invalid or unknown names. Every output ≤ 40 Unicode scalars.
fn describe_tool(name: &str) -> &'static str {
    // Validate: 1..=96 ASCII bytes, every byte in the allowed set.
    if name.is_empty()
        || name.len() > 96
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return "Using a tool…";
    }
    match name {
        "read" | "read_file" => "Reading a file…",
        "ls" | "list_dir" => "Listing files…",
        "grep" | "glob" | "find" | "search" | "dynamic_search" => "Searching files…",
        "write" | "write_file" => "Writing a file…",
        "edit" | "edit_file" | "hashline_edit" | "apply_patch" => "Editing a file…",
        "bash" | "run_cmd" | "run_command" | "exec" => "Running a command…",
        "web_search" => "Searching the web…",
        "web_fetch" | "fetch" => "Reading a web page…",
        "todo" => "Updating the plan…",
        "component_render" | "component_unmount" | "surface_patch" => "Updating the interface…",
        "component_wait" => "Waiting for input…",
        "slack_canvas" => "Working with a Slack canvas…",
        "subagent" => "Delegating a task…",
        "browser_navigate" | "browser_open_tab" => "Opening a web page…",
        "browser_click" => "Clicking in the browser…",
        "browser_type" | "browser_key" => "Entering browser input…",
        "browser_scroll" => "Scrolling the page…",
        "browser_read_page" => "Reading the page…",
        "browser_screenshot" => "Capturing the page…",
        "browser_list_tabs" => "Listing open tabs…",
        "browser_switch_tab" => "Switching tabs…",
        "browser_close_tab" => "Closing a tab…",
        _ if name.starts_with("browser_") => "Using the browser…",
        _ if name.starts_with("mcp__") => "Using a connected service…",
        _ if name.starts_with("offshore_") => "Coordinating remote work…",
        _ => "Using a tool…",
    }
}

/// Pure reducer: inspects ONLY the current tail turn to produce one LiveActivity.
///
/// Priority (first match wins):
///   1. focused_permission → WaitingForApproval
///   2. !streaming → Hidden
///   3. tail is User (no assistant yet) → GettingStarted
///   4. latest Running ToolCall → Tool(fixed descriptor)
///   5. classify newest tail-assistant block → Reasoning / Responding / Working
///   6. no block → GettingStarted
fn reduce_live_activity(
    turns: &[crate::model::Turn],
    streaming: bool,
    focused_permission: bool,
) -> LiveActivity {
    if focused_permission {
        return LiveActivity::WaitingForApproval;
    }
    if !streaming {
        return LiveActivity::Hidden;
    }
    let Some(tail) = turns.last() else {
        return LiveActivity::Hidden;
    };
    match tail.role {
        Role::User => return LiveActivity::GettingStarted,
        Role::Assistant => {}
    }
    // Reverse-scan for newest Running ToolCall (priority 4).
    for block in tail.blocks.iter().rev() {
        if let Block::ToolCall {
            name,
            status: ToolStatus::Running,
            ..
        } = block
        {
            let normalized = name.to_lowercase().replace(['-', '.'], "_");
            return LiveActivity::Tool(describe_tool(&normalized));
        }
    }
    // Classify newest block by position (priority 5).
    match tail.blocks.last() {
        Some(Block::Thinking { .. }) => LiveActivity::Reasoning,
        Some(Block::Text(_)) => LiveActivity::Responding,
        Some(Block::ToolCall { .. }) | Some(Block::Component { .. }) => LiveActivity::Working,
        None => LiveActivity::GettingStarted,
    }
}

/// One live-activity row at transcript tail, outside the turn For loop.
///
/// Renders `role="status" aria-live="polite" aria-atomic="true"` with
/// a WaveBadge + label. Non-wrapping, ellipsized, flush left on the
/// transcript rail. Stays hidden when the reducer returns `Hidden`.
#[component]
fn LiveActivityRow(activity: Signal<LiveActivity>) -> impl IntoView {
    let label = move || match activity.get() {
        LiveActivity::Hidden => None,
        LiveActivity::WaitingForApproval => Some("Waiting for approval…"),
        LiveActivity::GettingStarted => Some("Getting started…"),
        LiveActivity::Reasoning => Some("Reasoning…"),
        LiveActivity::Tool(d) => Some(d),
        LiveActivity::Responding => Some("Responding…"),
        LiveActivity::Working => Some("Working…"),
    };
    let hidden = move || matches!(activity.get(), LiveActivity::Hidden);
    let spinning = Memo::new(move |_| {
        !matches!(
            activity.get(),
            LiveActivity::Hidden | LiveActivity::WaitingForApproval
        )
    });
    view! {
        <Show when=move || !hidden()>
            <div
                class="live-activity-row"
                role="status"
                aria-live="polite"
                aria-atomic="true"
            >
                // Dynamic child so the badge re-renders when the memo
                // changes — dedupes so the canvas isn't rebuilt on every
                // activity tick, only on real spin flips.
                {move || view! { <WaveBadge spinning=spinning.get() compact=true /> }}
                <span class="live-activity-row__label">{move || label().unwrap_or("")}</span>
            </div>
        </Show>
    }
}

/// One renderable item in an assistant turn: a single non-tool/non-thinking
/// block, the turn's full set of tool calls tucked into one disclosure, or the
/// turn's full set of thinking segments tucked into one disclosure.
#[derive(Debug, Clone, PartialEq)]
enum RenderItem {
    Single(usize),
    ToolGroup(Vec<usize>),
    ThinkingGroup(Vec<usize>),
}

/// Collapse the turn's blocks into render items. Every `ToolCall` is tucked
/// into one `ToolGroup` ("tools (N)") at the first tool call's position, and
/// every `Thinking` segment is tucked into one `ThinkingGroup` ("thinking…")
/// at the first thinking segment's position — so a turn that streams
/// dozens of interleaved thinking deltas renders a SINGLE thinking disclosure
/// instead of a chip per segment (the "26-chip wall"). Other blocks (text,
/// component) stay `Single`s in their original order. Render-only: never
/// reorders `turn.blocks`, so both the live SSE reducer and stored-session
/// hydration paths are covered by this one coalescing pass.
fn render_items(blocks: &[Block]) -> Vec<RenderItem> {
    let mut items = Vec::new();
    let mut tools: Vec<usize> = Vec::new();
    let mut group_slot: Option<usize> = None;
    let mut thinking: Vec<usize> = Vec::new();
    let mut thinking_slot: Option<usize> = None;
    for (i, block) in blocks.iter().enumerate() {
        match block {
            Block::ToolCall { .. } => {
                if group_slot.is_none() {
                    group_slot = Some(items.len());
                    items.push(RenderItem::ToolGroup(Vec::new())); // placeholder, filled below
                }
                tools.push(i);
            }
            Block::Thinking { .. } => {
                if thinking_slot.is_none() {
                    thinking_slot = Some(items.len());
                    items.push(RenderItem::ThinkingGroup(Vec::new())); // placeholder, filled below
                }
                thinking.push(i);
            }
            _ => {
                items.push(RenderItem::Single(i));
            }
        }
    }
    if let Some(slot) = group_slot {
        items[slot] = RenderItem::ToolGroup(tools);
    }
    if let Some(slot) = thinking_slot {
        items[slot] = RenderItem::ThinkingGroup(thinking);
    }
    items
}

/// True when an assistant turn is made up ENTIRELY of tool calls (at least one,
/// no text/thinking/component). This is the shape a Codex/Sol round takes — the
/// provider emits one `TurnStarted` per model round, so a multi-round turn lands
/// as several single-tool assistant `Turn`s — and also the shape
/// `turns_from_session_transcript` rebuilds each persisted tool entry into. Such
/// turns are the ones that merge across turn boundaries into a single
/// disclosure; a turn carrying any prose/thinking/component is NOT tool-only, so
/// it anchors its own row and breaks the run (prose/thinking between tool runs
/// still splits the grouping).
fn is_tool_only_turn(turn: &crate::model::Turn) -> bool {
    turn.role == Role::Assistant
        && !turn.blocks.is_empty()
        && turn
            .blocks
            .iter()
            .all(|b| matches!(b, Block::ToolCall { .. }))
}

/// One top-level transcript row. A normal turn (a user turn, or an assistant
/// turn that carries prose/thinking/components) renders through [`TurnView`]
/// with its full per-turn DOM intact — TASK-52's de-bubbled layout untouched. A
/// `ToolRun` is a MAXIMAL run of consecutive tool-only assistant turns (see
/// [`is_tool_only_turn`]) merged into ONE disclosure — the fix for the "wall of
/// `tools (1)`" a multi-round Sol turn (or a rebuilt session) otherwise
/// produces. Identified by its anchor (the first turn index of the run), which
/// is stable as the run grows (turns only ever append), so the merged
/// disclosure keeps a stable `For` key and never re-mounts — nor drops the
/// user's expand state — while the run is still streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopRow {
    Turn(usize),
    ToolRun(usize),
}

/// Project the turn list into top-level rows, coalescing each maximal run of
/// consecutive tool-only assistant turns into one `ToolRun`. Render-only: never
/// mutates `turns`, so the model layer stays truthful and BOTH the live SSE
/// reducer path and the `turns_from_session_transcript` rebuild path collapse
/// identically.
fn top_rows(turns: &[crate::model::Turn]) -> Vec<TopRow> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < turns.len() {
        if is_tool_only_turn(&turns[i]) {
            let anchor = i;
            while i < turns.len() && is_tool_only_turn(&turns[i]) {
                i += 1;
            }
            rows.push(TopRow::ToolRun(anchor));
        } else {
            rows.push(TopRow::Turn(i));
            i += 1;
        }
    }
    rows
}

/// The `(turn_idx, block_idx)` address of every tool call in the tool-only run
/// that STARTS at `anchor`, in transcript order. Derived reactively by the
/// merged disclosure so a run still growing mid-stream (fresh tool-only turns
/// appending) keeps its count, aggregate status and failed count current — the
/// same "scan, don't snapshot" contract the per-turn groups use, extended across
/// the turn boundary.
fn tool_run_members(turns: &[crate::model::Turn], anchor: usize) -> Vec<(usize, usize)> {
    let mut members = Vec::new();
    let mut ti = anchor;
    while ti < turns.len() && is_tool_only_turn(&turns[ti]) {
        for (bi, block) in turns[ti].blocks.iter().enumerate() {
            if matches!(block, Block::ToolCall { .. }) {
                members.push((ti, bi));
            }
        }
        ti += 1;
    }
    members
}

/// The `ToolStatus` of the tool block at `(turn_idx, block_idx)`, if any.
fn member_status(turns: &[crate::model::Turn], ti: usize, bi: usize) -> Option<ToolStatus> {
    match turns.get(ti).and_then(|turn| turn.blocks.get(bi)) {
        Some(Block::ToolCall { status, .. }) => Some(*status),
        _ => None,
    }
}

/// Aggregate status of the tool-only run at `anchor`: `Err` if any call failed,
/// else `Running` if any is still in flight, else `Ok`. Mirrors the per-turn
/// [`ToolGroup`] rollup, extended across the run's turns.
fn tool_run_status(turns: &[crate::model::Turn], anchor: usize) -> ToolStatus {
    let mut any_running = false;
    for (ti, bi) in tool_run_members(turns, anchor) {
        match member_status(turns, ti, bi) {
            Some(ToolStatus::Err) => return ToolStatus::Err,
            Some(ToolStatus::Running) => any_running = true,
            _ => {}
        }
    }
    if any_running {
        ToolStatus::Running
    } else {
        ToolStatus::Ok
    }
}

/// Count of failed calls across the whole tool-only run at `anchor` — drives the
/// merged disclosure's `N failed` accent.
fn tool_run_failed_count(turns: &[crate::model::Turn], anchor: usize) -> usize {
    tool_run_members(turns, anchor)
        .into_iter()
        .filter(|&(ti, bi)| member_status(turns, ti, bi) == Some(ToolStatus::Err))
        .count()
}

#[component]
pub fn Transcript(daemon: Daemon, show_sessions: RwSignal<bool>) -> impl IntoView {
    let turns = daemon.turns;
    // Project turns into top-level rows: normal turns render one-to-one, but a
    // run of consecutive tool-only assistant turns collapses into a single
    // `ToolRun` (see [`top_rows`]). Re-derived on every turns mutation; the `For`
    // below keys rows by their anchor turn index (stable under append), so new
    // turns append and existing rows mutate in place without re-keying mid-stream.
    let rows = move || turns.with(|t| top_rows(t));

    // Auto-scroll: keep the viewport pinned to the latest output as turns
    // append and streaming deltas grow existing turns — but only when the user
    // is already at (or near) the bottom. If they've scrolled up to read
    // history, we leave them be. "Near bottom" is sampled continuously from the
    // scroll handler so the effect can decide *before* the DOM grows.
    let container: NodeRef<leptos::html::Div> = NodeRef::new();
    let pinned = RwSignal::new(true);
    // Content grew below the viewport while the user was reading history.
    // Drives the quiet "latest" return affordance; never scrolls on its own.
    let new_below = RwSignal::new(false);

    // px from the bottom within which we still consider the user "pinned".
    // Generous enough to survive a streaming delta landing between frames.
    const STICK_THRESHOLD: f64 = 80.0;

    let on_scroll = move |_| {
        if let Some(el) = container.get() {
            let el: &web_sys::Element = el.as_ref();
            let distance =
                el.scroll_height() as f64 - el.scroll_top() as f64 - el.client_height() as f64;
            let now_pinned = distance <= STICK_THRESHOLD;
            pinned.set(now_pinned);
            if now_pinned {
                new_below.set(false);
            }
        }
    };

    Effect::new(move |_| {
        // Track every mutation of the turns signal: new turns AND in-place
        // block growth mid-stream both flow through this one signal, so reading
        // it here subscribes the effect to every streaming delta.
        turns.with(|t| {
            let _total_blocks: usize = t.iter().map(|turn| turn.blocks.len()).sum();
        });
        if pinned.get_untracked() {
            scroll_to_bottom(container);
        } else if turns.with_untracked(|t| !t.is_empty()) {
            new_below.set(true);
        }
    });

    // Switching sessions is a fresh read, not history-reading: re-pin and jump
    // so the new transcript opens at its latest turn instead of stranding the
    // viewport wherever the previous session left it.
    Effect::new(move |previous: Option<Option<String>>| {
        let current = daemon.session_id.get();
        if let Some(previous) = previous {
            if previous != current {
                pinned.set(true);
                new_below.set(false);
                scroll_to_bottom(container);
            }
        }
        current
    });

    // Empty until the first turn lands. On a fresh load (no session, no
    // project) `turns` is empty, so without this the main pane would be a
    // blank scroll container — the operator's "blank right pane". Render a
    // usable landing instead: a clear "start typing" prompt that points at the
    // composer below, which creates a session on the first message. A selected
    // session always has ≥1 turn, so this never shadows a real transcript.
    let is_empty = move || turns.with(Vec::is_empty);

    // Focused permission: true iff daemon.session_id matches active AND
    // pending_permissions contains an entry for that session.
    let focused_permission = Signal::derive(move || {
        let active = daemon.session_id.get();
        let Some(ref active) = active else {
            return false;
        };
        // TASK-69: a card for the focused session, OR a degraded view that still
        // knows an id may be pending — both are "the operator has a decision to
        // make here" for the live-activity row.
        daemon.permission_view.with(|v| {
            v.cards().iter().any(|p| p.session_id == *active)
                || (v.is_unavailable() && !v.unconfirmed_ids().is_empty())
        })
    });

    // LiveActivity reducer: inspects ONLY current tail turn + streaming +
    // focused_permission. Re-evaluates on every turn/block signal change.
    let live_activity = Signal::derive(move || {
        let streaming = daemon.streaming.get();
        let focused = focused_permission.get();
        turns.with(|t| reduce_live_activity(t, streaming, focused))
    });

    let return_to_latest = move |_| {
        pinned.set(true);
        new_below.set(false);
        scroll_to_bottom(container);
    };

    view! {
        <div class="transcript" node_ref=container on:scroll=on_scroll>
            <Show when=is_empty>
                <div class="transcript__landing">
                    // Approved v22 hero landing: Soundings field with
                    // dispersive wave physics, letter etches, and the
                    // launcher etching in when a wavefront crosses it.
                    <div class="transcript__landing-reveal">
                        <crate::loader::SoundingsLanding />
                        <button
                            class="transcript__sessions-launcher"
                            on:click=move |_| show_sessions.set(true)
                        >
                            "Sessions"
                        </button>
                    </div>
                    <h1 class="transcript__landing-title">"Ocean"</h1>
                </div>
            </Show>
            <For
                each=rows
                key=|row| match row {
                    // Distinct tag per kind so a Turn and a ToolRun anchored at
                    // the same index never collide; both keys are stable under
                    // append (turns never reorder, a growing run keeps its anchor).
                    TopRow::Turn(i) => (0u8, *i),
                    TopRow::ToolRun(anchor) => (1u8, *anchor),
                }
                children=move |row| {
                    let daemon = daemon.clone();
                    // Snapshot streaming at creation: rows that mount while
                    // live-streaming get `is-new` so CSS can run a one-shot
                    // materialize entry. Hydrated history (session load) mounts
                    // with streaming=false — no class, no page-load choreography.
                    let is_new = daemon.streaming.get_untracked();
                    match row {
                        TopRow::Turn(idx) => view! {
                            <TurnView idx=idx turns=turns daemon=daemon is_new=is_new />
                        }
                        .into_any(),
                        // A merged run of tool-only turns. Wrap it in the same
                        // turn / assistant / body scaffold a lone tool-only turn
                        // uses so it shares the transcript's 18px rhythm and the
                        // de-bubbled assistant rail — the disclosure inside just
                        // spans several turns' worth of calls.
                        TopRow::ToolRun(anchor) => view! {
                            <div class="turn" class:is-new=move || is_new>
                                <div class="turn--assistant">
                                    <div class="turn__body">
                                        <MergedToolGroup anchor=anchor turns=turns daemon=daemon />
                                    </div>
                                </div>
                            </div>
                        }
                        .into_any(),
                    }
                }
            />
            // Live activity row: pure reducer-driven, one row at transcript
            // tail, outside the turn loop. Replaces SoundingsThinking + per-turn
            // ocean-status-row.
            <LiveActivityRow activity=live_activity />
            // Quiet return affordance: content is growing below while the
            // user reads history. Zero-height sticky dock — no layout shift,
            // never scrolls the viewport on its own.
            <Show when=move || new_below.get()>
                <div class="transcript__latest-dock">
                    <button class="transcript__latest" on:click=return_to_latest>
                        "↓ latest"
                    </button>
                </div>
            </Show>
        </div>
    }
}

/// Jump the transcript scroll container to its bottom. Deferred a frame so
/// just-appended DOM has laid out, then settled one more frame to absorb late
/// layout (images, fonts) that grows `scroll_height` after the first jump.
fn scroll_to_bottom(container: NodeRef<leptos::html::Div>) {
    if let Some(el) = container.get_untracked() {
        let el: web_sys::Element = el.unchecked_into();
        request_animation_frame(move || {
            el.set_scroll_top(el.scroll_height());
            let settle = el.clone();
            request_animation_frame(move || settle.set_scroll_top(settle.scroll_height()));
        });
    }
}

#[component]
fn TurnView(
    idx: usize,
    turns: RwSignal<Vec<crate::model::Turn>>,
    daemon: Daemon,
    is_new: bool,
) -> impl IntoView {
    // Role is stable for the life of a turn, so read it once reactively to
    // pick the layout, then let the body derive from the signal.
    let role = move || turns.with(|t| t.get(idx).map(|turn| turn.role));

    view! {
        <div class="turn" class:is-new=move || is_new>
            {move || match role() {
                Some(Role::User) => view! { <UserTurn idx=idx turns=turns /> }.into_any(),
                Some(Role::Assistant) => view! { <AssistantTurn idx=idx turns=turns daemon=daemon.clone() /> }.into_any(),
                None => ().into_any(),
            }}
        </div>
    }
}

#[component]
fn UserTurn(idx: usize, turns: RwSignal<Vec<crate::model::Turn>>) -> impl IntoView {
    let text = move || {
        turns.with(|t| {
            t.get(idx)
                .map(|turn| {
                    turn.blocks
                        .iter()
                        .filter_map(|b| match b {
                            Block::Text(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        })
    };
    view! {
        <div class="turn--user">
            <div class="turn__body">{text}</div>
        </div>
    }
}

#[component]
fn AssistantTurn(
    idx: usize,
    turns: RwSignal<Vec<crate::model::Turn>>,
    daemon: Daemon,
) -> impl IntoView {
    // Recompute the render-item list whenever the block set changes. Reading
    // the blocks here also subscribes to tool-status changes (a clone snapshot),
    // so the group summary updates as calls finish.
    let items = move || {
        turns.with(|t| {
            t.get(idx)
                .map(|turn| render_items(&turn.blocks))
                .unwrap_or_default()
        })
    };
    // `is-streaming` is scoped to the single turn actively being streamed. The
    // in-flight assistant turn is always the last turn in the vec (turns append;
    // deltas grow it in place), so gating on `streaming` AND "this is the last
    // turn" lights up exactly one turn — not every assistant turn.
    let streaming = daemon.streaming;
    let is_streaming = move || streaming.get() && turns.with(|t| t.len() == idx + 1);

    view! {
        <div class="turn--assistant" class:is-streaming=is_streaming>
            <div class="turn__body">
                <For
                    each=items
                    key=|item| match item {
                        RenderItem::Single(i) => (0u8, *i),
                        RenderItem::ToolGroup(ix) => (1u8, *ix.first().unwrap_or(&0)),
                        RenderItem::ThinkingGroup(ix) => (2u8, *ix.first().unwrap_or(&0)),
                    }
                    children=move |item| {
                        let daemon = daemon.clone();
                        match item {
                            RenderItem::Single(block_idx) => view! {
                                <BlockView turn_idx=idx block_idx=block_idx turns=turns daemon=daemon />
                            }
                            .into_any(),
                            // One transcript tuck per assistant turn; the rows
                            // inside stay individually expandable.
                            RenderItem::ToolGroup(_) => view! {
                                <ToolGroup turn_idx=idx turns=turns daemon=daemon />
                            }
                            .into_any(),
                            // One thinking disclosure per turn: every thinking
                            // segment collapses into it, a `thinking…` pill
                            // that carries a live dot while reasoning streams.
                            RenderItem::ThinkingGroup(_) => view! {
                                <ThinkingGroup turn_idx=idx turns=turns />
                            }
                            .into_any(),
                        }
                    }
                />
            </div>
        </div>
    }
}

/// A turn's tool calls, tucked into one collapsible `tools (N)` disclosure.
/// Collapsed by default and NEVER auto-opens — not when a call errors, not at
/// turn end. The header carries aggregate state (running/err tint) plus a
/// lowercase `N failed` count when any call failed; the reducer still expands
/// each failed call itself, so opening the group lands you on the error body.
/// Manual toggle is sticky: the user's collapse/expand choice is absolute and
/// survives later errors. Each row inside stays individually expandable.
///
/// Member indices are DERIVED reactively from `turns`, not snapshotted from a
/// render-item prop: tool calls keep appending mid-stream and the parent `For`
/// retains this component under a stable key, so a snapshot prop would freeze
/// the group at its first tool call and miss every later call. Scanning the
/// turn's blocks each update keeps the count, aggregate status, failed count,
/// and body current forever — same contract as [`ThinkingGroup`].
#[component]
fn ToolGroup(
    turn_idx: usize,
    turns: RwSignal<Vec<crate::model::Turn>>,
    daemon: Daemon,
) -> impl IntoView {
    let daemon = StoredValue::new(daemon);
    // Member indices are DERIVED reactively — same contract as ThinkingGroup
    // (tool calls keep appending mid-stream; a snapshot would freeze the group).
    let idxs = Signal::derive(move || {
        turns.with(|t| {
            t.get(turn_idx).map_or(Vec::new(), |turn| {
                turn.blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| matches!(b, Block::ToolCall { .. }).then_some(i))
                    .collect::<Vec<_>>()
            })
        })
    });
    // Aggregate status across the contained calls, read reactively.
    let agg = Signal::derive(move || {
        turns.with(|t| {
            let Some(turn) = t.get(turn_idx) else {
                return ToolStatus::Ok;
            };
            let mut any_running = false;
            for bi in idxs.get() {
                if let Some(Block::ToolCall { status, .. }) = turn.blocks.get(bi) {
                    match status {
                        ToolStatus::Err => return ToolStatus::Err,
                        ToolStatus::Running => any_running = true,
                        ToolStatus::Ok => {}
                    }
                }
            }
            if any_running {
                ToolStatus::Running
            } else {
                ToolStatus::Ok
            }
        })
    });
    // Collapsed by default, always. The group NEVER auto-opens — not on error,
    // not on turn-end sweeps. `user_override` carries the user's last toggle and
    // sticks; failures surface through the error-tinted header, the `N failed`
    // count, and the reducer expanding each failed call itself.
    let user_override: RwSignal<Option<bool>> = RwSignal::new(None);
    let open = Signal::derive(move || user_override.get().unwrap_or(false));
    // Count of failed calls in the group — drives the header's `N failed` label.
    let failed_count = Signal::derive(move || {
        turns.with(|t| {
            t.get(turn_idx).map_or(0usize, |turn| {
                idxs.get()
                    .into_iter()
                    .filter(|&bi| {
                        matches!(
                            turn.blocks.get(bi),
                            Some(Block::ToolCall {
                                status: ToolStatus::Err,
                                ..
                            })
                        )
                    })
                    .count()
            })
        })
    });
    let toggle = move |_| user_override.set(Some(!open.get()));

    let status_class = move || match agg.get() {
        ToolStatus::Running => "is-running",
        ToolStatus::Ok => "is-ok",
        ToolStatus::Err => "is-err",
    };
    let status_label = move || match agg.get() {
        ToolStatus::Running => "running".to_string(),
        ToolStatus::Ok => String::new(),
        ToolStatus::Err => format!("{} failed", failed_count.get()),
    };
    let glyph = move || if open.get() { "▾" } else { "▸" };

    view! {
        <div class=move || format!("tool-group transcript-disclosure--group {}", status_class()) class:is-open=open>
            <button class="transcript-disclosure__head"
                aria-expanded=move || open.get().to_string()
                on:click=toggle>
                <span class="transcript-disclosure__tick">{glyph}</span>
                <span class="transcript-disclosure__dot"></span>
                <span class="transcript-disclosure__label">{move || format!("tools ({})", idxs.with(|ix| ix.len()))}</span>
                <span class="transcript-disclosure__status">{status_label}</span>
            </button>
            <Show when=move || open.get()>
                <div class="transcript-disclosure__body">
                    <For
                        each=move || idxs.get()
                        key=|bi| *bi
                        children=move |bi| {
                            let daemon = daemon.get_value();
                            view! {
                                <BlockView turn_idx=turn_idx block_idx=bi turns=turns daemon=daemon />
                            }
                        }
                    />
                </div>
            </Show>
        </div>
    }
}

/// A contiguous run of tool-only assistant turns, merged into ONE `tools (N)`
/// disclosure that spans every turn in the run (see [`top_rows`] /
/// [`is_tool_only_turn`]). Visually and behaviourally identical to [`ToolGroup`]
/// — collapsed by default, never auto-opens, sticky user toggle, `N failed`
/// accent, each call individually expandable — the only difference is that its
/// members are addressed by `(turn_idx, block_idx)` across the run rather than
/// block index within a single turn.
///
/// Membership is DERIVED reactively from `turns` via [`tool_run_members`], not
/// snapshotted from the `anchor` prop: while the run is still streaming, fresh
/// tool-only turns keep appending, and the parent `For` retains this component
/// under its stable anchor key — so a snapshot would freeze the group at the
/// first turn and miss every later call. Re-scanning from the anchor each update
/// keeps the count, aggregate status, failed count and body current, and the
/// stable key means the disclosure never re-mounts (or loses an operator's
/// expand state) as the run grows.
#[component]
fn MergedToolGroup(
    anchor: usize,
    turns: RwSignal<Vec<crate::model::Turn>>,
    daemon: Daemon,
) -> impl IntoView {
    let daemon = StoredValue::new(daemon);
    // Members span the whole run, DERIVED reactively — a growing run must not
    // freeze at the anchor turn.
    let members = Signal::derive(move || turns.with(|t| tool_run_members(t, anchor)));
    // Aggregate status across every call in the run, read reactively.
    let agg = Signal::derive(move || turns.with(|t| tool_run_status(t, anchor)));
    // Collapsed by default, always — same contract as ToolGroup. `user_override`
    // sticks; failures surface through the tinted header and the `N failed` count.
    let user_override: RwSignal<Option<bool>> = RwSignal::new(None);
    let open = Signal::derive(move || user_override.get().unwrap_or(false));
    // Count of failed calls across the whole run — drives the `N failed` label.
    let failed_count = Signal::derive(move || turns.with(|t| tool_run_failed_count(t, anchor)));
    let toggle = move |_| user_override.set(Some(!open.get()));

    let status_class = move || match agg.get() {
        ToolStatus::Running => "is-running",
        ToolStatus::Ok => "is-ok",
        ToolStatus::Err => "is-err",
    };
    let status_label = move || match agg.get() {
        ToolStatus::Running => "running".to_string(),
        ToolStatus::Ok => String::new(),
        ToolStatus::Err => format!("{} failed", failed_count.get()),
    };
    let glyph = move || if open.get() { "▾" } else { "▸" };

    view! {
        <div class=move || format!("tool-group transcript-disclosure--group {}", status_class()) class:is-open=open>
            <button class="transcript-disclosure__head"
                aria-expanded=move || open.get().to_string()
                on:click=toggle>
                <span class="transcript-disclosure__tick">{glyph}</span>
                <span class="transcript-disclosure__dot"></span>
                <span class="transcript-disclosure__label">{move || format!("tools ({})", members.with(Vec::len))}</span>
                <span class="transcript-disclosure__status">{status_label}</span>
            </button>
            <Show when=move || open.get()>
                <div class="transcript-disclosure__body">
                    <For
                        each=move || members.get()
                        key=|addr| *addr
                        children=move |(ti, bi)| {
                            let daemon = daemon.get_value();
                            view! {
                                <BlockView turn_idx=ti block_idx=bi turns=turns daemon=daemon />
                            }
                        }
                    />
                </div>
            </Show>
        </div>
    }
}
/// A turn's thinking segments, tucked into one collapsible disclosure
/// positioned where the first thinking segment appears. A turn can stream
/// many thinking deltas and even interleave them with text/tools; without
/// this tuck each segment would render its own chip (a "26-chip wall").
/// All thinking blocks collapse into this single disclosure; each segment's
/// text renders as its own `<pre>` when expanded. Collapsed by default; the
/// toggle sticks. While reasoning is the streaming tail the head carries
/// `is-running`, a status dot, and the active label `thinking…`; once the
/// turn finishes (or a non-thinking block follows) the label switches to
/// past-tense `thought`, the dot drops, and the group stays collapsed.
/// Render-only: never reorders `turn.blocks`.
///
/// Member indices are DERIVED reactively from `turns`, not snapshotted from the
/// render-item prop: thinking segments keep appending mid-stream and the parent
/// `For` retains this component under a stable key, so a snapshot prop would
/// freeze the group at its first segment and miss every later delta. Scanning
/// the turn's blocks each update keeps the count and body current forever.
#[component]
fn ThinkingGroup(turn_idx: usize, turns: RwSignal<Vec<crate::model::Turn>>) -> impl IntoView {
    // Local expand state (collapsed by default). Coalescing many blocks into one
    // disclosure means there's no single model `expanded` field to mirror, so the
    // toggle owns its own state — same shape as `ToolGroup`'s user override.
    let open: RwSignal<bool> = RwSignal::new(false);
    // Every thinking block in the turn is a member of this one disclosure. Scan
    // the blocks on each update so segments arriving AFTER the group was first
    // rendered are picked up — the count and body stay current as deltas stream.
    let idxs = Signal::derive(move || {
        turns.with(|t| {
            t.get(turn_idx).map_or(Vec::new(), |turn| {
                turn.blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| matches!(b, Block::Thinking { .. }).then_some(i))
                    .collect::<Vec<_>>()
            })
        })
    });
    let glyph = move || if open.get() { "▾" } else { "▸" };
    // Thoughts are always past-tense collapsed history — the live activity
    // row now owns the reasoning indicator. No competing live label or dot.
    view! {
        <div
            class="block block--thinking transcript-disclosure--thinking"
            class:is-open=open
        >
            <button class="transcript-disclosure__head"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.set(!open.get())>
                <span class="transcript-disclosure__tick">{glyph}</span>
                // Always past-tense "thought" — the live activity row
                // now owns the reasoning indicator.
                <span class="transcript-disclosure__label">"thought"</span>
            </button>
            <Show when=move || open.get()>
                <For
                    each=move || idxs.get()
                    key=|bi| *bi
                    children=move |bi| {
                        let content = move || {
                            turns.with(|t| {
                                t.get(turn_idx)
                                    .and_then(|turn| turn.blocks.get(bi))
                                    .and_then(|block| match block {
                                        Block::Thinking { content, .. } => Some(content.clone()),
                                        _ => None,
                                    })
                                    .unwrap_or_default()
                            })
                        };
                        view! {
                            <pre class="transcript-disclosure__body">{content}</pre>
                        }
                    }
                />
            </Show>
        </div>
    }
}

#[component]
fn BlockView(
    turn_idx: usize,
    block_idx: usize,
    turns: RwSignal<Vec<crate::model::Turn>>,
    daemon: Daemon,
) -> impl IntoView {
    // Snapshot of this block, recomputed whenever turns changes.
    let block = move || {
        turns.with(|t| {
            t.get(turn_idx)
                .and_then(|turn| turn.blocks.get(block_idx).cloned())
        })
    };

    let toggle = move || {
        turns.update(|t| {
            if let Some(turn) = t.get_mut(turn_idx) {
                if let Some(Block::ToolCall { expanded, .. }) = turn.blocks.get_mut(block_idx) {
                    *expanded = !*expanded;
                }
            }
        });
    };

    move || {
        let daemon = daemon.clone();
        match block() {
            Some(Block::Text(_)) => {
                // Stream this text block as stable, keyed markdown blocks so only
                // the in-progress tail re-parses per delta and finalized blocks
                // keep their DOM. The source is read reactively from `turns` so
                // growth is picked up without re-parsing the whole message.
                let text = Signal::derive(move || match block() {
                    Some(Block::Text(t)) => t,
                    _ => String::new(),
                });
                view! {
                    <crate::markdown_stream::MarkdownStream
                        text=text
                        class="block block--text"
                    />
                }
                .into_any()
            }

            Some(Block::ToolCall {
                name,
                args_preview,
                output,
                status,
                expanded,
                ..
            }) => {
                let status_class = match status {
                    ToolStatus::Running => "is-running",
                    ToolStatus::Ok => "is-ok",
                    ToolStatus::Err => "is-err",
                };
                let status_label = match status {
                    ToolStatus::Running => "running",
                    ToolStatus::Ok => "",
                    ToolStatus::Err => "error",
                };
                let glyph = if expanded { "▾" } else { "▸" };
                let label = format!("{name}({args_preview})");
                let body = if output.trim().is_empty() {
                    "(no output yet)".to_string()
                } else {
                    output.clone()
                };
                view! {
                    <div class=format!("block block--tool drawer transcript-disclosure--row {status_class}")
                        class:is-open=move || expanded>
                        <button class="transcript-disclosure__head"
                            aria-expanded=move || expanded.to_string()
                            on:click=move |_| toggle()>
                            <span class="transcript-disclosure__tick">{glyph}</span>
                            <span class="transcript-disclosure__dot"></span>
                            <span class="transcript-disclosure__label">{label}</span>
                            <span class="transcript-disclosure__status">{status_label}</span>
                        </button>
                        <Show when=move || expanded>
                            <pre class="transcript-disclosure__body">{body.clone()}</pre>
                        </Show>
                    </div>
                }
                .into_any()
            }

            Some(Block::Component {
                component_id,
                kind,
                props,
            }) => view! {
                <ComponentView component_id kind kind_props=props daemon />
            }
            .into_any(),

            Some(Block::Thinking { .. }) | None => ().into_any(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ToolStatus, Turn};

    // Minimal ToolCall block for fixtures — only the fields render_items keys on
    // matter; the rest are inert defaults.
    fn tool_block(id: &str) -> Block {
        Block::ToolCall {
            call_id: id.into(),
            name: "read".into(),
            args_preview: String::new(),
            output: String::new(),
            status: ToolStatus::Ok,
            expanded: false,
        }
    }

    /// The bug: a turn that streams many thinking segments (even interleaved with
    /// text) used to render one `thinking…` chip per segment. Now every thinking
    /// block collapses into a single `ThinkingGroup` at the first thinking
    /// position; non-thinking blocks stay Singles in order.
    #[test]
    fn all_thinking_blocks_collapse_into_one_group() {
        let blocks = vec![
            Block::Thinking {
                content: "A".into(),
                expanded: false,
            },
            Block::Text("answer".into()),
            Block::Thinking {
                content: "B".into(),
                expanded: false,
            },
            Block::Thinking {
                content: "C".into(),
                expanded: false,
            },
        ];
        let items = render_items(&blocks);
        assert_eq!(items.len(), 2);
        match &items[0] {
            RenderItem::ThinkingGroup(idxs) => assert_eq!(idxs, &vec![0, 2, 3]),
            other => panic!("expected ThinkingGroup at 0, got {other:?}"),
        }
        assert!(matches!(&items[1], RenderItem::Single(1)));
    }

    /// A lone thinking segment still tucks into a (single-element) group so the
    /// render path is uniform — never a bare Single pointing at a thinking block.
    #[test]
    fn single_thinking_block_becomes_group_of_one() {
        let blocks = vec![
            Block::Thinking {
                content: "A".into(),
                expanded: false,
            },
            Block::Text("answer".into()),
        ];
        let items = render_items(&blocks);
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], RenderItem::ThinkingGroup(idxs) if idxs == &vec![0]));
        assert!(matches!(&items[1], RenderItem::Single(1)));
    }

    /// Thinking and tool groups coexist: each kind tucks independently, in the
    /// order of its first member. The second thinking segment joins the FIRST
    /// thinking group rather than opening a second one (one disclosure per kind).
    #[test]
    fn thinking_and_tool_groups_each_collapse_once() {
        let blocks = vec![
            Block::Thinking {
                content: "think".into(),
                expanded: false,
            },
            tool_block("c1"),
            Block::Thinking {
                content: "more".into(),
                expanded: false,
            },
            tool_block("c2"),
        ];
        let items = render_items(&blocks);
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], RenderItem::ThinkingGroup(idxs) if idxs == &vec![0, 2]));
        assert!(matches!(&items[1], RenderItem::ToolGroup(idxs) if idxs == &vec![1, 3]));
    }

    /// A turn with no thinking produces no ThinkingGroup — text stays Single,
    /// tools still tuck. Guards against an empty/placeholder group leaking in.
    #[test]
    fn no_thinking_produces_no_thinking_group() {
        let blocks = vec![Block::Text("hi".into()), tool_block("c1")];
        let items = render_items(&blocks);
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], RenderItem::Single(0)));
        assert!(matches!(&items[1], RenderItem::ToolGroup(_)));
    }

    /// Pure-thinking turn: one group holding every segment, nothing else.
    /// (Models the operator's 26-delta turn: 26 blocks → 1 disclosure.)
    #[test]
    fn only_thinking_yields_one_group() {
        let blocks: Vec<Block> = (0..26)
            .map(|i| Block::Thinking {
                content: i.to_string(),
                expanded: false,
            })
            .collect();
        let items = render_items(&blocks);
        assert_eq!(items.len(), 1);
        match &items[0] {
            RenderItem::ThinkingGroup(idxs) => assert_eq!(idxs.len(), 26),
            other => panic!("expected ThinkingGroup, got {other:?}"),
        }
    }

    // ── top_rows / tool-run merging (TASK-64) ───────────────────────────

    /// A tool-only assistant turn with the given per-call statuses. Models one
    /// Codex/Sol round (or one rebuilt persisted tool entry): all blocks are
    /// tool calls, nothing else.
    fn tool_only_turn(id: &str, statuses: &[ToolStatus]) -> Turn {
        Turn {
            turn_id: Some(id.into()),
            role: Role::Assistant,
            blocks: statuses
                .iter()
                .enumerate()
                .map(|(i, s)| Block::ToolCall {
                    call_id: format!("{id}-{i}"),
                    name: "read".into(),
                    args_preview: String::new(),
                    output: String::new(),
                    status: *s,
                    expanded: false,
                })
                .collect(),
        }
    }

    /// An assistant turn that interleaves a thinking segment and a tool call —
    /// NOT tool-only, so it must anchor its own row and break any run.
    fn thinking_plus_tool_turn(id: &str) -> Turn {
        Turn {
            turn_id: Some(id.into()),
            role: Role::Assistant,
            blocks: vec![
                Block::Thinking {
                    content: "hmm".into(),
                    expanded: false,
                },
                Block::ToolCall {
                    call_id: format!("{id}-tool"),
                    name: "bash".into(),
                    args_preview: String::new(),
                    output: String::new(),
                    status: ToolStatus::Running,
                    expanded: false,
                },
            ],
        }
    }

    /// is_tool_only_turn gates on "assistant + non-empty + every block a tool".
    #[test]
    fn is_tool_only_turn_gating() {
        assert!(is_tool_only_turn(&tool_only_turn("t", &[ToolStatus::Ok])));
        assert!(!is_tool_only_turn(&user_turn()));
        assert!(!is_tool_only_turn(&assistant_text_turn()));
        assert!(!is_tool_only_turn(&thinking_plus_tool_turn("t")));
        let empty = Turn {
            turn_id: Some("e".into()),
            role: Role::Assistant,
            blocks: vec![],
        };
        assert!(!is_tool_only_turn(&empty));
    }

    /// (a) One turn, many tools → one ToolRun row holding every call.
    #[test]
    fn one_turn_many_tools_is_one_run() {
        let turns = vec![tool_only_turn(
            "t1",
            &[ToolStatus::Ok, ToolStatus::Ok, ToolStatus::Ok],
        )];
        assert_eq!(top_rows(&turns), vec![TopRow::ToolRun(0)]);
        assert_eq!(tool_run_members(&turns, 0), vec![(0, 0), (0, 1), (0, 2)]);
    }

    /// (b) Many consecutive tool-only turns → ONE merged run with the correct
    /// total N (the exact defect: a wall of `tools (1)` collapses to one row).
    #[test]
    fn consecutive_tool_only_turns_merge_into_one_run() {
        let turns = vec![
            tool_only_turn("t1", &[ToolStatus::Ok]),
            tool_only_turn("t2", &[ToolStatus::Ok]),
            tool_only_turn("t3", &[ToolStatus::Ok]),
        ];
        assert_eq!(top_rows(&turns), vec![TopRow::ToolRun(0)]);
        assert_eq!(tool_run_members(&turns, 0), vec![(0, 0), (1, 0), (2, 0)]);
    }

    /// A leading user turn is untouched; the run anchors at the first tool turn.
    #[test]
    fn user_turn_then_tool_run() {
        let turns = vec![
            user_turn(),
            tool_only_turn("t1", &[ToolStatus::Ok]),
            tool_only_turn("t2", &[ToolStatus::Ok]),
        ];
        assert_eq!(top_rows(&turns), vec![TopRow::Turn(0), TopRow::ToolRun(1)]);
        assert_eq!(tool_run_members(&turns, 1), vec![(1, 0), (2, 0)]);
    }

    /// (c) Prose between two tool runs splits them into two disclosures.
    #[test]
    fn prose_between_tool_runs_splits() {
        let turns = vec![
            tool_only_turn("t1", &[ToolStatus::Ok]),
            assistant_text_turn(),
            tool_only_turn("t3", &[ToolStatus::Ok]),
        ];
        assert_eq!(
            top_rows(&turns),
            vec![TopRow::ToolRun(0), TopRow::Turn(1), TopRow::ToolRun(2)]
        );
        assert_eq!(tool_run_members(&turns, 0), vec![(0, 0)]);
        assert_eq!(tool_run_members(&turns, 2), vec![(2, 0)]);
    }

    /// (d) Thinking interleaved with tools follows the existing per-turn design:
    /// the mixed turn is NOT tool-only, so it anchors its own row (thinking
    /// between runs breaks grouping), and render_items still splits its blocks
    /// into a ThinkingGroup + a ToolGroup.
    #[test]
    fn thinking_tool_turns_do_not_merge() {
        let turns = vec![
            tool_only_turn("t1", &[ToolStatus::Ok]),
            thinking_plus_tool_turn("t2"),
            tool_only_turn("t3", &[ToolStatus::Ok]),
        ];
        assert_eq!(
            top_rows(&turns),
            vec![TopRow::ToolRun(0), TopRow::Turn(1), TopRow::ToolRun(2)]
        );
        let items = render_items(&turns[1].blocks);
        assert!(matches!(items[0], RenderItem::ThinkingGroup(_)));
        assert!(matches!(items[1], RenderItem::ToolGroup(_)));
    }

    /// (e) Failed count and aggregate status are correct ACROSS the merged turns.
    #[test]
    fn failed_count_and_status_span_merged_turns() {
        let turns = vec![
            tool_only_turn("t1", &[ToolStatus::Ok, ToolStatus::Err]),
            tool_only_turn("t2", &[ToolStatus::Err]),
            tool_only_turn("t3", &[ToolStatus::Ok]),
        ];
        assert_eq!(top_rows(&turns), vec![TopRow::ToolRun(0)]);
        assert_eq!(tool_run_members(&turns, 0).len(), 4);
        assert_eq!(tool_run_failed_count(&turns, 0), 2);
        assert_eq!(tool_run_status(&turns, 0), ToolStatus::Err);
    }

    /// A still-running call anywhere in the run rolls the aggregate up to
    /// Running (no failures yet), matching the per-turn ToolGroup rollup.
    #[test]
    fn running_call_makes_run_running() {
        let turns = vec![tool_only_turn("t1", &[ToolStatus::Ok, ToolStatus::Running])];
        assert_eq!(tool_run_status(&turns, 0), ToolStatus::Running);
        assert_eq!(tool_run_failed_count(&turns, 0), 0);
    }

    /// (f) A run growing mid-stream keeps the SAME anchor, so the `For` key is
    /// stable and the merged disclosure never re-mounts (nor drops expand state)
    /// — while its membership still grows to include the newly-arrived turn.
    #[test]
    fn growing_run_keeps_stable_anchor_key() {
        // First Sol round: one running tool, one row anchored at 0.
        let mut turns = vec![tool_only_turn("t1", &[ToolStatus::Running])];
        let TopRow::ToolRun(anchor_before) = top_rows(&turns)[0] else {
            panic!("expected a ToolRun");
        };
        assert_eq!(tool_run_members(&turns, 0).len(), 1);

        // That call finishes and the next round appends a fresh tool-only turn.
        turns[0] = tool_only_turn("t1", &[ToolStatus::Ok]);
        turns.push(tool_only_turn("t2", &[ToolStatus::Running]));

        let rows = top_rows(&turns);
        assert_eq!(rows, vec![TopRow::ToolRun(0)]);
        let TopRow::ToolRun(anchor_after) = rows[0] else {
            panic!("expected a ToolRun");
        };
        assert_eq!(
            anchor_before, anchor_after,
            "anchor (For key) must be stable"
        );
        assert_eq!(
            tool_run_members(&turns, 0),
            vec![(0, 0), (1, 0)],
            "membership grows across the appended turn"
        );
    }

    // ── describe_tool ───────────────────────────────────────────────────

    #[test]
    fn describe_tool_empty_unknown() {
        assert_eq!(describe_tool(""), "Using a tool…");
        assert_eq!(describe_tool(""), "Using a tool…");
    }

    #[test]
    fn describe_tool_too_long() {
        let long = "a".repeat(97);
        assert_eq!(describe_tool(&long), "Using a tool…");
    }

    #[test]
    fn describe_tool_invalid_chars() {
        assert_eq!(describe_tool("bad tool"), "Using a tool…");
        assert_eq!(describe_tool("tool-name"), "Using a tool…");
        assert_eq!(describe_tool("tool.name"), "Using a tool…");
    }

    #[test]
    fn describe_tool_known_matches() {
        assert_eq!(describe_tool("read"), "Reading a file…");
        assert_eq!(describe_tool("read_file"), "Reading a file…");
        assert_eq!(describe_tool("ls"), "Listing files…");
        assert_eq!(describe_tool("grep"), "Searching files…");
        assert_eq!(describe_tool("write"), "Writing a file…");
        assert_eq!(describe_tool("edit"), "Editing a file…");
        assert_eq!(describe_tool("bash"), "Running a command…");
        assert_eq!(describe_tool("web_search"), "Searching the web…");
        assert_eq!(describe_tool("web_fetch"), "Reading a web page…");
        assert_eq!(describe_tool("todo"), "Updating the plan…");
        assert_eq!(describe_tool("subagent"), "Delegating a task…");
    }

    #[test]
    fn describe_tool_browser_prefix() {
        assert_eq!(describe_tool("browser_read_page"), "Reading the page…");
        assert_eq!(describe_tool("browser_navigate"), "Opening a web page…");
        assert_eq!(describe_tool("browser_foobar"), "Using the browser…");
    }

    #[test]
    fn describe_tool_mcp_prefix() {
        assert_eq!(describe_tool("mcp__github"), "Using a connected service…");
    }

    #[test]
    fn describe_tool_offshore_prefix() {
        assert_eq!(
            describe_tool("offshore_delegate"),
            "Coordinating remote work…"
        );
    }

    #[test]
    fn describe_tool_unknown_name() {
        assert_eq!(describe_tool("nonexistent_tool"), "Using a tool…");
    }

    // ── reduce_live_activity ────────────────────────────────────────────

    fn user_turn() -> Turn {
        Turn {
            turn_id: None,
            role: Role::User,
            blocks: vec![Block::Text("hello".into())],
        }
    }

    fn assistant_text_turn() -> Turn {
        Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![Block::Text("response".into())],
        }
    }

    fn assistant_thinking_turn() -> Turn {
        Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![Block::Thinking {
                content: "reasoning".into(),
                expanded: false,
            }],
        }
    }

    fn assistant_running_tool_turn(name: &str) -> Turn {
        Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![Block::ToolCall {
                call_id: "c1".into(),
                name: name.into(),
                args_preview: String::new(),
                output: String::new(),
                status: ToolStatus::Running,
                expanded: false,
            }],
        }
    }

    fn assistant_completed_tool_turn() -> Turn {
        Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![Block::ToolCall {
                call_id: "c1".into(),
                name: "read".into(),
                args_preview: String::new(),
                output: "file contents".into(),
                status: ToolStatus::Ok,
                expanded: false,
            }],
        }
    }

    fn assistant_component_turn() -> Turn {
        Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![Block::Component {
                component_id: "comp-1".into(),
                kind: "progress".into(),
                props: serde_json::Value::Object(Default::default()),
            }],
        }
    }

    #[test]
    fn reduce_focused_permission_wins() {
        assert_eq!(
            reduce_live_activity(&[assistant_text_turn()], true, true),
            LiveActivity::WaitingForApproval
        );
    }

    #[test]
    fn reduce_not_streaming_hidden() {
        assert_eq!(
            reduce_live_activity(&[assistant_text_turn()], false, false),
            LiveActivity::Hidden
        );
    }

    #[test]
    fn reduce_empty_turns_streaming_hidden() {
        assert_eq!(reduce_live_activity(&[], true, false), LiveActivity::Hidden);
    }

    #[test]
    fn reduce_user_tail_getting_started() {
        assert_eq!(
            reduce_live_activity(&[user_turn()], true, false),
            LiveActivity::GettingStarted
        );
    }

    #[test]
    fn reduce_running_tool() {
        assert_eq!(
            reduce_live_activity(&[assistant_running_tool_turn("read")], true, false),
            LiveActivity::Tool("Reading a file…")
        );
    }

    #[test]
    fn reduce_running_tool_normalized() {
        assert_eq!(
            reduce_live_activity(&[assistant_running_tool_turn("web-fetch")], true, false),
            LiveActivity::Tool("Reading a web page…")
        );
    }

    #[test]
    fn reduce_thinking() {
        assert_eq!(
            reduce_live_activity(&[assistant_thinking_turn()], true, false),
            LiveActivity::Reasoning
        );
    }

    #[test]
    fn reduce_text_responding() {
        assert_eq!(
            reduce_live_activity(&[assistant_text_turn()], true, false),
            LiveActivity::Responding
        );
    }

    #[test]
    fn reduce_completed_tool_working() {
        assert_eq!(
            reduce_live_activity(&[assistant_completed_tool_turn()], true, false),
            LiveActivity::Working
        );
    }

    #[test]
    fn reduce_component_working() {
        assert_eq!(
            reduce_live_activity(&[assistant_component_turn()], true, false),
            LiveActivity::Working
        );
    }

    #[test]
    fn reduce_no_blocks_getting_started() {
        let empty = Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![],
        };
        assert_eq!(
            reduce_live_activity(&[empty], true, false),
            LiveActivity::GettingStarted
        );
    }

    #[test]
    fn reduce_running_tool_above_thinking() {
        // Running tool reverse-scan wins over a later Thinking block.
        let turn = Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![
                Block::ToolCall {
                    call_id: "c1".into(),
                    name: "bash".into(),
                    args_preview: String::new(),
                    output: String::new(),
                    status: ToolStatus::Running,
                    expanded: false,
                },
                Block::Thinking {
                    content: "hmm".into(),
                    expanded: false,
                },
            ],
        };
        assert_eq!(
            reduce_live_activity(&[turn], true, false),
            LiveActivity::Tool("Running a command…")
        );
    }

    // ── priority edge cases ──────────────────────────────────────────
    /// Permission must win even when streaming is false (priority 1 > 2).
    #[test]
    fn reduce_permission_beats_not_streaming() {
        assert_eq!(
            reduce_live_activity(&[assistant_text_turn()], false, true),
            LiveActivity::WaitingForApproval
        );
    }

    /// Focused-permission false must not interfere — reducer falls through
    /// to the normal streaming/tail classification.
    #[test]
    fn reduce_permission_false_no_effect() {
        assert_eq!(
            reduce_live_activity(&[assistant_thinking_turn()], true, false),
            LiveActivity::Reasoning
        );
    }

    // ── block transitions ────────────────────────────────────────────
    /// Tail block is Text after a Thinking block: last wins → Responding.
    #[test]
    fn reduce_thinking_then_text() {
        let turn = Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![
                Block::Thinking {
                    content: "hmm".into(),
                    expanded: false,
                },
                Block::Text("final answer".into()),
            ],
        };
        assert_eq!(
            reduce_live_activity(&[turn], true, false),
            LiveActivity::Responding
        );
    }

    /// Tail block is a Running ToolCall after a Text block: reverse-scan
    /// finds the Running tool first → Tool(descriptor).
    #[test]
    fn reduce_text_then_running_tool() {
        let turn = Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![
                Block::Text("looking into that…".into()),
                Block::ToolCall {
                    call_id: "c1".into(),
                    name: "grep".into(),
                    args_preview: String::new(),
                    output: String::new(),
                    status: ToolStatus::Running,
                    expanded: false,
                },
            ],
        };
        assert_eq!(
            reduce_live_activity(&[turn], true, false),
            LiveActivity::Tool("Searching files…")
        );
    }

    // ── parallel running tools ───────────────────────────────────────
    /// Reverse-scan finds the LATEST Running ToolCall; an older finished
    /// tool after a newer running one does not mask it.
    #[test]
    fn reduce_latest_running_wins_over_older_finished() {
        let turn = Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![
                Block::ToolCall {
                    call_id: "c1".into(),
                    name: "read".into(),
                    args_preview: String::new(),
                    output: "done".into(),
                    status: ToolStatus::Ok,
                    expanded: false,
                },
                Block::ToolCall {
                    call_id: "c2".into(),
                    name: "bash".into(),
                    args_preview: String::new(),
                    output: String::new(),
                    status: ToolStatus::Running,
                    expanded: false,
                },
                Block::ToolCall {
                    call_id: "c3".into(),
                    name: "ls".into(),
                    args_preview: String::new(),
                    output: "done".into(),
                    status: ToolStatus::Ok,
                    expanded: false,
                },
            ],
        };
        // Reverse: c3(Ok), c2(Running)→bingo. c2 is "bash".
        assert_eq!(
            reduce_live_activity(&[turn], true, false),
            LiveActivity::Tool("Running a command…")
        );
    }

    /// When every ToolCall is finished, classification falls through to
    /// the newest-block check (priority 5) → Working.
    #[test]
    fn reduce_all_tools_finished_falls_to_working() {
        let turn = Turn {
            turn_id: None,
            role: Role::Assistant,
            blocks: vec![
                Block::ToolCall {
                    call_id: "c1".into(),
                    name: "read".into(),
                    args_preview: String::new(),
                    output: "ok".into(),
                    status: ToolStatus::Ok,
                    expanded: false,
                },
                Block::ToolCall {
                    call_id: "c2".into(),
                    name: "ls".into(),
                    args_preview: String::new(),
                    output: "ok".into(),
                    status: ToolStatus::Ok,
                    expanded: false,
                },
            ],
        };
        assert_eq!(
            reduce_live_activity(&[turn], true, false),
            LiveActivity::Working
        );
    }

    // ── describe_tool additional exacts ──────────────────────────────
    /// Exact browser match beats the prefix catch-all.
    #[test]
    fn describe_tool_browser_exact_beats_prefix() {
        assert_eq!(describe_tool("browser_click"), "Clicking in the browser…");
        assert_eq!(describe_tool("browser_type"), "Entering browser input…");
        assert_eq!(describe_tool("browser_key"), "Entering browser input…");
        assert_eq!(describe_tool("browser_scroll"), "Scrolling the page…");
        assert_eq!(describe_tool("browser_screenshot"), "Capturing the page…");
        assert_eq!(describe_tool("browser_list_tabs"), "Listing open tabs…");
        assert_eq!(describe_tool("browser_switch_tab"), "Switching tabs…");
        assert_eq!(describe_tool("browser_close_tab"), "Closing a tab…");
        assert_eq!(describe_tool("browser_open_tab"), "Opening a web page…");
    }

    /// Every descriptor output ≤ 40 codepoints — the longest static
    /// string must stay within the UI budget.
    #[test]
    fn describe_tool_all_outputs_fit_40() {
        let names: &[&str] = &[
            "",
            "read",
            "write",
            "bash",
            "web_fetch",
            "browser_screenshot",
            "browser_read_page",
            "browser_switch_tab",
            "browser_list_tabs",
            "offshore_delegate",
            "mcp__github",
            "browser_unknown_tool",
            "nonexistent_42",
        ];
        for n in names {
            let out = describe_tool(n);
            assert!(
                out.chars().count() <= 40,
                "describe_tool({n:?}) → {out:?} is {} chars, exceeds 40",
                out.chars().count()
            );
        }
    }

    /// Non-ASCII raw names must not leak; they fall through to the
    /// catch-all "Using a tool…".
    #[test]
    fn describe_tool_non_ascii_falls_to_unknown() {
        assert_eq!(describe_tool("工具"), "Using a tool…");
        assert_eq!(describe_tool("outil"), "Using a tool…");
        assert_eq!(describe_tool("инструмент"), "Using a tool…");
    }
}
