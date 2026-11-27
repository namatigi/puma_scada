//! The alarm engine: evaluates every alarm on tag changes, runs delay and
//! shelve timers, applies operator actions and persists the outcome.

use crate::clock::Clock;
use crate::persist::{Persist, JOURNAL_KEEP};
use crate::writer::PersistWriter;
use crate::state::{display_order, AlarmEvent, AlarmState, EventKind, Now, Slot, StateKind};
use moviclone_model::Project;
use moviclone_tagdb::{Change, TagId, TagStore};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{broadcast, watch, Notify};
use tokio::time::{Duration, Instant};

/// Longest shelve an operator can request (one week).
pub const MAX_SHELVE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Changes processed under one lock before persisting and publishing.
const MAX_BATCH: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlarmError {
    UnknownAlarm(String),
    InvalidDuration(String),
}

impl fmt::Display for AlarmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AlarmError::UnknownAlarm(n) => write!(f, "alarm {n:?} does not exist"),
            AlarmError::InvalidDuration(m) => write!(f, "invalid shelve duration: {m}"),
        }
    }
}

impl std::error::Error for AlarmError {}

pub struct AlarmEngine {
    store: Arc<TagStore>,
    clock: Clock,
    inner: Mutex<Inner>,
    feed: watch::Sender<serde_json::Value>,
    events: broadcast::Sender<AlarmEvent>,
    /// Woken when an operator action moves a timer deadline (shelve, unshelve).
    wake: Notify,
}

struct Inner {
    slots: Vec<Slot>,
    by_name: HashMap<String, usize>,
    by_tag: HashMap<TagId, Vec<usize>>,
    journal: VecDeque<AlarmEvent>,
    /// Disk writes happen on its thread, never under this mutex.
    persist: PersistWriter,
}

impl AlarmEngine {
    /// Loads the alarm definitions of `project` and restores journal and state
    /// from `dir` (created if missing). Timestamps use the system clock.
    pub fn open(project: &Project, store: Arc<TagStore>, dir: &Path) -> anyhow::Result<Arc<Self>> {
        Self::open_with_clock(project, store, dir, Clock::system())
    }

    /// Like [`AlarmEngine::open`] with an explicit clock (tests use [`Clock::tokio`]).
    pub fn open_with_clock(project: &Project, store: Arc<TagStore>, dir: &Path, clock: Clock) -> anyhow::Result<Arc<Self>> {
        let (persist, journal, mut snapshot) = Persist::open(dir)?;
        let persist = PersistWriter::start(persist, journal.clone())?;
        let now = Now { at: Instant::now(), ms: clock.now_ms() };
        let mut slots = Vec::with_capacity(project.alarms.len());
        let mut by_name = HashMap::new();
        let mut by_tag: HashMap<TagId, Vec<usize>> = HashMap::new();
        for def in &project.alarms {
            if by_name.contains_key(&def.name) {
                tracing::warn!(alarm = %def.name, "duplicate alarm name; keeping the first definition");
                continue;
            }
            let tag = store.id(&def.tag);
            if tag.is_none() {
                tracing::warn!(alarm = %def.name, tag = %def.tag, "alarm refers to an unknown tag and will never activate");
            }
            let idx = slots.len();
            let mut slot = Slot::new(def.clone(), tag);
            if let Some(snap) = snapshot.remove(&def.name) {
                slot.restore(snap, now);
            }
            if let Some(id) = tag {
                by_tag.entry(id).or_default().push(idx);
            }
            by_name.insert(def.name.clone(), idx);
            slots.push(slot);
        }
        let inner = Inner { slots, by_name, by_tag, journal, persist };
        let (feed, _) = watch::channel(feed_message(&inner.slots));
        let (events, _) = broadcast::channel(1024);
        Ok(Arc::new(AlarmEngine { store, clock, inner: Mutex::new(inner), feed, events, wake: Notify::new() }))
    }

