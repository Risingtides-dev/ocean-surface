//! Dynamic, mode-separated Tauri titlebar Island.
//!
//! The compact object opens focused-agent interaction. Session switching and
//! transcript recall are distinct surfaces with distinct entry points and
//! result semantics; they are never stacked into one dashboard.

use leptos::ev::{KeyboardEvent, SubmitEvent};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::daemon::{Daemon, HistorySearchHit};
use crate::island::{
    compact_attention_counts, derive_attention_items, derive_island_sessions,
    permission_action_state, search_island, stop_action_state, IslandAttentionItem, IslandResult,
};
use crate::sessions::fmt_relative_time;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IslandMode {
    Closed,
    Agent,
    Sessions,
    Recall,
}

fn option_id(prefix: &str, value: &str) -> String {
    let encoded = value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}-{encoded}")
}

fn next_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (index + 1) % len
    }
}

fn previous_index(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (index + len - 1) % len
    }
}

fn history_result_label(hit: &HistorySearchHit) -> String {
    let title = if hit.session_title.trim().is_empty() {
        "Untitled session"
    } else {
        hit.session_title.trim()
    };
    format!("Open history match from {title}")
}

/// Leptos keyed rows capture snapshot strings in their child closures. Include
/// every rendered field so an authoritative update with the same id remounts
/// the row instead of retaining stale detail or action context.
fn agent_view_key(item: &IslandAttentionItem) -> String {
    format!(
        "{:?}",
        (
            &item.id,
            item.state,
            &item.request_id,
            &item.permission_id,
            &item.session_id,
            &item.session_title,
            &item.project,
            &item.tool,
            &item.detail,
            &item.updated_at,
        )
    )
}

fn history_view_key(hit: &HistorySearchHit) -> String {
    format!(
        "{:?}",
        (
            &hit.hit_id,
            &hit.session_id,
            &hit.session_title,
            &hit.role,
            &hit.excerpt,
            &hit.timestamp_ms,
            &hit.workspace_root,
            hit.score.to_bits(),
            &hit.match_kind,
        )
    )
}

