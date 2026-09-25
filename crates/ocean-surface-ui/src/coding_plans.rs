//! Coding plans — the Claude and Codex subscription logins on the machine
//! this session is attached to (web identity program, milestone M3).
//!
//! The daemon owns the credentials and the OAuth flow; the proxy forwards
//! `/v1/auth/providers/*` to the selected device and attaches its operator
//! credential server-side. This module only renders what the daemon reports
//! and collects intent: sign in, cancel a sign-in, sign out.
//!
//! Three rules shape it.
//!
//! 1. **The sign-in finishes on the device, not here.** The provider redirects
//!    to a loopback callback on the machine running the daemon, so the panel
//!    says so plainly and names that machine. It never pretends the browser in
//!    front of you completes the flow.
//! 2. **Nothing raw reaches the screen.** Every refusal is a typed `error`
//!    code; each one becomes a sentence here, and an unknown code becomes a
//!    generic sentence rather than a code or a JSON body.
//! 3. **Polling is owned by the panel.** A sign-in attempt is polled only
//!    while the panel is open; closing it, cancelling, or starting another
//!    attempt retires the loop by generation, so no timer outlives its reason.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::Deserialize;
use wasm_bindgen::JsValue;

/// Seconds between attempt polls.
const POLL_INTERVAL_MS: u32 = 2_000;
/// Give up waiting after this many polls (~10 minutes).
const POLL_LIMIT: u32 = 300;
/// Consecutive transport failures tolerated before a poll loop gives up.
const POLL_FAILURE_LIMIT: u32 = 5;

// ── wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProviderRow {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub expires_at_ms: Option<f64>,
    #[serde(default)]
    pub login: Option<LoginAttempt>,
}

impl ProviderRow {
    /// The name shown for the row; falls back to the provider id rather than
    /// rendering an empty heading.
    pub fn display_label(&self) -> String {
        if !self.label.trim().is_empty() {
            self.label.clone()
        } else {
            match self.provider.as_str() {
                "claude" => "Claude".to_string(),
                "codex" => "Codex".to_string(),
                other => other.to_string(),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LoginAttempt {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub attempt_id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ProvidersResponse {
    #[serde(default)]
    providers: Vec<ProviderRow>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LoginStarted {
    #[serde(default)]
    attempt_id: String,
    #[serde(default)]
    authorize_url: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LogoutResult {
    #[serde(default)]
    removed: bool,
}

/// `{ok:false, error, reason?, device?}` — every refusal on these routes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ApiError {
    #[serde(skip)]
    pub status: u16,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub device: String,
}

impl ApiError {
    fn transport() -> Self {
        Self {
            error: "network".into(),
            ..Self::default()
        }
    }
}

// ── pure decisions ────────────────────────────────────────────────

/// A provider's login status, closed over the daemon's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanStatus {
    SignedIn,
    Expired,
    SignedOut,
    Unknown,
}

impl PlanStatus {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "signed_in" => Self::SignedIn,
            "expired" => Self::Expired,
            "signed_out" => Self::SignedOut,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::SignedIn => "Signed in",
            Self::Expired => "Expired",
            Self::SignedOut => "Signed out",
            Self::Unknown => "Unknown",
        }
    }

    /// The `data-health` value the row carries, reusing the device picker's
    /// dot colours: signed in is healthy, expired needs attention, and signed
    /// out is a neutral fact rather than a failure.
    pub fn tone(self) -> &'static str {
        match self {
            Self::SignedIn => "ok",
            Self::Expired => "degraded",
            Self::SignedOut | Self::Unknown => "unknown",
        }
    }

    /// The sign-in button's words, or `None` when there is nothing to sign in
    /// to (already signed in).
    pub fn sign_in_label(self) -> Option<&'static str> {
        match self {
            Self::SignedIn => None,
            Self::Expired => Some("Sign in again"),
            Self::SignedOut | Self::Unknown => Some("Sign in"),
        }
    }
}

/// Where a credential came from, in words — only when it is not the saved
/// sign-in this panel manages.
pub fn source_hint(source: Option<&str>) -> Option<&'static str> {
    match source {
        Some("env") => Some("via environment"),
        Some("codex_cli") => Some("via Codex CLI login"),
        _ => None,
    }
}

/// May this row offer "Sign out"? Only when there is a credential, and not
/// when it comes from the machine's environment — removing a saved file
/// cannot unset an environment variable, so the button would lie.
pub fn can_sign_out(status: PlanStatus, source: Option<&str>) -> bool {
    matches!(status, PlanStatus::SignedIn | PlanStatus::Expired) && source != Some("env")
}

