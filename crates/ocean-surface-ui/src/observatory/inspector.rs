use leptos::prelude::*;

use super::domain::{short_id, ExecutionPhase, ObservatoryState, TruthProvenance};

#[component]
pub fn ExecutionInspector(
    state: RwSignal<ObservatoryState>,
    selected: RwSignal<Option<String>>,
    open_transcript: Callback<String>,
) -> impl IntoView {
    view! {
        <Show when=move || selected.get().is_some()>
            <aside class="ocean-floor__inspector" aria-label="Selected execution inspector">
                {move || {
                    let Some(execution_id) = selected.get() else {
                        return ().into_any();
                    };
                    let current = state.get();
                    let Some(node) = current.nodes.get(&execution_id).cloned() else {
                        return ().into_any();
                    };
                    let session_id = node.session_id.clone();
                    let transcript_session = session_id.clone();
                    view! {
                        <div class="ocean-floor__inspector-head">
                            <div>
                                <span class="ocean-floor__inspector-kicker">"Execution station"</span>
                                <h2>{node.label()}</h2>
                            </div>
                            <button
                                class="ocean-floor__icon-button"
                                type="button"
                                aria-label="close inspector"
                                on:click=move |_| selected.set(None)
                            >"✕"</button>
                        </div>
                        <div class="ocean-floor__status-line">
                            <span class=format!("ocean-floor__phase is-{}", node.phase.label().replace(' ', "-"))>
                                {node.phase.label()}
                            </span>
                            <span>{truth_label(node.truth)}</span>
                        </div>
                        <dl class="ocean-floor__facts">
                            <div><dt>"Execution"</dt><dd>{short_id(&node.execution_id)}</dd></div>
                            <div><dt>"Root"</dt><dd>{short_id(&node.root_execution_id)}</dd></div>
                            <div><dt>"Session"</dt><dd>{if session_id.is_empty() { "not projected".into() } else { short_id(&session_id) }}</dd></div>
                            <div><dt>"Cursor"</dt><dd>{node.last_cursor}</dd></div>
                            <div><dt>"Model"</dt><dd>{node.model_alias.clone().unwrap_or_else(|| "not projected".into())}</dd></div>
                            <div><dt>"Duration"</dt><dd>{duration_label(&node)}</dd></div>
                        </dl>
                        <Show when=move || node.permission_waiting>
                            <div class="ocean-floor__attention-callout" role="status">
                                <strong>"Needs attention"</strong>
                                <span>{node.permission_reason.clone().unwrap_or_else(|| "permission waiting".into())}</span>
                            </div>
                        </Show>
                        <section class="ocean-floor__activity">
                            <h3>"Current activity"</h3>
                            {if node.tools.is_empty() {
                                view! { <p>"No tools in flight."</p> }.into_any()
                            } else {
                                view! {
                                    <ul>
                                        {node.tools.values().map(|activity| view! {
                                            <li>
                                                <span class="ocean-floor__activity-dot"></span>
                                                <span>{activity.tool_name.clone()}</span>
                                                <code>{format!("@{}", activity.started_cursor)}</code>
                                            </li>
                                        }).collect_view()}
                                    </ul>
                                }.into_any()
                            }}
                        </section>
                        <button
                            class="ocean-floor__transcript-link"
                            type="button"
                            disabled=transcript_session.is_empty()
                            title=if transcript_session.is_empty() { "Session correlation is not available in this projection" } else { "Open the authoritative transcript" }
                            on:click=move |_| open_transcript.run(transcript_session.clone())
                        >"Open transcript"</button>
                    }.into_any()
                }}
            </aside>
        </Show>
    }
}

