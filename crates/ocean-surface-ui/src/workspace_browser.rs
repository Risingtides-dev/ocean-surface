//! Workspace Browser pane — live-streams the agent daemon's Chrome over the
//! `GET /v1/browser/screencast` SSE endpoint (web_sys::EventSource) and forwards
//! pointer/keyboard input back through `POST /v1/browser/input`.
//!
//! Tauri-only affordance: the WorkspacePane mounts this inside its Browser tab,
//! and the pane itself is gated on `host::running_in_tauri()`. The component is
//! host-agnostic (the daemon URL is absolute, same as every other daemon fetch)
//! — the desktop is simply the only host where a permanent browser pane fits.
//!
//! ## Screencast contract (daemon side)
//! The SSE stream emits named events:
//!   • `frame`  — data `{b64, w, h, ts}`: one JPEG frame; we paint the latest
//!     into `<img class="workspace-browser__frame">`.
//!   • `status` — data `{state:"no-browser"}`: no live agent browser yet; we
//!     show an empty state and KEEP the connection open (the daemon polls its
//!     browser registry and starts emitting frames once one appears).
//!
//! ## Input contract (daemon side)
//! `POST /v1/browser/input` takes `{kind, x?, y?, delta_y?, key?, text?}` and
//! dispatches into the agent's Chrome over CDP. Coordinates are in FRAME pixel
//! space; we map from the overlay's rendered rect (which sits exactly over the
//! image) using the last reported `{w, h}` — see [`map_point_to_frame`], a pure
//! fn with unit tests covering offset, non-square aspect, and degenerate sizes.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use serde_json::{json, Value};
use wasm_bindgen::{closure::Closure, JsCast};
use wasm_bindgen_futures::spawn_local;
use web_sys::{EventSource, MessageEvent};

use crate::daemon::Daemon;

/// `GET` SSE endpoint — the daemon's live screencast of the agent's Chrome.
const SCREENCAST_PATH: &str = "/v1/browser/screencast";
/// `POST` endpoint — forwards a single pointer/keyboard event into the agent's
/// Chrome via CDP.
const INPUT_PATH: &str = "/v1/browser/input";

/// Initial reconnect delay and the cap it doubles up to (ms). The native
/// EventSource auto-retries, but the screencast endpoint stays open across
/// no-browser periods, so on a real drop we reconnect ourselves with backoff.
const BACKOFF_START_MS: u32 = 1_000;
const BACKOFF_CAP_MS: u32 = 5_000;

// ─── pure coordinate mapping ─────────────────────────────────────────────

/// The rendered overlay rect in viewport (client) pixel space. A small struct
/// (not DomRect) so [`map_point_to_frame`] is pure and unit-testable with no
/// browser types.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DisplayRect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

/// Map a pointer position (clientX/clientY, in display px) into the frame's
/// native pixel space using the image's rendered rect and the last reported
/// screencast `{w, h}`. Linear in each axis — the `<img>` is stretched to fill
/// its viewport, so the overlay rect equals the image rect. Returns `None` for
/// degenerate (zero) sizes so callers no-op instead of dividing by zero.
fn map_point_to_frame(
    point: (f64, f64),
    rect: DisplayRect,
    frame: (u32, u32),
) -> Option<(f64, f64)> {
    let (frame_w, frame_h) = frame;
    if rect.width <= 0.0 || rect.height <= 0.0 || frame_w == 0 || frame_h == 0 {
        return None;
    }
    let x = (point.0 - rect.left) * frame_w as f64 / rect.width;
    let y = (point.1 - rect.top) * frame_h as f64 / rect.height;
    Some((x, y))
}

// ─── component ───────────────────────────────────────────────────────────

