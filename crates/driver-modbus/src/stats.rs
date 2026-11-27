//! Per-driver statistics, shared between the polling task and diagnostics readers.

use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Health of one station, as seen by the driver.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StationStatus {
    pub name: String,
    pub unit_id: u8,
    /// The last poll of this station succeeded.
    pub ok: bool,
    /// Failed polls in a row (reset by a successful one).
    pub consecutive_failures: u32,
}

/// Snapshot served by `GET /api/diagnostics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriverStatus {
    pub name: String,
    pub kind: &'static str,
    /// `host:port` the driver talks to.
    pub endpoint: String,
    pub connected: bool,
    /// Modbus requests sent (reads and writes).
    pub requests: u64,
    /// Requests that failed (timeouts, exceptions, transport errors) plus failed connects.
    pub errors: u64,
    pub last_error: Option<String>,
    /// Time of the last successful request, ms since the Unix epoch.
    pub last_ok_ts: Option<i64>,
    /// Successful (re)connects since start.
    pub connects: u64,
    pub stations: Vec<StationStatus>,
}

/// Cheap, cloneable handle to a driver's live statistics.
#[derive(Debug, Clone)]
pub struct DriverStats(Arc<Mutex<DriverStatus>>);

impl DriverStats {
    pub fn new(name: &str, endpoint: String, stations: Vec<(String, u8)>) -> Self {
        Self(Arc::new(Mutex::new(DriverStatus {
            name: name.to_string(),
            kind: "modbus-tcp",
            endpoint,
            connected: false,
            requests: 0,
            errors: 0,
            last_error: None,
            last_ok_ts: None,
            connects: 0,
            stations: stations
                .into_iter()
                .map(|(name, unit_id)| StationStatus { name, unit_id, ok: false, consecutive_failures: 0 })
                .collect(),
        })))
    }

    /// Copy of the current statistics.
    pub fn snapshot(&self) -> DriverStatus {
        self.lock().clone()
    }

    pub fn name(&self) -> String {
        self.lock().name.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DriverStatus> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn set_endpoint(&self, endpoint: String) {
        self.lock().endpoint = endpoint;
    }

    pub(crate) fn connected(&self) {
        let mut s = self.lock();
        s.connected = true;
        s.connects += 1;
    }

    pub(crate) fn disconnected(&self, error: &str) {
        let mut s = self.lock();
        s.connected = false;
        s.errors += 1;
        s.last_error = Some(error.to_string());
        for st in &mut s.stations {
            st.ok = false;
        }
    }

    pub(crate) fn request_ok(&self, station: Option<&str>, ts: i64) {
        let mut s = self.lock();
        s.requests += 1;
        s.last_ok_ts = Some(ts);
        if let Some(st) = station.and_then(|n| s.stations.iter_mut().find(|st| st.name == n)) {
            st.ok = true;
            st.consecutive_failures = 0;
        }
    }

    /// Records a failed request; returns the station's failures in a row.
    pub(crate) fn request_failed(&self, station: Option<&str>, error: &str) -> u32 {
        let mut s = self.lock();
        s.requests += 1;
        s.errors += 1;
        s.last_error = Some(error.to_string());
        match station.and_then(|n| s.stations.iter_mut().find(|st| st.name == n)) {
            Some(st) => {
                st.ok = false;
                st.consecutive_failures += 1;
                st.consecutive_failures
            }
            None => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_requests_and_failures() {
        let s = DriverStats::new("d", "h:502".into(), vec![("A".into(), 1)]);
        s.connected();
        s.request_ok(Some("A"), 5);
        assert_eq!(s.request_failed(Some("A"), "timeout"), 1);
        assert_eq!(s.request_failed(Some("A"), "timeout"), 2);
        s.request_ok(Some("A"), 9);
        let snap = s.snapshot();
        assert_eq!((snap.requests, snap.errors, snap.connects), (4, 2, 1));
        assert_eq!(snap.last_ok_ts, Some(9));
        assert_eq!(snap.last_error.as_deref(), Some("timeout"));
        assert!(snap.stations[0].ok);
        assert_eq!(snap.stations[0].consecutive_failures, 0);
        s.disconnected("reset");
        let snap = s.snapshot();
        assert!(!snap.connected && !snap.stations[0].ok);
    }
}
