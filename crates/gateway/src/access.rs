//! Sign-in for the HMI routes (with [`crate::GatewayConfig::auth`]).
//!
//! | Request | Needs |
//! |---|---|
//! | static files (`/hmi/`, `/studio/`, `/shared/`) | nothing: the pages show the sign-in |
//! | `/api/auth/…`, `/api/studio/…` | checked by those routes themselves |
//! | `GET /api/…`, `GET /ws` | viewer (or nobody, with guest viewing) |
//! | any other method on `/api/…` (writes, alarm actions) | operator |
//!
//! WebSocket writes are checked again on every write ([`ws`](crate::ws)), and
//! an open socket is closed when its session ends.
//!
//! The idle timeout runs on activity: page loads (`/api/bundle`, `/ws`),
//! writes and alarm actions. Polling (values, history, alarms lists) does not
//! keep a session open, so an unattended HMI signs out.

use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use moviclone_auth::{Actor, Auth, AuthError, Role};
use std::sync::Arc;

/// The role a request needs, or `None` when this layer does not check it.
pub(crate) fn needed_role(method: &Method, path: &str) -> Option<Role> {
    if !(path.starts_with("/api/") || path == "/ws") {
        return None;
    }
    if path.starts_with("/api/auth/") || path == "/api/auth" || path.starts_with("/api/studio/") {
        return None;
    }
    Some(if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) { Role::Viewer } else { Role::Operator })
}

/// Whether the request counts as activity for the idle timeout.
fn is_activity(method: &Method, path: &str) -> bool {
    !matches!(*method, Method::GET | Method::HEAD) || path == "/ws" || path == "/api/bundle"
}

pub(crate) async fn guard(State(auth): State<Arc<Auth>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let Some(need) = needed_role(req.method(), &path) else {
        return next.run(req).await;
    };
    let signed = match moviclone_auth::cookie(req.headers()) {
        Some(token) => auth.check(&token, is_activity(req.method(), &path)),
        None => Err(AuthError::Unauthorized("sign in first".into())),
    };
    match signed {
        Ok(me) => {
            if let Err(e) = me.require(need, if need == Role::Viewer { "watching" } else { "operating (writes, alarm actions)" }) {
                tracing::warn!(user = %me.name, role = %me.role, method = %req.method(), path, "refused: role too low");
                return e.into_response();
            }
            req.extensions_mut().insert(Actor(me.name.clone()));
            req.extensions_mut().insert(me);
            next.run(req).await
        }
        Err(_) if need == Role::Viewer && auth.guest_view() => next.run(req).await,
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_per_route() {
        assert_eq!(needed_role(&Method::GET, "/hmi/index.html"), None);
        assert_eq!(needed_role(&Method::GET, "/"), None);
        assert_eq!(needed_role(&Method::POST, "/api/auth/session"), None);
        assert_eq!(needed_role(&Method::PUT, "/api/studio/tags"), None);
        assert_eq!(needed_role(&Method::GET, "/api/values"), Some(Role::Viewer));
        assert_eq!(needed_role(&Method::GET, "/ws"), Some(Role::Viewer));
        assert_eq!(needed_role(&Method::GET, "/api/history"), Some(Role::Viewer));
        assert_eq!(needed_role(&Method::POST, "/api/write"), Some(Role::Operator));
        assert_eq!(needed_role(&Method::POST, "/api/alarms/X/ack"), Some(Role::Operator));
        assert_eq!(needed_role(&Method::POST, "/api/alarms/ack-all"), Some(Role::Operator));
        assert!(is_activity(&Method::POST, "/api/write"));
        assert!(is_activity(&Method::GET, "/api/bundle"));
        assert!(!is_activity(&Method::GET, "/api/history"));
    }
}
