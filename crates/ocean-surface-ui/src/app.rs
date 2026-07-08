//! Top-level app shell. Owns the Daemon, mounts the transcript + composer.

use leptos::ev::{self, SubmitEvent};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::components::PermissionPrompts;
use crate::daemon::{daemon_url_from_env, Daemon};
use crate::model::{Block, Role, Turn};
use crate::rooms::{Rooms, RoomsPanel};
use crate::sessions::SessionsPanel;
use crate::transcript::Transcript;
use crate::voice::VoiceOrb;

const COMPOSER_MIN_HEIGHT_PX: i32 = 40;
const COMPOSER_MAX_HEIGHT_PX: i32 = 240;

fn composer_height_px(scroll_height: i32) -> i32 {
    scroll_height.clamp(COMPOSER_MIN_HEIGHT_PX, COMPOSER_MAX_HEIGHT_PX)
}

fn composer_overflow_y(scroll_height: i32) -> &'static str {
    if scroll_height > COMPOSER_MAX_HEIGHT_PX {
        "auto"
    } else {
        "hidden"
    }
}

fn fit_composer_textarea(el: &web_sys::HtmlTextAreaElement) {
    let style = el
        .clone()
        .unchecked_into::<web_sys::HtmlElement>()
        .style();
    let _ = style.set_property("height", "auto");
    let scroll_height = el.scroll_height();
    let height = composer_height_px(scroll_height);
    let _ = style.set_property("height", &format!("{height}px"));
    let _ = style.set_property("overflow-y", composer_overflow_y(scroll_height));
}

fn reset_composer_textarea(el: &web_sys::HtmlTextAreaElement) {
    let style = el
        .clone()
        .unchecked_into::<web_sys::HtmlElement>()
        .style();
    let _ = style.set_property("height", &format!("{COMPOSER_MIN_HEIGHT_PX}px"));
    let _ = style.set_property("overflow-y", "hidden");
}

