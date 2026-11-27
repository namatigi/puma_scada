//! Cross-site request protection for every route of the gateway.
//!
//! The HMI routes have no authentication (the studio's do, with a session
//! cookie), so the browser itself is the attack surface: any page an operator opens could otherwise `fetch()` a
//! write to `http://<hmi-host>/api/write` (a "simple" request needs no CORS
//! preflight) or open `ws://<hmi-host>/ws` and send writes.
//!
//! - State-changing requests (anything but GET/HEAD/OPTIONS) and WebSocket
//!   upgrades are refused with 403 when they carry an `Origin` that is neither
//!   the server's own (`Host`) nor on the configured allow-list. Browsers always
//!   send `Origin` on such cross-origin requests; tools like curl send none and
//!   are unaffected.
//! - State-changing `/api/` requests with a body must say
//!   `Content-Type: application/json` (415 otherwise), so an old browser that
//!   omits `Origin` still cannot send a form-encoded or text/plain write.

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

/// Origins (besides the server's own) whose pages may change state.
#[derive(Debug, Default)]
pub(crate) struct OriginPolicy {
    allowed: Vec<String>,
}

impl OriginPolicy {
    pub(crate) fn new(allowed: &[String]) -> Self {
        Self { allowed: allowed.iter().map(|o| normalize_origin(o)).filter(|o| !o.is_empty()).collect() }
    }

    /// Whether a request with `origin` sent to `host` comes from an accepted page.
    pub(crate) fn allows(&self, origin: &str, host: Option<&str>) -> bool {
        let origin = normalize_origin(origin);
        if self.allowed.contains(&origin) {
            return true;
        }
        let Some((scheme, authority)) = origin.split_once("://") else {
            // "null" (sandboxed frames, file:// pages) and garbage.
            return false;
        };
        match host {
            Some(host) => strip_default_port(scheme, authority) == strip_default_port(scheme, &host.trim().to_ascii_lowercase()),
            None => false,
        }
    }
}

fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn strip_default_port<'a>(scheme: &str, authority: &'a str) -> &'a str {
    let default = match scheme {
        "http" | "ws" => ":80",
        "https" | "wss" => ":443",
        _ => return authority,
    };
    authority.strip_suffix(default).unwrap_or(authority)
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
}

fn changes_state(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn has_body(headers: &HeaderMap) -> bool {
    if headers.contains_key(header::TRANSFER_ENCODING) {
        return true;
    }
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|n| n > 0)
}

fn is_json(headers: &HeaderMap) -> bool {
    let Some(ct) = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) else { return false };
    let essence = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    essence == "application/json" || (essence.starts_with("application/") && essence.ends_with("+json"))
}

fn refuse(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({"ok": false, "error": error}))).into_response()
}

pub(crate) async fn guard(State(policy): State<Arc<OriginPolicy>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let mutating = changes_state(req.method());
    if mutating || is_upgrade(headers) {
        if let Some(origin) = headers.get(header::ORIGIN) {
            let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
            let ok = origin.to_str().is_ok_and(|o| policy.allows(o, host));
            if !ok {
                tracing::warn!(
                    method = %req.method(),
                    path = req.uri().path(),
                    origin = origin.to_str().unwrap_or("<not text>"),
                    "refused a cross-origin request"
                );
                return refuse(StatusCode::FORBIDDEN, "cross-origin request refused");
            }
        }
    }
    if mutating && req.uri().path().starts_with("/api/") && has_body(headers) && !is_json(headers) {
        return refuse(StatusCode::UNSUPPORTED_MEDIA_TYPE, "request body must be sent as Content-Type: application/json");
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_origin_is_allowed_with_and_without_default_port() {
        let p = OriginPolicy::default();
        assert!(p.allows("http://127.0.0.1:8080", Some("127.0.0.1:8080")));
        assert!(p.allows("HTTP://HMI.local", Some("hmi.local:80")));
        assert!(p.allows("https://hmi.local:443/", Some("hmi.local")));
        assert!(!p.allows("http://127.0.0.1:8081", Some("127.0.0.1:8080")));
        assert!(!p.allows("http://evil.example", Some("127.0.0.1:8080")));
        assert!(!p.allows("null", Some("127.0.0.1:8080")));
        assert!(!p.allows("http://127.0.0.1:8080", None));
    }

    #[test]
    fn allow_list_matches_exact_origins() {
        let p = OriginPolicy::new(&["https://scada.example.com/".into(), "  ".into()]);
        assert!(p.allows("https://SCADA.example.com", Some("10.0.0.5:8080")));
        assert!(!p.allows("https://scada.example.com:8443", Some("10.0.0.5:8080")));
    }

    #[test]
    fn json_content_types() {
        let mut h = HeaderMap::new();
        assert!(!is_json(&h));
        for (ct, ok) in [
            ("application/json", true),
            ("Application/JSON; charset=utf-8", true),
            ("application/merge-patch+json", true),
            ("text/plain", false),
            ("application/x-www-form-urlencoded", false),
            ("multipart/form-data; boundary=x", false),
        ] {
            h.insert(header::CONTENT_TYPE, ct.parse().unwrap());
            assert_eq!(is_json(&h), ok, "{ct}");
        }
    }
}