#[component]
fn AgentStage(
    daemon: Daemon,
    items: Memo<Vec<IslandAttentionItem>>,
    selected_index: RwSignal<usize>,
    on_close: Callback<()>,
    on_mode: Callback<IslandMode>,
) -> impl IntoView {
    let prompt = RwSignal::new(String::new());
    let prompt_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let focused_id = daemon.session_id;
    let permission_view = daemon.permission_view;
    let permission_list_stale = daemon.permission_list_stale;
    let active_decision_token = daemon.active_decision_token;
    let request_cancellations = daemon.request_cancellations;
    let status = daemon.status;
    let daemon = StoredValue::new(daemon);

    Effect::new(move |_| {
        let len = items.get().len();
        if len == 0 {
            selected_index.set(0);
            request_animation_frame(move || {
                if let Some(input) = prompt_ref.get() {
                    let _ = input.focus();
                }
            });
        } else if selected_index.get() >= len {
            selected_index.set(0);
        }
    });

    let submit = move |event: SubmitEvent| {
        event.prevent_default();
        let text = prompt.get_untracked().trim().to_string();
        if text.is_empty() {
            return;
        }
        daemon.get_value().send_prompt(text);
        prompt.set(String::new());
        on_close.run(());
    };

    view! {
        <section class="island-agent" aria-label="Agent interaction">
            <header class="island-stage__header">
                <div class="island-stage__identity">
                    <span class="island-stage__eyebrow">"Agent"</span>
                    <strong>{move || {
                        if items.get().is_empty() { "Focused agent" } else { "Current work" }
                    }}</strong>
                </div>
                <div class="island-stage__routes" aria-label="Island tools">
                    <button type="button" on:click=move |_| on_mode.run(IslandMode::Sessions)>
                        "Sessions"
                    </button>
                    <button type="button" on:click=move |_| on_mode.run(IslandMode::Recall)>
                        "Recall"
                    </button>
                </div>
            </header>

            // TASK-69 seam 2: the cross-session permission poll failed, so the
            // attention list below may show already-resolved gates. Flag it as
            // stale rather than presenting it as confidently authoritative.
            <Show when=move || permission_list_stale.with(Option::is_some)>
                <div class="island-agent__stale" role="status">
                    "Permissions couldn't refresh — attention may be out of date."
                </div>
            </Show>

            <Show when=move || items.get().is_empty()>
                <div class="island-agent__idle">
                    <span class="island-agent__pulse" aria-hidden="true"></span>
                    <div>
                        <strong>"Ocean is ready"</strong>
                        <p>{move || status.get()}</p>
                    </div>
                </div>
                <form class="island-agent__composer" on:submit=submit>
                    <input
                        node_ref=prompt_ref
                        type="text"
                        aria-label="Ask Ocean in the focused session"
                        placeholder="Ask Ocean…"
                        autocomplete="off"
                        prop:value=move || prompt.get()
                        on:input=move |event| prompt.set(event_target_value(&event))
                    />
                    <button type="submit" disabled=move || prompt.get().trim().is_empty()>
                        "Send"
                    </button>
                </form>
            </Show>

            <Show when=move || !items.get().is_empty()>
                <Show when=move || items.get().len().ge(&2)>
                    <div class="island-agent__position">
                        <span>{move || {
                            let len = items.get().len();
                            format!("Work {} of {len}", selected_index.get().min(len.saturating_sub(1)) + 1)
                        }}</span>
                        <div class="island-agent__stepper">
                            <button
                                type="button"
                                aria-label="Previous agent work item"
                                on:click=move |_| {
                                    let len = items.get_untracked().len();
                                    selected_index.update(|index| *index = previous_index(*index, len));
                                }
                            >"←"</button>
                            <button
                                type="button"
                                aria-label="Next agent work item"
                                on:click=move |_| {
                                    let len = items.get_untracked().len();
                                    selected_index.update(|index| *index = next_index(*index, len));
                                }
                            >"→"</button>
                        </div>
                    </div>
                </Show>

                <For
                    each=move || {
                        items
                            .get()
                            .get(selected_index.get())
                            .cloned()
                            .into_iter()
                            .collect::<Vec<_>>()
                    }
                    key=agent_view_key
                    children=move |item| {
                        let item_class = format!(
                            "island-agent__work {}",
                            item.state.class_name(),
                        );
                        let state_label = item.state.label();
                        let session_name = item.session_title.clone();
                        let detail = item.detail.clone();
                        let project = item.project.clone().unwrap_or_default();
                        let tool = item.tool.clone().unwrap_or_default();
                        let updated = if item.updated_at.is_empty() {
                            String::new()
                        } else {
                            fmt_relative_time(&item.updated_at)
                        };
                        let permission_id = StoredValue::new_local(item.permission_id.clone());
                        let permission_session = StoredValue::new_local(item.session_id.clone());
                        let open_target = StoredValue::new_local((
                            item.session_id.clone(),
                            item.session_title.clone(),
                        ));
                        let request_id = StoredValue::new_local(item.request_id.clone());
                        let item_state = item.state;
                        let permission_action = Memo::new(move |_| {
                            permission_view.with(|v| {
                                permission_action_state(
                                    permission_id.get_value().as_deref(),
                                    permission_session.get_value().as_deref(),
                                    focused_id.get().as_deref(),
                                    active_decision_token.get().is_some(),
                                    v.cards(),
                                )
                            })
                        });
                        let stop_action = Memo::new(move |_| {
                            stop_action_state(
                                item_state,
                                &request_id.get_value(),
                                &request_cancellations.get(),
                            )
                        });
                        let open_action = Memo::new(move |_| {
                            let (session_id, _) = open_target.get_value();
                            permission_action.get().is_none()
                                && session_id.is_some()
                                && session_id.as_deref() != focused_id.get().as_deref()
                        });

                        view! {
                            <article class=item_class>
                                <div class="island-agent__work-head">
                                    <span class="island-agent__state">
                                        <i aria-hidden="true"></i>
                                        {state_label}
                                    </span>
                                    <strong>{session_name}</strong>
                                </div>
                                <p class="island-agent__detail">{detail}</p>
                                <div class="island-agent__meta">
                                    <span>{project}</span>
                                    <span>{tool}</span>
                                    <span>{updated}</span>
                                </div>
                                <div class="island-agent__actions">
                                    <Show when=move || open_action.get()>
                                        <button
                                            class="island-agent__open"
                                            type="button"
                                            on:click=move |_| {
                                                let (session_id, title) = open_target.get_value();
                                                if let Some(session_id) = session_id {
                                                    daemon.get_value().switch_session(session_id, title);
                                                    on_close.run(());
                                                }
                                            }
                                        >"Open session"</button>
                                    </Show>
                                    <Show when=move || stop_action.get().is_some()>
                                        <button
                                            class="island-agent__stop"
                                            type="button"
                                            disabled=move || stop_action.get().unwrap_or(true)
                                            on:click=move |_| {
                                                daemon.get_value().cancel_request(request_id.get_value());
                                            }
                                        >
                                            {move || if stop_action.get() == Some(true) { "Stopping…" } else { "Stop" }}
                                        </button>
                                    </Show>
                                    <Show when=move || permission_action.get().is_some()>
                                        <button
                                            class="island-agent__deny"
                                            type="button"
                                            disabled=move || permission_action.get().unwrap_or(true)
                                            on:click=move |_| {
                                                if let Some(permission_id) = permission_id.get_value() {
                                                    daemon.get_value().decide_permission(permission_id, false);
                                                }
                                            }
                                        >"Deny"</button>
                                        <button
                                            class="island-agent__approve"
                                            type="button"
                                            disabled=move || permission_action.get().unwrap_or(true)
                                            on:click=move |_| {
                                                if let Some(permission_id) = permission_id.get_value() {
                                                    daemon.get_value().decide_permission(permission_id, true);
                                                }
                                            }
                                        >
                                            {move || if permission_action.get() == Some(true) { "Working…" } else { "Approve" }}
                                        </button>
                                    </Show>
                                </div>
                            </article>
                        }
                    }
                />
            </Show>
        </section>
    }
}

