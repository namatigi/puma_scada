//! HTTP routes from docs/PROTOCOL.md.
//!
//! POST bodies are optional JSON. With sign-in on, the journal records the
//! signed-in user (the gateway's [`Actor`]) and a `user` in the body is
//! ignored; without it, `{"user":"op1"}` / `{"minutes":30,"user":"op1"}` name
//! the user.

use crate::engine::{AlarmEngine, AlarmError};
use axum::body::Bytes;
use axum::extract::{Extension, Path, Query, State};
use moviclone_auth::Actor;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::time::Duration;

const DEFAULT_JOURNAL_LIMIT: usize = 200;
const MAX_JOURNAL_LIMIT: usize = 10_000;

/// `GET /api/alarms`, `POST /api/alarms/{name}/ack|shelve|unshelve`,
/// `POST /api/alarms/ack-all`, `GET /api/alarms/journal`.
pub fn routes(engine: Arc<AlarmEngine>) -> Router {
    Router::new()
        .route("/api/alarms", get(list))
        .route("/api/alarms/journal", get(journal))
        .route("/api/alarms/ack-all", post(ack_all))
        .route("/api/alarms/:name/ack", post(ack))
        .route("/api/alarms/:name/shelve", post(shelve))
        .route("/api/alarms/:name/unshelve", post(unshelve))
        .with_state(engine)
}

type Engine = State<Arc<AlarmEngine>>;

#[derive(Deserialize, Default)]
struct UserBody {
    user: Option<String>,
}

#[derive(Deserialize)]
struct ShelveBody {
    minutes: f64,
    user: Option<String>,
}

#[derive(Deserialize)]
struct JournalQuery {
    limit: Option<usize>,
}

async fn list(State(e): Engine) -> Json<serde_json::Value> {
    Json(json!({ "alarms": e.states() }))
}

async fn journal(State(e): Engine, Query(q): Query<JournalQuery>) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(DEFAULT_JOURNAL_LIMIT).min(MAX_JOURNAL_LIMIT);
    Json(json!({ "events": e.journal(limit) }))
}

/// Who acted: the signed-in user when sign-in is on (the gateway sets
/// [`Actor`]), otherwise the `user` the client sent, if any.
fn who(actor: Option<Extension<Actor>>, sent: Option<String>) -> Option<String> {
    actor.map(|Extension(a)| a.0).or(sent)
}

async fn ack(State(e): Engine, actor: Option<Extension<Actor>>, Path(name): Path<String>, body: Bytes) -> Response {
    let body: UserBody = match optional_body(&body) {
        Ok(b) => b.unwrap_or_default(),
        Err(r) => return r,
    };
    result(e.ack(&name, who(actor, body.user).as_deref()).map(|_| ()))
}

async fn ack_all(State(e): Engine, actor: Option<Extension<Actor>>, body: Bytes) -> Response {
    let body: UserBody = match optional_body(&body) {
        Ok(b) => b.unwrap_or_default(),
        Err(r) => return r,
    };
    let acked = e.ack_all(who(actor, body.user).as_deref());
    Json(json!({ "ok": true, "acked": acked })).into_response()
}

async fn shelve(State(e): Engine, actor: Option<Extension<Actor>>, Path(name): Path<String>, body: Bytes) -> Response {
    let body: ShelveBody = match optional_body(&body) {
        Ok(Some(b)) => b,
        Ok(None) => return error(StatusCode::BAD_REQUEST, "body {\"minutes\":N} is required".into()),
        Err(r) => return r,
    };
    if !body.minutes.is_finite() || body.minutes <= 0.0 {
        return error(StatusCode::BAD_REQUEST, format!("minutes must be a positive number, got {}", body.minutes));
    }
    let duration = Duration::from_secs_f64((body.minutes * 60.0).min(1e9));
    result(e.shelve(&name, duration, who(actor, body.user).as_deref()))
}

async fn unshelve(State(e): Engine, actor: Option<Extension<Actor>>, Path(name): Path<String>, body: Bytes) -> Response {
    let body: UserBody = match optional_body(&body) {
        Ok(b) => b.unwrap_or_default(),
        Err(r) => return r,
    };
    result(e.unshelve(&name, who(actor, body.user).as_deref()))
}

/// Parses an optional JSON body; an empty body is `None`, so a body-less
/// `fetch(url, {method:"POST"})` works. Behind the gateway, a non-empty body
/// must be sent as `application/json` and cross-site pages are refused (the
/// gateway's guard), so these routes cannot be driven by a form on another site.
#[allow(clippy::result_large_err)]
fn optional_body<T: DeserializeOwned>(bytes: &Bytes) -> Result<Option<T>, Response> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(bytes).map(Some).map_err(|e| error(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}")))
}

fn result(r: Result<(), AlarmError>) -> Response {
    match r {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e @ AlarmError::UnknownAlarm(_)) => error(StatusCode::NOT_FOUND, e.to_string()),
        Err(e @ AlarmError::InvalidDuration(_)) => error(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

fn error(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}
