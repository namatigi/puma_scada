//! Alarm persistence off the async runtime.
//!
//! The engine commits under its mutex from tokio tasks (tag changes, HTTP ack
//! and shelve handlers). Writing the journal and fsyncing the state snapshot
//! there would block a runtime worker, and every other alarm request behind
//! the mutex, for as long as the disk takes (tens to hundreds of ms on Windows
//! with a virus scanner). The engine therefore only queues work here; one
//! thread appends journal lines as they come and writes the snapshot at most
//! every [`SNAPSHOT_EVERY`] (always the newest one, and once more at the end).

use crate::persist::{Persist, JOURNAL_KEEP};
use crate::state::{AlarmEvent, Snapshot};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Longest a state change waits before it is on disk during an event flood.
pub(crate) const SNAPSHOT_EVERY: Duration = Duration::from_secs(1);

pub(crate) type StateSnapshot = BTreeMap<String, Snapshot>;

enum Msg {
    Events(Vec<AlarmEvent>),
    Snapshot(StateSnapshot),
    Flush(oneshot::Sender<()>),
}

/// Handle to the persistence thread. Dropping it writes what is pending and
/// waits for the thread, so a reopened engine reads everything back.
pub(crate) struct PersistWriter {
    tx: Option<mpsc::Sender<Msg>>,
    thread: Option<JoinHandle<()>>,
    #[cfg_attr(not(test), allow(dead_code))]
    snapshots_written: Arc<AtomicUsize>,
}

impl PersistWriter {
    /// Starts the thread; `tail` is the journal as loaded (for compaction).
    pub(crate) fn start(persist: Persist, tail: VecDeque<AlarmEvent>) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let snapshots_written = Arc::new(AtomicUsize::new(0));
        let counter = snapshots_written.clone();
        let thread = std::thread::Builder::new()
            .name("alarm-persist".into())
            .spawn(move || worker(persist, tail, rx, counter))?;
        Ok(Self { tx: Some(tx), thread: Some(thread), snapshots_written })
    }

    fn send(&self, msg: Msg) {
        if let Some(tx) = &self.tx {
            if tx.send(msg).is_err() {
                tracing::error!("alarm persistence thread has stopped; alarm events are no longer saved");
            }
        }
    }

    pub(crate) fn events(&self, events: Vec<AlarmEvent>) {
        self.send(Msg::Events(events));
    }

    pub(crate) fn snapshot(&self, snapshot: StateSnapshot) {
        self.send(Msg::Snapshot(snapshot));
    }

    /// Resolves once everything queued so far is written (snapshot included).
    pub(crate) fn flush(&self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.send(Msg::Flush(tx));
        rx
    }

    #[cfg(test)]
    pub(crate) fn snapshots_written(&self) -> usize {
        self.snapshots_written.load(Ordering::Relaxed)
    }
}

impl Drop for PersistWriter {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::error!("alarm persistence thread panicked");
            }
        }
    }
}

fn worker(mut persist: Persist, mut tail: VecDeque<AlarmEvent>, rx: mpsc::Receiver<Msg>, written: Arc<AtomicUsize>) {
    let mut pending: Option<StateSnapshot> = None;
    let mut last_write: Option<Instant> = None;
    let write = |persist: &Persist, pending: &mut Option<StateSnapshot>, last_write: &mut Option<Instant>| {
        if let Some(snap) = pending.take() {
            // Failures (disk full, a scanner holding the file) must not stop alarming.
            if let Err(e) = persist.write_snapshot(&snap) {
                tracing::error!(error = %e, "could not save the alarm state");
            }
            written.fetch_add(1, Ordering::Relaxed);
            *last_write = Some(Instant::now());
        }
    };
    loop {
        let msg = match (&pending, last_write) {
            (Some(_), Some(at)) => match rx.recv_timeout(SNAPSHOT_EVERY.saturating_sub(at.elapsed())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            (Some(_), None) => None,
            (None, _) => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
        };
        match msg {
            Some(Msg::Events(events)) => {
                if let Err(e) = persist.append(&events) {
                    tracing::error!(error = %e, "could not append to the alarm journal");
                }
                for event in events {
                    if tail.len() == JOURNAL_KEEP {
                        tail.pop_front();
                    }
                    tail.push_back(event);
                }
                if let Err(e) = persist.maybe_compact(&tail) {
                    tracing::error!(error = %e, "could not compact the alarm journal");
                }
            }
            Some(Msg::Snapshot(snap)) => pending = Some(snap),
            Some(Msg::Flush(done)) => {
                write(&persist, &mut pending, &mut last_write);
                let _ = done.send(());
            }
            None => {}
        }
        let due = last_write.map_or(true, |at| at.elapsed() >= SNAPSHOT_EVERY);
        if due {
            write(&persist, &mut pending, &mut last_write);
        }
    }
    write(&persist, &mut pending, &mut last_write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::{JOURNAL_FILE, STATE_FILE};
    use crate::state::EventKind;
    use moviclone_tagdb::Value;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moviclone-alarms-writer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn snap(active: bool) -> StateSnapshot {
        let mut map = BTreeMap::new();
        map.insert("A".to_string(), Snapshot { active, ..Snapshot::default() });
        map
    }

    #[tokio::test]
    async fn a_flood_of_snapshots_is_coalesced_and_the_newest_wins() {
        let dir = temp_dir("flood");
        let (persist, tail, _) = Persist::open(&dir).unwrap();
        let w = PersistWriter::start(persist, tail).unwrap();
        for i in 0..500 {
            w.events(vec![AlarmEvent { ts: i, name: "A".into(), event: EventKind::Activated, value: Value::Int(i), user: None }]);
            w.snapshot(snap(i % 2 == 1));
        }
        w.flush().await.unwrap();
        // The first snapshot goes out at once, the rest within the next interval or at the flush.
        assert!(w.snapshots_written() <= 3, "{} snapshot writes", w.snapshots_written());
        let text = std::fs::read_to_string(dir.join(STATE_FILE)).unwrap();
        let back: StateSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(back, snap(true), "the last snapshot (i = 499) is on disk");
        drop(w);
        let lines = std::fs::read_to_string(dir.join(JOURNAL_FILE)).unwrap().lines().count();
        assert_eq!(lines, 500, "every event is journalled");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dropping_the_writer_saves_the_pending_snapshot() {
        let dir = temp_dir("drop");
        let (persist, tail, _) = Persist::open(&dir).unwrap();
        let w = PersistWriter::start(persist, tail).unwrap();
        w.snapshot(snap(false));
        w.snapshot(snap(true));
        drop(w);
        let (_, _, restored) = Persist::open(&dir).unwrap();
        assert_eq!(restored, snap(true));
        std::fs::remove_dir_all(&dir).ok();
    }
}
