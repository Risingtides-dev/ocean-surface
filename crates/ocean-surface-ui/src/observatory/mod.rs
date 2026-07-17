pub mod adapter;
pub mod domain;
pub mod inspector;
pub mod layout;
pub mod reducer;
pub mod scene;

use leptos::prelude::*;

use crate::daemon::Daemon;
use adapter::ObservatoryClient;
use domain::IntegrityState;
use inspector::{ExecutionInspector, SemanticExecutionList};
use scene::FloorScene;

#[component]
pub fn OceanFloor(daemon: Daemon, open: RwSignal<bool>) -> impl IntoView {
    let client = ObservatoryClient::new();
    let selected = RwSignal::new(None::<String>);
    let semantic_view = RwSignal::new(forced_colors_active());
    let replay = RwSignal::new(false);
    let replay_max = RwSignal::new(0_u64);
    let initial_zoom = initial_floor_zoom();
    let zoom = RwSignal::new(initial_zoom);
    let base_url = daemon.url;

    client.connect(base_url);
    on_cleanup(move || client.stop());

    Effect::new(move |_| {
        let state = client.state.get();
        if !replay.get() {
            replay_max.set(state.cursor);
        }
        if selected
            .get()
            .as_ref()
            .is_some_and(|id| !state.nodes.contains_key(id))
        {
            selected.set(None);
        }
    });

    let daemon_for_transcript = daemon.clone();
    let open_transcript = Callback::new(move |session_id: String| {
        if session_id.is_empty() {
            return;
        }
        daemon_for_transcript.switch_session(session_id, "Observed session".into());
        open.set(false);
    });

    view! {
        <section class="ocean-floor" role="dialog" aria-modal="true" aria-label="Ocean Floor observatory">
            <header class="ocean-floor__header">
                <div class="ocean-floor__identity">
                    <crate::icons::WaveBadge spinning=false compact=true />
                    <div>
                        <strong>"Ocean Floor"</strong>
                        <span>"observatory · durable real events"</span>
                    </div>
                </div>
                <div class="ocean-floor__telemetry" aria-live="polite">
                    <span class=format!("ocean-floor__connection is-{}", client.connection.get().label())>
                        <span class="ocean-floor__connection-dot"></span>
                        {move || client.connection.get().label()}
                    </span>
                    <span>{move || {
                        let state = client.state.get();
                        format!("{} active · {} running", state.active_count(), state.running_count())
                    }}</span>
                    <span>{move || format!("cursor {}", client.state.get().cursor)}</span>
                </div>
                <div class="ocean-floor__header-actions">
                    <div class="ocean-floor__view-switch" role="group" aria-label="Observatory view">
                        <button
                            type="button"
                            class:active=move || !semantic_view.get()
                            aria-pressed=move || (!semantic_view.get()).to_string()
                            on:click=move |_| semantic_view.set(false)
                        >"Floor"</button>
                        <button
                            type="button"
                            class:active=move || semantic_view.get()
                            aria-pressed=move || semantic_view.get().to_string()
                            on:click=move |_| semantic_view.set(true)
                        >"List"</button>
                    </div>
                    <button
                        class="ocean-floor__icon-button"
                        type="button"
                        aria-label="close Ocean Floor"
                        on:click=move |_| open.set(false)
                    >"✕"</button>
                </div>
            </header>

            <Show when=move || !client.state.get().attention.is_empty()>
                <section class="ocean-floor__attention" aria-label="Needs attention">
                    <strong>"Needs attention"</strong>
                    <div class="ocean-floor__attention-items">
                        <For
                            each=move || client.state.get().attention
                            key=|item| format!("{}:{}", item.execution_id, item.reason)
                            children=move |item| {
                                let execution_id = item.execution_id.clone();
                                view! {
                                    <button
                                        type="button"
                                        class=format!("ocean-floor__attention-item is-{}", item.priority.label())
                                        on:click=move |_| selected.set(Some(execution_id.clone()))
                                    >
                                        <span class="ocean-floor__priority-shape" aria-hidden="true"></span>
                                        <span>{item.reason.replace('_', " ")}</span>
                                    </button>
                                }
                            }
                        />
                    </div>
                </section>
            </Show>

            <Show when=move || client.state.get().integrity != IntegrityState::Live>
                <div class="ocean-floor__integrity" role="status">
                    <strong>{move || client.state.get().integrity.label()}</strong>
                    <span>{move || client.state.get().last_error.unwrap_or_else(|| "Requesting a fresh snapshot.".into())}</span>
                    <button type="button" on:click=move |_| client.refresh(base_url)>"Refresh"</button>
                </div>
            </Show>

            <main class="ocean-floor__main" class:has-inspector=move || selected.get().is_some()>
                <Show
                    when=move || !semantic_view.get()
                    fallback=move || view! {
                        <SemanticExecutionList state=client.state selected=selected />
                    }
                >
                    <div class="ocean-floor__scene-shell">
                        <div class="ocean-floor__scene-tools" role="group" aria-label="Floor zoom">
                            <button
                                type="button"
                                aria-label="zoom out"
                                on:click=move |_| zoom.update(|value| *value = next_zoom(*value, false))
                            >"−"</button>
                            <span>{move || format!("{:.0}%", zoom.get() * 100.0)}</span>
                            <button
                                type="button"
                                aria-label="zoom in"
                                on:click=move |_| zoom.update(|value| *value = next_zoom(*value, true))
                            >"+"</button>
                            <button type="button" on:click=move |_| zoom.set(initial_zoom)>"Reset"</button>
                        </div>
                        <FloorScene state=client.state selected=selected zoom=zoom />
                    </div>
                </Show>
                <ExecutionInspector
                    state=client.state
                    selected=selected
                    open_transcript=open_transcript
                />
            </main>

            <footer class="ocean-floor__replay" aria-label="Replay controls">
                <button
                    type="button"
                    class:active=move || !replay.get()
                    on:click=move |_| {
                        replay.set(false);
                        client.connect(base_url);
                    }
                >"● Live"</button>
                <span>"Replay"</span>
                <input
                    type="range"
                    aria-label="Replay cursor"
                    min=move || client.state.get().earliest_cursor.to_string()
                    max=move || replay_max.get().max(client.state.get().cursor).to_string()
                    prop:value=move || client.state.get().cursor.to_string()
                    on:change=move |event| {
                        let value = event_target_value(&event).parse::<u64>().unwrap_or_default();
                        replay.set(true);
                        client.stop();
                        client.snapshot_at(base_url, value);
                    }
                />
                <output>{move || client.state.get().cursor}</output>
                <button type="button" on:click=move |_| client.refresh(base_url)>"Refresh"</button>
            </footer>

            <Show when=move || client.loading.get() && client.state.get().nodes.is_empty()>
                <div class="ocean-floor__loading" role="status">
                    <crate::icons::WaveBadge spinning=true compact=false />
                    <span>"Reading the durable floor…"</span>
                </div>
            </Show>
        </section>
    }
}

