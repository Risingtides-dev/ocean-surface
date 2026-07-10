//! Realtime voice chat (voice phases 2/3).
//!
//! Speech-to-speech over OpenAI's Realtime API: the daemon mints an ephemeral
//! client secret (`POST /v1/voice/realtime/client-secret`, same-origin via the
//! proxy — the API key never reaches this bundle), then the browser talks
//! WebRTC directly to OpenAI: mic track up, agent audio track down, and a
//! `oai-events` data channel for tool calls.
//!
//! Two tools round-trip through that channel:
//! - `render_component` — folded into the transcript locally via
//!   [`crate::daemon::Daemon::render_local_component`], the same upsert the
//!   SSE `ComponentRender` reducer performs.
//! - `write_handoff` — appended to the chat session through the daemon
//!   (`POST /v1/agent/sessions/{id}/messages`), so the text agent's next turn
//!   picks the note up.
//!
//! Layout contract (consumed by app.rs): [`stage`] drives the voice-chat
//! center-stage classes; [`level`] is a 0..1 mic level the orb's water binds
//! to. Both are reference-counted process-wide signals, so conditional mounts
//! and asynchronous WebRTC callbacks cannot outlive their reactive owner.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AnalyserNode, AudioContext, HtmlAudioElement, MediaStream, MediaStreamConstraints,
    MessageEvent, RtcDataChannel, RtcIceGatheringState, RtcPeerConnection, RtcSdpType,
    RtcSessionDescriptionInit, RtcTrackEvent,
};

use super::vad;
use crate::daemon::Daemon;
use crate::tts;

/// Where the browser posts its SDP offer, authorized by the ephemeral secret.
const CALLS_URL: &str = "https://api.openai.com/v1/realtime/calls";

/// What the realtime voice session is doing right now. Drives the voice-chat
/// layout classes and the menu entry label.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RealtimeStage {
    #[default]
    Off,
    Connecting,
    Live,
}

/// One entry in the voice-mode menu's realtime section. The menu reads this
/// to render the right label, show any error, and decide retry vs stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeMenuEntry {
    pub label: &'static str,
    pub visible_error: Option<String>,
    pub retryable: bool,
    pub active: bool,
}

// Process-wide voice status must not inherit the reactive owner active on first
// access: conditional VoiceOrb mounts and WebRTC/rAF callbacks outlive owners.
thread_local! {
    static STAGE: RefCell<Option<ArcRwSignal<RealtimeStage>>> = const { RefCell::new(None) };
    static LEVEL: RefCell<Option<ArcRwSignal<f32>>> = const { RefCell::new(None) };
    /// The daemon handle, installed once from app.rs so the orb's menu entry
    /// can start a session without threading props through VoiceOrb.
    static DAEMON: RefCell<Option<Daemon>> = const { RefCell::new(None) };
    /// Everything the live session must keep alive (tracks, closures, nodes).
    static SESSION: RefCell<Option<RealtimeSession>> = const { RefCell::new(None) };
    /// The last realtime connect failure message. Cleared on fresh start
    /// and clean stop; preserved on failure so the menu reopens to it.
    static LAST_ERROR: RefCell<Option<ArcRwSignal<Option<String>>>> = const { RefCell::new(None) };
}

/// The voice-chat stage signal (created on first access).
pub fn stage() -> ArcRwSignal<RealtimeStage> {
    STAGE.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| ArcRwSignal::new(RealtimeStage::Off))
            .clone()
    })
}

/// Live mic level 0..1 for the orb's audio-reactive water.
pub fn level() -> ArcRwSignal<f32> {
    LEVEL.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| ArcRwSignal::new(0.0))
            .clone()
    })
}

/// The last realtime connect failure, if any. Lazily created.
pub fn last_error() -> ArcRwSignal<Option<String>> {
    LAST_ERROR.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| ArcRwSignal::new(None))
            .clone()
    })
}

