//! Renders the conversation. No visible turn role headers; assistant turns
//! use unified `transcript-disclosure` classes for tool groups, thinking
//! groups, and individual tool rows — each with explicit `aria-expanded`.
//! Collapsed Thinking pills and tool chips with status color.
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

#[component]
pub fn Transcript(daemon: Daemon, show_sessions: RwSignal<bool>) -> impl IntoView {
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
    // Send → first token: `streaming` flips on at submit, but the assistant
    // turn only materializes when the first delta arrives — until then the
    // last turn is the user's. That window is the pending gap.
    let streaming = daemon.streaming;
    let pending_response = move || {
        streaming.get()
            && turns.with(|t| {
                t.last()
                    .map_or(false, |x| matches!(x.role, crate::model::Role::User))
            })
    };

    view! {
        <div class="transcript" node_ref=container on:scroll=on_scroll>
            <Show when=is_empty>
                <div class="transcript__landing">
                    // Approved v22 wordmark reveal: drip → splash → rising
                    // tide unveils OCEAN; orb rides the surface into idle.
                    // The scene SVG is aria-hidden (inside OceanReveal); the
                    // h1 stays for readers. The Sessions launcher lives
                    // INSIDE the scene, surfacing in the water zone when
                    // the rAF waterline floods past it (data-flooded stamp —
                    // no hardcoded delay; reduced-motion shows it always).
                    <div class="transcript__landing-reveal">
                        <crate::icons::OceanReveal />
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
                each=indices
                key=|i| *i
                children=move |idx| {
                    let daemon = daemon.clone();
                    // Snapshot streaming at creation: turns that mount while
                    // live-streaming get `is-new` so CSS can run a one-shot
                    // materialize entry. Hydrated history (session load) mounts
                    // with streaming=false — no class, no page-load choreography.
                    let is_new = daemon.streaming.get_untracked();
                    view! { <TurnView idx=idx turns=turns daemon=daemon is_new=is_new /> }
                }
            />
            // The reply's landing site while the daemon works (send → first
            // token): the Ocean badge alone, swells churning under a calm rim
            // where the text is about to appear — never dead air. Per the
            // logo handoff: no `ocean ▸` proto-header, no prompt-like glyphs.
            <Show when=pending_response>
                // Half-filled water card reveals "thinking…" as the tide
                // rises — pending gap between send and first token.
                <crate::icons::OceanThinking />
            </Show>
        </div>
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
                            // segment collapses into it, a plain `thinking…` label.
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
/// A turn's thinking segments, tucked into one collapsible `thinking…`
/// disclosure positioned where the first thinking segment appears. A turn can
/// stream many thinking deltas and even interleave them with text/tools;
/// without this tuck each segment would render its own chip (a "26-chip wall").
/// All thinking blocks collapse into this single disclosure; each segment's
/// text renders as its own `<pre>` when expanded. Collapsed by default; the
/// toggle sticks. The label is a plain `thinking…` — no char counter (that's
/// debug telemetry, not UI). Render-only: never reorders `turn.blocks`.
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
    view! {
        <div class="block block--thinking transcript-disclosure--thinking" class:is-open=open>
            <button class="transcript-disclosure__head"
                aria-expanded=move || open.get().to_string()
                on:click=move |_| open.set(!open.get())>
                {move || format!("{} thinking…", glyph())}
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
            Some(Block::Text(text)) => view! {
                <div class="block block--text" inner_html=render_md(&text)></div>
            }
            .into_any(),

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
    use crate::model::ToolStatus;

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
}
