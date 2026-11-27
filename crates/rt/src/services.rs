//! Historian and alarm engine, started on a core's tag store.
//!
//! Services are restarted as a unit when the project's alarms, data config,
//! tags or drivers change (see [`crate::reload`]). Their data lives in the
//! data folder and survives restarts: the historian reopens its redb journal,
//! the alarm engine restores its state snapshot and journal.
//!
//! WebSocket clients keep one alarm feed for the whole process lifetime: the
//! current engine's feed is copied into a stable channel, so the gateway does
//! not need rebuilding when the engine is replaced.

use anyhow::Context as _;
use axum::Router;
use moviclone_alarms::AlarmEngine;
use moviclone_historian::Historian;
use moviclone_model::Project;
use moviclone_tagdb::TagStore;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// URL prefixes the services answer; the gateway forwards them to the
/// current services router.
pub const PREFIXES: &[&str] = &["/api/history", "/api/alarms"];

/// How long `stop` waits for each service task. The historian commits its
/// last batch on the way out, which can take a while on a slow disk.
const STOP_GRACE: Duration = Duration::from_secs(30);
/// redb keeps an exclusive lock until the last handle is gone; a reopen right
/// after a stop can race the drop of in-flight request handlers.
const REOPEN_ATTEMPTS: u32 = 100;
const REOPEN_PAUSE: Duration = Duration::from_millis(100);

pub fn historian_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("historian")
}

pub fn alarms_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("alarms")
}

/// The initial message of the stable alarm feed (no alarms yet).
pub fn empty_alarm_feed() -> serde_json::Value {
    serde_json::json!({"type": "alarms", "alarms": []})
}

pub struct Services {
    pub historian: Arc<Historian>,
    pub alarms: Arc<AlarmEngine>,
    /// `/api/history…` and `/api/alarms…`.
    pub routes: Router,
    stop: watch::Sender<bool>,
    tasks: Vec<(&'static str, JoinHandle<()>)>,
}

impl Services {
    /// Opens the historian and the alarm engine for `project` and starts them.
    /// `alarm_feed` receives every alarm list message from now on.
    pub async fn start(
        project: &Project,
        store: Arc<TagStore>,
        data_dir: &Path,
        alarm_feed: watch::Sender<serde_json::Value>,
    ) -> anyhow::Result<Self> {
        let hdir = historian_dir(data_dir);
        let adir = alarms_dir(data_dir);
        std::fs::create_dir_all(&hdir).with_context(|| format!("cannot create {}", hdir.display()))?;
        std::fs::create_dir_all(&adir).with_context(|| format!("cannot create {}", adir.display()))?;

        let historian = open_historian(project, store.clone(), &hdir).await?;
        let alarms = AlarmEngine::open(project, store, &adir)
            .with_context(|| format!("cannot open the alarm journal in {}", adir.display()))?;

        let (stop, stop_rx) = watch::channel(false);
        let mut tasks = vec![
            ("historian", tokio::spawn(historian.clone().run(stop_rx.clone()))),
            ("alarm engine", tokio::spawn(alarms.clone().run(stop_rx.clone()))),
            ("alarm feed", tokio::spawn(forward_feed(alarms.ws_feed(), alarm_feed, stop_rx.clone()))),
        ];
        tasks.extend(start_forwarders(project, &historian, &stop_rx));

        let routes = moviclone_alarms::routes(alarms.clone()).merge(moviclone_historian::routes(historian.clone()));
        tracing::info!(alarms = project.alarms.len(), data = %data_dir.display(), "historian and alarm engine started");
        Ok(Self { historian, alarms, routes, stop, tasks })
    }

    /// Stops every task (the historian writes its last batch, the alarm engine
    /// its final snapshot) and releases the databases.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        for (name, task) in self.tasks {
            match tokio::time::timeout(STOP_GRACE, task).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("{name} task failed: {e}"),
                Err(_) => tracing::warn!("{name} did not stop in time"),
            }
        }
        tracing::debug!("historian and alarm engine stopped");
    }
}

async fn open_historian(project: &Project, store: Arc<TagStore>, dir: &Path) -> anyhow::Result<Arc<Historian>> {
    let mut attempt = 1;
    loop {
        match Historian::open(project, store.clone(), dir) {
            Ok(h) => return Ok(h),
            Err(e) if attempt < REOPEN_ATTEMPTS => {
                tracing::debug!(attempt, "historian database busy, retrying: {e:#}");
                attempt += 1;
                tokio::time::sleep(REOPEN_PAUSE).await;
            }
            Err(e) => return Err(e.context(format!("cannot open the historian in {}", dir.display()))),
        }
    }
}

/// Copies the engine's feed into the process-wide feed until `stop`.
async fn forward_feed(
    mut from: watch::Receiver<serde_json::Value>,
    to: watch::Sender<serde_json::Value>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let msg = from.borrow_and_update().clone();
        to.send_if_modified(|cur| {
            if *cur == msg {
                false
            } else {
                *cur = msg;
                true
            }
        });
        tokio::select! {
            r = from.changed() => if r.is_err() { return },
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

#[cfg(feature = "mssql")]
fn start_forwarders(
    project: &Project,
    historian: &Arc<Historian>,
    stop: &watch::Receiver<bool>,
) -> Vec<(&'static str, JoinHandle<()>)> {
    moviclone_historian::mssql::forwarders(project)
        .into_iter()
        .map(|f| {
            let task = historian.clone().run_forwarder(f, moviclone_historian::ForwardOptions::default(), stop.clone());
            ("SQL forwarder", tokio::spawn(task))
        })
        .collect()
}

#[cfg(not(feature = "mssql"))]
fn start_forwarders(
    project: &Project,
    _historian: &Arc<Historian>,
    _stop: &watch::Receiver<bool>,
) -> Vec<(&'static str, JoinHandle<()>)> {
    if !project.data.databases.is_empty() {
        tracing::info!(
            databases = project.data.databases.len(),
            "SQL forwarding is not built in (build moviclone-rt with --features mssql); history stays in the local journal"
        );
    }
    Vec::new()
}
