//! Renders the conversation. Layout mirrors the TUI's PM panel:
//! "you ▸" / "ocean ▸" headers, a single header per assistant turn even
//! when thinking + tools + text interleave, collapsed Thinking pills,
//! tool chips with status color.
//!
//! Everything derives from the `turns` signal so streaming deltas reflect
//! live. Turns are keyed by index for stable DOM; within a turn the block
//! list is rebuilt on each change (cheap for chat-sized content, and avoids
//! stale snapshots that would freeze streaming text).

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::ComponentView;
use crate::daemon::Daemon;
use crate::markdown::render as render_md;
use crate::model::{Block, Role, ToolStatus};

/// One renderable item in an assistant turn: either a single non-tool block or
/// the turn's full set of tool calls tucked into one disclosure.
#[derive(Clone, PartialEq)]
enum RenderItem {
    Single(usize),
    ToolGroup(Vec<usize>),
}

/// Collect every `ToolCall` block in the turn into a single `ToolGroup`,
/// rendered as one collapsible "tools (N)" disclosure positioned where the
/// first tool call appears. Non-tool blocks (text, thinking, component) stay
/// `Single`s in their original order. Render-only: never reorders `turn.blocks`.
fn render_items(blocks: &[Block]) -> Vec<RenderItem> {
    let mut items = Vec::new();
    let mut tools: Vec<usize> = Vec::new();
    let mut group_slot: Option<usize> = None;
    for (i, block) in blocks.iter().enumerate() {
        if matches!(block, Block::ToolCall { .. }) {
            if group_slot.is_none() {
                group_slot = Some(items.len());
                items.push(RenderItem::ToolGroup(Vec::new())); // placeholder, filled below
            }
            tools.push(i);
        } else {
            items.push(RenderItem::Single(i));
        }
    }
    if let Some(slot) = group_slot {
        items[slot] = RenderItem::ToolGroup(tools);
    }
    items
}

#[component]
pub fn Transcript(daemon: Daemon) -> impl IntoView {
    let turns = daemon.turns;
    // Key by turn index. New turns append; existing ones mutate in place and
    // their child views read the signal reactively, so re-keying isn't needed
    // mid-stream.
    let indices = move || (0..turns.with(Vec::len)).collect::<Vec<_>>();

    // Auto-scroll: keep the viewport pinned to the latest output as turns
    // append and streaming deltas grow existing turns — but only when the user
    // is already at (or near) the bottom. If they've scrolled up to read
    // history, we leave them be. "Near bottom" is sampled continuously from the
    // scroll handler so the effect can decide *before* the DOM grows.
    let container: NodeRef<leptos::html::Div> = NodeRef::new();
    let pinned = RwSignal::new(true);

    // px from the bottom within which we still consider the user "pinned".
    // Generous enough to survive a streaming delta landing between frames.
    const STICK_THRESHOLD: f64 = 80.0;

    let on_scroll = move |_| {
        if let Some(el) = container.get() {
            let el: &web_sys::Element = el.as_ref();
            let distance =
                el.scroll_height() as f64 - el.scroll_top() as f64 - el.client_height() as f64;
            pinned.set(distance <= STICK_THRESHOLD);
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
            if let Some(el) = container.get() {
                let el: web_sys::Element = el.unchecked_into();
                // Defer to next frame so the just-appended DOM has laid out and
                // scroll_height reflects the new content before we jump.
                let scroll = move || el.set_scroll_top(el.scroll_height());
                request_animation_frame(scroll);
            }
        }
    });

    // Empty until the first turn lands. On a fresh load (no session, no
    // project) `turns` is empty, so without this the main pane would be a
    // blank scroll container — the operator's "blank right pane". Render a
    // usable landing instead: a clear "start typing" prompt that points at the
    // composer below, which creates a session on the first message. A selected
    // session always has ≥1 turn, so this never shadows a real transcript.
    let is_empty = move || turns.with(Vec::is_empty);

    view! {
        <div class="transcript" node_ref=container on:scroll=on_scroll>
            <Show when=is_empty>
                <div class="transcript__landing">
                    // The OCEAN banner from the TUI splash (ocean-os
                    // ocean-tui/src/splash.rs), one solid ramp color per row —
                    // abyss to sunlit surface. This IS the brand mark; the
                    // rows are aria-hidden and the h1 stays for readers.
                    <pre class="transcript__landing-banner" aria-hidden="true">
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--1">"  d88888b     d8888b   8888888888        d8888 8888b    888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--2">"d88P   Y88b d88P  Y88b 888              d88888 88888b   888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--3">"888     888 888    888 888             d88P888 888888b  888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--4">"888     888 888        8888888        d88P 888 8888Y88b 888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--5">"888     888 888        888           d88P  888 8888 Y88b888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--6">"888     888 888    888 888          d88P   888 8888  Y88888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--7">"Y88b   d88P Y88b  d88P 888         d8888888888 8888   Y8888 "</span>
                        <span class="transcript__landing-banner-row transcript__landing-banner-row--8">"  Y88888P     Y8888P   8888888888 d88P     888 8888    Y888 "</span>
                    </pre>
                    <h1 class="transcript__landing-title">"Ocean"</h1>
                    <p class="transcript__landing-lead">
                        "Start typing below to begin a session."
                    </p>
                    <p class="transcript__landing-hint">
                        "Your first message starts a new conversation — no project required. "
                        "Open Sessions to start a project, resume a conversation, or begin a chat."
                    </p>
                </div>
            </Show>
            <For
                each=indices
                key=|i| *i
                children=move |idx| view! { <TurnView idx=idx turns=turns daemon=daemon.clone() /> }
            />
        </div>
    }
}

