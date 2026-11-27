//! Studio (editor) backend. Protocol: `docs/PROTOCOL.md`.
//!
//! ```ignore
//! let (reload_tx, reload_rx) = tokio::sync::watch::channel(0u64);
//! let app = axum::Router::new().merge(moviclone_studio::routes(project_dir, reload_tx));
//! // reload_rx changes after every successful save -> runtime reloads, clients get {"type":"reload"}
//! ```
//!
//! | Route | Writes |
//! |---|---|
//! | `GET /api/studio/project` | – returns `{"project":Project|null,"diagnostics":[…]}` |
//! | `PUT /api/studio/screens/{name}` body Screen | `screens/<name>.toml` |
//! | `DELETE /api/studio/screens/{name}` | removes the file it was loaded from |
//! | `PUT /api/studio/tags` body `[Tag…]` | `tags.toml` |
//! | `PUT /api/studio/alarms` body `[Alarm…]` | `alarms.toml` (removed when empty) |
//! | `PUT /api/studio/meta` body ProjectMeta | `project.toml` |
//!
//! Every body is deserialized into the model types before anything is written,
//! so a save can never produce a file the runtime refuses to load. Saves answer
//! `{"ok":true,"file":"…","diagnostics":[…]}` with the whole project re-validated;
//! refusals answer 4xx with `{"ok":false,"error":"…"}`. A project with errors is
//! still saved (engineers save work in progress); the diagnostics say what is wrong.
//! Screen names that differ only in letter case are refused with 409, because
//! they would share one file on Windows (see `screen_files`).

mod diag;
mod files;
mod names;
mod screen_files;

pub use diag::{snapshot, DiagnosticJson, Snapshot, LOAD_ERROR_CODE};
pub use files::write_atomic;
pub use names::check_screen_name;
pub use screen_files::{file_for_delete, file_for_save, Refusal};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use moviclone_model::{Alarm, ProjectMeta, Screen, Tag};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Largest accepted request body. Petrogas' biggest screen is ~40 kB as JSON.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    root: Arc<PathBuf>,
    reload: Arc<watch::Sender<u64>>,
    /// One save at a time: each save reads the folder back for diagnostics,
    /// and two interleaved saves would report each other's half-state.
    write_lock: Arc<Mutex<()>>,
}

/// The studio API, rooted at `project_dir`. `reload_tx` is bumped after every
/// successful save so the runtime can reload the project.
pub fn routes(project_dir: PathBuf, reload_tx: watch::Sender<u64>) -> Router {
    let state = AppState {
        root: Arc::new(project_dir),
        reload: Arc::new(reload_tx),
        write_lock: Arc::new(Mutex::new(())),
    };
    Router::new()
        .route("/api/studio/project", get(get_project))
        .route("/api/studio/screens/:name", put(put_screen).delete(delete_screen))
        .route("/api/studio/tags", put(put_tags))
        .route("/api/studio/alarms", put(put_alarms))
        .route("/api/studio/meta", put(put_meta))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

// ---------------------------------------------------------------- errors

#[derive(Debug)]
enum ApiError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Internal(m) => {
                tracing::error!("studio: {m}");
                (StatusCode::INTERNAL_SERVER_ERROR, m)
            }
        };
        (status, Json(json!({ "ok": false, "error": msg }))).into_response()
    }
}

fn parse_body<T: DeserializeOwned>(body: &Bytes, what: &str) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError::BadRequest(format!("invalid {what}: {e}")))
}

/// Runs a save on the blocking pool under the write lock, then re-validates
/// the project and bumps the reload counter.
async fn save<F>(state: AppState, what: &'static str, op: F) -> Result<Json<serde_json::Value>, ApiError>
where
    F: FnOnce(&Path) -> Result<PathBuf, ApiError> + Send + 'static,
{
    let root = state.root.clone();
    let lock = state.write_lock.clone();
    let (file, snap) = tokio::task::spawn_blocking(move || {
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let file = op(&root)?;
        Ok::<_, ApiError>((diag::relative(&root, &file), snapshot(&root)))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("save task failed: {e}")))??;

    state.reload.send_modify(|n| *n = n.wrapping_add(1));
    let errors = snap.diagnostics.iter().filter(|d| d.severity == "error").count();
    tracing::info!(file = %file, what, errors, diagnostics = snap.diagnostics.len(), "studio saved");
    Ok(Json(json!({ "ok": true, "file": file, "diagnostics": snap.diagnostics })))
}

// ---------------------------------------------------------------- handlers

async fn get_project(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let root = state.root.clone();
    let snap = tokio::task::spawn_blocking(move || snapshot(&root))
        .await
        .map_err(|e| ApiError::Internal(format!("load task failed: {e}")))?;
    let project = snap.loaded.map(|(p, _)| p);
    Ok(Json(json!({ "project": project, "diagnostics": snap.diagnostics })))
}

impl From<Refusal> for ApiError {
    fn from(r: Refusal) -> Self {
        match r {
            Refusal::Conflict(m) => ApiError::Conflict(m),
            Refusal::NotFound(m) => ApiError::NotFound(m),
        }
    }
}

async fn put_screen(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_screen_name(&name).map_err(ApiError::BadRequest)?;
    let screen: Screen = parse_body(&body, "screen")?;
    if screen.name != name {
        return Err(ApiError::BadRequest(format!(
            "screen name in the body ({:?}) differs from the URL ({name:?}); rename by saving the new name and deleting the old one",
            screen.name
        )));
    }
    if !(screen.width.is_finite() && screen.height.is_finite() && screen.width > 0.0 && screen.height > 0.0) {
        return Err(ApiError::BadRequest("screen width and height must be positive numbers".into()));
    }
    save(state, "screen", move |root| {
        let path = file_for_save(root, &name)?;
        files::write_screen(&path, &screen).map_err(ApiError::Internal)?;
        Ok(path)
    })
    .await
}

async fn delete_screen(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_screen_name(&name).map_err(ApiError::BadRequest)?;
    save(state, "screen deleted", move |root| {
        let path = file_for_delete(root, &name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(ApiError::NotFound(format!("screen {name:?} does not exist")))
            }
            Err(e) => Err(ApiError::Internal(format!("cannot remove {}: {e}", path.display()))),
        }
    })
    .await
}

async fn put_tags(State(state): State<AppState>, body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let tags: Vec<Tag> = parse_body(&body, "tag list")?;
    save(state, "tags", move |root| {
        files::write_tags(root, tags).map_err(ApiError::Internal)?;
        Ok(files::tags_file(root))
    })
    .await
}

async fn put_alarms(State(state): State<AppState>, body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let alarms: Vec<Alarm> = parse_body(&body, "alarm list")?;
    save(state, "alarms", move |root| {
        files::write_alarms(root, alarms).map_err(ApiError::Internal)?;
        Ok(files::alarms_file(root))
    })
    .await
}

async fn put_meta(State(state): State<AppState>, body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let meta: ProjectMeta = parse_body(&body, "project settings")?;
    if meta.name.trim().is_empty() {
        return Err(ApiError::BadRequest("project name is empty".into()));
    }
    save(state, "project settings", move |root| {
        files::write_meta(root, &meta).map_err(ApiError::Internal)?;
        Ok(files::project_file(root))
    })
    .await
}
