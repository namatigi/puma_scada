//! Modbus TCP client driver.
//!
//! One driver instance (a `drivers/<name>.toml` of kind `modbus-tcp`) owns one
//! TCP connection to its host and polls every station on it by unit id.
//!
//! ```ignore
//! // Registers the write channel synchronously, then polls in a task.
//! let (stats, task) = moviclone_driver_modbus::spawn(driver, store.clone(), shutdown_rx);
//! stats.snapshot();   // DriverStatus for GET /api/diagnostics (serde::Serialize)
//!
//! // Or, with a stats handle created up front:
//! let stats = moviclone_driver_modbus::stats_for(&driver);
//! moviclone_driver_modbus::run(driver, store, stats.clone(), shutdown_rx).await?;
//! ```
//!
//! Behaviour:
//! - Reads come from `moviclone_check::plan_polls` restricted to this driver
//!   (see [`plan::DriverPlan`]), issued every `poll_ms`.
//! - `addressing = "one-based"`: configured register N is protocol address N-1.
//! - Values are decoded by data type with [`codec`]: big-endian, high word first;
//!   strings are 32 bytes ASCII; booleans come from coils / discrete inputs.
//! - Writes arrive through `TagStore::register_writer(&driver.name, …)`. Coils use
//!   FC5 (FC15 when `write = "multiple"`); holding registers use FC6 only for
//!   one-register values with `write = "single"`, FC22 for `mask`, otherwise FC16.
//!   After the device confirms, the tag is updated in the store.
//! - Connection loss (refused, reset, EOF): every tag of the driver goes bad and
//!   the driver reconnects with exponential backoff (250 ms … 10 s).
//! - A station that times out goes uncertain, then bad after `max_retries`
//!   misses in a row, and is retried with its own backoff while the other
//!   stations keep polling. A Modbus exception marks the affected read bad.
//! - `keep_open = false` closes the connection after every poll cycle; a write
//!   between cycles opens a connection for itself and closes it again.
//! - A station timeout replaces the connection (a late reply must not be taken
//!   for the next answer) and the cycle goes on with the other stations.

pub mod codec;
pub mod plan;
mod runner;
pub mod stats;

pub use stats::{DriverStats, DriverStatus, StationStatus};

use moviclone_model::{Driver, DriverConfig};
use moviclone_tagdb::TagStore;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Pending writes a driver buffers before callers wait.
const WRITE_QUEUE: usize = 64;

/// A statistics handle for `driver`, before it runs.
pub fn stats_for(driver: &Driver) -> DriverStats {
    let DriverConfig::ModbusTcp(cfg) = &driver.config;
    DriverStats::new(
        &driver.name,
        format!("{}:{}", cfg.host, cfg.port),
        cfg.stations.iter().map(|s| (s.name.clone(), s.unit_id)).collect(),
    )
}

/// Runs the driver until `shutdown` becomes `true` (or its sender is dropped).
pub async fn run(
    driver: Driver,
    store: Arc<TagStore>,
    stats: DriverStats,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel(WRITE_QUEUE);
    store.register_writer(&driver.name, tx);
    run_with(driver, store, stats, rx, shutdown).await
}

/// Registers the driver's write channel now and polls in a new task.
pub fn spawn(
    driver: Driver,
    store: Arc<TagStore>,
    shutdown: watch::Receiver<bool>,
) -> (DriverStats, JoinHandle<anyhow::Result<()>>) {
    let stats = stats_for(&driver);
    let (tx, rx) = mpsc::channel(WRITE_QUEUE);
    store.register_writer(&driver.name, tx);
    let task = tokio::spawn(run_with(driver, store, stats.clone(), rx, shutdown));
    (stats, task)
}

async fn run_with(
    driver: Driver,
    store: Arc<TagStore>,
    stats: DriverStats,
    rx: mpsc::Receiver<moviclone_tagdb::WriteRequest>,
    shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let DriverConfig::ModbusTcp(cfg) = &driver.config;
    stats.set_endpoint(format!("{}:{}", cfg.host, cfg.port));
    runner::Runner::new(&driver, store, stats, rx, shutdown).run().await
}