    /// Evaluates alarms until `shutdown` turns true (or its sender is dropped),
    /// then writes a final state snapshot.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        // Subscribe before the initial evaluation so no change slips between the two.
        let mut rx = self.store.subscribe();
        self.evaluate_all();
        while !*shutdown.borrow() {
            let deadline = self.next_deadline();
            let timer = async move {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                res = shutdown.changed() => if res.is_err() { break },
                msg = rx.recv() => match msg {
                    Ok(first) => {
                        let mut batch = vec![first];
                        while batch.len() < MAX_BATCH {
                            match rx.try_recv() {
                                Ok(c) => batch.push(c),
                                Err(_) => break,
                            }
                        }
                        self.on_changes(&batch);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "alarm engine fell behind the tag bus; re-reading all alarm tags");
                        self.evaluate_all();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = timer => self.on_time(),
                _ = self.wake.notified() => {}
            }
        }
        let saved = {
            let inner = self.lock();
            inner.persist.snapshot(snapshot_of(&inner.slots));
            inner.persist.flush()
        };
        if saved.await.is_err() {
            tracing::error!("could not save alarm state on shutdown");
        }
    }

    /// Every configured alarm, in display order (unacknowledged first).
    pub fn states(&self) -> Vec<AlarmState> {
        sorted_states(&self.lock().slots)
    }

    /// Alarms that are not `normal` or `disabled`.
    pub fn active(&self) -> Vec<AlarmState> {
        self.states().into_iter().filter(|s| !matches!(s.state, StateKind::Normal | StateKind::Disabled)).collect()
    }

    pub fn state(&self, name: &str) -> Option<AlarmState> {
        let inner = self.lock();
        inner.by_name.get(name).map(|&i| inner.slots[i].view())
    }

    /// Journal events, newest first.
    pub fn journal(&self, limit: usize) -> Vec<AlarmEvent> {
        self.lock().journal.iter().rev().take(limit).cloned().collect()
    }