/// The voice-menu entry label and error state for the realtime row.
/// Off with an error → "Retry voice chat" with the error visible.
/// Live → "End voice chat", error cleared.
pub fn voice_menu_realtime_entry(
    stage: RealtimeStage,
    last_error: Option<&str>,
) -> RealtimeMenuEntry {
    match stage {
        RealtimeStage::Off => {
            if let Some(err) = last_error {
                RealtimeMenuEntry {
                    label: "Retry voice chat",
                    visible_error: Some(err.to_string()),
                    retryable: true,
                    active: false,
                }
            } else {
                RealtimeMenuEntry {
                    label: "Voice chat",
                    visible_error: None,
                    retryable: false,
                    active: false,
                }
            }
        }
        RealtimeStage::Connecting => RealtimeMenuEntry {
            label: "Connecting voice chat…",
            visible_error: None,
            retryable: false,
            active: true,
        },
        RealtimeStage::Live => RealtimeMenuEntry {
            label: "End voice chat",
            visible_error: None,
            retryable: false,
            active: true,
        },
    }
}

/// Map a raw realtime error to a user-facing message, hiding credential
/// leak details. Errors containing "no OpenAI credential configured" get
/// the exact text "Voice chat needs an OpenAI API key."; all other messages
/// are kept concise and unchanged.
pub fn user_facing_realtime_error(raw: &str) -> String {
    if raw.contains("no OpenAI credential configured") {
        "Voice chat needs an OpenAI API key.".to_string()
    } else {
        raw.to_string()
    }
}

/// Install the daemon handle once from the application shell. Process-wide
/// status signals are reference-counted and therefore safe to create lazily.
pub fn install(daemon: Daemon) {
    DAEMON.with(|d| *d.borrow_mut() = Some(daemon));

}

fn daemon() -> Option<Daemon> {
    DAEMON.with(|d| d.borrow().clone())
}

/// Shared, optional handle to the self-rescheduling level-meter frame closure.
type FrameCell = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

/// Latest animation frame owned by the level meter. Each frame replaces the
/// completed handle with its successor so teardown cancels the frame that is
/// actually still queued before dropping the callback closure.
#[derive(Clone)]
struct LevelMeterRafHandle(Rc<Cell<Option<i32>>>);

impl LevelMeterRafHandle {
    fn empty() -> Self {
        Self(Rc::new(Cell::new(None)))
    }

    #[cfg(test)]
    fn new(handle: i32) -> Self {
        let slot = Self::empty();
        slot.replace_after_frame(handle);
        slot
    }

    fn replace_after_frame(&self, handle: i32) {
        self.0.set(Some(handle));
    }

    fn take_for_teardown(&self) -> Option<i32> {
        self.0.take()
    }
}

/// Live session state. Dropping this (via [`stop`]) releases the mic, closes
/// the peer connection, and silences the remote audio element.
struct RealtimeSession {
    pc: RtcPeerConnection,
    channel: RtcDataChannel,
    mic: MediaStream,
    audio_el: HtmlAudioElement,
    ctx: Option<AudioContext>,
    raf_handle: LevelMeterRafHandle,
    frame_cell: Option<FrameCell>,
    running: Rc<RefCell<bool>>,
    /// Event/track/message closures that must outlive their registration.
    _closures: Vec<Box<dyn std::any::Any>>,
}

/// Start a realtime voice chat. No-op while one is already connecting/live.
/// Failures reset the stage to `Off` and surface through the shared voice
/// status line. The active chat session (when any) scopes the daemon
/// briefing and the handoff target.
pub fn start() {
    if stage().get_untracked() != RealtimeStage::Off {
        return;
    }
    // Realtime owns the audio path: stop any classic spoken reply before
    // opening the duplex WebRTC session so the two outputs cannot overlap.
    if tts::is_playing() {
        tts::stop();
    }
    let Some(daemon) = daemon() else {
        super::report_status("voice chat unavailable — daemon handle not installed".into());
        return;
    };
    let session_id = daemon.session_id.get_untracked();
    // Clear any stale failure from the last session.
    last_error().set(None);
    stage().set(RealtimeStage::Connecting);
    spawn_local(async move {
        match connect(daemon, session_id).await {
            Ok(session) => {
                SESSION.with(|s| *s.borrow_mut() = Some(session));
                // Live is flipped by the data channel's `onopen`; if the
                // channel opened before we stored the session (fast network),
                // it already set Live and this is a no-op.
            }
            Err(msg) => {
                stage().set(RealtimeStage::Off);
                level().set(0.0);
                let humanized = user_facing_realtime_error(&msg);
                last_error().set(Some(humanized));
                super::report_status(format!("voice chat failed: {msg}"));
            }
        }
    });
}

