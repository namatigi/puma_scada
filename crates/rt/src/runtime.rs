//! Start-up, hot reload and shutdown of the whole runtime.

use crate::core::Core;
use crate::reload::{self, Change, DEBOUNCE};
use crate::services::{self, Services};
use crate::switch::RouterSwitch;
use crate::Args;
use anyhow::Context as _;
use axum::Router;
use moviclone_gateway::SharedProject;
use moviclone_model::Project;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

/// How long shutdown waits for the HTTP server to finish open requests.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// Time old WebSocket sessions get to send the reload notice on a core restart.
const RELOAD_NOTICE: Duration = Duration::from_millis(500);
const SERVICES: &str = "the historian and alarm engine";

/// A running runtime. Dropping it does not stop it; call [`Runtime::shutdown`].
pub struct Runtime {
    addr: SocketAddr,
    shutdown: watch::Sender<bool>,
    supervisor: JoinHandle<anyhow::Result<()>>,
    live: Arc<Mutex<Live>>,
    project: SharedProject,
}

/// Parts that live as long as the process.
struct Fixed {
    args: Args,
    data_dir: PathBuf,
    project: SharedProject,
    /// What the listener serves: the current core's gateway.
    app: RouterSwitch,
    /// What `/api/history…` and `/api/alarms…` reach: the current services.
    services: RouterSwitch,
    alarm_feed: watch::Sender<serde_json::Value>,
    studio: Option<Router>,
    /// Users and sessions for the HMI and the studio (`<data>/users.toml`).
    auth: Arc<moviclone_auth::Auth>,
}

/// Parts replaced by reloads.
struct Live {
    project: Arc<Project>,
    /// `None` only while shutting down.
    core: Option<Core>,
    services: Option<Services>,
}

/// Loads and checks the project, starts drivers, historian, alarm engine and
/// (with `--studio`) the studio API, and starts listening.
pub async fn start(args: Args) -> anyhow::Result<Runtime> {
    let project = reload::load_checked(&args.project, args.allow_errors)?;
    let data_dir = args.data_dir();
    std::fs::create_dir_all(&data_dir).with_context(|| format!("cannot create the data folder {}", data_dir.display()))?;
    if !args.web.is_dir() {
        tracing::warn!(web = %args.web.display(), "web folder not found; /hmi/ and /studio/ will answer 404");
    }

    let users = moviclone_auth::users::users_file(&data_dir);
    let auth = moviclone_auth::Auth::open(users, args.hmi_guest).map_err(anyhow::Error::msg).context("users")?;
    let (studio_tx, studio_rx) = watch::channel(0u64);
    let studio = args.studio.then(|| moviclone_studio::routes(args.project.clone(), auth.clone(), studio_tx));
    let fixed = Arc::new(Fixed {
        data_dir,
        project: SharedProject::new(project.clone()),
        app: RouterSwitch::unavailable("the runtime"),
        services: RouterSwitch::unavailable(SERVICES),
        alarm_feed: watch::channel(services::empty_alarm_feed()).0,
        studio,
        auth,
        args,
    });

    let core = Core::start(&project, fixed.args.simulate).await?;
    fixed.app.set(gateway(&fixed, &core));
    let svc = Services::start(&project, core.store.clone(), &fixed.data_dir, fixed.alarm_feed.clone()).await?;
    fixed.services.set(svc.routes.clone());

    let listener = tokio::net::TcpListener::bind(fixed.args.listen)
        .await
        .with_context(|| format!("cannot listen on {}", fixed.args.listen))?;
    let addr = listener.local_addr()?;
    tracing::info!(
        project = %project.meta.name,
        tags = core.store.len(),
        drivers = core.driver_count(),
        alarms = project.alarms.len(),
        simulate = fixed.args.simulate,
        studio = fixed.studio.is_some(),
        data = %fixed.data_dir.display(),
        "Moviclone runtime ready: HMI http://{addr}/hmi/  studio http://{addr}/studio/"
    );
    if fixed.studio.is_none() {
        tracing::info!("studio API not mounted; start with --studio to edit the project from the browser");
    }

    let (shutdown, shutdown_rx) = watch::channel(false);
    let mut stop = shutdown_rx.clone();
    let server = tokio::spawn(moviclone_gateway::serve(listener, fixed.app.router(), async move {
        let _ = stop.wait_for(|s| *s).await;
    }));

    let live = Arc::new(Mutex::new(Live { project: Arc::new(project), core: Some(core), services: Some(svc) }));
    let reloads = fixed.studio.is_some().then_some(studio_rx);
    let project = fixed.project.clone();
    let supervisor = tokio::spawn(supervise(fixed, live.clone(), server, reloads, shutdown_rx));
    Ok(Runtime { addr, shutdown, supervisor, live, project })
}

impl Runtime {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The project being served right now.
    pub fn project(&self) -> Arc<Project> {
        self.project.get()
    }

    /// Address of the Modbus simulator of the current core (with `--simulate`).
    pub async fn simulator_addr(&self) -> Option<SocketAddr> {
        self.live.lock().await.core.as_ref().and_then(Core::simulator_addr)
    }

    /// Runs until `signal` completes or the server fails, then shuts down.
    pub async fn run_until(mut self, signal: impl Future<Output = ()>) -> anyhow::Result<()> {
        tokio::select! {
            _ = signal => {
                tracing::info!("shutting down");
                self.shutdown().await
            }
            r = &mut self.supervisor => joined(r),
        }
    }