    /// Every journal event from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<AlarmEvent> {
        self.events.subscribe()
    }

    /// Always holds the complete WebSocket message
    /// `{"type":"alarms","alarms":[AlarmState…]}`; changes on every state change.
    pub fn ws_feed(&self) -> watch::Receiver<serde_json::Value> {
        self.feed.subscribe()
    }

    /// Acknowledges one alarm. Returns whether anything changed (acking an
    /// alarm that needs no acknowledgement is not an error).
    pub fn ack(&self, name: &str, user: Option<&str>) -> Result<bool, AlarmError> {
        let now = self.now();
        let mut inner = self.lock();
        let idx = index(&inner, name)?;
        let kinds = inner.slots[idx].ack(user);
        let changed = !kinds.is_empty();
        self.commit(&mut inner, vec![(idx, kinds)], user, now);
        Ok(changed)
    }

    /// Acknowledges every unacknowledged alarm; returns how many.
    pub fn ack_all(&self, user: Option<&str>) -> usize {
        let now = self.now();
        let mut inner = self.lock();
        let changes: Vec<_> = (0..inner.slots.len()).map(|i| (i, inner.slots[i].ack(user))).filter(|(_, k)| !k.is_empty()).collect();
        let n = changes.len();
        self.commit(&mut inner, changes, user, now);
        n
    }

    /// Shelves an alarm for `duration` (1 s up to [`MAX_SHELVE`]); it unshelves
    /// itself when the time is up.
    pub fn shelve(&self, name: &str, duration: Duration, user: Option<&str>) -> Result<(), AlarmError> {
        if duration < Duration::from_secs(1) || duration > MAX_SHELVE {
            return Err(AlarmError::InvalidDuration(format!("{} s is outside 1 s ..= {} s", duration.as_secs_f64(), MAX_SHELVE.as_secs())));
        }
        let now = self.now();
        let mut inner = self.lock();
        let idx = index(&inner, name)?;
        let kinds = inner.slots[idx].shelve(duration, now);
        self.commit(&mut inner, vec![(idx, kinds)], user, now);
        drop(inner);
        self.wake.notify_one();
        Ok(())
    }

    pub fn unshelve(&self, name: &str, user: Option<&str>) -> Result<(), AlarmError> {
        let now = self.now();
        let mut inner = self.lock();
        let idx = index(&inner, name)?;
        let kinds = inner.slots[idx].manual_unshelve(now);
        self.commit(&mut inner, vec![(idx, kinds)], user, now);
        drop(inner);
        self.wake.notify_one();
        Ok(())
    }

    fn now(&self) -> Now {
        Now { at: Instant::now(), ms: self.clock.now_ms() }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.lock().slots.iter().filter_map(Slot::next_deadline).min()
    }

    fn evaluate_all(&self) {
        let now = self.now();
        let mut inner = self.lock();
        let changes: Vec<_> = (0..inner.slots.len())
            .filter_map(|i| {
                let id = inner.slots[i].tag?;
                let sample = self.store.get(id);
                Some((i, inner.slots[i].on_sample(&sample, now)))
            })
            .collect();
        self.commit(&mut inner, changes, None, now);
    }

    fn on_changes(&self, batch: &[Change]) {
        let now = self.now();
        let mut inner = self.lock();
        let mut changes = Vec::new();
        for change in batch {
            let Some(idxs) = inner.by_tag.get(&change.id).cloned() else { continue };
            for i in idxs {
                changes.push((i, inner.slots[i].on_sample(&change.sample, now)));
            }
        }
        self.commit(&mut inner, changes, None, now);
    }

    fn on_time(&self) {
        let now = self.now();
        let mut inner = self.lock();
        let changes: Vec<_> = (0..inner.slots.len()).map(|i| (i, inner.slots[i].on_time(now))).collect();
        self.commit(&mut inner, changes, None, now);
    }

    /// Journals, broadcasts and persists the events of one batch, then
    /// publishes the new alarm list if anything happened.
    fn commit(&self, inner: &mut Inner, changes: Vec<(usize, Vec<EventKind>)>, user: Option<&str>, now: Now) {
        let mut events = Vec::new();
        for (idx, kinds) in changes {
            for kind in kinds {
                let slot = &inner.slots[idx];
                let user = match kind {
                    EventKind::Acked | EventKind::Shelved | EventKind::Unshelved => user.map(str::to_string),
                    EventKind::Activated | EventKind::Cleared => None,
                };
                events.push(AlarmEvent { ts: now.ms, name: slot.def.name.clone(), event: kind, value: slot.view().value, user });
            }
        }
        if events.is_empty() {
            return;
        }
        for event in &events {
            tracing::info!(alarm = %event.name, event = ?event.event, "alarm event");
            if inner.journal.len() == JOURNAL_KEEP {
                inner.journal.pop_front();
            }
            inner.journal.push_back(event.clone());
            let _ = self.events.send(event.clone());
        }
        // Queued only: the persistence thread does the disk work.
        inner.persist.events(events);
        inner.persist.snapshot(snapshot_of(&inner.slots));
        self.feed.send_replace(feed_message(&inner.slots));
    }
}

fn index(inner: &Inner, name: &str) -> Result<usize, AlarmError> {
    inner.by_name.get(name).copied().ok_or_else(|| AlarmError::UnknownAlarm(name.to_string()))
}

fn sorted_states(slots: &[Slot]) -> Vec<AlarmState> {
    let mut states: Vec<_> = slots.iter().map(Slot::view).collect();
    states.sort_by(display_order);
    states
}

fn feed_message(slots: &[Slot]) -> serde_json::Value {
    serde_json::json!({ "type": "alarms", "alarms": sorted_states(slots) })
}

fn snapshot_of(slots: &[Slot]) -> BTreeMap<String, crate::state::Snapshot> {
    slots.iter().map(|s| (s.def.name.clone(), s.snapshot())).collect()
}