/// End the voice chat: release the mic, close the channel + peer connection,
/// drop the remote audio, stop the level meter, stage → Off.
pub fn stop() {
    let session = SESSION.with(|s| s.borrow_mut().take());
    if let Some(session) = session {
        *session.running.borrow_mut() = false;
        if let (Some(handle), Some(win)) =
            (session.raf_handle.take_for_teardown(), web_sys::window())
        {
            let _ = win.cancel_animation_frame(handle);
        }
        if let Some(cell) = &session.frame_cell {
            cell.borrow_mut().take();
        }
        if let Some(ctx) = &session.ctx {
            let _ = ctx.close();
        }
        // Closing the transports queues `close`/track events. Detach their
        // wasm-bindgen callbacks before dropping `_closures`, or the browser
        // can invoke a freed closure on the next event-loop turn.
        session.channel.set_onopen(None);
        session.channel.set_onmessage(None);
        session.channel.set_onclose(None);
        session.pc.set_ontrack(None);
        session.channel.close();
        session.pc.close();
        for track in session.mic.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
        session.audio_el.set_src_object(None);
    }
    stage().set(RealtimeStage::Off);
    level().set(0.0);
    last_error().set(None);
}

/// Full connect flow: secret → mic → peer connection → SDP round-trip.
async fn connect(daemon: Daemon, session_id: Option<String>) -> Result<RealtimeSession, String> {
    // 1. Ephemeral secret from the daemon (same-origin through the proxy).
    let secret = daemon.realtime_client_secret(session_id.clone()).await?;

    // 2. Mic with echo cancellation — same constraints as the listen loop, so
    //    the agent's own voice doesn't feed back into the conversation.
    let window = web_sys::window().ok_or("no window")?;
    let media = window
        .navigator()
        .media_devices()
        .map_err(|_| "this browser cannot record audio".to_string())?;
    let constraints = MediaStreamConstraints::new();
    let audio_opts = js_sys::Object::new();
    let set = |k: &str, v: bool| {
        let _ = js_sys::Reflect::set(&audio_opts, &JsValue::from_str(k), &JsValue::from_bool(v));
    };
    set("echoCancellation", true);
    set("noiseSuppression", true);
    set("autoGainControl", true);
    constraints.set_audio(&audio_opts);
    let mic: MediaStream = JsFuture::from(
        media
            .get_user_media_with_constraints(&constraints)
            .map_err(|_| "microphone unavailable".to_string())?,
    )
    .await
    .map_err(|_| "microphone permission denied".to_string())?
    .dyn_into()
    .map_err(|_| "no media stream".to_string())?;

    // 3. Peer connection: mic track up, remote audio into a detached <audio>.
    let pc = RtcPeerConnection::new().map_err(|_| "cannot create peer connection".to_string())?;
    for track in mic.get_tracks().iter() {
        if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
            let _ = pc.add_track(&track, &mic, &js_sys::Array::new());
        }
    }
    let document = window.document().ok_or("no document")?;
    let audio_el: HtmlAudioElement = document
        .create_element("audio")
        .map_err(|_| "cannot create audio element".to_string())?
        .unchecked_into();
    audio_el.set_autoplay(true);

    let mut closures: Vec<Box<dyn std::any::Any>> = Vec::new();

    let audio_for_track = audio_el.clone();
    let ontrack = Closure::wrap(Box::new(move |ev: RtcTrackEvent| {
        let streams = ev.streams();
        if streams.length() > 0 {
            let stream: MediaStream = streams.get(0).unchecked_into();
            audio_for_track.set_src_object(Some(&stream));
            let _ = audio_for_track.play();
        }
    }) as Box<dyn FnMut(RtcTrackEvent)>);
    pc.set_ontrack(Some(ontrack.as_ref().unchecked_ref()));
    closures.push(Box::new(ontrack));

    // 4. Data channel for Realtime events (tool calls ride here).
    let channel = pc.create_data_channel("oai-events");
    let onopen = Closure::wrap(Box::new(move || {
        stage().set(RealtimeStage::Live);
    }) as Box<dyn FnMut()>);
    channel.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    closures.push(Box::new(onopen));

    let daemon_for_events = daemon.clone();
    let session_for_events = session_id.clone();
    let onmessage = Closure::wrap(Box::new(move |ev: MessageEvent| {
        if let Some(text) = ev.data().as_string() {
            handle_server_event(&daemon_for_events, session_for_events.as_deref(), &text);
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    channel.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    closures.push(Box::new(onmessage));

    // Channel/transport failure must not strand the UI in a phantom Live.
    let onclose = Closure::wrap(Box::new(move || {
        if stage().get_untracked() == RealtimeStage::Live {
            super::report_status("voice chat ended".into());
            stop();
        }
    }) as Box<dyn FnMut()>);
    channel.set_onclose(Some(onclose.as_ref().unchecked_ref()));
    closures.push(Box::new(onclose));

    // 5. SDP offer → OpenAI, answer back. Wait briefly for ICE gathering so
    //    the offer carries candidates (the calls endpoint is non-trickle).
    let offer = JsFuture::from(pc.create_offer())
        .await
        .map_err(|_| "createOffer failed".to_string())?;
    let offer_init: RtcSessionDescriptionInit = offer.unchecked_into();
    JsFuture::from(pc.set_local_description(&offer_init))
        .await
        .map_err(|_| "setLocalDescription failed".to_string())?;
    for _ in 0..20 {
        if pc.ice_gathering_state() == RtcIceGatheringState::Complete {
            break;
        }
        sleep_ms(100).await;
    }
    let sdp = pc
        .local_description()
        .ok_or("no local description".to_string())?
        .sdp();

    let call_url = format!("{CALLS_URL}?model={}", urlencode(&secret.model));
    let resp = gloo_net::http::Request::post(&call_url)
        .header("authorization", &format!("Bearer {}", secret.client_secret))
        .header("content-type", "application/sdp")
        .body(sdp)
        .map_err(|e| format!("offer encode failed: {e}"))?
        .send()
        .await
        .map_err(|e| format!("offer POST failed: {e}"))?;
    if !resp.ok() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "realtime call rejected ({}): {text}",
            resp.status()
        ));
    }
    let answer_sdp = resp
        .text()
        .await
        .map_err(|e| format!("answer unreadable: {e}"))?;
    let answer_init = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
    answer_init.set_sdp(&answer_sdp);
    JsFuture::from(pc.set_remote_description(&answer_init))
        .await
        .map_err(|_| "setRemoteDescription failed".to_string())?;

    // 6. Mic level meter for the orb's water (same analyser+rAF pattern as
    //    the hands-free listen loop; ~60Hz, torn down by `stop`).
    let (ctx, raf_handle, frame_cell, running) = start_level_meter(&mic)?;

    Ok(RealtimeSession {
        pc,
        channel,
        mic,
        audio_el,
        ctx: Some(ctx),
        raf_handle,
        frame_cell: Some(frame_cell),
        running,
        _closures: closures,
    })
}

