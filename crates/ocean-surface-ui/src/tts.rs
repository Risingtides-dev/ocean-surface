//! Text-to-speech playback of assistant replies.
//!
//! When an assistant turn finishes, POST `{text}` to the voice transport
//! resolved by [`crate::voice::transport`] — the same-origin proxy adapter
//! (`/api/tts`) on web, or the daemon's `/v1/voice/tts` directly on Tauri and
//! the extension — receive mp3 bytes, wrap them in a Blob, `URL.createObjectURL`
//! it, and play through a **persistent** `<audio>` element mounted in the DOM.
//! A reactive `muted` signal gates the whole thing. The response is validated
//! (2xx + audio content type) before any bytes reach the element, so a
//! missing-credential JSON error or an HTML fallback is never decoded as audio.
//!
//! Mobile autoplay: iOS Safari blocks `audio.play()` from async callbacks.
//! We get around this by calling `prime()` from a user-gesture handler
//! (VoiceOrb pointer-down) — the browser remembers the element is trusted
//! and subsequent plays work even though they're async.

use gloo_net::http::Request;
use leptos::prelude::*;
use serde::Serialize;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Blob, BlobPropertyBag, HtmlAudioElement, Url};

#[derive(Serialize)]
struct TtsRequest<'a> {
    text: &'a str,
}

// A single persistent audio element shared across all TTS calls. Created and
// primed from a user-gesture handler so mobile browsers trust its .play()
// calls even from async contexts.
thread_local! {
    static TTS_AUDIO: std::cell::OnceCell<HtmlAudioElement> = const { std::cell::OnceCell::new() };
    // The previous blob URL, revoked when the next one is set.
    static PREV_URL: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Must be called from a user-gesture handler (e.g., VoiceOrb pointer-down).
/// Creates the persistent audio element, mounts it in the document body, and
/// plays/pauses it so the browser marks it as user-trusted. Without this,
/// iOS Safari will silently reject every async `.play()` call.
pub fn prime() {
    TTS_AUDIO.with(|cell| {
        let audio = cell.get_or_init(|| {
            let el = HtmlAudioElement::new().expect("failed to create audio element");
            el.set_preload("none");
            // Mount in the DOM so the browser treats it as a real element,
            // not a detached node (important for iOS autoplay rules).
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                if let Some(body) = doc.body() {
                    let _ = body.append_child(&el);
                }
            }
            el
        });
        // Calling .play() from a gesture handler primes the element for iOS.
        // We pause as soon as playback starts so there's no audible artifact.
        let _ = audio.play();
        let audio2 = audio.clone();
        let on_playing = Closure::once_into_js(move || {
            let _ = audio2.pause();
        });
        audio.set_onplaying(Some(on_playing.unchecked_ref()));
    });
}

/// Speak `text` unless muted. No-ops on empty text.
pub fn speak(text: String, muted: RwSignal<bool>) {
    if muted.get_untracked() || text.trim().is_empty() {
        return;
    }
    spawn_local(async move {
        use crate::voice::transport;
        let url =
            transport::voice_transport_url(&transport::daemon_base(), transport::VoiceRoute::Tts);
        let body = TtsRequest { text: &text };
        let req = Request::post(&url)
            .header("content-type", "application/json")
            .json(&body);
        let resp = match req {
            Ok(r) => r.send().await,
            Err(err) => {
                log::warn!("tts encode error: {err}");
                return;
            }
        };
        let resp = match resp {
            Ok(r) => r,
            Err(err) => {
                log::warn!("tts request failed: {err}");
                return;
            }
        };
        // Validate status AND content type BEFORE reading the body: a missing
        // credential (JSON error) or an HTML/SPA fallback from the shell or
        // extension origin must never be decoded as mp3 and handed to <audio>.
        let content_type = resp.headers().get("content-type");
        if !transport::should_play_tts(resp.ok(), content_type.as_deref()) {
            log::warn!(
                "tts: non-audio response (status {}, type {:?})",
                resp.status(),
                content_type
            );
            return;
        }
        let bytes = match resp.binary().await {
            Ok(b) => b,
            Err(err) => {
                log::warn!("tts body read failed: {err}");
                return;
            }
        };
        play_mp3(bytes);
    });
}

