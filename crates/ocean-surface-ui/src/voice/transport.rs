//! Host-neutral voice transport: where STT/TTS requests go, whether the voice
//! affordance is offered at all, and whether a response may be decoded/played.
//!
//! The daemon owns the provider credential and serves `POST /v1/voice/stt` and
//! `POST /v1/voice/tts`. Three first-party hosts reach those routes differently:
//!
//! - **Web/PWA** is served same-origin by `ocean-surface-proxy`, which exposes
//!   `/api/stt` + `/api/tts` and forwards them to the daemon's `/v1/voice/*`.
//!   The browser never learns the daemon origin, so `Daemon::url` is empty and
//!   we post to the relative proxy adapter.
//! - **Tauri** and the **Chrome extension** have no proxy in front of them, so
//!   `Daemon::url` is a concrete daemon origin and we post daemon-direct to
//!   `/v1/voice/*` (daemon CORS admits `tauri://` and `chrome-extension://`).
//!
//! No provider secret ever crosses into this crate: readiness is a capability
//! bool and the request bodies carry only user audio / text. Missing-credential
//! state arrives per request as a non-audio / error response, which the
//! validators below reject before any HTML/SPA fallback can reach `play_mp3` or
//! the STT decoder.

use leptos::prelude::*;
use serde::Deserialize;

/// Which voice route a transport URL targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceRoute {
    Stt,
    Tts,
}

/// Build the transport URL for `route` from the resolved daemon base
/// (`Daemon::url`). An empty base means "same-origin proxy adapter" (web/PWA);
/// any concrete base posts daemon-direct to `/v1/voice/*` (Tauri, extension, or
/// a web build handed an explicit daemon origin). Pure — every host decision is
/// a function of the base string alone, and no credential is ever embedded.
pub fn voice_transport_url(daemon_base: &str, route: VoiceRoute) -> String {
    let base = daemon_base.trim_end_matches('/');
    if base.is_empty() {
        match route {
            VoiceRoute::Stt => "/api/stt".to_string(),
            VoiceRoute::Tts => "/api/tts".to_string(),
        }
    } else {
        match route {
            VoiceRoute::Stt => format!("{base}/v1/voice/stt"),
            VoiceRoute::Tts => format!("{base}/v1/voice/tts"),
        }
    }
}

/// Host-neutral voice readiness.
///
/// `proxy_has_auth` is `Some(flag)` when the same-origin proxy answered
/// `/api/config` (web/PWA) and `None` when there is no proxy to ask (Tauri,
/// extension, or web served without the proxy). Hosts without a proxy reach the
/// daemon's voice routes directly, so voice is offered and any missing-credential
/// state surfaces per request through response validation — never as an exposed
/// secret. The only input is a capability bool; there is no path for a key.
pub fn voice_ready_decision(proxy_has_auth: Option<bool>) -> bool {
    proxy_has_auth.unwrap_or(true)
}

/// Is `content_type` playable audio (as opposed to an HTML/SPA fallback or a
/// JSON error body)? Gates TTS bytes before they reach the `<audio>` element so
/// a shell/extension-origin HTML fallback can never be decoded as mp3. Compares
/// only the media type, ignoring any `; charset=` parameter.
pub fn is_audio_content_type(content_type: Option<&str>) -> bool {
    media_type(content_type).is_some_and(|ct| ct.starts_with("audio/"))
}

/// Is `content_type` a JSON body? Gates the STT decode so an HTML/SPA fallback
/// is rejected before `serde` ever parses it.
pub fn is_json_content_type(content_type: Option<&str>) -> bool {
    media_type(content_type).is_some_and(|ct| ct == "application/json" || ct.ends_with("+json"))
}

/// Should a TTS response's bytes be played? Only when the request succeeded AND
/// the body is audio. Any other combination (error status, HTML fallback, JSON
/// error) is rejected before playback.
pub fn should_play_tts(status_ok: bool, content_type: Option<&str>) -> bool {
    status_ok && is_audio_content_type(content_type)
}

