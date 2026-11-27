//! HTTP routes from docs/PROTOCOL.md:
//!
//! - `GET /api/history?tags=A,B&from=MS&to=MS&points=600`
//! - `GET /api/history/table?source=FM01&from=MS&to=MS&limit=500[&kind=historian|logger]`
//!
//! `to` defaults to now, `from` to one hour before `to`. Queries run on the
//! blocking pool so a long range does not stall the WebSocket.

use crate::historian::{Historian, QueryError, SourceKind};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

const DEFAULT_SPAN_MS: i64 = 3_600_000;
const DEFAULT_POINTS: usize = 600;
const MAX_POINTS: usize = 20_000;
const DEFAULT_LIMIT: usize = 500;
const MAX_LIMIT: usize = 50_000;
const MAX_TAGS: usize = 64;

pub fn routes(h: Arc<Historian>) -> Router {
    Router::new().route("/api/history", get(history)).route("/api/history/table", get(table)).with_state(h)
}

#[derive(Deserialize)]
struct HistoryQuery {
    tags: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    points: Option<usize>,
}

#[derive(Deserialize)]
struct TableQuery {
    source: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    limit: Option<usize>,
    kind: Option<String>,
}

async fn history(State(h): State<Arc<Historian>>, Query(q): Query<HistoryQuery>) -> Response {
    let tags: Vec<String> = q.tags.as_deref().unwrap_or("").split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect();
    if tags.is_empty() {
        return bad_request("tags is required, e.g. ?tags=A,B".into());
    }
    if tags.len() > MAX_TAGS {
        return bad_request(format!("at most {MAX_TAGS} tags per request"));
    }
    let (from, to) = match range(&h, q.from, q.to) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let points = q.points.unwrap_or(DEFAULT_POINTS).clamp(2, MAX_POINTS);
    let result = tokio::task::spawn_blocking(move || h.query(&tags, from, to, points)).await;
    match result {
        Ok(Ok(series)) => Json(json!({ "series": series })).into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

async fn table(State(h): State<Arc<Historian>>, Query(q): Query<TableQuery>) -> Response {
    let Some(source) = q.source.filter(|s| !s.is_empty()) else {
        return bad_request("source is required, e.g. ?source=FM01".into());
    };
    let kind = match q.kind.as_deref() {
        None | Some("") => None,
        Some("historian") => Some(SourceKind::Historian),
        Some("logger") => Some(SourceKind::Logger),
        Some(other) => return bad_request(format!("kind must be historian or logger, got {other:?}")),
    };
    let (from, to) = match range(&h, q.from, q.to) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let result = tokio::task::spawn_blocking(move || h.table(&source, kind, from, to, limit)).await;
    match result {
        Ok(Ok(table)) => Json(table).into_response(),
        Ok(Err(e)) => match e.downcast_ref::<QueryError>() {
            Some(q @ QueryError::UnknownSource(_)) => (StatusCode::NOT_FOUND, Json(json!({ "error": q.to_string() }))).into_response(),
            None => internal(e),
        },
        Err(e) => internal(e.into()),
    }
}

#[allow(clippy::result_large_err)]
fn range(h: &Historian, from: Option<i64>, to: Option<i64>) -> Result<(i64, i64), Response> {
    let to = to.unwrap_or_else(|| h.now_ms());
    let from = from.unwrap_or(to.saturating_sub(DEFAULT_SPAN_MS));
    if from > to {
        return Err(bad_request(format!("from ({from}) is after to ({to})")));
    }
    Ok((from, to))
}

fn bad_request(message: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

fn internal(e: anyhow::Error) -> Response {
    tracing::error!(error = %e, "history query failed");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": "history query failed; see the runtime log" }))).into_response()
}