fn initial_floor_zoom() -> f64 {
    let width = web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|value| value.as_f64())
        .unwrap_or(1_024.0);
    if width < 500.0 {
        0.5
    } else if width < 900.0 {
        0.75
    } else {
        1.0
    }
}

const ZOOM_LEVELS: [f64; 5] = [0.5, 0.75, 1.0, 1.5, 2.0];

fn next_zoom(current: f64, increase: bool) -> f64 {
    if increase {
        ZOOM_LEVELS
            .into_iter()
            .find(|level| *level > current)
            .unwrap_or(2.0)
    } else {
        ZOOM_LEVELS
            .into_iter()
            .rev()
            .find(|level| *level < current)
            .unwrap_or(0.5)
    }
}

fn forced_colors_active() -> bool {
    web_sys::window()
        .and_then(|window| window.match_media("(forced-colors: active)").ok())
        .flatten()
        .map(|query| query.matches())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::domain::{ExecutionPhase, ExecutionState, ObservatoryState, TruthProvenance};

    #[test]
    fn active_counts_are_truthful_and_terminal_aware() {
        let mut state = ObservatoryState::default();
        for (id, phase) in [
            ("running", ExecutionPhase::Running),
            ("done", ExecutionPhase::Finished),
        ] {
            state.nodes.insert(
                id.into(),
                ExecutionState {
                    execution_id: id.into(),
                    root_execution_id: id.into(),
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
                    last_cursor: 1,
                    duration_millis: None,
                },
            );
        }
        assert_eq!(state.active_count(), 1);
        assert_eq!(state.running_count(), 1);
    }
}