#[component]
pub fn DynamicIsland(
    daemon: Daemon,
    mode: RwSignal<IslandMode>,
    focus_request: RwSignal<u64>,
    on_open: Callback<IslandMode>,
) -> impl IntoView {
    let session_query = RwSignal::new(String::new());
    let history_query = RwSignal::new(String::new());
    let session_selected = RwSignal::new(0usize);
    let history_selected = RwSignal::new(0usize);
    let agent_selected = RwSignal::new(0usize);
    let history_debounce = RwSignal::new(0u64);
    let previous_mode = RwSignal::new(IslandMode::Closed);
    let previous_focus = StoredValue::new_local(None::<web_sys::HtmlElement>);
    let last_focus_request = RwSignal::new(0u64);
    let session_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let history_input_ref: NodeRef<leptos::html::Input> = NodeRef::new();
    let chip_ref: NodeRef<leptos::html::Button> = NodeRef::new();
    let stage_ref: NodeRef<leptos::html::Section> = NodeRef::new();

    #[cfg(target_arch = "wasm32")]
    let attention_interval = {
        let daemon = daemon.clone();
        send_wrapper::SendWrapper::new(gloo_timers::callback::Interval::new(3_000, move || {
            daemon.fetch_attention();
        }))
    };
    #[cfg(target_arch = "wasm32")]
    on_cleanup(move || drop(attention_interval));

    let session_list = daemon.session_list;
    let request_list = daemon.request_list;
    let permission_list = daemon.permission_list;
    let projects = daemon.projects;
    let focused_id = daemon.session_id;
    let history_results = daemon.history_results;
    let history_searching = daemon.history_searching;
    let history_error = daemon.history_error;
    let daemon_value = StoredValue::new(daemon.clone());

    daemon.fetch_sessions();
    daemon.fetch_projects();
    daemon.fetch_attention();

    let sessions = Memo::new(move |_| {
        derive_island_sessions(
            &session_list.get(),
            &projects.get(),
            focused_id.get().as_deref(),
        )
    });
    let attention_items = Memo::new(move |_| {
        derive_attention_items(&request_list.get(), &permission_list.get(), &sessions.get())
    });
    let attention_counts = Memo::new(move |_| compact_attention_counts(&attention_items.get()));
    let session_results = Memo::new(move |_| {
        let mut matches = search_island(&session_query.get(), &sessions.get());
        matches.truncate(if session_query.get().trim().is_empty() {
            8
        } else {
            20
        });
        matches
    });

    Effect::new(move |_| {
        let len = session_results.get().len();
        if len == 0 || session_selected.get() >= len {
            session_selected.set(0);
        }
    });
    Effect::new(move |_| {
        let len = history_results.get().len();
        if len == 0 || history_selected.get() >= len {
            history_selected.set(0);
        }
    });

    Effect::new(move |_| {
        let next = mode.get();
        let request = focus_request.get();
        let previous = previous_mode.get_untracked();
        let previous_request = last_focus_request.get_untracked();
        if next != IslandMode::Closed && previous == IslandMode::Closed {
            previous_focus.set_value(
                web_sys::window()
                    .and_then(|window| window.document())
                    .and_then(|document| document.active_element())
                    .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok()),
            );
            let daemon = daemon_value.get_value();
            daemon.fetch_sessions();
            daemon.fetch_projects();
            daemon.fetch_attention();
        }
        let should_focus = next != IslandMode::Closed
            && (previous == IslandMode::Closed || next != previous || request != previous_request);
        if should_focus {
            request_animation_frame(move || match next {
                IslandMode::Sessions => {
                    if let Some(input) = session_input_ref.get() {
                        let _ = input.focus();
                        input.select();
                    }
                }
                IslandMode::Recall => {
                    if let Some(input) = history_input_ref.get() {
                        let _ = input.focus();
                        input.select();
                    }
                }
                IslandMode::Agent => {
                    if let Some(stage) = stage_ref.get() {
                        let _ = stage.focus();
                    }
                }
                IslandMode::Closed => {}
            });
        } else if next == IslandMode::Closed && previous != IslandMode::Closed {
            request_animation_frame(move || {
                if let Some(element) = previous_focus.get_value().filter(|el| el.is_connected()) {
                    let _ = element.focus();
                } else if let Some(chip) = chip_ref.get() {
                    let _ = chip.focus();
                }
            });
        }
        previous_mode.set(next);
        last_focus_request.set(request);
    });

    let close = Callback::new(move |()| mode.set(IslandMode::Closed));
    let switch_mode = Callback::new(move |next| on_open.run(next));
    let selected_session_id = move || {
        session_results
            .get()
            .get(session_selected.get())
            .map(|result| match result {
                IslandResult::Session(item) => option_id("island-session", &item.id),
            })
            .unwrap_or_default()
    };
    let selected_history_id = move || {
        history_results
            .get()
            .get(history_selected.get())
            .map(|hit| option_id("island-history", &hit.hit_id))
            .unwrap_or_default()
    };

    let on_keydown = move |event: KeyboardEvent| {
        if event.is_composing() || mode.get_untracked() == IslandMode::Closed {
            return;
        }
        if event.key() == "Escape" {
            event.prevent_default();
            event.stop_propagation();
            close.run(());
            return;
        }

        match mode.get_untracked() {
            IslandMode::Sessions => {
                let from_input = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
                    .is_some_and(|element| element.class_list().contains("island-search__input"));
                if !from_input {
                    return;
                }
                let len = session_results.get_untracked().len();
                match event.key().as_str() {
                    "ArrowDown" => {
                        event.prevent_default();
                        session_selected.update(|index| *index = next_index(*index, len));
                    }
                    "ArrowUp" => {
                        event.prevent_default();
                        session_selected.update(|index| *index = previous_index(*index, len));
                    }
                    "Enter" => {
                        event.prevent_default();
                        if let Some(IslandResult::Session(item)) = session_results
                            .get_untracked()
                            .get(session_selected.get_untracked())
                        {
                            daemon_value
                                .get_value()
                                .switch_session(item.id.clone(), item.title.clone());
                            close.run(());
                        }
                    }
                    _ => {}
                }
            }
            IslandMode::Recall => {
                let from_input = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
                    .is_some_and(|element| element.class_list().contains("island-recall__input"));
                if !from_input {
                    return;
                }
                let len = history_results.get_untracked().len();
                match event.key().as_str() {
                    "ArrowDown" => {
                        event.prevent_default();
                        history_selected.update(|index| *index = next_index(*index, len));
                    }
                    "ArrowUp" => {
                        event.prevent_default();
                        history_selected.update(|index| *index = previous_index(*index, len));
                    }
                    "Enter" => {
                        event.prevent_default();
                        if let Some(hit) = history_results
                            .get_untracked()
                            .get(history_selected.get_untracked())
                        {
                            daemon_value
                                .get_value()
                                .switch_session(hit.session_id.clone(), hit.session_title.clone());
                            close.run(());
                        }
                    }
                    _ => {}
                }
            }
            IslandMode::Agent => match event.key().as_str() {
                "ArrowLeft" => {
                    let len = attention_items.get_untracked().len();
                    if len > 1 {
                        event.prevent_default();
                        agent_selected.update(|index| *index = previous_index(*index, len));
                    }
                }
                "ArrowRight" => {
                    let len = attention_items.get_untracked().len();
                    if len > 1 {
                        event.prevent_default();
                        agent_selected.update(|index| *index = next_index(*index, len));
                    }
                }
                _ => {}
            },
            IslandMode::Closed => {}
        }
    };

    let focused_session = Memo::new(move |_| {
        sessions
            .get()
            .into_iter()
            .find(|session| session.is_focused)
    });
    let chip_status = move || {
        let (needs_human, running) = attention_counts.get();
        if needs_human == 1 {
            "1 needs you".to_string()
        } else if needs_human > 1 {
            format!("{needs_human} need you")
        } else if running > 0 {
            format!("{running} running")
        } else {
            "Ready".to_string()
        }
    };
    let chip_label = move || {
        focused_session.get().map_or_else(
            || format!("Ocean agent. {}. Open agent interaction", chip_status()),
            |session| {
                format!(
                    "Ocean agent in session {}. {}. Open agent interaction",
                    session.title,
                    chip_status(),
                )
            },
        )
    };
    let stage_label = move || match mode.get() {
        IslandMode::Agent => "Agent interaction",
        IslandMode::Sessions => "Switch session",
        IslandMode::Recall => "Recall transcript history",
        IslandMode::Closed => "Ocean Island",
    };

    view! {
        <section class="island-host island-host--dynamic" on:keydown=on_keydown>
            <button
                class=move || {
                    let (needs_human, running) = attention_counts.get();
                    if needs_human > 0 {
                        "island-chip is-attention"
                    } else if running > 0 {
                        "island-chip is-running"
                    } else {
                        "island-chip"
                    }
                }
                type="button"
                node_ref=chip_ref
                aria-label=chip_label
                aria-haspopup="dialog"
                aria-expanded=move || (mode.get() != IslandMode::Closed).to_string()
                aria-controls="island-stage"
                on:click=move |_| {
                    if mode.get_untracked() == IslandMode::Closed {
                        on_open.run(IslandMode::Agent);
                    } else {
                        close.run(());
                    }
                }
            >
                <span
                    class=move || {
                        let (needs_human, running) = attention_counts.get();
                        if needs_human > 0 {
                            "island-chip__dot is-attention"
                        } else if running > 0 {
                            "island-chip__dot is-running"
                        } else if focused_session.get().is_none() {
                            "island-chip__dot is-empty"
                        } else {
                            "island-chip__dot"
                        }
                    }
                    aria-hidden="true"
                ></span>
                <span class="island-chip__title">
                    {move || focused_session.get().map(|item| item.title).unwrap_or_else(|| "Ocean".into())}
                </span>
                <Show when=move || focused_session.get().and_then(|item| item.project).is_some()>
                    <span class="island-chip__separator" aria-hidden="true">"·"</span>
                    <span class="island-chip__project">
                        {move || focused_session.get().and_then(|item| item.project).unwrap_or_default()}
                    </span>
                </Show>
                <span class="island-chip__separator" aria-hidden="true">"·"</span>
                <span class="island-chip__count">{move || chip_status()}</span>
            </button>
            <span class="island-live" aria-live="polite" aria-atomic="true">
                {move || chip_status()}
            </span>

            <Show when=move || mode.get() != IslandMode::Closed>
                <div class="island-dismiss-layer" aria-hidden="true" on:click=move |_| close.run(())></div>
                <section
                    id="island-stage"
                    class=move || format!(
                        "island-stage island-stage--{}",
                        match mode.get() {
                            IslandMode::Agent => "agent",
                            IslandMode::Sessions => "sessions",
                            IslandMode::Recall => "recall",
                            IslandMode::Closed => "closed",
                        }
                    )
                    node_ref=stage_ref
                    tabindex="-1"
                    role="dialog"
                    aria-modal="false"
                    aria-label=stage_label
                >
                    <Show when=move || mode.get() == IslandMode::Agent>
                        <AgentStage
                            daemon=daemon_value.get_value()
                            items=attention_items
                            selected_index=agent_selected
                            on_close=close
                            on_mode=switch_mode
                        />
                    </Show>

                    <Show when=move || mode.get() == IslandMode::Sessions>
                        <section class="island-sessions" aria-label="Sessions">
                            <header class="island-stage__header">
                                <div class="island-stage__identity">
                                    <span class="island-stage__eyebrow">"Sessions"</span>
                                    <strong>"Switch focus"</strong>
                                </div>
                                <span class="island-stage__hint">"⌘P"</span>
                            </header>
                            <div class="island-search">
                                <input
                                    class="island-search__input"
                                    type="search"
                                    role="combobox"
                                    aria-label="Search sessions"
                                    aria-autocomplete="list"
                                    aria-expanded="true"
                                    aria-controls="island-session-results"
                                    aria-activedescendant=move || {
                                        let id = selected_session_id();
                                        (!id.is_empty()).then_some(id)
                                    }
                                    placeholder="Session, project, path, branch…"
                                    autocomplete="off"
                                    node_ref=session_input_ref
                                    prop:value=move || session_query.get()
                                    on:input=move |event| {
                                        session_query.set(event_target_value(&event));
                                        session_selected.set(0);
                                    }
                                />
                            </div>
                            <div id="island-session-results" class="island-results" role="listbox">
                                <Show
                                    when=move || !session_results.get().is_empty()
                                    fallback=|| view! { <div class="island-empty">"No matching sessions"</div> }
                                >
                                    <For
                                        each=move || session_results.get()
                                        key=|result| match result { IslandResult::Session(item) => item.id.clone() }
                                        children=move |result| {
                                            let IslandResult::Session(item) = result;
                                            let row_id = option_id("island-session", &item.id);
                                            let selected_id = row_id.clone();
                                            let aria_id = row_id.clone();
                                            let switch_id = item.id.clone();
                                            let switch_title = item.title.clone();
                                            let title = item.title.clone();
                                            let project = item.project.clone().unwrap_or_default();
                                            let cwd = item.cwd.clone();
                                            let branch = item.branch.clone().unwrap_or_default();
                                            let updated = fmt_relative_time(&item.updated_at);
                                            let is_focused = item.is_focused;
                                            view! {
                                                <button
                                                    id=row_id
                                                    class="island-result"
                                                    class:island-result--selected=move || selected_session_id() == selected_id
                                                    class:island-result--focused=is_focused
                                                    type="button"
                                                    role="option"
                                                    tabindex="-1"
                                                    aria-selected=move || (selected_session_id() == aria_id).to_string()
                                                    on:click=move |_| {
                                                        daemon_value.get_value().switch_session(
                                                            switch_id.clone(),
                                                            switch_title.clone(),
                                                        );
                                                        close.run(());
                                                    }
                                                >
                                                    <span class="island-result__primary">
                                                        <span class="island-result__title">{title}</span>
                                                        <Show when=move || is_focused>
                                                            <span class="island-result__focused">"Focused"</span>
                                                        </Show>
                                                    </span>
                                                    <span class="island-result__meta">
                                                        <span>{project}</span>
                                                        <span class="island-result__path">{cwd}</span>
                                                        <span>{branch}</span>
                                                        <span>{updated}</span>
                                                        <span>{format!("{} turns", item.turn_count)}</span>
                                                    </span>
                                                </button>
                                            }
                                        }
                                    />
                                </Show>
                            </div>
                        </section>
                    </Show>

                    <Show when=move || mode.get() == IslandMode::Recall>
                        <section class="island-recall" aria-label="Recall history">
                            <header class="island-stage__header">
                                <div class="island-stage__identity">
                                    <span class="island-stage__eyebrow">"Recall"</span>
                                    <strong>"Search transcript history"</strong>
                                </div>
                                <span class="island-stage__hint">"⌘⇧F"</span>
                            </header>
                            <div class="island-recall__search">
                                <input
                                    class="island-recall__input"
                                    type="search"
                                    role="combobox"
                                    aria-label="Search transcript history"
                                    aria-autocomplete="list"
                                    aria-expanded="true"
                                    aria-controls="island-history-results"
                                    aria-activedescendant=move || {
                                        let id = selected_history_id();
                                        (!id.is_empty()).then_some(id)
                                    }
                                    placeholder="What did we discuss, decide, or change?"
                                    autocomplete="off"
                                    node_ref=history_input_ref
                                    prop:value=move || history_query.get()
                                    on:input=move |event| {
                                        let query = event_target_value(&event);
                                        history_query.set(query.clone());
                                        history_selected.set(0);
                                        let daemon = daemon_value.get_value();
                                        daemon.invalidate_history_search();
                                        if !query.trim().is_empty() {
                                            daemon.history_searching.set(true);
                                        }
                                        let generation = history_debounce.get_untracked().wrapping_add(1);
                                        history_debounce.set(generation);
                                        wasm_bindgen_futures::spawn_local(async move {
                                            #[cfg(target_arch = "wasm32")]
                                            gloo_timers::future::TimeoutFuture::new(220).await;
                                            if history_debounce.get_untracked() == generation {
                                                daemon.search_history(query);
                                            }
                                        });
                                    }
                                />
                            </div>
                            <div id="island-history-results" class="island-recall__results" role="listbox">
                                <Show when=move || history_query.get().trim().is_empty()>
                                    <div class="island-recall__prompt">
                                        "Search the words inside prior user and assistant turns—not just session titles."
                                    </div>
                                </Show>
                                <Show when=move || history_searching.get()>
                                    <div class="island-recall__prompt">"Searching history…"</div>
                                </Show>
                                <Show when=move || history_error.get().is_some()>
                                    <div class="island-recall__error">
                                        {move || history_error.get().unwrap_or_default()}
                                    </div>
                                </Show>
                                <Show when=move || {
                                    !history_query.get().trim().is_empty()
                                        && !history_searching.get()
                                        && history_error.get().is_none()
                                        && history_results.get().is_empty()
                                }>
                                    <div class="island-recall__prompt">"No transcript matches"</div>
                                </Show>
                                <For
                                    each=move || history_results.get()
                                    key=history_view_key
                                    children=move |hit| {
                                        let row_id = option_id("island-history", &hit.hit_id);
                                        let selected_id = row_id.clone();
                                        let aria_id = row_id.clone();
                                        let session_id = hit.session_id.clone();
                                        let session_title = hit.session_title.clone();
                                        let title = if hit.session_title.trim().is_empty() {
                                            "Untitled session".to_string()
                                        } else {
                                            hit.session_title.clone()
                                        };
                                        let role = hit.role.clone();
                                        let excerpt = hit.excerpt.clone();
                                        let workspace = hit.workspace_root.clone().unwrap_or_default();
                                        let match_kind = hit.match_kind.clone();
                                        let label = history_result_label(&hit);
                                        view! {
                                            <button
                                                id=row_id
                                                class="island-recall__result"
                                                class:island-recall__result--selected=move || selected_history_id() == selected_id
                                                type="button"
                                                role="option"
                                                tabindex="-1"
                                                aria-label=label
                                                aria-selected=move || (selected_history_id() == aria_id).to_string()
                                                on:click=move |_| {
                                                    daemon_value.get_value().switch_session(
                                                        session_id.clone(),
                                                        session_title.clone(),
                                                    );
                                                    close.run(());
                                                }
                                            >
                                                <span class="island-recall__excerpt">{excerpt}</span>
                                                <span class="island-recall__meta">
                                                    <span>{title}</span>
                                                    <span>{role}</span>
                                                    <span>{workspace}</span>
                                                    <span>{match_kind}</span>
                                                </span>
                                            </button>
                                        }
                                    }
                                />
                            </div>
                        </section>
                    </Show>
                </section>
            </Show>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_agent_and_recall_rows_refresh_when_same_id_content_changes() {
        let mut work = IslandAttentionItem {
            id: "request:a".into(),
            request_id: "a".into(),
            permission_id: None,
            session_id: Some("session-a".into()),
            session_title: "Session A".into(),
            project: None,
            tool: None,
            detail: "first detail".into(),
            updated_at: "2026-07-15T00:00:00Z".into(),
            state: crate::island::IslandAttentionState::Running,
        };
        let first_work_key = agent_view_key(&work);
        work.detail = "replacement detail".into();
        assert_ne!(first_work_key, agent_view_key(&work));

        let mut hit = HistorySearchHit {
            hit_id: "session-a:1".into(),
            session_id: "session-a".into(),
            session_title: "Session A".into(),
            role: "assistant".into(),
            excerpt: "first excerpt".into(),
            timestamp_ms: None,
            workspace_root: None,
            score: 1.0,
            match_kind: "fuzzy".into(),
        };
        let first_hit_key = history_view_key(&hit);
        hit.excerpt = "replacement excerpt".into();
        assert_ne!(first_hit_key, history_view_key(&hit));
    }

    #[test]
    fn modes_are_distinct_interactions() {
        assert_ne!(IslandMode::Agent, IslandMode::Sessions);
        assert_ne!(IslandMode::Sessions, IslandMode::Recall);
        assert_ne!(IslandMode::Agent, IslandMode::Recall);
    }

    #[test]
    fn bounded_session_results_keep_browse_small() {
        let items = (0..30)
            .map(|index| crate::island::IslandSession {
                id: format!("session-{index}"),
                title: format!("Session {index}"),
                project: None,
                cwd: "/tmp".into(),
                branch: None,
                origin: None,
                updated_at: format!("2026-07-15T00:{index:02}:00Z"),
                turn_count: 1,
                is_focused: index == 0,
                priority: if index == 0 {
                    crate::island::IslandPriority::Focused
                } else {
                    crate::island::IslandPriority::Background
                },
            })
            .collect::<Vec<_>>();
        let mut results = search_island("", &items);
        results.truncate(8);
        assert_eq!(results.len(), 8);
    }

    #[test]
    fn index_navigation_wraps() {
        assert_eq!(next_index(2, 3), 0);
        assert_eq!(previous_index(0, 3), 2);
        assert_eq!(next_index(0, 0), 0);
    }
}