/// Expiry is only worth a line once it has passed: a live token refreshes
/// itself, and counting down to that would read as a warning it is not.
pub fn expiry_note(status: PlanStatus, expires_at_ms: Option<f64>, now_ms: f64) -> Option<String> {
    if status != PlanStatus::Expired {
        return None;
    }
    let at = expires_at_ms.filter(|at| at.is_finite() && *at > 0.0)?;
    let ago = ((now_ms - at) / 1000.0).max(0.0);
    let text = if ago < 60.0 {
        "expired just now".to_string()
    } else if ago < 3_600.0 {
        plural("expired", (ago / 60.0).floor(), "minute")
    } else if ago < 86_400.0 {
        plural("expired", (ago / 3_600.0).floor(), "hour")
    } else {
        plural("expired", (ago / 86_400.0).floor(), "day")
    };
    Some(text)
}

fn plural(prefix: &str, n: f64, unit: &str) -> String {
    let n = n as u64;
    let s = if n == 1 { "" } else { "s" };
    format!("{prefix} {n} {unit}{s} ago")
}

/// The machine's name for sentences, or a description when the proxy has
/// not named one (single-device and off-proxy hosts).
pub fn device_phrase(device: &str) -> String {
    if device.trim().is_empty() {
        "the machine running this Ocean".to_string()
    } else {
        device.trim().to_string()
    }
}

/// The same-machine caveat, stated once per attempt.
pub fn same_machine_caveat(device: &str) -> String {
    if device.trim().is_empty() {
        "Finish sign-in in a browser on the machine running this Ocean \u{2014} the \
         provider sends you back to that machine."
            .to_string()
    } else {
        format!(
            "Finish sign-in in a browser on the machine running this Ocean ({}) \u{2014} \
             the provider sends you back to that machine.",
            device.trim()
        )
    }
}

/// Every refusal as a sentence. `device` is the selected machine's name.
pub fn error_message(error: &ApiError, device: &str) -> String {
    let machine = device_phrase(device);
    match error.error.as_str() {
        "device_unavailable" => {
            let named = if error.device.trim().is_empty() {
                device
            } else {
                error.device.as_str()
            };
            crate::devices::unavailable_message(named, &error.reason)
        }
        "operator_credential_unavailable" => format!(
            "Ocean can't manage sign-ins on {machine} yet \u{2014} its operator key isn't set \
             up for this web surface."
        ),
        "login_unavailable" => format!(
            "Sign-in isn't available for this provider on {machine} right now. Try again in \
             a moment."
        ),
        "login_in_progress" => {
            "A sign-in for this provider is already in progress. Finish or cancel it first."
                .to_string()
        }
        "unknown_attempt" => "That sign-in attempt is no longer active. Start again.".to_string(),
        "unknown_provider" => format!("{machine} doesn't support this provider."),
        "network" => "Couldn't reach Ocean. Check your connection and try again.".to_string(),
        _ => match error.status {
            401 => "Your Ocean session has ended. Reload the page and sign in again.".to_string(),
            403 => format!("This surface isn't allowed to manage sign-ins on {machine}."),
            404 => {
                format!("{machine} doesn't support coding plan sign-in yet. Update Ocean there.")
            }
            _ => format!("Something went wrong talking to {machine}. Try again."),
        },
    }
}

/// A sign-in attempt's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptState {
    Pending,
    Succeeded,
    Failed,
    Cancelled,
}

impl AttemptState {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" | "canceled" => Self::Cancelled,
            _ => Self::Pending,
        }
    }

    pub fn is_terminal(self) -> bool {
        self != Self::Pending
    }
}

/// The line shown once an attempt ends. `error` is the daemon's code for a
/// failed attempt; it is never rendered verbatim.
pub fn attempt_outcome(label: &str, state: AttemptState, error: Option<&str>) -> Notice {
    match state {
        AttemptState::Succeeded => Notice::ok(format!("{label} is signed in.")),
        AttemptState::Cancelled => Notice::ok(format!("{label} sign-in cancelled.")),
        AttemptState::Pending => Notice::ok(String::new()),
        AttemptState::Failed => {
            let why = match error.unwrap_or_default() {
                "timeout" | "expired" => " It took too long \u{2014} start again.",
                "access_denied" | "denied" => " Access was declined in the browser.",
                "state_mismatch" | "invalid_state" => " The browser returned a stale response.",
                _ => " Try again.",
            };
            Notice::err(format!("{label} sign-in didn't finish.{why}"))
        }
    }
}

/// A line under the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub is_error: bool,
}

impl Notice {
    fn ok(text: String) -> Self {
        Self {
            text,
            is_error: false,
        }
    }
    fn err(text: String) -> Self {
        Self {
            text,
            is_error: true,
        }
    }
}

/// The result line after a sign-out.
pub fn logout_outcome(label: &str, removed: bool, device: &str) -> Notice {
    let machine = device_phrase(device);
    if removed {
        Notice::ok(format!("Signed out of {label} on {machine}."))
    } else {
        Notice::ok(format!(
            "There was no saved {label} sign-in on {machine} to remove."
        ))
    }
}

fn provider_path(base: &str, provider: &str, tail: &str) -> String {
    format!(
        "{}/v1/auth/providers/{}{}",
        base.trim_end_matches('/'),
        encode_segment(provider),
        tail
    )
}

