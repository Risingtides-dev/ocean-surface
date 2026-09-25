//! "Continue with GitHub" — the proxy's second way in.
//!
//! A GitHub OAuth App proves WHO is at the browser; the roster still decides
//! WHAT they may reach. A GitHub account maps to exactly one roster entry by
//! that entry's numeric `github_id` — never the login, which its owner can
//! rename and a stranger can then register — and the session it earns is the same derived
//! cookie a password login earns for that entry, so every route behind the
//! gate sees one kind of signed-in person regardless of how they got in.
//!
//! What this module never does: hold a provider credential, keep the GitHub
//! access token past the callback that used it, or admit an account that is
//! not on the roster. When an org is configured, the account must also be an
//! ACTIVE member of it at sign-in time.
//!
//! Configuration (all read once at startup):
//!
//! - `OCEAN_SURFACE_GITHUB_CLIENT_ID` — enables the flow when non-empty.
//! - `OCEAN_SURFACE_GITHUB_CLIENT_SECRET_FILE` — mode-0600 file holding the
//!   OAuth App secret; defaults to `~/.config/ocean-surface/github-client-secret`.
//! - `OCEAN_SURFACE_PUBLIC_URL` — the origin GitHub redirects back to, e.g.
//!   `https://ocean.agentsworld.org`. Required: the callback URL registered on
//!   the OAuth App must be `<public url>/auth/github/callback`.
//! - `OCEAN_SURFACE_GITHUB_ORG` — optional org whose active membership is
//!   required. Asking for it adds the `read:org` scope.
//! - `OCEAN_SURFACE_GITHUB_OAUTH_URL` / `OCEAN_SURFACE_GITHUB_API_URL` — test
//!   seams for a stub GitHub; production never sets them.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use base64::Engine;
use serde::Deserialize;

use crate::{
    constant_time_eq, cookie_value, has_valid_session, percent_encode_path_segment,
    read_mode_0600_secret, request_is_https, AppState, ProxyUser, SESSION_COOKIE,
    SESSION_MAX_AGE_SECONDS,
};

/// The anti-CSRF `state` round-trip cookie. SameSite=Lax, not Strict: the
/// callback is a top-level navigation FROM github.com, and a Strict cookie is
/// not sent on it — the state check would fail for everyone. Under HTTPS it
/// carries the `__Host-` prefix (Secure, `Path=/`, no `Domain`), the only form
/// a sibling `*.agentsworld.org` host cannot plant.
const STATE_COOKIE: &str = "ocean_gh_state";
const HOST_STATE_COOKIE: &str = "__Host-ocean_gh_state";
/// Ten minutes to finish a GitHub consent screen is generous; the cookie is
/// single-use either way.
const STATE_MAX_AGE_SECONDS: u64 = 600;
const USER_AGENT: &str = "ocean-surface-proxy";

pub(crate) const START_PATH: &str = "/auth/github";
pub(crate) const CALLBACK_PATH: &str = "/auth/github/callback";

#[derive(Clone)]
pub(crate) struct GithubLogin {
    client_id: String,
    client_secret: String,
    org: Option<String>,
    public_url: String,
    oauth_base: String,
    api_base: String,
}

impl std::fmt::Debug for GithubLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubLogin")
            .field("client_id", &self.client_id)
            .field("client_secret", &"[redacted]")
            .field("org", &self.org)
            .field("public_url", &self.public_url)
            .finish()
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn default_secret_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/ocean-surface/github-client-secret")
}