#[component]
fn TurnView(idx: usize, turns: RwSignal<Vec<crate::model::Turn>>, daemon: Daemon) -> impl IntoView {
    // Role is stable for the life of a turn, so read it once reactively to
    // pick the layout, then let the body derive from the signal.
    let role = move || turns.with(|t| t.get(idx).map(|turn| turn.role));

    view! {
        <div class="turn">
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
            <div class="turn__header">"you ▸"</div>
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

    view! {
        <div class="turn--assistant">
            <div class="turn__header">"ocean ▸"</div>
            <div class="turn__body">
                <For
                    each=items
                    key=|item| match item {
                        RenderItem::Single(i) => (0u8, *i),
                        RenderItem::ToolGroup(ix) => (1u8, *ix.first().unwrap_or(&0)),
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
                            RenderItem::ToolGroup(block_idxs) => view! {
                                <ToolGroup
                                    turn_idx=idx
                                    block_idxs=block_idxs
                                    turns=turns
                                    daemon=daemon
                                />
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
/// Collapsed by default so the transcript reads as prose + thinking + one tidy
/// tools tuck; auto-opens while any contained call errored (failures must stay
/// visible — the reducer also expands the failed call itself). A manual toggle
/// overrides the auto rule and sticks. Each row inside stays individually
/// expandable via `BlockView`.
#[component]
fn ToolGroup(
    turn_idx: usize,
    block_idxs: Vec<usize>,
    turns: RwSignal<Vec<crate::model::Turn>>,
    daemon: Daemon,
) -> impl IntoView {
    let n = block_idxs.len();
    let idxs = StoredValue::new(block_idxs);
    let daemon = StoredValue::new(daemon);
    // Aggregate status across the contained calls, read reactively.
    let agg = Signal::derive(move || {
        turns.with(|t| {
            let Some(turn) = t.get(turn_idx) else {
                return ToolStatus::Ok;
            };
            let mut any_running = false;
            for bi in idxs.get_value() {
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
    // None = follow the auto rule (open iff any error); Some(_) = user's choice.
    let user_override: RwSignal<Option<bool>> = RwSignal::new(None);
    let open = Signal::derive(move || {
        user_override
            .get()
            .unwrap_or_else(|| matches!(agg.get(), ToolStatus::Err))
    });
    // A newly-arrived failure must re-surface even if the user had collapsed the
    // group — including a second failure inside an already-errored group. Key the
    // reset off the count of failed calls, not the aggregate status: whenever that
    // count rises, drop the manual override so `open` follows the auto rule again
    // (the user can collapse afterward). Mirrors the reducer expanding each failed
    // call itself.
    let err_count = Signal::derive(move || {
        turns.with(|t| {
            t.get(turn_idx).map_or(0usize, |turn| {
                idxs.get_value()
                    .into_iter()
                    .filter(|&bi| {
                        matches!(
                            turn.blocks.get(bi),
                            Some(Block::ToolCall { status: ToolStatus::Err, .. })
                        )
                    })
                    .count()
            })
        })
    });
    let prev_err_count = RwSignal::new(0usize);
    Effect::new(move |_| {
        let n = err_count.get();
        if n > prev_err_count.get_untracked() {
            user_override.set(None);
        }
        prev_err_count.set(n);
    });
    let toggle = move |_| user_override.set(Some(!open.get()));

    let status_class = move || match agg.get() {
        ToolStatus::Running => "is-running",
        ToolStatus::Ok => "is-ok",
        ToolStatus::Err => "is-err",
    };
    let status_label = move || match agg.get() {
        ToolStatus::Running => "running",
        ToolStatus::Ok => "done",
        ToolStatus::Err => "error",
    };
    let glyph = move || if open.get() { "▾" } else { "▸" };

    view! {
        <div class=move || format!("tool-group {}", status_class()) class:is-open=open>
            <button class="tool-group__head" on:click=toggle>
                <span class="tool-group__tick">{glyph}</span>
                <span class="tool-group__dot"></span>
                <span class="tool-group__label">{move || format!("tools ({n})")}</span>
                <span class="tool-group__status">{status_label}</span>
            </button>
            <Show when=move || open.get()>
                <div class="tool-group__body">
                    <For
                        each=move || idxs.get_value()
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
                if let Some(b) = turn.blocks.get_mut(block_idx) {
                    match b {
                        Block::Thinking { expanded, .. } => *expanded = !*expanded,
                        Block::ToolCall { expanded, .. } => *expanded = !*expanded,
                        _ => {}
                    }
                }
            }
        });
    };

    move || {
        let daemon = daemon.clone();
        match block() {
            Some(Block::Text(text)) => view! {
                <div class="block block--text" inner_html=render_md(&text)></div>
            }
            .into_any(),

            Some(Block::Thinking { content, expanded }) => {
                let count = content.chars().count();
                let glyph = if expanded { "▾" } else { "▸" };
                view! {
                    <div class="block block--thinking">
                        <button class="block__pill" on:click=move |_| toggle()>
                            {format!("{glyph} thinking… ({count} chars)")}
                        </button>
                        <Show when=move || expanded>
                            <pre class="block__thinking-body">{content.clone()}</pre>
                        </Show>
                    </div>
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
                    ToolStatus::Ok => "done",
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
                    <div class=format!("block block--tool drawer {status_class}")
                        class:is-open=move || expanded>
                        <button class="drawer__head" on:click=move |_| toggle()>
                            <span class="drawer__tick">{glyph}</span>
                            <span class="drawer__dot"></span>
                            <span class="drawer__label">{label}</span>
                            <span class="drawer__status">{status_label}</span>
                        </button>
                        <Show when=move || expanded>
                            <pre class="drawer__body">{body.clone()}</pre>
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

            None => ().into_any(),
        }
    }
}
