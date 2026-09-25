//! Coding plans — the proxy half of web identity M3.
//!
//! Forwards the daemon's operator-only `/v1/auth/providers*` routes (contract:
//! §9 of `ocean-os/docs/specs/2026-09-25-ocean-web-identity-and-node-linking-program.md`)
//! for the device the signed-in person is attached to, attaching that
//! device's room-operator key server side. The browser never sees the key, and
//! the key it gets is only ever the one belonging to a machine on ITS OWN
//! roster: `credentials_for_device` hands the process-default key only to the
//! process-default daemon, so one person cannot sign another's plans in or out.
//!
//! Like the rooms allowlist this is exact shapes, not a passthrough:
//!
//! - `GET    /v1/auth/providers`
//! - `POST   /v1/auth/providers/{provider}/login`
//! - `GET    /v1/auth/providers/{provider}/login/{attempt_id}`
//! - `DELETE /v1/auth/providers/{provider}/login/{attempt_id}`
//! - `POST   /v1/auth/providers/{provider}/logout`
//!
//! The upstream request is built fresh, so the browser's Cookie, Origin and
//! Referer never reach the daemon — the daemon refuses a request that carries
//! them, by rule.

use axum::{
    body::Bytes,
    extract::Request,
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
};

use crate::{
    auth_off_room_mutation_source_allowed, device_unreachable, has_dot_segment,
    read_room_operator_key, room_operator_key_path, AppState, ResolvedDaemon,
};

/// Every one of these requests is bodiless; anything larger is not ours.
const BODY_LIMIT: usize = 4096;

fn is_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `method path` is one of the five routes above.
pub(crate) fn allowed(method: &Method, path: &str) -> bool {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segments.len() < 3 || segments[..3] != ["v1", "auth", "providers"] {
        return false;
    }
    match (method, &segments[3..]) {
        (&Method::GET, []) => true,
        (&Method::POST, [provider, "login"]) => is_segment(provider),
        (&Method::POST, [provider, "logout"]) => is_segment(provider),
        (&Method::GET | &Method::DELETE, [provider, "login", attempt]) => {
            is_segment(provider) && is_segment(attempt)
        }
        _ => false,
    }
}

fn json_error(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        Bytes::from(format!(r#"{{"ok":false,"error":"{code}"}}"#)),
    )
        .into_response()
}

/// Called from `main.rs`'s `proxy_coding_plans` with the device the auth
/// gate resolved, so this lane takes its upstream from the same one resolver
/// every other daemon route does.
pub(crate) async fn forward(state: &AppState, daemon: ResolvedDaemon, req: Request) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    if has_dot_segment(&path) || !allowed(&method, &path) {
        return json_error(StatusCode::NOT_FOUND, "unknown_route");
    }
    // With login off (trusted localhost only) there is no session to tell the
    // operator's own page from a cross-site one, so browser-sourced requests
    // must name this exact loopback host — the rooms authority rule.
    if state.basic_auth.is_none() && !auth_off_room_mutation_source_allowed(req.headers()) {
        return json_error(StatusCode::FORBIDDEN, "cross_site_operator_request_refused");
    }
    let Some(key_path) = room_operator_key_path(&daemon) else {
        tracing::warn!(device = %daemon.device, "coding plans: no operator key for device");
        return json_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "operator_credential_unavailable",
        );
    };
    let key = match read_room_operator_key(&key_path) {
        Ok(key) => key,
        Err(error) => {
            tracing::warn!(%error, device = %daemon.device, "coding plans: operator key unreadable");
            return json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "operator_credential_unavailable",
            );
        }
    };
    if axum::body::to_bytes(req.into_body(), BODY_LIMIT)
        .await
        .is_err()
    {
        return json_error(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large");
    }

    let url = format!("{}{path}", daemon.base());
    let upstream = state
        .http_json
        .request(method, &url)
        .header("X-Ocean-Operator", key);
    match upstream.send().await {
        Ok(resp) => {
            let status =
                StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
        }
        Err(err) => device_unreachable(&daemon, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::allowed;
    use axum::http::Method;

    #[test]
    fn only_the_five_contract_shapes_are_forwarded() {
        for (method, path) in [
            (Method::GET, "/v1/auth/providers"),
            (Method::POST, "/v1/auth/providers/claude/login"),
            (Method::POST, "/v1/auth/providers/codex/logout"),
            (Method::GET, "/v1/auth/providers/claude/login/0123abcd"),
            (Method::DELETE, "/v1/auth/providers/claude/login/0123abcd"),
        ] {
            assert!(allowed(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::POST, "/v1/auth/providers"),
            (Method::DELETE, "/v1/auth/providers/claude/logout"),
            (Method::GET, "/v1/auth/providers/claude/login"),
            (Method::POST, "/v1/auth/providers/claude/login/x/y"),
            (Method::POST, "/v1/auth/providers/cl%2Fa/login"),
            (Method::POST, "/v1/auth/providers//login"),
            (Method::GET, "/v1/auth/other"),
            (Method::PUT, "/v1/auth/providers/claude/login/abc"),
        ] {
            assert!(!allowed(&method, path), "{method} {path}");
        }
    }
}