impl GithubLogin {
    /// `Ok(None)` when no client id is configured: GitHub sign-in is off and
    /// the login page shows only the password form, exactly as before.
    pub(crate) fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(client_id) = env_nonempty("OCEAN_SURFACE_GITHUB_CLIENT_ID") else {
            return Ok(None);
        };
        let secret_path = std::env::var_os("OCEAN_SURFACE_GITHUB_CLIENT_SECRET_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(default_secret_path);
        let client_secret = read_mode_0600_secret(&secret_path, "GitHub OAuth client secret")?;
        let public_url = env_nonempty("OCEAN_SURFACE_PUBLIC_URL").context(
            "OCEAN_SURFACE_GITHUB_CLIENT_ID is set but OCEAN_SURFACE_PUBLIC_URL is not; \
             GitHub needs the exact origin to redirect back to",
        )?;
        Self::new(
            client_id,
            client_secret,
            env_nonempty("OCEAN_SURFACE_GITHUB_ORG"),
            public_url,
            env_nonempty("OCEAN_SURFACE_GITHUB_OAUTH_URL")
                .unwrap_or_else(|| "https://github.com".into()),
            env_nonempty("OCEAN_SURFACE_GITHUB_API_URL")
                .unwrap_or_else(|| "https://api.github.com".into()),
        )
        .map(Some)
    }

    pub(crate) fn new(
        client_id: String,
        client_secret: String,
        org: Option<String>,
        public_url: String,
        oauth_base: String,
        api_base: String,
    ) -> anyhow::Result<Self> {
        let public_url = public_url.trim_end_matches('/').to_string();
        if !(public_url.starts_with("https://") || public_url.starts_with("http://")) {
            anyhow::bail!("OCEAN_SURFACE_PUBLIC_URL must be an http(s) origin, got {public_url:?}");
        }
        Ok(Self {
            client_id,
            client_secret,
            org,
            public_url,
            oauth_base: oauth_base.trim_end_matches('/').to_string(),
            api_base: api_base.trim_end_matches('/').to_string(),
        })
    }

    pub(crate) fn org(&self) -> Option<&str> {
        self.org.as_deref()
    }

    fn redirect_uri(&self) -> String {
        format!("{}{CALLBACK_PATH}", self.public_url)
    }

    fn authorize_url(&self, state: &str) -> String {
        // `read:org` only when membership must be checked: private org
        // membership is invisible without it, and asking for a scope we do not
        // use is a consent screen that teaches people to click through.
        let scope = if self.org.is_some() { "read:org" } else { "" };
        format!(
            "{}/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={}&allow_signup=false",
            self.oauth_base,
            percent_encode_path_segment(&self.client_id),
            percent_encode_path_segment(&self.redirect_uri()),
            percent_encode_path_segment(scope),
            percent_encode_path_segment(state),
        )
    }

    /// Exchange the callback code and return the GitHub account it belongs to,
    /// after the org check when one is configured. The access token lives only
    /// for the length of this call.
    async fn identify(&self, http: &reqwest::Client, code: &str) -> Result<Account, Refusal> {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: Option<String>,
        }
        #[derive(Deserialize)]
        struct UserResponse {
            id: u64,
            login: String,
        }
        #[derive(Deserialize)]
        struct MembershipResponse {
            state: String,
        }