/// Teardown bundle [`start_level_meter`] hands back: audio context, rAF id,
/// the frame cell, and the running flag `stop` flips.
type LevelMeterHandles = (
    AudioContext,
    LevelMeterRafHandle,
    FrameCell,
    Rc<RefCell<bool>>,
);

/// Analyser + self-rescheduling rAF loop writing [`level`]. Returns the
/// handles [`stop`] needs for teardown.
fn start_level_meter(mic: &MediaStream) -> Result<LevelMeterHandles, String> {
    let window = web_sys::window().ok_or("no window")?;
    let ctx = AudioContext::new().map_err(|_| "no audio context".to_string())?;
    let source = ctx
        .create_media_stream_source(mic)
        .map_err(|_| "cannot tap mic".to_string())?;
    let analyser: AnalyserNode = ctx
        .create_analyser()
        .map_err(|_| "cannot create analyser".to_string())?;
    analyser.set_fft_size(1024);
    source
        .connect_with_audio_node(&analyser)
        .map_err(|_| "cannot connect analyser".to_string())?;

    let running = Rc::new(RefCell::new(true));
    let running2 = running.clone();
    let buf_len = analyser.fft_size() as usize;
    let frame_cell: FrameCell = Rc::new(RefCell::new(None));
    let frame_cell2 = frame_cell.clone();
    let window2 = window.clone();
    let raf_handle = LevelMeterRafHandle::empty();
    let raf_handle2 = raf_handle.clone();

    let on_frame = Closure::wrap(Box::new(move || {
        if !*running2.borrow() {
            return;
        }
        let mut samples = vec![0.0f32; buf_len];
        analyser.get_float_time_domain_data(&mut samples);
        // RMS of speech peaks around ~0.25 with AGC; scale into 0..1.
        let v = (vad::rms(&samples) * 4.0).clamp(0.0, 1.0);
        level().set(v);
        if let Some(cb) = frame_cell2.borrow().as_ref() {
            if let Ok(handle) = window2.request_animation_frame(cb.as_ref().unchecked_ref()) {
                raf_handle2.replace_after_frame(handle);
            }
        }
    }) as Box<dyn FnMut()>);
    let handle = window
        .request_animation_frame(on_frame.as_ref().unchecked_ref())
        .map_err(|_| "cannot schedule level meter".to_string())?;
    raf_handle.replace_after_frame(handle);
    *frame_cell.borrow_mut() = Some(on_frame);
    Ok((ctx, raf_handle, frame_cell, running))
}