/// Build a Blob from mp3 bytes, object-URL it, and play through the persistent
/// audio element. Revokes the URL when playback ends.
fn play_mp3(bytes: Vec<u8>) {
    // Uint8Array → Blob([..], {type:"audio/mpeg"}).
    let array = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let bag = BlobPropertyBag::new();
    bag.set_type("audio/mpeg");
    let blob = match Blob::new_with_buffer_source_sequence_and_options(&parts, &bag) {
        Ok(b) => b,
        Err(_) => {
            log::warn!("tts: failed to build blob");
            return;
        }
    };
    let url = match Url::create_object_url_with_blob(&blob) {
        Ok(u) => u,
        Err(_) => {
            log::warn!("tts: failed to create object url");
            return;
        }
    };

    // Reuse the persistent element instead of creating a new one each time.
    // Mobile browsers remember that THIS element was primed by a user gesture,
    // so .play() from an async context will be allowed.
    TTS_AUDIO.with(|cell| {
        let audio = match cell.get() {
            Some(a) => a,
            None => {
                log::warn!("tts: not primed — call tts::prime() from a gesture handler");
                let _ = Url::revoke_object_url(&url);
                return;
            }
        };

        audio.set_src(&url);

        // Revoke the previous blob URL to avoid leaking object URLs when
        // successive TTS responses reuse the persistent element.
        PREV_URL.with(|prev| {
            if let Some(old) = prev.borrow_mut().replace(url.clone()) {
                let _ = Url::revoke_object_url(&old);
            }
        });

        // Revoke the object URL once playback finishes (one-shot).
        let cleanup_url = url.clone();
        let on_ended = Closure::once_into_js(move |_e: wasm_bindgen::JsValue| {
            let _ = Url::revoke_object_url(&cleanup_url);
        });
        audio.set_onended(Some(on_ended.unchecked_ref()));

        // play() returns a promise that rejects if autoplay is blocked. With a
        // primed element this should succeed even on mobile. If it still fails
        // the audio is silently skipped (no crash).
        if let Ok(promise) = audio.play() {
            spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            });
        }
    });
}

/// Is Ocean currently speaking? True only while the persistent audio element is
/// actively playing (primed, not paused, not finished). Used by the hands-free
/// modes to decide whether an incoming utterance should barge in.
pub fn is_playing() -> bool {
    TTS_AUDIO.with(|cell| {
        cell.get()
            .map(|audio| !audio.paused() && !audio.ended() && audio.current_time() > 0.0)
            .unwrap_or(false)
    })
}

/// Stop playback immediately (barge-in). Pauses the persistent element, rewinds
/// it, and revokes the in-flight blob URL. Safe to call when nothing is
/// playing — it simply no-ops.
pub fn stop() {
    TTS_AUDIO.with(|cell| {
        if let Some(audio) = cell.get() {
            let _ = audio.pause();
            audio.set_current_time(0.0);
        }
    });
    PREV_URL.with(|prev| {
        if let Some(old) = prev.borrow_mut().take() {
            let _ = Url::revoke_object_url(&old);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tts_request_body_carries_only_text_no_provider_secret() {
        // The daemon owns the xAI credential; the surface must never send (or be
        // handed) a provider secret. The outbound TTS body is exactly the user
        // text — nothing else can ride along.
        let body = TtsRequest { text: "hello" };
        let json = serde_json::to_value(&body).expect("serialize tts body");
        assert_eq!(json, serde_json::json!({ "text": "hello" }));
        let obj = json.as_object().expect("tts body is an object");
        assert_eq!(obj.len(), 1);
        assert!(obj.contains_key("text"));
    }
}