/// Path-segment encoding for ids the daemon minted; they are expected to be
/// URL-safe already, so anything else is percent-encoded rather than trusted.
fn encode_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ── reactive handle ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadPhase {
    Idle,
    Loading,
    Ready,
    Failed(String),
}

/// A sign-in the panel is waiting on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveAttempt {
    pub provider: String,
    pub label: String,
    pub attempt_id: String,
    /// `None` when the attempt was adopted from the list (started earlier,
    /// before the panel was reopened) — its URL is not re-served.
    pub authorize_url: Option<String>,
}

/// The panel's state, created once at app scope.
#[derive(Clone, Copy)]
pub struct CodingPlansState {
    pub open: RwSignal<bool>,
    pub providers: RwSignal<Vec<ProviderRow>>,
    pub phase: RwSignal<LoadPhase>,
    pub attempt: RwSignal<Option<ActiveAttempt>>,
    /// Provider with a request (login start / logout) in flight.
    pub busy: RwSignal<Option<String>>,
    /// Provider whose sign-out is armed and waiting for confirmation.
    pub confirm_logout: RwSignal<Option<String>>,
    pub notice: RwSignal<Option<Notice>>,
    /// Daemon base URL (same origin behind the proxy).
    base: RwSignal<String>,
    /// Selected device's name, for sentences.
    pub device: RwSignal<String>,
    /// Retires poll loops and stale list replies.
    generation: RwSignal<u64>,
}

impl CodingPlansState {
    pub fn new(base: RwSignal<String>, device: RwSignal<String>) -> Self {
        Self {
            open: RwSignal::new(false),
            providers: RwSignal::new(Vec::new()),
            phase: RwSignal::new(LoadPhase::Idle),
            attempt: RwSignal::new(None),
            busy: RwSignal::new(None),
            confirm_logout: RwSignal::new(None),
            notice: RwSignal::new(None),
            base,
            device,
            generation: RwSignal::new(0),
        }
    }

    /// Open the panel and read the list fresh. The only door in; the header
    /// overflow row and the palette command both call it.
    pub fn show(self) {
        self.notice.set(None);
        self.confirm_logout.set(None);
        self.open.set(true);
        self.refresh();
    }

    /// Close the panel and retire any poll loop. A pending attempt keeps
    /// running on the device; reopening the panel adopts it from the list.
    pub fn close(self) {
        self.bump();
        self.open.set(false);
        self.attempt.set(None);
        self.busy.set(None);
        self.confirm_logout.set(None);
    }

    fn bump(self) -> u64 {
        let next = self.generation.get_untracked().wrapping_add(1);
        self.generation.set(next);
        next
    }

    fn current(self, claimed: u64) -> bool {
        crate::devices::claim_is_current(claimed, self.generation.get_untracked())
    }

    pub fn refresh(self) {
        let claimed = self.generation.get_untracked();
        let base = self.base.get_untracked();
        if self.providers.get_untracked().is_empty() {
            self.phase.set(LoadPhase::Loading);
        }
        spawn_local(async move {
            let result = fetch_providers(&base).await;
            if !self.current(claimed) || !self.open.get_untracked() {
                return;
            }
            match result {
                Ok(rows) => {
                    let adopt = if self.attempt.get_untracked().is_none() {
                        rows.iter().find_map(|row| {
                            let login = row.login.as_ref()?;
                            (AttemptState::parse(&login.state) == AttemptState::Pending
                                && !login.attempt_id.is_empty())
                            .then(|| ActiveAttempt {
                                provider: row.provider.clone(),
                                label: row.display_label(),
                                attempt_id: login.attempt_id.clone(),
                                authorize_url: None,
                            })
                        })
                    } else {
                        None
                    };
                    self.providers.set(rows);
                    self.phase.set(LoadPhase::Ready);
                    if let Some(attempt) = adopt {
                        self.watch(attempt);
                    }
                }
                Err(error) => {
                    let device = self.device.get_untracked();
                    self.phase
                        .set(LoadPhase::Failed(error_message(&error, &device)));
                }
            }
        });
    }

    /// Start a sign-in. `popup` is a blank tab opened synchronously inside the
    /// click (so the browser does not block it); it is pointed at the
    /// provider once the daemon answers, or closed if it refuses.
    pub fn sign_in(self, provider: String, label: String, popup: Option<web_sys::Window>) {
        if self.busy.get_untracked().is_some() || self.attempt.get_untracked().is_some() {
            return;
        }
        self.notice.set(None);
        self.confirm_logout.set(None);
        self.busy.set(Some(provider.clone()));
        let base = self.base.get_untracked();
        let claimed = self.generation.get_untracked();
        spawn_local(async move {
            let result = post_login(&base, &provider).await;
            if !self.current(claimed) {
                if let Some(popup) = popup {
                    let _ = popup.close();
                }
                return;
            }
            self.busy.set(None);
            match result {
                Ok(started) => {
                    let url = started.authorize_url.clone();
                    let opened = match popup {
                        Some(popup) => popup.location().set_href(&url).is_ok(),
                        None => crate::host::open_external_url(&url).await,
                    };
                    if !opened {
                        log::debug!("sign-in tab not opened; the panel offers a link instead");
                    }
                    self.watch(ActiveAttempt {
                        provider,
                        label,
                        attempt_id: started.attempt_id,
                        authorize_url: (!url.is_empty()).then_some(url),
                    });
                }
                Err(error) => {
                    if let Some(popup) = popup {
                        let _ = popup.close();
                    }
                    let device = self.device.get_untracked();
                    self.notice
                        .set(Some(Notice::err(error_message(&error, &device))));
                }
            }
        });
    }

