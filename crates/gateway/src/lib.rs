//! HTTP/WebSocket gateway. Protocol: `docs/PROTOCOL.md`.
//!
//! ```ignore
//! let project = SharedProject::new(project);          // swap later with project.replace(p)
//! let mut cfg = GatewayConfig::new(project.clone(), store.clone(), "web".into());
//! cfg.extra = historian_routes;                        // merged as-is (historian, alarms, studio)
//! cfg.ws_feeds.push(alarms_rx);                        // watch::Receiver<serde_json::Value>: whole WS messages
//! cfg.reload = Some(reload_rx);                        // watch::Receiver<u64>: bump → {"type":"reload"}
//! cfg.diagnostics = Some(Arc::new(move || driver_status_json()));
//! cfg.shutdown = Some(shutdown_rx);                    // closes WebSocket sessions on shutdown
//! let app: axum::Router = moviclone_gateway::router(cfg);
//! moviclone_gateway::serve(listener, app, shutdown_future).await?;
//! ```
//!
//! Routes: `GET /api/bundle`, `GET /api/values`, `POST /api/write`,
//! `GET /api/diagnostics`, `GET /ws`, static `/hmi/`, `/studio/`, `/shared/`
//! (from `web_root/<name>`), `/` → `/hmi/`.
//!
//! WebSocket sessions send `hello`, then the current value of every `ws_feeds`
//! channel and every later change of it; `subscribe` answers immediately with
//! the current values, then changes are batched every `sampling.publish_ms`
//! with only the latest sample per tag. A client that falls behind the change
//! bus gets all its subscribed values again. `{"type":"reload"}` is sent when
//! [`SharedProject::replace`] is called or the optional `reload` channel changes;
//! `/api/bundle` always serves the current project.
//!
//! Every route (including `extra`) is behind a cross-site guard: state-changing
//! requests and WebSocket upgrades from a page of another origin get 403 unless
//! that origin is in `allowed_origins`, and `/api/` bodies must be JSON (415).
//! Writes to one tag are serialized; a pulse on a tag that is still being
//! written is refused with 409.
//!
//! With [`GatewayConfig::auth`], the HMI routes need a signed-in user: viewers
//! watch, operators also write and act on alarms (see `access`). Without it
//! (tests, embedding) everything is open.

mod access;
mod api;
mod guard;
mod project;
mod statics;
mod writes;
mod ws;

pub use project::SharedProject;

use axum::routing::{get, post};
use axum::Router;
use moviclone_tagdb::TagStore;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Produces the `drivers` array of `GET /api/diagnostics`.
pub type DiagnosticsFn = Arc<dyn Fn() -> Vec<serde_json::Value> + Send + Sync>;

pub struct GatewayConfig {
    pub project: SharedProject,
    pub store: Arc<TagStore>,
    /// Folder holding `hmi/`, `studio/` and `shared/`.
    pub web_root: PathBuf,
    /// Extra routes merged into the gateway (historian, alarms, studio).
    pub extra: Router,
    /// Each holds a complete WebSocket message, e.g. `{"type":"alarms",…}`.
    pub ws_feeds: Vec<watch::Receiver<serde_json::Value>>,
    /// External reload trigger (in addition to [`SharedProject::replace`]).
    pub reload: Option<watch::Receiver<u64>>,
    pub diagnostics: Option<DiagnosticsFn>,
    /// `true` (or a dropped sender) ends every WebSocket session.
    pub shutdown: Option<watch::Receiver<bool>>,
    /// Reported in `hello` and diagnostics.
    pub version: String,
    /// Origins besides the server's own whose pages may write, ack alarms or
    /// open the WebSocket, e.g. `https://scada.example.com` behind a proxy.
    pub allowed_origins: Vec<String>,
    /// Users and sessions: when set, the HMI routes need a signed-in user.
    pub auth: Option<Arc<moviclone_auth::Auth>>,
}

/// Environment variable with a comma-separated list for [`GatewayConfig::allowed_origins`].
pub const ALLOWED_ORIGINS_ENV: &str = "MOVICLONE_ALLOWED_ORIGINS";

/// Parses a comma-separated origin list (as in [`ALLOWED_ORIGINS_ENV`]).
pub fn parse_origins(list: &str) -> Vec<String> {
    list.split(',').map(str::trim).filter(|o| !o.is_empty()).map(str::to_string).collect()
}

impl GatewayConfig {
    pub fn new(project: SharedProject, store: Arc<TagStore>, web_root: PathBuf) -> Self {
        Self {
            project,
            store,
            web_root,
            extra: Router::new(),
            ws_feeds: Vec::new(),
            reload: None,
            diagnostics: None,
            shutdown: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
            allowed_origins: Vec::new(),
            auth: None,
        }
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    project: SharedProject,
    store: Arc<TagStore>,
    ws_feeds: Arc<Vec<watch::Receiver<serde_json::Value>>>,
    reload: Option<watch::Receiver<u64>>,
    diagnostics: Option<DiagnosticsFn>,
    shutdown: Option<watch::Receiver<bool>>,
    started: Instant,
    ws_clients: Arc<AtomicUsize>,
    version: Arc<str>,
    write_locks: Arc<writes::WriteLocks>,
    auth: Option<Arc<moviclone_auth::Auth>>,
}

/// Builds the gateway's router (static files, REST, WebSocket, plus `extra`).
pub fn router(cfg: GatewayConfig) -> Router {
    let state = AppState {
        project: cfg.project,
        store: cfg.store,
        ws_feeds: Arc::new(cfg.ws_feeds),
        reload: cfg.reload,
        diagnostics: cfg.diagnostics,
        shutdown: cfg.shutdown,
        started: Instant::now(),
        ws_clients: Arc::new(AtomicUsize::new(0)),
        version: cfg.version.into(),
        write_locks: Arc::new(writes::WriteLocks::default()),
        auth: cfg.auth.clone(),
    };
    let policy = Arc::new(guard::OriginPolicy::new(&cfg.allowed_origins));
    let api = Router::new()
        .route("/api/bundle", get(api::bundle))
        .route("/api/values", get(api::values))
        .route("/api/write", post(api::write))
        .route("/api/diagnostics", get(api::diagnostics))
        .route("/ws", get(ws::upgrade))
        .with_state(state);
    let mut app = statics::routes(cfg.web_root).merge(api).merge(cfg.extra);
    if let Some(auth) = cfg.auth {
        app = app.merge(moviclone_auth::routes(auth.clone())).layer(axum::middleware::from_fn_with_state(auth, access::guard));
    }
    // Outermost: a cross-site request is refused before anything else looks at it.
    app.layer(axum::middleware::from_fn_with_state(policy, guard::guard))
}

/// Serves `app` on `listener` until `shutdown` completes. Handlers can read the
/// client address (`ConnectInfo<SocketAddr>`), e.g. the studio's loopback-only setup.
pub async fn serve(listener: TcpListener, app: Router, shutdown: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}
