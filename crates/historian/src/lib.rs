//! Historian.  OWNER: history agent (phase 4). Protocol: docs/PROTOCOL.md.
//!
//! Store-and-forward historian on redb (`<dir>/historian.redb`):
//!
//! - Records every change of tags whose `historian` names an enabled
//!   historian, plus a heartbeat sample (default every 60 s) so flat signals
//!   still have points, plus a sample of every historized tag at start-up.
//! - Records data-logger rows (`project.data.loggers`): `on-change` writes one
//!   row per change of any column tag holding the current value of every
//!   column; `every-s` writes periodic rows.
//! - Recovers from broadcast lag by re-reading current values.
//! - Prunes records older than each historian's / logger's `max_age_s`.
//! - Answers trend queries (per-bucket min/max downsampling) and
//!   history-table queries (newest first).
//! - Forwards the journal to SQL targets through [`Forwarder`]s with a durable
//!   watermark: the local journal is the source of truth, SQL a copy that
//!   catches up after outages. The SQL Server forwarder is behind the
//!   `mssql` cargo feature (tiberius + rustls).
//!
//! Final API:
//! ```ignore
//! impl Historian {
//!     pub fn open(project: &Project, store: Arc<TagStore>, dir: &Path) -> anyhow::Result<Arc<Self>>;
//!     pub fn open_with(project, store, dir, cfg: HistorianConfig) -> anyhow::Result<Arc<Self>>;
//!     pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>);            // records
//!     pub async fn run_forwarder(self: Arc<Self>, fwd: Arc<dyn Forwarder>, opts: ForwardOptions,
//!                                shutdown: watch::Receiver<bool>);
//!     pub fn query(&self, tags: &[String], from_ms: i64, to_ms: i64, max_points: usize)
//!         -> anyhow::Result<Vec<Series>>;
//!     pub fn table(&self, source: &str, kind: Option<SourceKind>, from_ms, to_ms, limit: usize)
//!         -> anyhow::Result<Table>;                                                  // QueryError::UnknownSource
//!     pub fn prune(&self) -> anyhow::Result<usize>;
//!     pub fn forward_backlog(&self, forwarder: &str) -> anyhow::Result<u64>;
//! }
//! pub fn routes(h: Arc<Historian>) -> axum::Router;   // GET /api/history, GET /api/history/table
//! #[cfg(feature = "mssql")]
//! pub fn mssql::forwarders(project: &Project) -> Vec<Arc<dyn Forwarder>>;           // one per MSSQL database in use
//! ```
//!
//! Wiring (integrator):
//! ```ignore
//! let hist = Historian::open(&project, store.clone(), &data_dir.join("historian"))?;
//! tokio::spawn(hist.clone().run(shutdown_rx.clone()));
//! #[cfg(feature = "mssql")]
//! for f in moviclone_historian::mssql::forwarders(&project) {
//!     tokio::spawn(hist.clone().run_forwarder(f, ForwardOptions::default(), shutdown_rx.clone()));
//! }
//! router.merge(moviclone_historian::routes(hist.clone()));
//! ```
//!
//! History tables: `source` names a historian (columns `time, tag, value,
//! quality`, one row per sample) or a data logger (columns `time, <column…>`,
//! one row per record). When a historian and a logger share a name the
//! historian is used unless `kind=logger` is given.

mod clock;
mod codec;
mod db;
mod downsample;
mod forward;
mod historian;
mod plan;
mod recorder;
mod routes;
pub mod secret;

#[cfg(feature = "mssql")]
pub mod mssql;

pub use clock::Clock;
pub use downsample::Point;
pub use forward::{BoxFuture, ForwardData, ForwardEntry, ForwardOptions, Forwarder, LoggedCell, MemoryForwarder};
pub use historian::{Historian, HistorianConfig, QueryError, Series, SourceKind, Table, DB_FILE};
pub use routes::routes;
