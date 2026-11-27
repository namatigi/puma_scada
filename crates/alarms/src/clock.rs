//! Wall-clock source for event timestamps.
//!
//! Timers (delays, shelving) run on tokio's clock so tests can pause and
//! advance time. Timestamps written to the journal must follow the same clock
//! in tests, otherwise a "30 minute shelve" would show a wall time that never
//! moves. [`Clock::tokio`] derives wall time from tokio's clock; production
//! uses [`Clock::system`].

use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Clock(Kind);

#[derive(Debug, Clone)]
enum Kind {
    System,
    Tokio { base_ms: i64, start: tokio::time::Instant },
}

impl Clock {
    /// Real wall-clock time.
    pub fn system() -> Self {
        Clock(Kind::System)
    }

    /// Starts at the current wall time and then advances with tokio's clock
    /// (which tests may pause and advance).
    pub fn tokio() -> Self {
        Self::tokio_at(system_ms())
    }

    /// Like [`Clock::tokio`], starting at `base_ms`.
    pub fn tokio_at(base_ms: i64) -> Self {
        Clock(Kind::Tokio { base_ms, start: tokio::time::Instant::now() })
    }

    /// Milliseconds since the Unix epoch.
    pub fn now_ms(&self) -> i64 {
        match &self.0 {
            Kind::System => system_ms(),
            Kind::Tokio { base_ms, start } => base_ms + start.elapsed().as_millis() as i64,
        }
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::system()
    }
}

fn system_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
