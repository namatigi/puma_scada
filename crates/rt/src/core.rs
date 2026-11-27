//! The core of a running project: tag store, drivers, optional simulator and
//! the gateway router built on them.
//!
//! A core is bound to one tag list (the store indexes tags by position) and
//! one driver configuration, so it is replaced as a whole when either changes.

use axum::Router;
use moviclone_driver_modbus::DriverStats;
use moviclone_gateway::{GatewayConfig, SharedProject};
use moviclone_model::{DriverConfig, Project};
use moviclone_sim::Simulator;
use moviclone_tagdb::TagStore;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// How long `stop` waits for each driver.
const STOP_GRACE: Duration = Duration::from_secs(5);

pub struct Core {
    pub store: Arc<TagStore>,
    stats: Vec<DriverStats>,
    drivers: Vec<JoinHandle<anyhow::Result<()>>>,
    sim: Option<Simulator>,
    /// Ends this core's drivers and the WebSocket sessions of its gateway.
    stop: watch::Sender<bool>,
}

impl Core {
    /// Creates the tag store and starts every driver. With `simulate`, a
    /// Modbus simulator for `project` runs on a free local port and every
    /// modbus-tcp driver is pointed at it (the served project keeps the
    /// configured hosts).
    pub async fn start(project: &Project, simulate: bool) -> anyhow::Result<Self> {
        let store = TagStore::new(project);
        let (stop, stop_rx) = watch::channel(false);

        let mut drivers = project.drivers.clone();
        let sim = if simulate {
            let sim = moviclone_sim::start(project, SocketAddr::from(([127, 0, 0, 1], 0))).await?;
            let addr = sim.local_addr();
            for d in &mut drivers {
                let DriverConfig::ModbusTcp(cfg) = &mut d.config;
                tracing::info!(driver = %d.name, configured = %format!("{}:{}", cfg.host, cfg.port), simulator = %addr, "driver redirected to the simulator");
                cfg.host = addr.ip().to_string();
                cfg.port = addr.port();
            }
            Some(sim)
        } else {
            None
        };

        let mut stats = Vec::with_capacity(drivers.len());
        let mut tasks = Vec::with_capacity(drivers.len());
        for d in drivers {
            let (s, task) = moviclone_driver_modbus::spawn(d, store.clone(), stop_rx.clone());
            stats.push(s);
            tasks.push(task);
        }
        Ok(Self { store, stats, drivers: tasks, sim, stop })
    }

    /// Address of the simulator, when running with `simulate`.
    pub fn simulator_addr(&self) -> Option<SocketAddr> {
        self.sim.as_ref().map(Simulator::local_addr)
    }

    pub fn driver_count(&self) -> usize {
        self.stats.len()
    }

    /// The gateway for this core: REST, WebSocket and static files, plus `extra`.
    pub fn gateway(
        &self,
        project: SharedProject,
        web: PathBuf,
        extra: Router,
        ws_feeds: Vec<watch::Receiver<serde_json::Value>>,
        auth: Arc<moviclone_auth::Auth>,
    ) -> Router {
        let mut cfg = GatewayConfig::new(project, self.store.clone(), web);
        cfg.auth = Some(auth);
        cfg.extra = extra;
        cfg.ws_feeds = ws_feeds;
        cfg.shutdown = Some(self.stop.subscribe());
        if let Ok(list) = std::env::var(moviclone_gateway::ALLOWED_ORIGINS_ENV) {
            cfg.allowed_origins = moviclone_gateway::parse_origins(&list);
        }
        let stats = self.stats.clone();
        cfg.diagnostics = Some(Arc::new(move || {
            stats.iter().map(|s| serde_json::to_value(s.snapshot()).unwrap_or_default()).collect()
        }));
        moviclone_gateway::router(cfg)
    }

    /// Stops the drivers, closes this core's WebSocket sessions and stops the
    /// simulator.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        for task in self.drivers {
            match tokio::time::timeout(STOP_GRACE, task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => tracing::warn!("driver ended with an error: {e:#}"),
                Ok(Err(e)) => tracing::warn!("driver task failed: {e}"),
                Err(_) => tracing::warn!("driver did not stop in time"),
            }
        }
        if let Some(sim) = self.sim {
            sim.stop().await;
        }
    }
}