        let response = http
            .post(format!("{}/login/oauth/access_token", self.oauth_base))
            .header(header::ACCEPT, "application/json")
            .header(header::USER_AGENT, USER_AGENT)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", self.redirect_uri().as_str()),
            ])
            .send()
            .await
            .map_err(|e| Refusal::Upstream(format!("token exchange: {e}")))?;
        // A 429 or 5xx is GitHub being unavailable: retryable, not a used code.
        if !response.status().is_success() {
            return Err(Refusal::Upstream(format!(
                "token exchange: {}",
                response.status()
            )));
        }
        let token: TokenResponse = response
            .json()
            .await
            .map_err(|e| Refusal::Upstream(format!("token exchange body: {e}")))?;
        // GitHub answers a bad or reused code with 200 and an `error` field, so
        // absence of a token is the failure signal, not the status.
        let access_token = token.access_token.ok_or(Refusal::CodeRejected)?;

        let api = |path: String| {
            http.get(format!("{}{path}", self.api_base))
                .bearer_auth(&access_token)
                .header(header::ACCEPT, "application/vnd.github+json")
                .header(header::USER_AGENT, USER_AGENT)
        };

        let user = api("/user".into())
            .send()
            .await
            .map_err(|e| Refusal::Upstream(format!("user lookup: {e}")))?;
        if !user.status().is_success() {
            return Err(Refusal::Upstream(format!("user lookup: {}", user.status())));
        }
        let UserResponse { id, login } = user
            .json::<UserResponse>()
            .await
            .map_err(|e| Refusal::Upstream(format!("user body: {e}")))?;

        if let Some(org) = self.org.as_deref() {
            let membership = api(format!(
                "/user/memberships/orgs/{}",
                percent_encode_path_segment(org)
            ))
            .send()
            .await
            .map_err(|e| Refusal::Upstream(format!("org membership: {e}")))?;
            // Only a 404 means "not a member we can see". A 403 is ambiguous —
            // GitHub's primary AND secondary rate limits answer 403 (the
            // secondary one without touching `x-ratelimit-remaining`), and so
            // does an org that has not approved this OAuth app — so it is never
            // read as "go accept the invite": it gets its own retryable answer
            // naming both causes. 429/5xx are GitHub being unavailable. A
            // pending invite answers 200 with state "pending", also not a
            // member yet.
            match membership.status() {
                status if status.is_success() => {}
                reqwest::StatusCode::NOT_FOUND => return Err(Refusal::NotInOrg(login)),
                reqwest::StatusCode::FORBIDDEN => return Err(Refusal::MembershipCheckRefused),
                status => {
                    return Err(Refusal::Upstream(format!("org membership: {status}")));
                }
            }
            let state = membership
                .json::<MembershipResponse>()
                .await
                .map_err(|e| Refusal::Upstream(format!("org membership body: {e}")))?
                .state;
            if state != "active" {
                return Err(Refusal::NotInOrg(login));
            }
        }
        Ok(Account { id, login })
    }
}

/// The GitHub account a callback proved. `login` is for messages only.
struct Account {
    id: u64,
    login: String,
}

#[derive(Debug)]
enum Refusal {
    StateMismatch,
    CodeRejected,
    Denied,
    NotInOrg(String),
    /// GitHub answered the membership check 403: a rate limit, or the org has
    /// not approved this OAuth app. Retryable; never "accept the invite".
    MembershipCheckRefused,
    NotOnRoster(String),
    Upstream(String),
}

fn random_state() -> Option<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn secure_suffix(state: &AppState, headers: &HeaderMap) -> &'static str {
    if request_is_https(headers, state.secure_cookie) {
        "; Secure"
    } else {
        ""
    }
}

/// `GET /auth/github` — send the browser to GitHub's consent screen.
pub(crate) async fn start(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(github) = state.github.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if has_valid_session(&state, &headers) {
        return Redirect::to("/").into_response();
    }
    let Some(nonce) = random_state() else {
        tracing::error!("no randomness for the GitHub OAuth state; refusing to start");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let secure = secure_suffix(&state, &headers);
    let (name, path) = state_cookie(secure);
    let mut response = Redirect::to(&github.authorize_url(&nonce)).into_response();
    no_store(&mut response);
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!(
            "{name}={nonce}; Path={path}; HttpOnly; SameSite=Lax; \
             Max-Age={STATE_MAX_AGE_SECONDS}{secure}"
        )
        .parse()
        .expect("state cookie must be a valid header"),
    );
    response
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Name and path of the state cookie. `__Host-` requires Secure and
/// `Path=/`, so it is only usable over HTTPS; plain-HTTP local development
/// keeps the bare name scoped to the GitHub routes.
fn state_cookie(secure: &str) -> (&'static str, &'static str) {
    if secure.is_empty() {
        (STATE_COOKIE, START_PATH)
    } else {
        (HOST_STATE_COOKIE, "/")
    }
}

/// The roster entry this GitHub account id belongs to.
fn roster_user_for(state: &AppState, id: u64) -> Option<&ProxyUser> {
    state.users.iter().find(|user| user.github_id == Some(id))
}

/// `GET /auth/github/callback` — GitHub's redirect back.
pub(crate) async fn callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(github) = state.github.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let secure = secure_suffix(&state, &headers);
    let (name, path) = state_cookie(secure);
    let clear_state = format!("{name}=; Path={path}; HttpOnly; SameSite=Lax; Max-Age=0{secure}");

    let outcome = async {
        if query.error.is_some() {
            return Err(Refusal::Denied);
        }
        let expected = cookie_value(&headers, name).unwrap_or_default();
        let presented = query.state.as_deref().unwrap_or_default();
        if expected.is_empty() || !constant_time_eq(expected.as_bytes(), presented.as_bytes()) {
            return Err(Refusal::StateMismatch);
        }
        let code = query.code.as_deref().filter(|c| !c.is_empty());
        let code = code.ok_or(Refusal::CodeRejected)?;
        let account = github.identify(&state.http_json, code).await?;
        roster_user_for(&state, account.id)
            .map(|user| (user.username.clone(), user.session_token.clone()))
            .ok_or(Refusal::NotOnRoster(account.login))
    }
    .await;

    let mut response = match outcome {
        Ok((username, token)) => {
            tracing::info!(user = %username, "GitHub sign-in");
            let mut response = landing_html().into_response();
            response.headers_mut().append(
                header::SET_COOKIE,
                format!(
                    "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; \
                     Max-Age={SESSION_MAX_AGE_SECONDS}{secure}"
                )
                .parse()
                .expect("session cookie must be a valid header"),
            );
            response
        }
        Err(refusal) => {
            tracing::warn!(?refusal, "GitHub sign-in refused");
            let (status, message) = refusal_message(&refusal, github.org());
            (status, refusal_html(&message)).into_response()
        }
    };
    response.headers_mut().append(
        header::SET_COOKIE,
        clear_state
            .parse()
            .expect("expired state cookie must be a valid header"),
    );
    no_store(&mut response);
    response
}