    /// Poll `attempt` until it ends, the panel closes, or another intent
    /// supersedes it.
    fn watch(self, attempt: ActiveAttempt) {
        let claimed = self.bump();
        self.attempt.set(Some(attempt.clone()));
        let base = self.base.get_untracked();
        spawn_local(async move {
            let url = provider_path(
                &base,
                &attempt.provider,
                &format!("/login/{}", encode_segment(&attempt.attempt_id)),
            );
            let mut failures = 0;
            for _ in 0..POLL_LIMIT {
                gloo_timers::future::TimeoutFuture::new(POLL_INTERVAL_MS).await;
                if !self.current(claimed) || !self.open.get_untracked() {
                    return;
                }
                let result = get_json::<LoginAttempt>(gloo_net::http::Request::get(&url)).await;
                if !self.current(claimed) {
                    return;
                }
                match result {
                    Ok(status) => {
                        failures = 0;
                        let state = AttemptState::parse(&status.state);
                        if state.is_terminal() {
                            self.finish(attempt_outcome(
                                &attempt.label,
                                state,
                                status.error.as_deref(),
                            ));
                            return;
                        }
                    }
                    Err(error) if error.error == "unknown_attempt" || error.status == 404 => {
                        let device = self.device.get_untracked();
                        self.finish(Notice::err(error_message(
                            &ApiError {
                                error: "unknown_attempt".into(),
                                ..error
                            },
                            &device,
                        )));
                        return;
                    }
                    Err(error) => {
                        failures += 1;
                        if failures >= POLL_FAILURE_LIMIT {
                            let device = self.device.get_untracked();
                            self.finish(Notice::err(error_message(&error, &device)));
                            return;
                        }
                    }
                }
            }
            if self.current(claimed) {
                self.finish(Notice::err(format!(
                    "Stopped waiting for {} sign-in. If you finished it, reopen this panel.",
                    attempt.label
                )));
            }
        });
    }

    fn finish(self, notice: Notice) {
        self.bump();
        self.attempt.set(None);
        if !notice.text.is_empty() {
            self.notice.set(Some(notice));
        }
        self.refresh();
    }

    /// Cancel the attempt being watched. Best effort on the wire: the local
    /// wait ends either way.
    pub fn cancel(self) {
        let Some(attempt) = self.attempt.get_untracked() else {
            return;
        };
        self.bump();
        self.attempt.set(None);
        let base = self.base.get_untracked();
        let claimed = self.generation.get_untracked();
        spawn_local(async move {
            let url = provider_path(
                &base,
                &attempt.provider,
                &format!("/login/{}", encode_segment(&attempt.attempt_id)),
            );
            if let Err(error) = send(gloo_net::http::Request::delete(&url)).await {
                log::debug!("cancel sign-in: {}", error.error);
            }
            if self.current(claimed) {
                self.notice.set(Some(attempt_outcome(
                    &attempt.label,
                    AttemptState::Cancelled,
                    None,
                )));
                self.refresh();
            }
        });
    }

    pub fn sign_out(self, provider: String, label: String) {
        if self.busy.get_untracked().is_some() {
            return;
        }
        self.confirm_logout.set(None);
        self.notice.set(None);
        self.busy.set(Some(provider.clone()));
        let base = self.base.get_untracked();
        let claimed = self.generation.get_untracked();
        spawn_local(async move {
            let url = provider_path(&base, &provider, "/logout");
            let result = get_json::<LogoutResult>(gloo_net::http::Request::post(&url)).await;
            if !self.current(claimed) {
                return;
            }
            self.busy.set(None);
            let device = self.device.get_untracked();
            let notice = match result {
                Ok(done) => logout_outcome(&label, done.removed, &device),
                Err(error) => Notice::err(error_message(&error, &device)),
            };
            self.notice.set(Some(notice));
            self.refresh();
        });
    }
}

// ── transport ─────────────────────────────────────────────────────

async fn read_error(response: gloo_net::http::Response) -> ApiError {
    let status = response.status();
    let mut error = response.json::<ApiError>().await.unwrap_or_default();
    error.status = status;
    error
}

