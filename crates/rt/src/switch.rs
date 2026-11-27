//! A router that can be swapped while the server keeps listening.
//!
//! Requests (including WebSocket upgrades: the request keeps its upgrade
//! extension) are handed to whatever router is current when they arrive.
//! Requests already in flight finish on the router they started on.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use std::sync::{Arc, RwLock};
use tower::ServiceExt;

#[derive(Clone)]
pub struct RouterSwitch {
    current: Arc<RwLock<Router>>,
}

impl RouterSwitch {
    pub fn new(router: Router) -> Self {
        Self { current: Arc::new(RwLock::new(router)) }
    }

    /// A switch that answers 503 until [`RouterSwitch::set`] is called.
    pub fn unavailable(what: &'static str) -> Self {
        Self::new(unavailable_router(what))
    }

    /// Routes new requests to `router` from now on.
    pub fn set(&self, router: Router) {
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = router;
    }

    /// Replaces the current router with one answering 503 (e.g. while a
    /// service restarts), dropping the previous one.
    pub fn set_unavailable(&self, what: &'static str) {
        self.set(unavailable_router(what));
    }

    fn current(&self) -> Router {
        self.current.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub async fn dispatch(&self, req: Request) -> Response {
        match self.current().oneshot(req).await {
            Ok(resp) => resp,
            Err(never) => match never {},
        }
    }

    /// A router sending every request to the current router.
    pub fn router(&self) -> Router {
        let me = self.clone();
        Router::new().fallback(move |req: Request| {
            let me = me.clone();
            async move { me.dispatch(req).await }
        })
    }

    /// A router sending only requests under `prefixes` (each `"/api/x"` covers
    /// `/api/x` and `/api/x/…`) to the current router. Unlike [`Self::router`]
    /// it has no fallback, so it can be merged into another router. Request
    /// extensions are not passed on (no WebSocket upgrades on these routes),
    /// except the signed-in user.
    pub fn routes_for(&self, prefixes: &[&str]) -> Router {
        let mut r = Router::new();
        for p in prefixes {
            for path in [p.to_string(), format!("{p}/*rest")] {
                let me = self.clone();
                r = r.route(
                    &path,
                    any(move |req: Request| {
                        let me = me.clone();
                        async move { me.dispatch(without_extensions(req)).await }
                    }),
                );
            }
        }
        r
    }
}

/// The outer route's path parameters live in the request extensions, and the
/// inner router would see them next to its own ("wrong number of path
/// arguments"). The prefixed routes are plain REST, so nothing else in the
/// extensions is needed downstream but who made the request.
fn without_extensions(req: Request) -> Request {
    let (mut parts, body) = req.into_parts();
    let mut kept = axum::http::Extensions::new();
    // Who is signed in (set by the gateway's access check), for the alarm journal.
    if let Some(actor) = parts.extensions.get::<moviclone_auth::Actor>() {
        kept.insert(actor.clone());
    }
    if let Some(signed) = parts.extensions.get::<moviclone_auth::Signed>() {
        kept.insert(signed.clone());
    }
    parts.extensions = kept;
    Request::from_parts(parts, body)
}

fn unavailable_router(what: &'static str) -> Router {
    Router::new().fallback(move || async move {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"ok": false, "error": format!("{what} is restarting")})))
            .into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::routing::get;

    async fn status_and_body(r: &Router, path: &str) -> (u16, String) {
        let resp = r.clone().oneshot(Request::builder().uri(path).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn swaps_the_router_for_new_requests() {
        let sw = RouterSwitch::new(Router::new().route("/a", get(|| async { "one" })));
        let outer = sw.router();
        assert_eq!(status_and_body(&outer, "/a").await, (200, "one".into()));
        sw.set(Router::new().route("/a", get(|| async { "two" })));
        assert_eq!(status_and_body(&outer, "/a").await, (200, "two".into()));
        sw.set_unavailable("thing");
        let (status, body) = status_and_body(&outer, "/a").await;
        assert_eq!(status, 503);
        assert!(body.contains("thing is restarting"), "{body}");
    }

    #[tokio::test]
    async fn prefix_routes_merge_into_another_router() {
        let sw = RouterSwitch::new(
            Router::new()
                .route("/api/x", get(|| async { "root" }))
                .route("/api/x/:a/path", get(|axum::extract::Path(a): axum::extract::Path<String>| async move { a })),
        );
        let app = Router::new().route("/other", get(|| async { "other" })).merge(sw.routes_for(&["/api/x"]));
        assert_eq!(status_and_body(&app, "/api/x").await, (200, "root".into()));
        assert_eq!(status_and_body(&app, "/api/x/deep/path").await, (200, "deep".into()), "inner path parameters only");
        assert_eq!(status_and_body(&app, "/other").await, (200, "other".into()));
        assert_eq!(status_and_body(&app, "/api/y").await.0, 404);
    }
}
