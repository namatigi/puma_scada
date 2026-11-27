//! Development server for the studio without the runtime.
//!
//! ```text
//! cargo run -p moviclone-studio --example serve -- <project-dir> [--web <web-dir>] [--port 8095]
//! ```
//!
//! Serves `web/` statically (`/studio/`, `/shared/`, `/hmi/`), the studio API and
//! `GET /api/bundle` read fresh from disk, so a save is visible on the next fetch.
//! There is no `/ws`: the studio shows "runtime offline" and works without live values.
//! Studio users are kept in `<project-dir>/data/users.toml`, as with the runtime.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Json, Router};
use std::net::SocketAddr;
use std::path::PathBuf;
use tower_http::services::ServeDir;

struct Args {
    project: PathBuf,
    web: PathBuf,
    port: u16,
}

fn parse_args() -> Result<Args, String> {
    let mut project = None;
    let mut web = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web");
    let mut port = 8095u16;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--web" => web = it.next().map(PathBuf::from).ok_or("--web needs a folder")?,
            "--port" => {
                port = it.next().and_then(|p| p.parse().ok()).ok_or("--port needs a number")?;
            }
            "-h" | "--help" => return Err(String::new()),
            other if project.is_none() => project = Some(PathBuf::from(other)),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    let project = project.ok_or("missing project folder")?;
    Ok(Args { project, web, port })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()),
        )
        .init();
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("error: {msg}");
            }
            eprintln!("usage: serve <project-dir> [--web <web-dir>] [--port 8095]");
            std::process::exit(2);
        }
    };
    let project = args.project.canonicalize().map_err(|e| anyhow::anyhow!("{}: {e}", args.project.display()))?;

    let (reload_tx, mut reload_rx) = tokio::sync::watch::channel(0u64);
    let users = moviclone_studio::users::users_file(&project.join("data"));
    let auth = moviclone_studio::Auth::open(users).map_err(anyhow::Error::msg)?;
    tokio::spawn(async move {
        while reload_rx.changed().await.is_ok() {
            tracing::info!(n = *reload_rx.borrow(), "project changed on disk (runtime would reload)");
        }
    });

    let bundle_dir = project.clone();
    let app = Router::new()
        .route("/", get(|| async { Redirect::temporary("/studio/") }))
        .route(
            "/api/bundle",
            get(move || {
                let dir = bundle_dir.clone();
                async move {
                    match tokio::task::spawn_blocking(move || moviclone_model::load_project(&dir)).await {
                        Ok(Ok((p, _))) => Json(p).into_response(),
                        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"ok":false,"error":e.to_string()}))).into_response(),
                        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
                    }
                }
            }),
        )
        .merge(moviclone_studio::routes(project.clone(), auth, reload_tx))
        .nest_service("/studio", ServeDir::new(args.web.join("studio")))
        .nest_service("/shared", ServeDir::new(args.web.join("shared")))
        .nest_service("/hmi", ServeDir::new(args.web.join("hmi")));

    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(project = %project.display(), "studio dev server on http://{addr}/studio/");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