async fn get_json<T: serde::de::DeserializeOwned>(
    request: gloo_net::http::RequestBuilder,
) -> Result<T, ApiError> {
    let response = request.send().await.map_err(|_| ApiError::transport())?;
    if !response.ok() {
        return Err(read_error(response).await);
    }
    let status = response.status();
    response.json::<T>().await.map_err(|_| ApiError {
        status,
        error: "unreadable".into(),
        ..ApiError::default()
    })
}

async fn send(request: gloo_net::http::RequestBuilder) -> Result<(), ApiError> {
    let response = request.send().await.map_err(|_| ApiError::transport())?;
    if response.ok() {
        Ok(())
    } else {
        Err(read_error(response).await)
    }
}

async fn fetch_providers(base: &str) -> Result<Vec<ProviderRow>, ApiError> {
    let url = format!("{}/v1/auth/providers", base.trim_end_matches('/'));
    get_json::<ProvidersResponse>(gloo_net::http::Request::get(&url))
        .await
        .map(|body| body.providers)
}

async fn post_login(base: &str, provider: &str) -> Result<LoginStarted, ApiError> {
    let url = provider_path(base, provider, "/login");
    get_json::<LoginStarted>(gloo_net::http::Request::post(&url)).await
}

/// Open a blank tab synchronously, inside the click, so a popup blocker lets
/// it through; the sign-in URL is assigned once the daemon answers. Returns
/// `None` in the native shell (the OS browser opens instead) and when the
/// browser refuses anyway (the panel then offers a link).
fn open_blank_tab() -> Option<web_sys::Window> {
    if crate::host::running_in_tauri() {
        return None;
    }
    let popup = web_sys::window()?
        .open_with_url_and_target("", "_blank")
        .ok()
        .flatten()?;
    // Sever the back-reference before the provider's page loads in it.
    let _ = popup.set_opener(&JsValue::NULL);
    Some(popup)
}

// ── view ──────────────────────────────────────────────────────────

/// The panel. A self-contained overlay that consumes its own Escape, like
/// the device picker it sits beside (and whose styles it shares).
#[component]
pub fn CodingPlansPanel(state: CodingPlansState) -> impl IntoView {
    let panel: NodeRef<leptos::html::Div> = NodeRef::new();
    Effect::new(move |_| {
        if state.open.get() {
            if let Some(element) = panel.get() {
                let _ = element.focus();
            }
        }
    });
    // A device switch while the panel is open describes the machine we left.
    Effect::new(move |previous: Option<String>| {
        let now = state.device.get();
        if let Some(previous) = previous {
            if previous != now && state.open.get_untracked() {
                state.close();
                state.show();
            }
        }
        now
    });
    let device = move || device_phrase(&state.device.get());
    view! {
        <div
            class="devices-overlay coding-plans-overlay"
            hidden=move || !state.open.get()
            on:click=move |_| state.close()
        >
            <div
                class="devices-panel coding-plans"
                node_ref=panel
                role="dialog"
                aria-modal="true"
                aria-labelledby="coding-plans-title"
                tabindex="-1"
                on:click=|event| event.stop_propagation()
                on:keydown=move |event| {
                    if event.key() == "Escape" {
                        event.stop_propagation();
                        state.close();
                    }
                }
            >
                <header class="devices-panel__head">
                    <h2 class="devices-panel__title" id="coding-plans-title">"Coding plans"</h2>
                    <button
                        class="sessions-panel__close"
                        type="button"
                        aria-label="Close coding plans"
                        on:click=move |_| state.close()
                    >
                        "✕"
                    </button>
                </header>
                <p class="devices-panel__note">
                    {move || {
                        format!(
                            "Claude and Codex subscriptions signed in on {}. Agents on that machine use them.",
                            device(),
                        )
                    }}
                </p>
                <Show when=move || matches!(state.phase.get(), LoadPhase::Loading)>
                    <p class="devices-panel__note" role="status">"Checking sign-ins\u{2026}"</p>
                </Show>
                <Show when=move || matches!(state.phase.get(), LoadPhase::Failed(_))>
                    <p class="devices-panel__error" role="alert">
                        {move || match state.phase.get() {
                            LoadPhase::Failed(message) => message,
                            _ => String::new(),
                        }}
                    </p>
                </Show>
                <Show when=move || state.attempt.get().is_some()>
                    <WaitingForBrowser state=state />
                </Show>
                <ul class="devices-panel__list coding-plans__list">
                    <For
                        each=move || state.providers.get()
                        key=|row| format!("{row:?}")
                        let:row
                    >
                        <PlanRow state=state row=row />
                    </For>
                </ul>
                <Show when=move || state.notice.get().is_some()>
                    {move || {
                        let notice = state.notice.get()?;
                        Some(if notice.is_error {
                            view! {
                                <p class="devices-panel__error" role="alert">{notice.text}</p>
                            }
                                .into_any()
                        } else {
                            view! {
                                <p class="devices-panel__note coding-plans__notice" role="status">
                                    {notice.text}
                                </p>
                            }
                                .into_any()
                        })
                    }}
                </Show>
            </div>
        </div>
    }
}