#[component]
pub fn SemanticExecutionList(
    state: RwSignal<ObservatoryState>,
    selected: RwSignal<Option<String>>,
) -> impl IntoView {
    let nodes = Memo::new(move |_| state.get().nodes.into_values().collect::<Vec<_>>());
    view! {
        <div class="ocean-floor__semantic" aria-label="Execution list">
            <div class="ocean-floor__semantic-head">
                <h2>"Executions"</h2>
                <span>{move || format!("{} tracked", state.get().nodes.len())}</span>
            </div>
            <Show
                when=move || !state.get().nodes.is_empty()
                fallback=|| view! {
                    <div class="ocean-floor__empty">
                        <strong>"The floor is quiet."</strong>
                        <span>"Start an Ocean turn and its truthful execution state will appear here."</span>
                    </div>
                }
            >
                <ul class="ocean-floor__execution-list">
                    <For
                        each=move || nodes.get()
                        key=|node| node.execution_id.clone()
                        children=move |node| {
                            let execution_id = node.execution_id.clone();
                            let selected_id = node.execution_id.clone();
                            let label = node.label();
                            let detail = node_detail(&node);
                            let phase = node.phase;
                            view! {
                                <li>
                                    <button
                                        type="button"
                                        class="ocean-floor__execution-row"
                                        class:is-selected=move || selected.get().as_deref() == Some(selected_id.as_str())
                                        on:click=move |_| selected.set(Some(execution_id.clone()))
                                    >
                                        <span class=format!("ocean-floor__lamp is-{}", phase.label().replace(' ', "-")) aria-hidden="true"></span>
                                        <span class="ocean-floor__execution-copy">
                                            <strong>{label}</strong>
                                            <span>{detail}</span>
                                        </span>
                                        <span class="ocean-floor__phase-label">{phase.label()}</span>
                                    </button>
                                </li>
                            }
                        }
                    />
                </ul>
            </Show>
        </div>
    }
}

fn node_detail(node: &super::domain::ExecutionState) -> String {
    if node.permission_waiting {
        return "permission waiting".into();
    }
    if let Some(tool) = node.tools.values().next() {
        return format!("using {}", tool.tool_name);
    }
    match node.phase {
        ExecutionPhase::Admitted => "admitted".into(),
        ExecutionPhase::Running => "running".into(),
        ExecutionPhase::Finished => "execution complete".into(),
        ExecutionPhase::Error => "execution error".into(),
        ExecutionPhase::Canceled => "interrupted".into(),
        ExecutionPhase::TimedOut => "timed out".into(),
    }
}

fn truth_label(truth: TruthProvenance) -> &'static str {
    match truth {
        TruthProvenance::HostObserved => "host observed",
        TruthProvenance::ExtensionAttested => "extension reported",
        TruthProvenance::Derived => "derived",
    }
}

fn duration_label(node: &super::domain::ExecutionState) -> String {
    node.duration_millis
        .map(format_duration)
        .unwrap_or_else(|| {
            if node.phase.is_terminal() {
                "not projected".into()
            } else {
                "in progress".into()
            }
        })
}

fn format_duration(duration_millis: u64) -> String {
    if duration_millis < 1_000 {
        format!("{duration_millis} ms")
    } else {
        format!("{:.1} s", duration_millis as f64 / 1_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observatory::domain::{ExecutionState, TruthProvenance};

    fn node(phase: ExecutionPhase, duration_millis: Option<u64>) -> ExecutionState {
        ExecutionState {
            execution_id: "execution".into(),
            root_execution_id: "root".into(),
            parent_execution_id: None,
            session_id: String::new(),
            turn_id: String::new(),
            phase,
            truth: TruthProvenance::HostObserved,
            labels: vec![],
            tools: Default::default(),
            permission_waiting: false,
            permission_reason: None,
            model_alias: None,
            floor_slot: 0,
            started_at: "2026-07-17T00:00:00Z".into(),
            last_cursor: 1,
            duration_millis,
        }
    }

    #[test]
    fn absent_duration_never_invents_progress_for_terminal_execution() {
        assert_eq!(
            duration_label(&node(ExecutionPhase::Finished, None)),
            "not projected"
        );
        assert_eq!(
            duration_label(&node(ExecutionPhase::Running, None)),
            "in progress"
        );
    }
}