    /// Stops the server, the services and the drivers, in that order.
    pub async fn shutdown(self) -> anyhow::Result<()> {
        let _ = self.shutdown.send(true);
        joined(self.supervisor.await)
    }
}

fn joined(r: Result<anyhow::Result<()>, tokio::task::JoinError>) -> anyhow::Result<()> {
    r.unwrap_or_else(|e| Err(anyhow::anyhow!("runtime task failed: {e}")))
}

/// The gateway of `core` with the studio and services routes.
fn gateway(fixed: &Fixed, core: &Core) -> Router {
    let mut extra = fixed.services.routes_for(services::PREFIXES);
    if let Some(studio) = &fixed.studio {
        extra = extra.merge(studio.clone());
    }
    core.gateway(fixed.project.clone(), fixed.args.web.clone(), extra, vec![fixed.alarm_feed.subscribe()], fixed.auth.clone())
}

/// Applies studio saves until shutdown, then stops everything.
async fn supervise(
    fixed: Arc<Fixed>,
    live: Arc<Mutex<Live>>,
    mut server: JoinHandle<anyhow::Result<()>>,
    mut reloads: Option<watch::Receiver<u64>>,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let server_result = loop {
        tokio::select! {
            () = stopped(&mut shutdown) => break None,
            r = &mut server => break Some(r),
            Some(()) = next_reload(&mut reloads) => {
                let dir = fixed.args.project.clone();
                let allow = fixed.args.allow_errors;
                let loaded = tokio::task::spawn_blocking(move || reload::load_checked(&dir, allow)).await;
                match loaded {
                    Ok(Ok(project)) => apply(&fixed, &mut *live.lock().await, project).await,
                    Ok(Err(e)) => tracing::warn!("studio save not applied, the runtime keeps the previous project: {e:#}"),
                    Err(e) => tracing::error!("reload task failed: {e}"),
                }
            }
        }
    };

    // The server stops accepting; WebSocket sessions end when the core stops.
    let mut live = live.lock().await;
    fixed.services.set_unavailable(SERVICES);
    if let Some(svc) = live.services.take() {
        svc.stop().await;
    }
    if let Some(core) = live.core.take() {
        core.stop().await;
    }
    let result = match server_result {
        Some(r) => r,
        None => match tokio::time::timeout(STOP_GRACE, server).await {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!("gateway did not stop in time");
                Ok(Ok(()))
            }
        },
    };
    match result {
        Ok(Ok(())) => {
            tracing::info!("stopped");
            Ok(())
        }
        Ok(Err(e)) => Err(e.context("gateway stopped")),
        Err(e) => Err(anyhow::anyhow!("gateway task failed: {e}")),
    }
}

/// Resolves after a studio save, once saves have paused for [`DEBOUNCE`].
/// Never resolves without a studio channel.
async fn next_reload(reloads: &mut Option<watch::Receiver<u64>>) -> Option<()> {
    let Some(rx) = reloads.as_mut() else {
        return std::future::pending().await;
    };
    if rx.changed().await.is_err() {
        *reloads = None;
        return std::future::pending().await;
    }
    loop {
        match tokio::time::timeout(DEBOUNCE, rx.changed()).await {
            Err(_) => return Some(()),
            Ok(Ok(())) => continue,
            Ok(Err(_)) => return Some(()),
        }
    }
}

/// Applies `new` with the smallest restart (see [`crate::reload`]).
async fn apply(fixed: &Fixed, live: &mut Live, new: Project) {
    let change = reload::classify(&live.project, &new);
    tracing::info!(?change, "applying studio save");
    match change {
        // Even an unchanged project is re-sent so clients pick up the save.
        Change::None | Change::Project => fixed.project.replace(new.clone()),
        Change::Services => {
            stop_services(fixed, live).await;
            fixed.project.replace(new.clone());
            start_services(fixed, live, &new).await;
        }
        Change::Core => {
            let core = match Core::start(&new, fixed.args.simulate).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("cannot start the new drivers, keeping the previous project: {e:#}");
                    return;
                }
            };
            stop_services(fixed, live).await;
            fixed.app.set(gateway(fixed, &core));
            fixed.project.replace(new.clone());
            // Let sessions of the old core deliver {"type":"reload"} before
            // their socket closes, so clients know why they reconnect.
            tokio::time::sleep(RELOAD_NOTICE).await;
            if let Some(old) = live.core.replace(core) {
                old.stop().await;
            }
            start_services(fixed, live, &new).await;
        }
    }
    live.project = Arc::new(new);
    tracing::info!(?change, "studio save applied");
}

async fn stop_services(fixed: &Fixed, live: &mut Live) {
    // Drop the routes first: they hold the databases open.
    fixed.services.set_unavailable(SERVICES);
    if let Some(svc) = live.services.take() {
        svc.stop().await;
    }
}

async fn start_services(fixed: &Fixed, live: &mut Live, project: &Project) {
    let Some(core) = &live.core else { return };
    match Services::start(project, core.store.clone(), &fixed.data_dir, fixed.alarm_feed.clone()).await {
        Ok(svc) => {
            fixed.services.set(svc.routes.clone());
            live.services = Some(svc);
        }
        Err(e) => tracing::error!("{SERVICES} could not restart and stay unavailable until the next save: {e:#}"),
    }
}

/// Resolves once `shutdown` is true or its sender is gone. (`wait_for`'s
/// guard is not `Send`, so it must not be a `select!` output.)
async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|s| *s).await;
}