/// The wait while a sign-in is open in another tab.
#[component]
fn WaitingForBrowser(state: CodingPlansState) -> impl IntoView {
    let attempt = move || state.attempt.get();
    view! {
        <div class="devices-panel__warning coding-plans__waiting" role="status" aria-live="polite">
            <p class="devices-panel__note coding-plans__waiting-line">
                <strong>
                    {move || {
                        attempt()
                            .map(|a| format!("{}: waiting for you to finish in the browser\u{2026}", a.label))
                            .unwrap_or_default()
                    }}
                </strong>
            </p>
            <p class="devices-panel__note coding-plans__waiting-line">
                {move || same_machine_caveat(&state.device.get())}
            </p>
            <div class="devices-panel__head coding-plans__actions">
                <Show when=move || attempt().and_then(|a| a.authorize_url).is_some()>
                    <a
                        class="swipedeck-btn coding-plans__link"
                        href=move || attempt().and_then(|a| a.authorize_url).unwrap_or_default()
                        target="_blank"
                        rel="noopener noreferrer"
                        on:click=crate::host::open_external_link_click
                    >
                        "Open sign-in page"
                    </a>
                </Show>
                <button
                    class="swipedeck-btn swipedeck-btn--danger"
                    type="button"
                    on:click=move |_| state.cancel()
                >
                    "Cancel"
                </button>
            </div>
        </div>
    }
}