/// The decoded STT body, tolerant of both transport shapes: the proxy adapter's
/// `{ok, text}` / `{ok:false, error}` (both ride HTTP 200) and the daemon-direct
/// raw `{text}` (on 2xx) / `{error}` (on a non-2xx status).
#[derive(Debug, Default, Deserialize)]
pub struct SttBody {
    #[serde(default)]
    pub ok: Option<bool>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// What an STT response resolved to once status and body are reconciled.
pub enum SttOutcome {
    /// A usable transcript to deliver.
    Transcript(String),
    /// A user-facing status message (credential missing, nothing heard, ...).
    /// Hands-free drops these silently; push-to-talk surfaces them.
    Report(String),
}

/// Reconcile an STT response's HTTP success with its decoded body. A proxy error
/// rides a 200 with `ok:false`; a daemon-direct error rides a non-2xx status
/// with `{error}`. Either is a failure and yields the daemon's message (or a
/// neutral default) rather than a transcript.
pub fn interpret_stt(status_ok: bool, body: &SttBody) -> SttOutcome {
    let succeeded = status_ok && body.ok != Some(false);
    let text = body.text.as_deref().map(str::trim).unwrap_or("");
    if succeeded && !text.is_empty() {
        SttOutcome::Transcript(text.to_string())
    } else {
        SttOutcome::Report(
            body.error
                .clone()
                .unwrap_or_else(|| "no transcript heard".to_string()),
        )
    }
}

fn media_type(content_type: Option<&str>) -> Option<String> {
    content_type.map(|ct| {
        ct.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase()
    })
}

// The daemon-URL signal, installed once from the app shell so the STT upload
// paths and TTS `speak` can build a transport URL at call time. Read live rather
// than snapshotted: the URL is learned during bootstrap (and can change on a
// later reconfigure), so a snapshot would strand voice on the startup fallback.
thread_local! {
    static DAEMON_URL: std::cell::RefCell<Option<RwSignal<String>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the daemon-URL signal once from the application shell.
pub fn install_daemon_url(url: RwSignal<String>) {
    DAEMON_URL.with(|d| *d.borrow_mut() = Some(url));
}

/// The live daemon base URL, or empty (same-origin proxy adapter) when nothing
/// has been installed — matching the web/PWA default.
pub fn daemon_base() -> String {
    DAEMON_URL.with(|d| {
        d.borrow()
            .as_ref()
            .copied()
            .map(|u| u.get_untracked())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_relative_transport_url_shape() {
        // Empty daemon base = same-origin proxy adapter (web/PWA).
        assert_eq!(voice_transport_url("", VoiceRoute::Stt), "/api/stt");
        assert_eq!(voice_transport_url("", VoiceRoute::Tts), "/api/tts");
    }

    #[test]
    fn tauri_direct_transport_url_shape() {
        let base = "http://127.0.0.1:4780";
        assert_eq!(
            voice_transport_url(base, VoiceRoute::Stt),
            "http://127.0.0.1:4780/v1/voice/stt"
        );
        assert_eq!(
            voice_transport_url(base, VoiceRoute::Tts),
            "http://127.0.0.1:4780/v1/voice/tts"
        );
    }

    #[test]
    fn extension_direct_transport_url_shape() {
        // The side panel targets the loopback daemon directly (no proxy origin).
        let base = "http://127.0.0.1:4780";
        assert_eq!(
            voice_transport_url(base, VoiceRoute::Stt),
            "http://127.0.0.1:4780/v1/voice/stt"
        );
        assert_eq!(
            voice_transport_url(base, VoiceRoute::Tts),
            "http://127.0.0.1:4780/v1/voice/tts"
        );
    }

    #[test]
    fn custom_tauri_daemon_url_is_honored_and_normalized() {
        // A user-configured daemon origin, with a trailing slash to trim.
        let base = "http://192.168.1.50:4780/";
        assert_eq!(
            voice_transport_url(base, VoiceRoute::Stt),
            "http://192.168.1.50:4780/v1/voice/stt"
        );
        assert_eq!(
            voice_transport_url(base, VoiceRoute::Tts),
            "http://192.168.1.50:4780/v1/voice/tts"
        );
    }

    #[test]
    fn readiness_is_host_neutral_and_credential_free() {
        // Web: honor the proxy's capability flag.
        assert!(voice_ready_decision(Some(true)));
        // Missing voice credential / readiness-false from the proxy.
        assert!(!voice_ready_decision(Some(false)));
        // No proxy (Tauri / extension / proxy-less web): daemon-direct, offered.
        assert!(voice_ready_decision(None));
    }

    #[test]
    fn non_audio_tts_response_rejected_before_playback() {
        // HTML/SPA fallback and JSON error bodies must never reach play_mp3.
        assert!(!should_play_tts(true, Some("text/html")));
        assert!(!should_play_tts(true, Some("text/html; charset=utf-8")));
        assert!(!should_play_tts(true, Some("application/json")));
        assert!(!should_play_tts(true, None));
        // A non-2xx status is rejected even if it somehow claims audio.
        assert!(!should_play_tts(false, Some("audio/mpeg")));
        // The one accepted case: success + audio (charset param tolerated).
        assert!(should_play_tts(true, Some("audio/mpeg")));
        assert!(should_play_tts(true, Some("audio/mpeg; charset=binary")));
    }

    #[test]
    fn stt_content_type_guard() {
        assert!(is_json_content_type(Some("application/json")));
        assert!(is_json_content_type(Some(
            "application/json; charset=utf-8"
        )));
        assert!(is_json_content_type(Some("application/problem+json")));
        assert!(!is_json_content_type(Some("text/html")));
        assert!(!is_json_content_type(None));
    }

    #[test]
    fn stt_interpret_proxy_and_daemon_shapes() {
        // Proxy success: {ok:true, text}.
        let body = SttBody {
            ok: Some(true),
            text: Some("  hello  ".into()),
            error: None,
        };
        assert!(matches!(
            interpret_stt(true, &body),
            SttOutcome::Transcript(t) if t == "hello"
        ));

        // Daemon-direct success: raw {text}, no `ok` field.
        let body = SttBody {
            ok: None,
            text: Some("world".into()),
            error: None,
        };
        assert!(matches!(
            interpret_stt(true, &body),
            SttOutcome::Transcript(t) if t == "world"
        ));

        // Proxy error rides a 200 with ok:false.
        let body = SttBody {
            ok: Some(false),
            text: None,
            error: Some("nothing heard".into()),
        };
        assert!(matches!(
            interpret_stt(true, &body),
            SttOutcome::Report(m) if m == "nothing heard"
        ));

        // Daemon-direct missing credential: non-2xx with {error}.
        let body = SttBody {
            ok: None,
            text: None,
            error: Some("no xAI credential configured".into()),
        };
        assert!(matches!(
            interpret_stt(false, &body),
            SttOutcome::Report(m) if m == "no xAI credential configured"
        ));

        // Empty transcript, no error → neutral default.
        let body = SttBody {
            ok: Some(true),
            text: Some("   ".into()),
            error: None,
        };
        assert!(matches!(
            interpret_stt(true, &body),
            SttOutcome::Report(m) if m == "no transcript heard"
        ));
    }
}