/// Live browser pane for the workspace. Streams the daemon's screencast SSE and
/// forwards pointer/keyboard input. The caller (WorkspacePane's Browser tab)
/// gates mounting on `running_in_tauri()`; this component is itself host-agnostic.
#[component]
pub fn WorkspaceBrowser(daemon: Daemon) -> impl IntoView {
    let url = daemon.url;
    let frame_src = RwSignal::new(String::new());
    let frame_size = RwSignal::new(None::<(u32, u32)>);
    let connected = RwSignal::new(false);
    let no_browser = RwSignal::new(false);
    let input_error = RwSignal::new(None::<String>);
    let overlay_ref: NodeRef<leptos::html::Div> = NodeRef::new();

    // ── screencast SSE: open → stream frames → on drop, backoff & reopen ──
    //
    // `active` is shared between the reconnect loop and on_cleanup; flipping it
    // false on unmount makes the loop exit within one poll tick and drop the
    // open EventSource + its listener closures.
    let active = Arc::new(AtomicBool::new(true));
    on_cleanup({
        let active = Arc::clone(&active);
        move || active.store(false, Ordering::SeqCst)
    });

    spawn_local({
        let active = Arc::clone(&active);

        async move {
            let mut backoff = BACKOFF_START_MS;
            while active.load(Ordering::SeqCst) {
                let base = url.get_untracked();
                let endpoint = format!("{}{}", base.trim_end_matches('/'), SCREENCAST_PATH);
                let es = match EventSource::new(&endpoint) {
                    Ok(es) => es,
                    Err(_) => {
                        // Construction itself failed (rare — bad URL). Back off
                        // and try again rather than spinning.
                        TimeoutFuture::new(backoff).await;
                        backoff = (backoff * 2).min(BACKOFF_CAP_MS);
                        continue;
                    }
                };
                connected.set(true);
                no_browser.set(false);

                let errored = Rc::new(Cell::new(false));

                let on_frame = Closure::wrap(Box::new({
                    move |ev: MessageEvent| {
                        let Some(text) = ev.data().as_string() else {
                            return;
                        };
                        let Ok(v) = serde_json::from_str::<Value>(&text) else {
                            return;
                        };
                        let Some(b64) = v.get("b64").and_then(Value::as_str) else {
                            return;
                        };
                        let w = v.get("w").and_then(Value::as_u64).unwrap_or(0) as u32;
                        let h = v.get("h").and_then(Value::as_u64).unwrap_or(0) as u32;
                        frame_src.set(format!("data:image/jpeg;base64,{b64}"));
                        if w > 0 && h > 0 {
                            frame_size.set(Some((w, h)));
                        }
                        // Any frame means a browser is live; clear a stale
                        // no-browser state without waiting for a status event.
                        no_browser.set(false);
                    }
                }) as Box<dyn FnMut(MessageEvent)>);
                let _ =
                    es.add_event_listener_with_callback("frame", on_frame.as_ref().unchecked_ref());

                let on_status = Closure::wrap(Box::new({
                    move |ev: MessageEvent| {
                        let Some(text) = ev.data().as_string() else {
                            return;
                        };
                        let Ok(v) = serde_json::from_str::<Value>(&text) else {
                            return;
                        };
                        if v.get("state").and_then(Value::as_str) == Some("no-browser") {
                            no_browser.set(true);
                            frame_src.set(String::new());
                        }
                    }
                }) as Box<dyn FnMut(MessageEvent)>);
                let _ = es
                    .add_event_listener_with_callback("status", on_status.as_ref().unchecked_ref());

                let on_error = Closure::wrap(Box::new({
                    let errored = Rc::clone(&errored);
                    move |_ev: web_sys::Event| errored.set(true)
                }) as Box<dyn FnMut(web_sys::Event)>);
                es.set_onerror(Some(on_error.as_ref().unchecked_ref()));

                // Hold the connection until it errors or the pane unmounts.
                // Polling (vs. a channel) keeps teardown dead simple: the
                // listener closures are dropped HERE, outside any callback, so
                // we never free a wasm Closure while its trampoline is on stack.
                loop {
                    TimeoutFuture::new(120).await;
                    if !active.load(Ordering::SeqCst) || errored.get() {
                        break;
                    }
                }
                es.close();
                drop(on_frame);
                drop(on_status);
                drop(on_error);
                connected.set(false);
                if !active.load(Ordering::SeqCst) {
                    break;
                }
                TimeoutFuture::new(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_CAP_MS);
            }
        }
    });

    // ── input forwarding ──────────────────────────────────────────────────
    let on_click = {
        move |ev: web_sys::MouseEvent| {
            let Some(el) = overlay_ref.get() else { return };
            // Focus so subsequent keydown lands on the overlay (the capture
            // surface), and so a screen reader announces it as interactive.
            let _ = el.focus();
            let Some(frame) = frame_size.get_untracked() else {
                return;
            };
            let r = el.get_bounding_client_rect();
            let rect = DisplayRect {
                left: r.left(),
                top: r.top(),
                width: r.width(),
                height: r.height(),
            };
            let Some((x, y)) =
                map_point_to_frame((ev.client_x() as f64, ev.client_y() as f64), rect, frame)
            else {
                return;
            };
            let base = url.get_untracked();
            let target = format!("{}{}", base.trim_end_matches('/'), INPUT_PATH);
            post_input(
                target,
                json!({ "kind": "click", "x": x, "y": y }),
                input_error,
            );
        }
    };

    let on_wheel = {
        move |ev: web_sys::WheelEvent| {
            let Some(el) = overlay_ref.get() else { return };
            let Some(frame) = frame_size.get_untracked() else {
                return;
            };
            let r = el.get_bounding_client_rect();
            let rect = DisplayRect {
                left: r.left(),
                top: r.top(),
                width: r.width(),
                height: r.height(),
            };
            let Some((x, y)) =
                map_point_to_frame((ev.client_x() as f64, ev.client_y() as f64), rect, frame)
            else {
                return;
            };
            let base = url.get_untracked();
            let target = format!("{}{}", base.trim_end_matches('/'), INPUT_PATH);
            post_input(
                target,
                json!({ "kind": "scroll", "x": x, "y": y, "delta_y": ev.delta_y() }),
                input_error,
            );
        }
    };

    let on_key = {
        move |ev: web_sys::KeyboardEvent| {
            // The overlay only sees keys while it holds focus, so every event
            // is intended for the remote browser. Prevent default to stop the
            // pane itself from scrolling/eating space and arrow keys.
            ev.prevent_default();
            let key = ev.key();
            let base = url.get_untracked();
            let target = format!("{}{}", base.trim_end_matches('/'), INPUT_PATH);
            // Single-char keys are printable → type{text}; multi-char are
            // control keys (Enter, Backspace, ArrowLeft, …) → key{key}.
            let body = if key.len() == 1 {
                json!({ "kind": "type", "text": key })
            } else {
                json!({ "kind": "key", "key": key })
            };
            post_input(target, body, input_error);
        }
    };

    let status_label = Signal::derive(move || {
        if daemon.browser_active.get() {
            "Agent browser live"
        } else if connected.get() {
            "Connected"
        } else {
            "Reconnecting…"
        }
        .to_string()
    });

    view! {
        <div class="workspace-browser">
            <div class="workspace-browser__toolbar">
                <span
                    class="workspace-browser__dot"
                    class:is-live=move || daemon.browser_active.get()
                    class:is-on=move || connected.get()
                    aria-hidden="true"
                ></span>
                <span class="workspace-browser__label">{status_label}</span>
                <Show when=move || input_error.get().is_some()>
                    <span class="workspace-browser__flash" role="alert">
                        {move || input_error.get().unwrap_or_default()}
                    </span>
                </Show>
            </div>
            <div class="workspace-browser__viewport">
                <img
                    class="workspace-browser__frame"
                    alt="agent browser"
                    src=move || {
                        let s = frame_src.get();
                        if s.is_empty() { None } else { Some(s) }
                    }
                />
                <div
                    class="workspace-browser__overlay"
                    tabindex="0"
                    node_ref=overlay_ref
                    on:click=on_click
                    on:wheel=on_wheel
                    on:keydown=on_key
                ></div>
                <Show when=move || frame_src.get().is_empty()>
                    <div class="workspace-browser__empty">
                        {move || {
                            if no_browser.get() {
                                "No agent browser running — the pane lights up when the agent opens one."
                            } else {
                                "Waiting for the agent's browser stream…"
                            }
                            .to_string()
                        }}
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// Fire-and-forget a single input event to the daemon. Failures (network error
/// or non-2xx) flash a small "input failed" cue that self-clears after 1.5s;
/// success is silent. Each call is its own task so a slow dispatch never blocks
/// the next pointer event.
fn post_input(target: String, body: Value, err: RwSignal<Option<String>>) {
    let flash = move || {
        err.set(Some("input failed".to_string()));

        spawn_local(async move {
            TimeoutFuture::new(1_500).await;
            err.set(None);
        });
    };
    spawn_local(async move {
        let req = match Request::post(&target)
            .header("content-type", "application/json")
            .json(&body)
        {
            Ok(r) => r,
            Err(_) => {
                flash();
                return;
            }
        };
        match req.send().await {
            Ok(resp) if resp.ok() => {}
            _ => flash(),
        }
    });
}

// ─── tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: f64, top: f64, width: f64, height: f64) -> DisplayRect {
        DisplayRect {
            left,
            top,
            width,
            height,
        }
    }

    #[test]
    fn maps_center_with_uniform_scale() {
        // 100×100 display, 200×200 frame → 2× scale; center maps to center.
        assert_eq!(
            map_point_to_frame((50.0, 50.0), rect(0.0, 0.0, 100.0, 100.0), (200, 200)),
            Some((100.0, 100.0))
        );
    }

    #[test]
    fn accounts_for_rendered_offset() {
        // Same image rendered at viewport offset (10, 20); the point shifts
        // with it and still maps to the frame center.
        assert_eq!(
            map_point_to_frame((60.0, 70.0), rect(10.0, 20.0, 100.0, 100.0), (200, 200)),
            Some((100.0, 100.0))
        );
    }

    #[test]
    fn scales_axes_independently_for_non_square_aspect() {
        // 200×100 display, 800×600 frame: x scales 4×, y scales 6×.
        assert_eq!(
            map_point_to_frame((100.0, 50.0), rect(0.0, 0.0, 200.0, 100.0), (800, 600)),
            Some((400.0, 300.0))
        );
    }

    #[test]
    fn none_on_zero_rect() {
        assert_eq!(
            map_point_to_frame((10.0, 10.0), rect(0.0, 0.0, 0.0, 0.0), (200, 200)),
            None
        );
    }

    #[test]
    fn none_on_zero_frame_dimension() {
        assert_eq!(
            map_point_to_frame((10.0, 10.0), rect(0.0, 0.0, 100.0, 100.0), (0, 200)),
            None
        );
        assert_eq!(
            map_point_to_frame((10.0, 10.0), rect(0.0, 0.0, 100.0, 100.0), (200, 0)),
            None
        );
    }
}
