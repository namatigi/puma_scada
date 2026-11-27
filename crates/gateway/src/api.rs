//! REST endpoints: bundle, values, write, diagnostics.

use crate::writes::WriteLocks;
use crate::AppState;
use axum::body::Bytes;
use axum::extract::{Extension, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use moviclone_model::WriteMode;
use moviclone_tagdb::{Quality, Sample, TagStore, Value, WriteError};
use serde::Deserialize;
use serde_json::json;

/// `{ "<tag>": {"v":…,"q":…,"ts":…}, … }` for the given tags.
pub(crate) fn values_json(store: &TagStore, ids: impl Iterator<Item = moviclone_tagdb::TagId>) -> serde_json::Map<String, serde_json::Value> {
    ids.map(|id| (store.name(id).to_string(), sample_json(&store.get(id)))).collect()
}

pub(crate) fn sample_json(s: &Sample) -> serde_json::Value {
    serde_json::to_value(s).unwrap_or(serde_json::Value::Null)
}

pub(crate) async fn bundle(State(st): State<AppState>) -> Response {
    (
        [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-cache")],
        st.project.json(),
    )
        .into_response()
}

pub(crate) async fn values(State(st): State<AppState>) -> Response {
    let map = values_json(&st.store, st.store.ids());
    ([(header::CACHE_CONTROL, "no-store")], Json(serde_json::Value::Object(map))).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteBody {
    pub tag: String,
    pub value: serde_json::Value,
    #[serde(default)]
    pub mode: WriteMode,
}

/// A write failure with the HTTP status that describes it.
pub(crate) struct Failure(pub StatusCode, pub String);

pub(crate) async fn execute_write(
    store: &TagStore,
    locks: &WriteLocks,
    tag: &str,
    value: serde_json::Value,
    mode: WriteMode,
) -> Result<(), Failure> {
    let id = store.id(tag).ok_or_else(|| Failure(StatusCode::NOT_FOUND, WriteError::UnknownTag(tag.into()).to_string()))?;
    let value: Value = serde_json::from_value(value)
        .map_err(|_| Failure(StatusCode::BAD_REQUEST, "value must be null, a boolean, a number or a string".into()))?;
    // Held until the write (for a pulse: write, wait, restore) is complete.
    let _guard = if mode == WriteMode::Pulse {
        locks
            .try_lock(id)
            .ok_or_else(|| Failure(StatusCode::CONFLICT, format!("a write to {tag} is still in progress")))?
    } else {
        locks.lock(id).await
    };
    store.write_mode(id, value, mode).await.map_err(|e| {
        let status = match e {
            WriteError::UnknownTag(_) => StatusCode::NOT_FOUND,
            WriteError::ReadOnly(_) => StatusCode::FORBIDDEN,
            WriteError::BadValue(_) => StatusCode::BAD_REQUEST,
            WriteError::NoDriver(_) => StatusCode::SERVICE_UNAVAILABLE,
            WriteError::Device(_) => StatusCode::BAD_GATEWAY,
            WriteError::Timeout => StatusCode::GATEWAY_TIMEOUT,
        };
        Failure(status, e.to_string())
    })
}

pub(crate) async fn write(State(st): State<AppState>, actor: Option<Extension<moviclone_auth::Actor>>, body: Bytes) -> Response {
    let body: WriteBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(e) => return (StatusCode::BAD_REQUEST, Json(json!({"ok": false, "error": format!("invalid request: {e}")}))).into_response(),
    };
    let user = actor.map(|Extension(a)| a.0);
    tracing::info!(tag = %body.tag, mode = ?body.mode, user = user.as_deref().unwrap_or("-"), "write via REST");
    match execute_write(&st.store, &st.write_locks, &body.tag, body.value, body.mode).await {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(Failure(status, error)) => (status, Json(json!({"ok": false, "error": error}))).into_response(),
    }
}

pub(crate) async fn diagnostics(State(st): State<AppState>) -> Response {
    let drivers = st.diagnostics.as_ref().map(|f| f()).unwrap_or_default();
    let (mut good, mut uncertain, mut bad) = (0usize, 0usize, 0usize);
    for id in st.store.ids() {
        match st.store.get(id).quality {
            Quality::Good => good += 1,
            Quality::Uncertain => uncertain += 1,
            Quality::Bad => bad += 1,
        }
    }
    let body = json!({
        "drivers": drivers,
        "uptime_s": st.started.elapsed().as_secs(),
        "tags": {"total": st.store.len(), "good": good, "uncertain": uncertain, "bad": bad},
        "ws_clients": st.ws_clients.load(std::sync::atomic::Ordering::Relaxed),
        "version": &*st.version,
    });
    ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}
