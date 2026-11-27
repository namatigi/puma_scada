//! Alarm engine.  OWNER: history agent (phase 4). Protocol: docs/PROTOCOL.md.
//!
//! Evaluates `moviclone_model::Alarm` definitions against the tag store and
//! keeps ISA-18.2 state per alarm: `normal`, `active-unacked`, `active-acked`,
//! `cleared-unacked`, `shelved`, `disabled`.
//!
//! - Conditions `above` / `below` (with deadband on clear), `equals`,
//!   `is-true`, `is-false`, `bad-quality`. A value condition on a bad-quality
//!   sample keeps the alarm's state instead of clearing it.
//! - `delay_s`: the condition must hold that long before the alarm activates.
//! - `ack_required = false`: the alarm returns to `normal` when it clears.
//! - Shelving for N minutes (max one week) with automatic unshelve; an alarm
//!   still active when it unshelves is announced again (`active-unacked`).
//! - Every event (`activated`, `cleared`, `acked`, `shelved`, `unshelved`) is
//!   appended to `<dir>/alarm-journal.jsonl`; state is snapshotted to
//!   `<dir>/alarm-state.json` (on a background thread, at most once a second
//!   during an event flood, and on shutdown) and restored on restart (active alarms stay
//!   active, pending acknowledgements stay pending, unexpired shelves resume).
//!
//! Final API:
//! ```ignore
//! impl AlarmEngine {
//!     pub fn open(project: &Project, store: Arc<TagStore>, dir: &Path) -> anyhow::Result<Arc<Self>>;
//!     pub fn open_with_clock(project, store, dir, clock: Clock) -> anyhow::Result<Arc<Self>>;
//!     pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>);
//!     pub fn states(&self) -> Vec<AlarmState>;          // all alarms, display order
//!     pub fn active(&self) -> Vec<AlarmState>;          // not normal / disabled
//!     pub fn state(&self, name: &str) -> Option<AlarmState>;
//!     pub fn journal(&self, limit: usize) -> Vec<AlarmEvent>;   // newest first
//!     pub fn subscribe(&self) -> broadcast::Receiver<AlarmEvent>;
//!     pub fn ws_feed(&self) -> watch::Receiver<serde_json::Value>; // {"type":"alarms","alarms":[…]}
//!     pub fn ack(&self, name, user: Option<&str>) -> Result<bool, AlarmError>;
//!     pub fn ack_all(&self, user: Option<&str>) -> usize;
//!     pub fn shelve(&self, name, duration: Duration, user) -> Result<(), AlarmError>;
//!     pub fn unshelve(&self, name, user) -> Result<(), AlarmError>;
//! }
//! pub fn routes(e: Arc<AlarmEngine>) -> axum::Router;
//! //   GET /api/alarms, GET /api/alarms/journal?limit=, POST /api/alarms/ack-all,
//! //   POST /api/alarms/{name}/ack | shelve {"minutes":N} | unshelve
//! ```
//!
//! Wiring (integrator): `let alarms = AlarmEngine::open(&project, store.clone(), &data_dir.join("alarms"))?;
//! tokio::spawn(alarms.clone().run(shutdown_rx));` merge `routes(alarms.clone())`
//! into the gateway and forward `alarms.ws_feed()` to WebSocket clients.
//!
//! `AlarmState.value` is the tag value at the last transition (what raised or
//! cleared the alarm); the feed changes on state changes, not on every value change.

mod clock;
mod condition;
mod engine;
mod persist;
mod routes;
mod state;
mod writer;

pub use clock::Clock;
pub use engine::{AlarmEngine, AlarmError, MAX_SHELVE};
pub use persist::{JOURNAL_FILE, STATE_FILE};
pub use routes::routes;
pub use state::{AlarmEvent, AlarmState, EventKind, Snapshot, StateKind};
