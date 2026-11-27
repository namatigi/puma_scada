//! Modbus TCP simulator for development and tests.
//!
//! Serves every station of a project's modbus-tcp drivers on one socket and
//! answers for each unit id, with plausible values for a petroleum loading
//! manifold (see [`process`]): mass flow 0–150 mTon/hr with slow waves, noise and
//! occasional stops; density by product (FM_01..FM_04 → HFO, gasoil, JET A-1,
//! gasoline); temperature 20–45 °C; volume rate = mass flow × 1000 / density (m³/hr);
//! totals and inventories integrating the flow; external pressure 2–8 bar.
//! START coils gate the totalizers; writing `true` to a Reset_Total coil zeroes
//! that meter's totals. Anything a client writes persists (the simulation stops
//! driving that address). Unknown tags do a slow random walk. Addressing is
//! interpreted per station exactly like the driver does (one-based by default).
//!
//! ```ignore
//! // Runs until the listener fails:
//! moviclone_sim::serve(project, "127.0.0.1:5020".parse()?).await?;
//!
//! // Or keep a handle (bind port 0 to get a free port):
//! let sim = moviclone_sim::start(&project, "127.0.0.1:0".parse()?).await?;
//! let addr = sim.local_addr();
//! sim.read_tag("FM_01_MassFlow");               // what a client would read now
//! sim.write_tag("FM_01_MassFlow", &Value::Float(42.0))?;  // pin a value
//! sim.stop().await;                             // closes the listener and every connection
//! ```

mod image;
pub mod process;
mod server;

pub use image::SimState;

use moviclone_model::Project;
use moviclone_tagdb::Value;
use server::{lock, Shared};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// How often the process model advances.
pub const TICK: Duration = Duration::from_millis(100);

/// A running simulator.
pub struct Simulator {
    addr: SocketAddr,
    state: Shared,
    stop: watch::Sender<bool>,
    task: JoinHandle<std::io::Result<()>>,
}

/// Binds `addr` and starts serving. Port 0 picks a free port: see [`Simulator::local_addr`].
pub async fn start(project: &Project, addr: SocketAddr) -> anyhow::Result<Simulator> {
    let listener = server::bind(addr).map_err(|e| anyhow::anyhow!("simulator cannot listen on {addr}: {e}"))?;
    let addr = listener.local_addr()?;
    let state: Shared = Arc::new(Mutex::new(SimState::new(project)));
    let units = lock(&state).unit_ids();
    tracing::info!(%addr, ?units, "modbus simulator listening");
    let (stop, stop_rx) = watch::channel(false);

    let ticker_state = Arc::clone(&state);
    let mut ticker_stop = stop_rx.clone();
    let ticker = async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last = tokio::time::Instant::now();
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let now = tokio::time::Instant::now();
                    let dt = now.duration_since(last).as_secs_f64();
                    last = now;
                    lock(&ticker_state).tick(dt);
                }
                _ = ticker_stop.wait_for(|s| *s) => return,
            }
        }
    };
    let accept = server::accept_loop(listener, Arc::clone(&state), stop_rx);
    let task = tokio::spawn(async move {
        let ticker = tokio::spawn(ticker);
        let result = accept.await;
        ticker.abort();
        result
    });
    Ok(Simulator { addr, state, stop, task })
}

/// Serves `project` on `addr` until the listener fails.
pub async fn serve(project: Project, addr: SocketAddr) -> anyhow::Result<()> {
    start(&project, addr).await?.wait().await
}

impl Simulator {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// The value a client would read for `tag` right now.
    pub fn read_tag(&self, tag: &str) -> Option<Value> {
        lock(&self.state).read_tag(tag)
    }

    /// Sets `tag` as if a client had written it (the value then persists).
    pub fn write_tag(&self, tag: &str, value: &Value) -> Result<(), String> {
        lock(&self.state).write_tag(tag, value)
    }

    /// Stops listening and drops every connection, like a device switched off.
    pub async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
        tracing::info!(addr = %self.addr, "modbus simulator stopped");
    }

    /// Waits until the simulator ends (it only does on a listener error).
    pub async fn wait(self) -> anyhow::Result<()> {
        // Keep the stop sender alive: dropping it would stop the simulator.
        let _stop = self.stop;
        match self.task.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow::anyhow!("simulator stopped: {e}")),
            Err(e) => Err(anyhow::anyhow!("simulator task failed: {e}")),
        }
    }
}