/// One server event off the data channel. Tool calls are dispatched from the
/// completed response (`response.done` carries every `function_call` output
/// item with its full `arguments` string — no delta assembly needed).
fn handle_server_event(daemon: &Daemon, session_id: Option<&str>, text: &str) {
    let Ok(event) = serde_json::from_str::<Value>(text) else {
        return;
    };
    match event.pointer("/type").and_then(Value::as_str) {
        Some("response.done") => {
            let items = event
                .pointer("/response/output")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for item in &items {
                if item.pointer("/type").and_then(Value::as_str) != Some("function_call") {
                    continue;
                }
                let name = item.pointer("/name").and_then(Value::as_str).unwrap_or("");
                let call_id = item
                    .pointer("/call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let args: Value = item
                    .pointer("/arguments")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str(raw).ok())
                    .unwrap_or(Value::Null);
                dispatch_tool(daemon, session_id, name, call_id, args);
            }
        }
        Some("error") => {
            let msg = event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("realtime error");
            super::report_status(format!("voice chat: {msg}"));
        }
        _ => {}
    }
}

/// Run one voice-agent tool call and answer it on the data channel so the
/// agent can keep talking about what it did.
fn dispatch_tool(
    daemon: &Daemon,
    session_id: Option<&str>,
    name: &str,
    call_id: String,
    args: Value,
) {
    match name {
        "render_component" => {
            let (component_id, kind, props) = parse_render_args(&args);
            daemon.render_local_component(component_id, kind, props);
            send_tool_output(&call_id, "rendered");
        }
        "write_handoff" => {
            let note = args
                .pointer("/note")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let Some(sid) = session_id.map(str::to_string) else {
                send_tool_output(&call_id, "no chat session to hand off to");
                return;
            };
            if note.is_empty() {
                send_tool_output(&call_id, "empty note ignored");
                return;
            }
            let daemon = daemon.clone();
            spawn_local(async move {
                let out = match daemon
                    .append_session_message(&sid, &note, Some("handoff"))
                    .await
                {
                    Ok(()) => "noted",
                    Err(err) => {
                        super::report_status(format!("handoff failed: {err}"));
                        "handoff failed"
                    }
                };
                send_tool_output(&call_id, out);
            });
        }
        _ => send_tool_output(&call_id, "unknown tool"),
    }
}