/// One provider.
#[component]
fn PlanRow(state: CodingPlansState, row: ProviderRow) -> impl IntoView {
    let status = PlanStatus::parse(&row.status);
    let label = row.display_label();
    let provider = row.provider.clone();
    let source = row.source.clone();
    let hint = source_hint(source.as_deref());
    let expiry = expiry_note(status, row.expires_at_ms, js_sys::Date::now());
    let detail = [
        Some(status.label().to_string()),
        hint.map(str::to_string),
        expiry,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" \u{00B7} ");
    let sign_in_text = status.sign_in_label();
    let sign_out_allowed = can_sign_out(status, source.as_deref());

    let busy = {
        let provider = provider.clone();
        move || state.busy.get().as_deref() == Some(provider.as_str())
    };
    let blocked = move || state.busy.get().is_some() || state.attempt.get().is_some();
    let armed = {
        let provider = provider.clone();
        move || state.confirm_logout.get().as_deref() == Some(provider.as_str())
    };

    let sign_in = {
        let provider = provider.clone();
        let label = label.clone();
        move |_| state.sign_in(provider.clone(), label.clone(), open_blank_tab())
    };
    let arm_logout = {
        let provider = provider.clone();
        move |_| state.confirm_logout.set(Some(provider.clone()))
    };
    let confirm_logout = {
        let provider = provider.clone();
        let label = label.clone();
        move |_| state.sign_out(provider.clone(), label.clone())
    };
    let confirm_text = {
        let label = label.clone();
        move || {
            format!(
                "Sign out of {label}? This removes the saved sign-in from {}.",
                device_phrase(&state.device.get())
            )
        }
    };
    let busy_for_in = busy.clone();
    let label_for_actions = label.clone();

    view! {
        <li class="devices-panel__row coding-plans__row" data-provider=provider.clone()>
            <div class="devices-panel__pick coding-plans__summary" data-health=status.tone()>
                <span class="devices-panel__dot" aria-hidden="true"></span>
                <span class="devices-panel__name">{label.clone()}</span>
                <span class="devices-panel__health coding-plans__detail">{detail}</span>
                <span class="devices-panel__current coding-plans__status" data-status=status.tone()>
                    {status.label()}
                </span>
            </div>
            <Show
                when=armed.clone()
                fallback=move || {
                    view! {
                        <div class="devices-panel__head coding-plans__actions">
                            {sign_in_text
                                .map(|text| {
                                    let busy = busy_for_in.clone();
                                    view! {
                                        <button
                                            class="swipedeck-btn coding-plans__sign-in"
                                            type="button"
                                            disabled=blocked
                                            aria-label=format!("{text} to {}", label_for_actions.clone())
                                            on:click=sign_in.clone()
                                        >
                                            {move || if busy() { "Starting\u{2026}" } else { text }}
                                        </button>
                                    }
                                })}
                            {sign_out_allowed
                                .then(|| {
                                    view! {
                                        <button
                                            class="swipedeck-btn coding-plans__sign-out"
                                            type="button"
                                            disabled=blocked
                                            on:click=arm_logout.clone()
                                        >
                                            "Sign out"
                                        </button>
                                    }
                                })}
                        </div>
                    }
                }
            >
                <div class="devices-panel__warning coding-plans__confirm" role="group" aria-label="Confirm sign out">
                    <p class="devices-panel__note coding-plans__waiting-line">{confirm_text.clone()}</p>
                    <div class="devices-panel__head coding-plans__actions">
                        <button
                            class="swipedeck-btn"
                            type="button"
                            on:click=move |_| state.confirm_logout.set(None)
                        >
                            "Keep"
                        </button>
                        <button
                            class="swipedeck-btn swipedeck-btn--danger coding-plans__confirm-sign-out"
                            type="button"
                            disabled=busy.clone()
                            on:click=confirm_logout.clone()
                        >
                            {
                                let busy = busy.clone();
                                move || if busy() { "Signing out\u{2026}" } else { "Sign out" }
                            }
                        </button>
                    </div>
                </div>
            </Show>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_as_words_and_an_unknown_one_is_neutral() {
        assert_eq!(PlanStatus::parse("signed_in").label(), "Signed in");
        assert_eq!(PlanStatus::parse("expired").label(), "Expired");
        assert_eq!(PlanStatus::parse("signed_out").label(), "Signed out");
        assert_eq!(PlanStatus::parse("unknown").label(), "Unknown");
        assert_eq!(PlanStatus::parse("rotating").label(), "Unknown");
        assert_eq!(PlanStatus::parse("signed_in").tone(), "ok");
        assert_eq!(PlanStatus::parse("expired").tone(), "degraded");
        // Signed out is a fact, not a failure: it must not borrow red.
        assert_eq!(PlanStatus::parse("signed_out").tone(), "unknown");
    }

    #[test]
    fn the_sign_in_button_says_again_only_when_expired() {
        assert_eq!(PlanStatus::SignedIn.sign_in_label(), None);
        assert_eq!(PlanStatus::Expired.sign_in_label(), Some("Sign in again"));
        assert_eq!(PlanStatus::SignedOut.sign_in_label(), Some("Sign in"));
        assert_eq!(PlanStatus::Unknown.sign_in_label(), Some("Sign in"));
    }

    #[test]
    fn source_is_hinted_only_when_it_is_not_the_saved_sign_in() {
        assert_eq!(source_hint(Some("env")), Some("via environment"));
        assert_eq!(source_hint(Some("codex_cli")), Some("via Codex CLI login"));
        assert_eq!(source_hint(Some("auth_file")), None);
        assert_eq!(source_hint(None), None);
        assert_eq!(source_hint(Some("keychain")), None);
    }

    #[test]
    fn sign_out_is_offered_only_for_a_removable_credential() {
        assert!(can_sign_out(PlanStatus::SignedIn, Some("auth_file")));
        assert!(can_sign_out(PlanStatus::Expired, None));
        assert!(can_sign_out(PlanStatus::SignedIn, Some("codex_cli")));
        // An environment variable cannot be removed by deleting a file.
        assert!(!can_sign_out(PlanStatus::SignedIn, Some("env")));
        assert!(!can_sign_out(PlanStatus::SignedOut, None));
        assert!(!can_sign_out(PlanStatus::Unknown, None));
    }

    #[test]
    fn expiry_is_mentioned_only_once_it_has_passed() {
        let now = 10_000_000_000.0;
        assert_eq!(
            expiry_note(PlanStatus::SignedIn, Some(now + 60_000.0), now),
            None
        );
        assert_eq!(expiry_note(PlanStatus::Expired, None, now), None);
        assert_eq!(
            expiry_note(PlanStatus::Expired, Some(now - 10_000.0), now).as_deref(),
            Some("expired just now")
        );
        assert_eq!(
            expiry_note(PlanStatus::Expired, Some(now - 60_000.0), now).as_deref(),
            Some("expired 1 minute ago")
        );
        assert_eq!(
            expiry_note(PlanStatus::Expired, Some(now - 3.0 * 3_600_000.0), now).as_deref(),
            Some("expired 3 hours ago")
        );
        assert_eq!(
            expiry_note(PlanStatus::Expired, Some(now - 2.0 * 86_400_000.0), now).as_deref(),
            Some("expired 2 days ago")
        );
        // A clock skewed into the future reads as just now, not negative.
        assert_eq!(
            expiry_note(PlanStatus::Expired, Some(now + 5_000.0), now).as_deref(),
            Some("expired just now")
        );
    }

    fn api(status: u16, error: &str) -> ApiError {
        ApiError {
            status,
            error: error.into(),
            ..ApiError::default()
        }
    }

    #[test]
    fn every_error_code_is_a_sentence_and_never_the_code() {
        let codes = [
            (409, "login_unavailable"),
            (503, "operator_credential_unavailable"),
            (404, "unknown_attempt"),
            (404, "unknown_provider"),
            (409, "login_in_progress"),
            (0, "network"),
            (500, "internal"),
            (401, ""),
            (403, "forbidden"),
            (404, ""),
            (418, "teapot_mode"),
        ];
        for (status, code) in codes {
            let text = error_message(&api(status, code), "studio");
            assert!(text.ends_with('.'), "{code}: {text}");
            assert!(!text.contains('{'), "{code}: {text}");
            if !code.is_empty() {
                assert!(!text.contains(code), "{code} leaked: {text}");
            }
        }
        assert!(
            error_message(&api(503, "operator_credential_unavailable"), "studio")
                .contains("studio")
        );
        assert!(error_message(&api(409, "login_unavailable"), "")
            .contains("the machine running this Ocean"));
    }

    #[test]
    fn device_unavailable_reuses_the_picker_wording() {
        let error = ApiError {
            status: 503,
            error: "device_unavailable".into(),
            reason: "unreachable".into(),
            device: "studio".into(),
        };
        assert_eq!(
            error_message(&error, "mini"),
            "studio isn't answering. Pick another machine, or wake it up."
        );
        // No device in the body: fall back to the selected one.
        let bare = ApiError {
            device: String::new(),
            ..error
        };
        assert!(error_message(&bare, "mini").starts_with("mini isn't answering"));
    }

    #[test]
    fn the_caveat_names_the_machine_when_it_can() {
        assert_eq!(
            same_machine_caveat("studio"),
            "Finish sign-in in a browser on the machine running this Ocean (studio) \u{2014} \
             the provider sends you back to that machine."
        );
        assert!(!same_machine_caveat("  ").contains("()"));
    }

    #[test]
    fn attempt_states_end_where_the_daemon_says_they_end() {
        assert!(!AttemptState::parse("pending").is_terminal());
        assert!(!AttemptState::parse("").is_terminal());
        assert!(AttemptState::parse("succeeded").is_terminal());
        assert!(AttemptState::parse("failed").is_terminal());
        assert!(AttemptState::parse("cancelled").is_terminal());
        let ok = attempt_outcome("Claude", AttemptState::Succeeded, None);
        assert!(!ok.is_error);
        assert_eq!(ok.text, "Claude is signed in.");
        let failed = attempt_outcome("Codex", AttemptState::Failed, Some("weird_internal_code"));
        assert!(failed.is_error);
        assert!(!failed.text.contains("weird_internal_code"));
    }

    #[test]
    fn sign_out_result_is_honest_about_nothing_to_remove() {
        let removed = logout_outcome("Claude", true, "studio");
        assert_eq!(removed.text, "Signed out of Claude on studio.");
        let nothing = logout_outcome("Claude", false, "");
        assert!(nothing.text.contains("no saved Claude sign-in"));
        assert!(!nothing.is_error);
    }

    #[test]
    fn the_provider_list_decodes_the_daemons_shape() {
        let body: ProvidersResponse = serde_json::from_str(
            r#"{"ok":true,"providers":[
                {"provider":"claude","label":"Claude","kind":"oauth","status":"signed_in",
                 "source":"auth_file","expires_at_ms":1790000000000,"login":null},
                {"provider":"codex","label":"Codex","kind":"oauth","status":"signed_out",
                 "source":null,"expires_at_ms":null,
                 "login":{"provider":"codex","attempt_id":"a1","state":"pending"}}
            ]}"#,
        )
        .expect("providers");
        assert_eq!(body.providers.len(), 2);
        assert_eq!(body.providers[0].source.as_deref(), Some("auth_file"));
        assert_eq!(body.providers[0].expires_at_ms, Some(1_790_000_000_000.0));
        assert!(body.providers[0].login.is_none());
        let login = body.providers[1].login.as_ref().expect("login");
        assert_eq!(login.attempt_id, "a1");
        assert_eq!(AttemptState::parse(&login.state), AttemptState::Pending);

        // A sparse row still has a heading.
        let sparse: ProviderRow = serde_json::from_str(r#"{"provider":"codex"}"#).expect("row");
        assert_eq!(sparse.display_label(), "Codex");
        assert_eq!(PlanStatus::parse(&sparse.status), PlanStatus::Unknown);
    }

    #[test]
    fn login_start_and_error_bodies_decode() {
        let started: LoginStarted = serde_json::from_str(
            r#"{"ok":true,"provider":"claude","attempt_id":"att_1","state":"pending",
                "authorize_url":"https://claude.ai/oauth/authorize?x=1","same_machine_required":true}"#,
        )
        .expect("started");
        assert_eq!(started.attempt_id, "att_1");
        assert!(started.authorize_url.starts_with("https://"));
        let error: ApiError =
            serde_json::from_str(r#"{"ok":false,"error":"login_unavailable"}"#).expect("error");
        assert_eq!(error.error, "login_unavailable");
        let logout: LogoutResult =
            serde_json::from_str(r#"{"ok":true,"provider":"codex","removed":true}"#)
                .expect("logout");
        assert!(logout.removed);
    }

    #[test]
    fn paths_encode_what_the_daemon_minted() {
        assert_eq!(
            provider_path("https://ocean.example/", "claude", "/login"),
            "https://ocean.example/v1/auth/providers/claude/login"
        );
        assert_eq!(
            provider_path("", "co dex", &format!("/login/{}", encode_segment("a/b"))),
            "/v1/auth/providers/co%20dex/login/a%2Fb"
        );
    }
}
