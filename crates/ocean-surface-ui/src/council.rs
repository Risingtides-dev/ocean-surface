use leptos::prelude::*;

use crate::daemon::Daemon;
use crate::model::{Block, Role, ToolStatus, Turn};

#[derive(Clone, Copy, PartialEq, Eq)]
enum NodeState {
    Idle,
    Thinking,
    Running,
    Done,
    Error,
}

#[derive(Clone)]
struct ToolNode {
    name: String,
    status: ToolStatus,
    args: String,
}

fn first_text(turn: &Turn) -> Option<String> {
    turn.blocks.iter().find_map(|block| match block {
        Block::Text(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        _ => None,
    })
}

fn latest_user_prompt(turns: &[Turn]) -> Option<String> {
    turns.iter().rev().find(|t| t.role == Role::User).and_then(first_text)
}

fn latest_assistant_turn(turns: &[Turn]) -> Option<&Turn> {
    turns.iter().rev().find(|t| t.role == Role::Assistant)
}

fn latest_assistant_summary(turns: &[Turn]) -> Option<String> {
    latest_assistant_turn(turns).and_then(first_text)
}

fn latest_tools(turns: &[Turn]) -> Vec<ToolNode> {
    latest_assistant_turn(turns)
        .map(|turn| {
            turn.blocks
                .iter()
                .filter_map(|block| match block {
                    Block::ToolCall {
                        name,
                        status,
                        args_preview,
                        ..
                    } => Some(ToolNode {
                        name: name.clone(),
                        status: *status,
                        args: args_preview.clone(),
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn assistant_state(turns: &[Turn], streaming: bool) -> NodeState {
    let tools = latest_tools(turns);
    if tools.iter().any(|tool| tool.status == ToolStatus::Err) {
        NodeState::Error
    } else if tools.iter().any(|tool| tool.status == ToolStatus::Running) {
        NodeState::Running
    } else if streaming {
        NodeState::Thinking
    } else if latest_assistant_turn(turns).is_some() {
        NodeState::Done
    } else {
        NodeState::Idle
    }
}

fn state_class(state: NodeState) -> &'static str {
    match state {
        NodeState::Idle => "is-idle",
        NodeState::Thinking => "is-thinking",
        NodeState::Running => "is-running",
        NodeState::Done => "is-done",
        NodeState::Error => "is-error",
    }
}

fn state_label(state: NodeState, browser_active: bool) -> &'static str {
    if browser_active && matches!(state, NodeState::Running | NodeState::Thinking) {
        "driving"
    } else {
        match state {
            NodeState::Idle => "idle",
            NodeState::Thinking => "thinking",
            NodeState::Running => "running",
            NodeState::Done => "settled",
            NodeState::Error => "error",
        }
    }
}

fn trim_line(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        let mut out = text.chars().take(limit.saturating_sub(1)).collect::<String>();
        out.push('…');
        out
    }
}

#[component]
pub fn CouncilStage(daemon: Daemon) -> impl IntoView {
    let turns = daemon.turns;
    let streaming = daemon.streaming;
    let browser_active = daemon.browser_active;
    let session_title = daemon.session_title;
    let status = daemon.status;
    let model = daemon.model;

    let goal_title = move || {
        let title = session_title.get();
        if title.trim().is_empty() {
            "current session".to_string()
        } else {
            trim_line(&title, 44)
        }
    };
    let prompt_label = move || latest_user_prompt(&turns.get()).map(|s| trim_line(&s, 72));
    let assistant_blurb = move || latest_assistant_summary(&turns.get()).map(|s| trim_line(&s, 96));
    let tool_nodes = move || latest_tools(&turns.get());
    let assistant_state_sig = move || assistant_state(&turns.get(), streaming.get());
    let has_anything = move || {
        !turns.get().is_empty() || streaming.get() || !status.get().trim().is_empty()
    };

    view! {
        <div class="ocean-council-stage">
            <Show
                when=has_anything
                fallback=|| {
                    view! {
                        <div class="ocean-council-stage__empty">
                            <div class="ocean-council-stage__empty-title">
                                "no live workflow yet"
                            </div>
                            <div class="ocean-council-stage__empty-body">
                                "start a turn, convene a room, or run a council — workflow nodes appear here as the surface receives live daemon state"
                            </div>
                        </div>
                    }
                }
            >
                <div class="ocean-council-stage__meta">
                    <span class="ocean-council-stage__eyebrow">"workflow stage"</span>
                    <span class="ocean-council-stage__status">{move || trim_line(&status.get(), 80)}</span>
                </div>
                <div class="ocean-council-stage__scene">
                    <svg class="ocean-council-stage__edges" viewBox="0 0 1080 720" preserveAspectRatio="none" aria-hidden="true">
                        <line class="ocean-council-edge" x1="540" y1="96" x2="220" y2="256" />
                        <line class="ocean-council-edge" x1="540" y1="96" x2="540" y2="256" />
                        {move || {
                            tool_nodes()
                                .into_iter()
                                .enumerate()
                                .map(|(idx, tool)| {
                                    let x = 220 + (idx as i32 * 220);
                                    let edge_class = match tool.status {
                                        ToolStatus::Running => "ocean-council-edge ocean-council-edge--live",
                                        ToolStatus::Err => "ocean-council-edge ocean-council-edge--error",
                                        ToolStatus::Ok => "ocean-council-edge",
                                    };
                                    view! {
                                        <line class=edge_class x1="540" y1="416" x2={x.to_string()} y2="576" />
                                    }
                                })
                                .collect_view()
                        }}
                    </svg>

                    <div class="ocean-council-node ocean-council-node--goal" style="left: 420px; top: 32px; width: 240px;">
                        <div class="ocean-council-node__label">"goal"</div>
                        <div class="ocean-council-node__title">{goal_title}</div>
                        <div class="ocean-council-node__meta">"session focus"</div>
                    </div>

                    <Show when=move || prompt_label().is_some()>
                        <div class="ocean-council-node ocean-council-node--prompt" style="left: 80px; top: 208px; width: 280px;">
                            <div class="ocean-council-node__label">"operator prompt"</div>
                            <div class="ocean-council-node__body">{move || prompt_label().unwrap_or_default()}</div>
                        </div>
                    </Show>

                    <div
                        class=move || format!(
                            "ocean-council-node ocean-council-node--assistant {}{}",
                            state_class(assistant_state_sig()),
                            if browser_active.get() { " is-browser-active" } else { "" }
                        )
                        style="left: 400px; top: 208px; width: 280px;"
                    >
                        <div class="ocean-council-node__label">"ocean"</div>
                        <div class="ocean-council-node__title">
                            {move || model.get().unwrap_or_else(|| "daemon default".into())}
                        </div>
                        <div class="ocean-council-node__meta-row">
                            <span class="ocean-council-node__state-dot"></span>
                            <span class="ocean-council-node__meta">{move || state_label(assistant_state_sig(), browser_active.get())}</span>
                        </div>
                        <Show when=move || assistant_blurb().is_some()>
                            <div class="ocean-council-node__body">{move || assistant_blurb().unwrap_or_default()}</div>
                        </Show>
                    </div>

                    {move || {
                        tool_nodes()
                            .into_iter()
                            .enumerate()
                            .map(|(idx, tool)| {
                                let x = 80 + (idx * 220);
                                let state = match tool.status {
                                    ToolStatus::Running => NodeState::Running,
                                    ToolStatus::Ok => NodeState::Done,
                                    ToolStatus::Err => NodeState::Error,
                                };
                                let left = format!("left: {}px; top: 520px; width: 200px;", x);
                                let body = trim_line(&tool.args, 52);
                                let has_body = !body.is_empty();
                                let body_text = body.clone();
                                view! {
                                    <div class=move || format!(
                                        "ocean-council-node ocean-council-node--tool {}",
                                        state_class(state)
                                    ) style=left.clone()>
                                        <div class="ocean-council-node__label">"tool"</div>
                                        <div class="ocean-council-node__title">{tool.name.clone()}</div>
                                        <div class="ocean-council-node__meta-row">
                                            <span class="ocean-council-node__state-dot"></span>
                                            <span class="ocean-council-node__meta">{state_label(state, false)}</span>
                                        </div>
                                        <Show when=move || has_body>
                                            <div class="ocean-council-node__body">{body_text.clone()}</div>
                                        </Show>
                                    </div>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </Show>
        </div>
    }
}