#[component]
pub fn App() -> impl IntoView {
    let daemon = Daemon::new(daemon_url_from_env());
    // Zero-config boot: fetch /api/config from the same-origin proxy to learn
    // the daemon URL + confirm auth is preconfigured, THEN connect AND fetch the
    // model catalogue — in that order, inside bootstrap. Falls back to
    // daemon_url_from_env() if no proxy answers.
    //
    // Do NOT add an eager daemon.fetch_models() (or any url-dependent call)
    // here: it would run before bootstrap learns the real origin, succeed by
    // luck on localhost, and silently fail from ocean.risingtidesviral.com
    // (wrong URL → empty model picker). Any startup fetch that needs the daemon
    // URL belongs INSIDE bootstrap_then_connect, after url.set().
    daemon.bootstrap_then_connect();

    let input = RwSignal::new(String::new());
    let textarea_ref: NodeRef<leptos::html::Textarea> = NodeRef::new();
    let daemon_council = daemon.clone();

    // Daemon holds only Copy signal handles, so cloning per-closure is cheap
    // and avoids fighting the borrow checker over a single moved value.
    let status = daemon.status;
    let status_detail = daemon.status_detail;
    let turns = daemon.turns;
    let streaming = daemon.streaming;
    let voice_ready = daemon.voice_ready;
    let last_turn_tokens = daemon.last_turn_tokens;
    let session_tokens = daemon.session_tokens;
    // `daemon.model` (the live global model signal) is no longer bound here —
    // its only consumer, the header model picker, was removed in OCEAN-202. The
    // composer's per-turn `model_override` is the surface's model control now.
    let models = daemon.models;
    // Browser-control indicator (OCEAN-92): lit while the agent is driving the
    // browser (set from the daemon's `browser_activity` SSE event), with the
    // most recent `browser_*` action shown alongside.
    let browser_active = daemon.browser_active;
    let livekit_token_path = daemon.livekit_token_path;
    let browser_last_action = daemon.browser_last_action;
    // Canvas patch stream (OCEAN-178): patches the agent applied this session,
    // streamed over the daemon's `surface_patch` SSE event. The GPUI native
    // shell renders these on a full canvas; the web surface renders a basic
    // representation so the data is no longer dropped at the transport layer.
    let canvas_patches = daemon.canvas_patches;
    // Per-turn overrides (OCEAN-79): reasoning effort + model. Both ride on the
    // next turn's request; `None` leaves the daemon defaults untouched.
    let thinking_level = daemon.thinking_level;
    let model_override = daemon.model_override;
    // Predicates pulled out of the view! macro: a bare `>` inside an attribute
    // expression would be parsed as the element's closing bracket.
    let has_tokens = move || session_tokens.get().total() > 0;
    let has_rate = move || {
        last_turn_tokens
            .get()
            .map(|t| t.tokens_per_second > 0.0)
            .unwrap_or(false)
    };

    // Sessions panel overlay.
    let show_sessions = RwSignal::new(false);
    // Council/quorum observability deck overlay (OCEAN-96). A native Leptos
    // stage (crate::council::CouncilStage) inside a full-screen modal — no
    // iframe, no proxied static page. It reads the daemon's folded Longhouse
    // topics snapshot (GET /v1/longhouse/topics), polled while the deck is
    // open.
    let show_council = RwSignal::new(false);
    // Call controls row — created early so Rooms::new can share the signal.
    let show_livekit_controls = RwSignal::new(false);
    let show_rooms = RwSignal::new(false);
    // Persistent Rooms panel (OCEAN-108). Shares the Daemon's `url` signal so it
    // targets the same origin; opens a right-hand overlay like Sessions.
    let rooms = Rooms::new(
        &daemon,
        daemon.livekit_room_id,
        daemon.livekit_token_path,
        show_livekit_controls,
        show_rooms,
    );
    let in_room_mode = Signal::derive(move || {
        rooms.open_key.get().is_some()
            && show_livekit_controls.get()
            && !livekit_token_path.get().trim().is_empty()
    });

    // TTS: speak the assistant's final text each time a turn finishes
    // (streaming flips true→false). Gated by `muted`. We track the previous
    // streaming value so we only fire on the falling edge, and remember the
    // last spoken turn so re-renders don't double-speak.
    let muted = RwSignal::new(false);
    let prev_streaming = RwSignal::new(false);
    let last_spoken: RwSignal<Option<String>> = RwSignal::new(None);
    Effect::new(move |_| {
        let now = streaming.get();
        let was = prev_streaming.get_untracked();
        prev_streaming.set(now);
        // Falling edge = a turn just completed.
        if was && !now {
            if let Some((id, text)) = latest_assistant_text(&turns.get_untracked()) {
                if last_spoken.get_untracked().as_deref() != Some(id.as_str()) {
                    last_spoken.set(Some(id));
                    crate::tts::speak(text, muted);
                }
            }
        }
    });

    // Pointer light: ONE window mousemove listener feeds cursor position to
    // :root as viewport percentages. Opted-in surfaces (.ocean-lit, defined
    // in styles/base.css) paint a faint radial specular there so they read
    // as catching one overhead light source. Cheap direct set per event —
    // two custom properties, no rAF. Bound + on_cleanup so the listener
    // lives with the App scope and is torn down on unmount.
    let _pointer_light = window_event_listener(ev::mousemove, move |e: web_sys::MouseEvent| {
        let Some(win) = web_sys::window() else { return };
        let Some(w) = win.inner_width().ok().and_then(|v| v.as_f64()) else { return };
        let Some(h) = win.inner_height().ok().and_then(|v| v.as_f64()) else { return };
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let x = e.client_x() as f64 / w * 100.0;
        let y = e.client_y() as f64 / h * 100.0;
        let Some(doc) = win.document() else { return };
        let Some(root) = doc.document_element() else { return };
        let Ok(root) = root.dyn_into::<web_sys::HtmlElement>() else { return };
        let style = root.style();
        let _ = style.set_property("--pointer-x", &format!("{x:.2}%"));
        let _ = style.set_property("--pointer-y", &format!("{y:.2}%"));
    });
    on_cleanup(move || _pointer_light.remove());

    let submit = {
        let daemon = daemon.clone();
        move |ev: SubmitEvent| {
            ev.prevent_default();
            let text = input.get_untracked();
            if text.trim().is_empty() {
                return;
            }
            input.set(String::new());
            daemon.send_prompt(text);
            // Refocus + collapse the textarea so a long prior prompt doesn't
            // leave the next turn trapped in a tall empty scrollbox.
            if let Some(el) = textarea_ref.get_untracked() {
                reset_composer_textarea(&el);
                let _ = el.focus();
            }
        }
    };

    // Wrap submit in a StoredValue so it can be shared across closures
    // without being consumed (the composer's submit handler needs it).
    let submit = StoredValue::new(submit);

    // Clone reserved for the SessionsPanel.
    let daemon_for_panel = daemon.clone();

    // Permission-approval overlay (OCEAN-64). Stored (Copy) so it can be handed
    // a fresh clone wherever the component is mounted without moving the main
    // `daemon` out of scope.
    let daemon_for_perms = StoredValue::new(daemon.clone());

    // Voice → text: drop the transcript into the composer and submit it via the
    // voice send path, which tags the turn `client_type="leo-voice"` so the
    // daemon applies its concise, speakable voice system prompt (OCEAN-181).
    // Otherwise the transcript would be tagged like a typed message.
    let on_transcript = {
        let daemon = daemon.clone();
        Callback::new(move |text: String| {
            let text = text.trim().to_string();
            if text.is_empty() {
                return;
            }
            input.set(text.clone());
            daemon.send_voice_prompt(text);
            input.set(String::new());
        })
    };
    let on_voice_status = Callback::new(move |msg: String| status.set(msg));

    // Clones for the composer's per-turn override controls (OCEAN-79). These
    // controls live INSIDE the chat-branch <Show> fallback, which must be `Fn`,
    // so they go through StoredValue (Copy) — a plain clone would be moved out of
    // the fallback environment and make it `FnOnce`.
    let daemon_thinking = StoredValue::new(daemon.clone());
    let daemon_model_override = StoredValue::new(daemon.clone());
    // StoredValue is Copy, so the halt button's closure (inside the chat-branch
    // <Show> fallback, which must be Fn) can grab the daemon without the
    // fallback moving a plain clone out of its environment.
    let daemon_halt = StoredValue::new(daemon.clone());
    // Screenshot capture button (OCEAN-138): StoredValue (Copy) so the on:click
    // closure can grab the daemon to stage the captured image for the next turn.
    let daemon_capture = StoredValue::new(daemon.clone());
    // Header overflow menu (<details>): council/rooms/mute/capture live behind
    // one "⋯" affordance instead of a row of buttons. Item clicks close it.
    let more_ref: NodeRef<leptos::html::Details> = NodeRef::new();
    // Call/collaboration controls are explicit reveals, not permanent top
    // chrome. The overflow menu opens them; the row below the header exists only
    // while one is intentionally active.
    let show_phone_dialer = RwSignal::new(false);
    let daemon_livekit = StoredValue::new(daemon.clone());
    let daemon_phone_call = StoredValue::new(daemon.clone());

    // In the Chrome side panel the cockpit lives in a ~360px-wide column. Tag
    // the root so the shared stylesheet's compact `.ocean-surface--extension`
    // rules apply, without forking the layout for the full-width web app.
    let root_class = if crate::daemon::running_as_extension() {
        "ocean-surface ocean-surface--extension"
    } else {
        "ocean-surface"
    };

    view! {
        <main class=root_class>
            <header class="ocean-header">
                <div class="ocean-brand" aria-label="Ocean">
                    // The OCEAN wordmark carries the TUI splash depth ramp,
                    // one solid color per letter (no gradient text) — the same
                    // xterm ramp the terminal banner paints per line.
                    <span class="ocean-brand__word" aria-hidden="true">
                        <span class="ocean-brand__ch ocean-brand__ch--1">"O"</span>
                        <span class="ocean-brand__ch ocean-brand__ch--2">"C"</span>
                        <span class="ocean-brand__ch ocean-brand__ch--3">"E"</span>
                        <span class="ocean-brand__ch ocean-brand__ch--4">"A"</span>
                        <span class="ocean-brand__ch ocean-brand__ch--5">"N"</span>
                    </span>
                </div>
                <div class="ocean-header__right">
                    // Sessions is the single header affordance — opens the
                    // centered sessions modal (chat / create project / resume).
                    // No project picker, active-session title, or cwd lives in
                    // the chrome; all session control moved into the modal.
                    <button
                        class="ocean-sessions-trigger"
                        type="button"
                        aria-label="sessions"
                        title="Sessions"
                        on:click=move |_| show_sessions.update(|v| *v = !*v)
                    >
                        "Sessions"
                    </button>
                    // Ambient runtime readouts — token usage, the browser-driving
                    // cue, and connection status — grouped into one demoted cluster
                    // so they read as secondary telemetry, not equal-weight peers
                    // to the primary header controls.
                    <div class="ocean-runtime">
                    // Token usage: session total, with a per-turn + cache
                    // breakdown on hover. Hidden until the first turn finishes.
                    <Show when=has_tokens>
                        <div
                            class="ocean-tokens"
                            title=move || {
                                let s = session_tokens.get();
                                let last = last_turn_tokens.get().unwrap_or_default();
                                format!(
                                    "Session — in {} · out {} · cache {} · total {}\nLast turn — in {} · out {} · {:.1} tok/s",
                                    s.input, s.output, s.cache_read, s.total(),
                                    last.input, last.output, last.tokens_per_second,
                                )
                            }
                        >
                            <span class="ocean-tokens__io">
                                {move || {
                                    let s = session_tokens.get();
                                    format!("↑{} ↓{}", fmt_tokens(s.input), fmt_tokens(s.output))
                                }}
                            </span>
                            <Show when=has_rate>
                                <span class="ocean-tokens__rate">
                                    {move || format!("{:.0} t/s", last_turn_tokens.get().unwrap_or_default().tokens_per_second)}
                                </span>
                            </Show>
                        </div>
                    </Show>
                    // Browser-control indicator (OCEAN-92). Visible only while
                    // Ocean is driving the browser; shows the last browser action
                    // (e.g. "navigate", "click") so the user sees what's happening
                    // in their tab. Driven by the daemon's browser_activity stream.
                    <Show when=move || browser_active.get()>
                        <div
                            class="ocean-browser-control"
                            title=move || match browser_last_action.get() {
                                Some(a) => format!("Ocean is driving the browser — last action: {a}"),
                                None => "Ocean is driving the browser".to_string(),
                            }
                        >
                            <span class="ocean-browser-control__dot"></span>
                            <span class="ocean-browser-control__label">
                                {move || match browser_last_action.get() {
                                    Some(a) => format!(
                                        "driving · {}",
                                        a.strip_prefix("browser_").unwrap_or(&a),
                                    ),
                                    None => "driving browser".to_string(),
                                }}
                            </span>
                        </div>
                    </Show>
                    // Tooltip carries the full raw payload, but only while the
                    // displayed status is the exact string stored alongside it —
                    // any later status.set (error or benign) drops the tooltip
                    // instead of leaking a stale payload.
                    <div
                        class="ocean-status"
                        class:is-quiet=move || matches!(
                            status.get().as_str(),
                            "connected" | "new session" | "session loaded"
                        )
                        aria-label=move || format!("status: {}", status.get())
                        title=move || {
                            status_detail
                                .get()
                                .filter(|(s, _)| *s == status.get())
                                .map(|(_, detail)| detail)
                                .unwrap_or_else(|| status.get())
                        }
                    >
                        <span class="ocean-status__dot"></span>
                        <span class="ocean-status__text">{move || status.get()}</span>
                    </div>
                    </div>
                    // Secondary actions live behind one overflow control:
                    // council deck, rooms, voice mute, extension tab capture.
                    // Death-by-buttons is a design defect; the header keeps
                    // exactly one icon button (sessions) plus this "⋯".
                    <details class="ocean-more" node_ref=more_ref>
                        <summary class="ocean-more__btn" aria-label="more actions" title="More">
                            "⋯"
                        </summary>
                        <div class="ocean-more__menu" role="menu">
                            <Show when=move || !livekit_token_path.get().trim().is_empty()>
                                <button
                                    class="ocean-more__item"
                                    type="button"
                                    role="menuitem"
                                    on:click=move |_| {
                                        if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                        show_phone_dialer.set(false);
                                        show_livekit_controls.set(true);
                                    }
                                >
                                    "Join room call"
                                </button>
                            </Show>
                            <button
                                class="ocean-more__item"
                                type="button"
                                role="menuitem"
                                on:click=move |_| {
                                    if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                    show_livekit_controls.set(false);
                                    show_phone_dialer.set(true);
                                }
                            >
                                "Dial phone"
                            </button>
                            <button
                                class="ocean-more__item"
                                type="button"
                                role="menuitem"
                                on:click=move |_| {
                                    if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                    show_council.set(true);
                                }
                            >
                                "Council deck"
                            </button>
                            <button
                                class="ocean-more__item"
                                type="button"
                                role="menuitem"
                                on:click=move |_| {
                                    if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                    show_rooms.update(|v| *v = !*v);
                                }
                            >
                                "Rooms"
                            </button>
                            <Show when=move || voice_ready.get()>
                                <button
                                    class="ocean-more__item"
                                    type="button"
                                    role="menuitem"
                                    on:click=move |_| {
                                        if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                        muted.update(|m| *m = !*m);
                                    }
                                >
                                    {move || if muted.get() { "Unmute voice" } else { "Mute voice" }}
                                </button>
                            </Show>
                            <Show when=crate::daemon::running_as_extension>
                                <button
                                    class="ocean-more__item"
                                    type="button"
                                    role="menuitem"
                                    on:click=move |_| {
                                        if let Some(d) = more_ref.get() { let _ = d.remove_attribute("open"); }
                                        daemon_capture.get_value().capture_and_attach_visible_tab();
                                    }
                                >
                                    "Capture tab"
                                </button>
                            </Show>
                        </div>
                    </details>
                </div>
            </header>

            // Chat surface. (The Leptos component "gauntlet" toggle was removed
            // in OCEAN-202 — it was a dev-only component harness, not shipping UI.)
            // Call/collaboration utility line. Hidden while idle so the app has
            // one top chrome bar, not a second row of quiet-but-visible buttons.
            <Show when=move || show_phone_dialer.get()>
                <div class="ocean-utility-row">
                    // Place-call control (OCEAN-261): revealed from overflow.
                    // On PSTN success, CallPanel below takes over from the
                    // daemon's call_started event.
                    <crate::place_call::PlaceCallControl
                        daemon=daemon_phone_call.get_value()
                        open=show_phone_dialer
                    />
                </div>
            </Show>

            // LiveKit collaboration presence (OCEAN-83): a single mount that
            // either renders as a compact utility panel or expands into the
            // full room stage once a room join routes the shared LiveKit
            // signals. Never double-mount the singleton bridge.
            <crate::livekit::LiveKitPanel
                daemon=daemon_livekit.get_value()
                open=show_livekit_controls
                stage=in_room_mode
            />

            <Show when=move || !in_room_mode.get()>

                        // Live call-mode view (OCEAN-CALL). Self-contained: it
                        // subscribes to the daemon's `/v1/events` control stream
                        // for the `call_*` frames and stays hidden until a
                        // `call_started` arrives, then shows the live transcript,
                        // rolling summary, detected action items, and wake orb;
                        // it collapses again on `call_ended`. Purely additive.
                        <crate::call::CallPanel daemon=daemon.clone() />

                        <Transcript daemon=daemon.clone() show_sessions=show_sessions />

                        // Agent canvas (OCEAN-178 → OCEAN-248). Folds the
                        // daemon's `surface_patch` stream into a client-side
                        // ledger and renders it spatially — positioned cards +
                        // SVG edges — instead of a text changelog. The GPUI
                        // native shell renders the full interactive canvas.
                        <crate::canvas::CanvasRender canvas_patches=canvas_patches />

                        // Blocking permission prompts sit just above the composer
                        // so a gated mutating turn can't be missed or scrolled past.
                        <PermissionPrompts daemon=daemon_for_perms.get_value() />

                        <form class="ocean-composer ocean-lit" on:submit=move |ev| submit.with_value(|s| s(ev))>
                            // Push-to-talk only when the proxy has a usable xAI key;
                            // otherwise a dim, disabled placeholder explains why.
                            <Show
                                when=move || voice_ready.get()
                                fallback=|| view! {
                                    <div class="voice-wrap">
                                        <button class="voice-orb is-disabled" type="button" disabled=true
                                                title="voice off — set xAI key in ~/.config/ocean-surface/xai.key">
                                            <span class="voice-orb__glyph"><crate::icons::Amplitude /></span>
                                        </button>
                                        <span class="voice-hint">"voice off"</span>
                                    </div>
                                }
                            >
                                <VoiceOrb on_transcript=on_transcript on_status=on_voice_status />
                            </Show>
                            // Per-turn overrides (OCEAN-79): reasoning effort +
                            // model. Compact pills next to the composer. Both
                            // default to "daemon default" so an untouched control
                            // sends no override and preserves prior behavior.
                            <div class="ocean-turn-controls">
                                <select
                                    class="ocean-thinking"
                                    aria-label="reasoning effort"
                                    title="Reasoning effort (this turn onward)"
                                    prop:value=move || thinking_level.get().unwrap_or_default()
                                    on:change=move |ev| {
                                        let v = event_target_value(&ev);
                                        daemon_thinking.with_value(|d| {
                                            d.set_thinking_level((!v.is_empty()).then_some(v))
                                        });
                                    }
                                >
                                    // Values map 1:1 to ocean_protocol::ThinkingLevel
                                    // (serde lowercase): off | minimal | low | medium
                                    // | high | xhigh. Empty = no override (daemon
                                    // default). These are the exact levels the daemon
                                    // accepts — anything else round-trips to a serde
                                    // error. (OCEAN-202)
                                    <option value="" prop:selected=move || thinking_level.get().is_none()>
                                        "think: default"
                                    </option>
                                    <option value="off" prop:selected=move || thinking_level.get().as_deref() == Some("off")>
                                        "think: off"
                                    </option>
                                    <option value="minimal" prop:selected=move || thinking_level.get().as_deref() == Some("minimal")>
                                        "think: minimal"
                                    </option>
                                    <option value="low" prop:selected=move || thinking_level.get().as_deref() == Some("low")>
                                        "think: low"
                                    </option>
                                    <option value="medium" prop:selected=move || thinking_level.get().as_deref() == Some("medium")>
                                        "think: medium"
                                    </option>
                                    <option value="high" prop:selected=move || thinking_level.get().as_deref() == Some("high")>
                                        "think: high"
                                    </option>
                                    <option value="xhigh" prop:selected=move || thinking_level.get().as_deref() == Some("xhigh")>
                                        "think: xhigh"
                                    </option>
                                    // Unknown persisted value (stale pref, daemon
                                    // drift): still render it selected — the same
                                    // guard the model select has. Without this the
                                    // controlled select desyncs and renders BLANK.
                                    <Show when=move || {
                                        matches!(
                                            thinking_level.get().as_deref(),
                                            Some(v) if !matches!(v, "off" | "minimal" | "low" | "medium" | "high" | "xhigh")
                                        )
                                    }>
                                        <option prop:value=move || thinking_level.get().unwrap_or_default() prop:selected=true>
                                            {move || format!("think: {}", thinking_level.get().unwrap_or_default())}
                                        </option>
                                    </Show>
                                </select>
                                // Per-turn model override (distinct from the
                                // header picker's global swap). Drawn from the
                                // same /v1/models catalogue.
                                <select
                                    class="ocean-model-override"
                                    aria-label="model override"
                                    title="Model for this turn (overrides daemon default)"
                                    prop:value=move || model_override.get().unwrap_or_default()
                                    on:change=move |ev| {
                                        let id = event_target_value(&ev);
                                        daemon_model_override.with_value(|d| {
                                            d.set_model_override((!id.is_empty()).then_some(id))
                                        });
                                    }
                                >
                                    <option prop:value="" prop:selected=move || model_override.get().is_none()>
                                        "model: default"
                                    </option>
                                    // If a persisted override isn't in the
                                    // catalogue yet, still show it selected.
                                    <Show when=move || {
                                        let cur = model_override.get();
                                        cur.is_some()
                                            && !models.get().iter().any(|m| Some(&m.id) == cur.as_ref())
                                    }>
                                        <option prop:value=move || model_override.get().unwrap_or_default() prop:selected=true>
                                            {move || model_override.get().unwrap_or_default()}
                                        </option>
                                    </Show>
                                    <For
                                        each=move || models.get()
                                        key=|m| m.id.clone()
                                        children=move |m| {
                                            let id = m.id.clone();
                                            let id_sel = m.id.clone();
                                            let label = if m.label.is_empty() { m.id.clone() } else { m.label.clone() };
                                            view! {
                                                <option
                                                    prop:value=id.clone()
                                                    prop:selected=move || model_override.get().as_deref() == Some(id_sel.as_str())
                                                >
                                                    {label}
                                                </option>
                                            }
                                        }
                                    />
                                </select>
                            </div>
                            <textarea
                                class="ocean-composer__input"
                                placeholder="message Ocean…"
                                node_ref=textarea_ref
                                prop:value=move || input.get()
                                on:input=move |ev| {
                                    input.set(event_target_value(&ev));
                                    if let Some(target) = ev.target() {
                                        if let Ok(el) = target.dyn_into::<web_sys::HtmlTextAreaElement>() {
                                            fit_composer_textarea(&el);
                                        }
                                    }
                                }
                                on:keydown=move |ev| {
                                    // Enter to submit, Shift+Enter for newline.
                                    if ev.key() == "Enter" && !ev.shift_key() {
                                        ev.prevent_default();
                                        if let Some(target) = ev.target() {
                                            if let Ok(el) = target.dyn_into::<web_sys::HtmlElement>() {
                                                if let Ok(Some(form)) = el.closest("form") {
                                                    if let Ok(form) = form.dyn_into::<web_sys::HtmlFormElement>()
                                                    {
                                                        let _ = form.request_submit();
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            />
                            // Halt the in-flight turn. Only shown while streaming.
                            <Show when=move || streaming.get()>
                                <button
                                    class="ocean-composer__halt"
                                    type="button"
                                    aria-label="stop"
                                    title="Stop the running turn"
                                    on:click=move |_| daemon_halt.with_value(|d| d.halt())
                                >
                                    "■ Stop"
                                </button>
                            </Show>
                            <button
                                class="ocean-composer__send"
                                type="submit"
                                disabled=move || input.get().trim().is_empty()
                            >
                                "Send"
                            </button>
                        </form>

            </Show>

            <SessionsPanel daemon=daemon_for_panel open=show_sessions />

            // Persistent Rooms panel (OCEAN-108). Lightweight browse/create
            // overlay only — a successful join closes this panel and promotes
            // the main surface into room mode.
            <RoomsPanel rooms=rooms open=show_rooms />

            // Council/quorum observability deck (OCEAN-96). Native workflow
            // stage now lives inside the surface instead of an iframe.
            <Show when=move || show_council.get()>
                <div class="ocean-council-modal" role="dialog" aria-label="Council stage">
                    <div class="ocean-council-modal__bar">
                        <span class="ocean-council-modal__title">"Council — workflow stage"</span>
                        <button
                            class="ocean-council-modal__close"
                            type="button"
                            aria-label="close council stage"
                            title="Close"
                            on:click=move |_| show_council.set(false)
                        >
                            "✕"
                        </button>
                    </div>
                    <crate::council::CouncilStage daemon=daemon_council.clone() />
                </div>
            </Show>
        </main>
    }
}

/// Pull the most recent assistant turn's concatenated text blocks, paired
/// with its turn id (used to dedupe TTS). Skips thinking + tool output.
fn latest_assistant_text(turns: &[Turn]) -> Option<(String, String)> {
    let turn = turns.iter().rev().find(|t| t.role == Role::Assistant)?;
    let id = turn.turn_id.clone()?;
    let mut text = String::new();
    for block in &turn.blocks {
        if let Block::Text(buf) = block {
            text.push_str(buf);
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some((id, text))
    }
}

/// Humanize a token count for the header chip: 942 → "942", 12_345 → "12.3k",
/// 1_580_000 → "1.6M". Keeps the readout compact.
fn fmt_tokens(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{composer_height_px, composer_overflow_y, COMPOSER_MAX_HEIGHT_PX, COMPOSER_MIN_HEIGHT_PX};

    #[test]
    fn composer_height_clamps_to_min_and_max() {
        assert_eq!(composer_height_px(0), COMPOSER_MIN_HEIGHT_PX);
        assert_eq!(composer_height_px(72), 72);
        assert_eq!(composer_height_px(999), COMPOSER_MAX_HEIGHT_PX);
    }

    #[test]
    fn composer_overflow_switches_only_past_max_height() {
        assert_eq!(composer_overflow_y(COMPOSER_MAX_HEIGHT_PX), "hidden");
        assert_eq!(composer_overflow_y(COMPOSER_MAX_HEIGHT_PX + 1), "auto");
    }
}