/// Both OAuth responses set a cookie; no cache may keep them. The
/// `static_cache_headers` layer says the same for every `/auth/` path — this
/// holds even if that layer is reordered or its classification drifts.
fn no_store(response: &mut Response) {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store, private"),
    );
}

fn refusal_message(refusal: &Refusal, org: Option<&str>) -> (StatusCode, String) {
    match refusal {
        Refusal::Denied => (
            StatusCode::FORBIDDEN,
            "GitHub sign-in was cancelled.".into(),
        ),
        Refusal::StateMismatch | Refusal::CodeRejected => (
            StatusCode::BAD_REQUEST,
            "That sign-in link expired or was already used. Start again.".into(),
        ),
        Refusal::NotInOrg(login) => (
            StatusCode::FORBIDDEN,
            format!(
                "@{} is not an active member of {}. Accept the org invite, then try again.",
                html_escape(login),
                html_escape(org.unwrap_or("the organization"))
            ),
        ),
        Refusal::MembershipCheckRefused => (
            StatusCode::BAD_GATEWAY,
            "GitHub would not confirm your organization membership right now. Try again in a \
             minute; if it keeps happening, the organization may need to approve this app."
                .into(),
        ),
        Refusal::NotOnRoster(login) => (
            StatusCode::FORBIDDEN,
            format!(
                "@{} is not set up on this Ocean yet. Ask the operator to add you.",
                html_escape(login)
            ),
        ),
        Refusal::Upstream(detail) => {
            tracing::warn!(%detail, "GitHub did not answer the sign-in");
            (
                StatusCode::BAD_GATEWAY,
                "GitHub did not answer. Try again in a moment.".into(),
            )
        }
    }
}