/// Pull `{component_id?, kind, props}` out of the tool args, tolerating a
/// nested `component` wrapper and filling honest defaults. A generated id
/// keeps repeat renders distinct; the surface's component renderer handles
/// unknown kinds with its existing fallback card.
fn parse_render_args(args: &Value) -> (String, String, Value) {
    let inner = args.pointer("/component").unwrap_or(args);
    let kind = inner
        .pointer("/kind")
        .and_then(Value::as_str)
        .unwrap_or("markdown")
        .to_string();
    let props = inner
        .pointer("/props")
        .cloned()
        .unwrap_or_else(|| inner.clone());
    let component_id = inner
        .pointer("/component_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(fresh_component_id);
    (component_id, kind, props)
}

/// Monotonic per-process component id — unique within a session, which is
/// all the transcript upsert needs. A plain atomic works on wasm (single
/// thread) and native (tests) alike; `js_sys::Date` would panic off-wasm.
fn fresh_component_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("voice-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Answer a tool call: `function_call_output` + a fresh `response.create` so
/// the agent voices the result.
fn send_tool_output(call_id: &str, output: &str) {
    send_event(&json!({
        "type": "conversation.item.create",
        "item": { "type": "function_call_output", "call_id": call_id, "output": output }
    }));
    send_event(&json!({ "type": "response.create" }));
}

fn send_event(event: &Value) {
    SESSION.with(|s| {
        if let Some(session) = s.borrow().as_ref() {
            let _ = session.channel.send_with_str(&event.to_string());
        }
    });
}

/// Promise-backed setTimeout — the ICE-gathering poll's only clock.
async fn sleep_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(win) = web_sys::window() {
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// Minimal query-value escape for the model id (alphanumerics, `.`, `-`, `_`
/// pass through — exactly the shape of Realtime model ids).
fn urlencode(raw: &str) -> String {
    raw.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                vec![c]
            } else {
                format!("%{:02X}", c as u32).chars().collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;


    fn assert_arc_rw_signal<T>(_signal: ArcRwSignal<T>) {}

    #[test]
    fn level_meter_teardown_takes_latest_rescheduled_animation_frame_handle_once() {
        let handle = LevelMeterRafHandle::new(4);
        let frame_owner = handle.clone();

        frame_owner.replace_after_frame(9);

        assert_eq!(handle.take_for_teardown(), Some(9));
        assert_eq!(handle.take_for_teardown(), None);
    }

    #[test]
    fn process_wide_realtime_status_signals_are_owner_independent_arc_rw_signals() {
        assert_arc_rw_signal::<RealtimeStage>(stage());
        assert_arc_rw_signal::<f32>(level());
        assert_arc_rw_signal::<Option<String>>(last_error());
    }

    #[test]
    fn render_args_accept_flat_and_nested_shapes() {
        let flat = json!({ "component_id": "a", "kind": "table", "props": { "rows": [] } });
        let (id, kind, props) = parse_render_args(&flat);
        assert_eq!((id.as_str(), kind.as_str()), ("a", "table"));
        assert_eq!(props, json!({ "rows": [] }));

        let nested = json!({ "component": { "kind": "kanban", "props": { "cols": 3 } } });
        let (id, kind, props) = parse_render_args(&nested);
        assert!(id.starts_with("voice-"));
        assert_eq!(kind, "kanban");
        assert_eq!(props, json!({ "cols": 3 }));
    }

    #[test]
    fn render_args_degrade_to_markdown_with_whole_args_as_props() {
        let loose = json!({ "text": "hello" });
        let (_, kind, props) = parse_render_args(&loose);
        assert_eq!(kind, "markdown");
        assert_eq!(props, loose);
    }

    #[test]
    fn failed_start_error_stays_visible_and_retryable_in_menu_entry() {
        // Wished-for production seam: realtime startup failures should be stored
        // and folded into the voice menu's realtime entry instead of disappearing
        // into the transient shared status line.
        let failed = super::voice_menu_realtime_entry(
            RealtimeStage::Off,
            Some("microphone permission denied"),
        );
        assert_eq!(failed.label, "Retry voice chat");
        assert_eq!(
            failed.visible_error.as_deref(),
            Some("microphone permission denied")
        );
        assert!(failed.retryable);
        assert!(!failed.active);

        let live = super::voice_menu_realtime_entry(RealtimeStage::Live, Some("stale failure"));
        assert_eq!(live.label, "End voice chat");
        assert_eq!(live.visible_error, None);
        assert!(!live.retryable);
        assert!(live.active);
    }

    #[test]
    fn user_facing_realtime_error_hides_secret_mint_transport_json() {
        let raw_secret_mint_error = r#"secret mint failed: {"error":"no OpenAI credential configured (OCEAN_OPENAI_API_KEY / OPENAI_API_KEY / auth.json)"}"#;

        assert_eq!(
            super::user_facing_realtime_error(raw_secret_mint_error),
            "Voice chat needs an OpenAI API key."
        );
    }

    #[test]
    fn urlencode_passes_model_ids_untouched() {
        assert_eq!(urlencode("gpt-realtime-2"), "gpt-realtime-2");
        assert_eq!(urlencode("a b"), "a%20b");
    }
}