fn html_escape(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The callback answers with a page that navigates itself to `/` rather than a
/// 3xx. The callback request was started by github.com, so a redirect chain
/// from it is still cross-site and the browser would withhold the brand-new
/// SameSite=Strict session cookie on the landing — the person would bounce
/// straight back to `/login`. A same-origin meta refresh is a fresh, same-site
/// navigation that carries it.
fn landing_html() -> Html<&'static str> {
    Html(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta http-equiv="refresh" content="0;url=/"><title>Signing in · Ocean</title><style>body{background:#060606;color:#aab2bd;font:15px system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0}</style></head><body><p>Signed in. <a href="/" style="color:#00d7d7">Continue</a></p></body></html>"#,
    )
}

fn refusal_html(message: &str) -> Html<String> {
    Html(format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Sign in · Ocean</title><style>body{{background:#060606;color:#fafcff;font:15px system-ui,sans-serif;display:grid;place-items:center;min-height:100vh;margin:0;padding:24px;box-sizing:border-box}}main{{max-width:380px;padding:32px;border:1px solid #272b31;border-radius:24px;background:#0d0f12}}p{{color:#d6dbe2}}a{{color:#00d7d7}}</style></head><body><main><h1>Ocean</h1><p role="alert">{message}</p><p><a href="/login">Back to sign in</a></p></main></body></html>"#
    ))
}

/// The login page's GitHub affordance, empty when the flow is off.
pub(crate) fn login_button(state: &AppState) -> &'static str {
    if state.github.is_some() {
        r#"<a class="gh" href="/auth/github">Continue with GitHub</a><p class="or">or use a password</p>"#
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;
    use tower::ServiceExt;

    use super::GithubLogin;
    use crate::{
        build_app, AppState, DeviceSelections, ProxyDevice, ProxyUser, SELECTION_CHANGE_BACKLOG,
    };

    /// A stub github.com + api.github.com. `member` decides the org answer.
    async fn stub_github(member: &'static str) -> String {
        let app = Router::new()
            .route(
                "/login/oauth/access_token",
                post(|body: String| async move {
                    if body.contains("code=good") {
                        (
                            StatusCode::OK,
                            Json(json!({"access_token": "gho_stub", "token_type": "bearer"})),
                        )
                    } else if body.contains("code=busy") {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            Json(json!({"error": "rate_limited"})),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            Json(json!({"error": "bad_verification_code"})),
                        )
                    }
                }),
            )
            .route(
                "/user",
                get(|| async { Json(json!({"id": 202, "login": "ecfromthedc"})) }),
            )
            .route(
                "/user/memberships/orgs/{org}",
                get(move || async move {
                    match member {
                        "outage" => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [("x-ratelimit-remaining", "10")],
                            Json(json!({"message": "unavailable"})),
                        ),
                        "none-last-request" => (
                            StatusCode::NOT_FOUND,
                            [("x-ratelimit-remaining", "0")],
                            Json(json!({"message": "Not Found"})),
                        ),
                        "secondary-limit" => (
                            StatusCode::FORBIDDEN,
                            [("x-ratelimit-remaining", "4000")],
                            Json(json!({"message": "You have exceeded a secondary rate limit"})),
                        ),
                        "ratelimited" => (
                            StatusCode::FORBIDDEN,
                            [("x-ratelimit-remaining", "0")],
                            Json(json!({"message": "API rate limit exceeded"})),
                        ),
                        "active" | "pending" => (
                            StatusCode::OK,
                            [("x-ratelimit-remaining", "10")],
                            Json(json!({"state": member})),
                        ),
                        _ => (
                            StatusCode::NOT_FOUND,
                            [("x-ratelimit-remaining", "10")],
                            Json(json!({"message": "Not Found"})),
                        ),
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn roster_user(name: &str, github_id: Option<u64>, token: &str) -> ProxyUser {
        ProxyUser {
            username: name.into(),
            password: None,
            github_id,
            devices: vec![ProxyDevice {
                name: "studio".into(),
                daemon_url: "http://127.0.0.1:1".into(),
                observer_token_path: None,
                operator_key_path: None,
                is_default: true,
            }],
            session_token: token.into(),
        }
    }

    fn state(github_base: Option<&str>, users: Vec<ProxyUser>) -> Arc<AppState> {
        let github = github_base.map(|base| {
            GithubLogin::new(
                "client-123".into(),
                "shh".into(),
                Some("KINGMAKER-SYSTEMS".into()),
                "https://ocean.example.org/".into(),
                base.into(),
                base.into(),
            )
            .unwrap()
        });
        Arc::new(AppState {
            http: reqwest::Client::new(),
            http_json: reqwest::Client::new(),
            http_probe: reqwest::Client::new(),
            device_selections: Arc::new(DeviceSelections::load(PathBuf::from(
                "/nonexistent/ocean-surface-test/device-selections.json",
            ))),
            selection_changes: tokio::sync::broadcast::channel(SELECTION_CHANGE_BACKLOG).0,
            voice_profile: "leo".into(),
            daemon_url: "http://127.0.0.1:1".into(),
            default_livekit_room_id: "project:surface-test".into(),
            tldraw_sync_uri: None,
            maps_key: None,
            maps_map_id: "DEMO_MAP_ID".into(),
            basic_auth: Some(("ocean".into(), "surface".into())),
            session_token: "operator-session".into(),
            users,
            secure_cookie: true,
            observer_token_path: PathBuf::from("/not-used"),
            operator_key_path: PathBuf::from("/not-used"),
            github,
        })
    }

    fn app(state: Arc<AppState>) -> Router {
        let dist = tempfile::tempdir().unwrap();
        build_app(state, dist.path())
    }

    fn set_cookies(response: &axum::response::Response) -> Vec<String> {
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_string())
            .collect()
    }

    async fn get_with_cookie(app: Router, uri: &str, cookie: &str) -> axum::response::Response {
        app.oneshot(
            Request::get(uri)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn start_is_public_and_sends_the_browser_to_github_with_a_lax_state_cookie() {
        let app = app(state(Some("https://gh.test"), Vec::new()));
        let response = app
            .oneshot(Request::get("/auth/github").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response.status().is_redirection(), "{}", response.status());
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, private"
        );
        let location = response.headers()[header::LOCATION].to_str().unwrap();
        assert!(location.starts_with("https://gh.test/login/oauth/authorize?client_id=client-123"));
        assert!(location
            .contains("redirect_uri=https%3A%2F%2Focean.example.org%2Fauth%2Fgithub%2Fcallback"));
        assert!(location.contains("scope=read%3Aorg"));
        let cookies = set_cookies(&response);
        let state_cookie = cookies
            .iter()
            .find(|c| c.starts_with("__Host-ocean_gh_state="))
            .expect("state cookie");
        assert!(state_cookie.contains("SameSite=Lax"), "{state_cookie}");
        assert!(state_cookie.contains("HttpOnly"));
        assert!(state_cookie.contains("Secure"));
        let nonce = state_cookie
            .trim_start_matches("__Host-ocean_gh_state=")
            .split(';')
            .next()
            .unwrap();
        assert!(nonce.len() >= 40);
        assert!(location.contains(&format!("state={nonce}")));
    }

    #[tokio::test]
    async fn github_routes_do_not_exist_when_unconfigured() {
        let app = app(state(None, Vec::new()));
        let response = app
            .clone()
            .oneshot(Request::get("/auth/github").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let login = app
            .oneshot(
                Request::get("/login")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(login.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("/auth/github"));
    }

    #[tokio::test]
    async fn login_page_offers_github_when_configured() {
        let app = app(state(Some("https://gh.test"), Vec::new()));
        let login = app
            .oneshot(Request::get("/login").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(login.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(r#"href="/auth/github""#));
        assert!(body.contains(r#"name="password""#), "password form stays");
    }

    #[tokio::test]
    async fn callback_refuses_a_state_that_does_not_match_the_cookie() {
        let base = stub_github("active").await;
        let app = app(state(
            Some(&base),
            vec![roster_user("ecfromthedc", Some(202), "ec-token")],
        ));
        // The last case is a bare-named cookie with the RIGHT value: under HTTPS
        // only the `__Host-` name counts, and a sibling subdomain can never
        // set that one (it forbids `Domain`), so a planted cookie is ignored.
        for cookie in ["__Host-ocean_gh_state=aaaa", "", "ocean_gh_state=bbbb"] {
            let response = get_with_cookie(
                app.clone(),
                "/auth/github/callback?code=good&state=bbbb",
                cookie,
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "cookie {cookie:?}"
            );
            assert!(!set_cookies(&response)
                .iter()
                .any(|c| c.starts_with("ocean_session=ec-token")));
        }
    }

    #[tokio::test]
    async fn an_active_org_member_on_the_roster_gets_that_entrys_session() {
        let base = stub_github("active").await;
        let app = app(state(
            Some(&base),
            vec![
                roster_user("smaths", Some(101), "smaths-token"),
                roster_user("ecfromthedc", Some(202), "ec-token"),
            ],
        ));
        let response = get_with_cookie(
            app.clone(),
            "/auth/github/callback?code=good&state=nonce-1",
            "__Host-ocean_gh_state=nonce-1",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "no-store, private",
            "a response carrying a session cookie must never be cached"
        );
        let cookies = set_cookies(&response);
        let session = cookies
            .iter()
            .find(|c| c.starts_with("ocean_session="))
            .expect("session cookie");
        // The account id, not roster order, picks the person.
        assert!(session.starts_with("ocean_session=ec-token;"), "{session}");
        assert!(session.contains("SameSite=Strict"));
        assert!(cookies
            .iter()
            .any(|c| c.starts_with("__Host-ocean_gh_state=;") && c.contains("Max-Age=0")));
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&body).contains(r#"http-equiv="refresh" content="0;url=/""#)
        );

        // And that cookie is a real session: /api/config names the person.
        let config = get_with_cookie(app, "/api/config", "ocean_session=ec-token").await;
        assert_eq!(config.status(), StatusCode::OK);
        let body = axum::body::to_bytes(config.into_body(), usize::MAX)
            .await
            .unwrap();
        let config: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(config["user_id"], "ecfromthedc");
    }

    #[tokio::test]
    async fn non_members_pending_invites_and_unknown_accounts_are_refused() {
        // "active" with a different id is the renamed-login case: the stub
        // still answers login "ecfromthedc", but the roster holds another
        // account's id, and the login is never consulted.
        // "none-last-request": a 404 carrying an exhausted quota header is
        // still a real non-member answer, not an outage.
        for (member, roster_id) in [
            ("none", 202),
            ("none-last-request", 202),
            ("pending", 202),
            ("active", 999),
        ] {
            let base = stub_github(member).await;
            let app = app(state(
                Some(&base),
                vec![roster_user("ecfromthedc", Some(roster_id), "ec-token")],
            ));
            let response = get_with_cookie(
                app,
                "/auth/github/callback?code=good&state=n",
                "__Host-ocean_gh_state=n",
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{member}/{roster_id}"
            );
            assert!(!set_cookies(&response)
                .iter()
                .any(|c| c.starts_with("ocean_session=")));
        }
    }

    #[tokio::test]
    async fn a_github_outage_is_a_retryable_502_not_a_membership_refusal() {
        for kind in ["outage", "ratelimited", "secondary-limit"] {
            let base = stub_github(kind).await;
            let app = app(state(
                Some(&base),
                vec![roster_user("ecfromthedc", Some(202), "ec-token")],
            ));
            let response = get_with_cookie(
                app,
                "/auth/github/callback?code=good&state=n",
                "__Host-ocean_gh_state=n",
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY, "{kind}");
        }
    }

    #[tokio::test]
    async fn a_rate_limited_token_exchange_is_a_retryable_502() {
        let base = stub_github("active").await;
        let app = app(state(
            Some(&base),
            vec![roster_user("ecfromthedc", Some(202), "ec-token")],
        ));
        let response = get_with_cookie(
            app,
            "/auth/github/callback?code=busy&state=n",
            "__Host-ocean_gh_state=n",
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn a_rejected_code_is_refused() {
        let base = stub_github("active").await;
        let app = app(state(
            Some(&base),
            vec![roster_user("ecfromthedc", Some(202), "ec-token")],
        ));
        let response = get_with_cookie(
            app,
            "/auth/github/callback?code=reused&state=n",
            "__Host-ocean_gh_state=n",
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_github_only_entry_cannot_use_the_password_form() {
        let app = app(state(
            Some("https://gh.test"),
            vec![roster_user("ecfromthedc", Some(202), "ec-token")],
        ));
        for password in ["", "\u{0}no-password"] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/login")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!(
                            "username=ecfromthedc&password={}",
                            crate::percent_encode_path_segment(password)
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}
