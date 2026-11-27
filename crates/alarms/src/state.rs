//! Per-alarm ISA-18.2 state machine. Pure and synchronous: the engine feeds it
//! samples, operator actions and the passing of time, and journals the events
//! it returns.

use crate::condition;
use moviclone_model::{Alarm, AlarmSeverity};
use moviclone_tagdb::{Sample, TagId, Value};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, Instant};

/// ISA-18.2 alarm state as sent to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateKind {
    Normal,
    ActiveUnacked,
    ActiveAcked,
    ClearedUnacked,
    Shelved,
    Disabled,
}

impl StateKind {
    /// Display priority: what needs the operator's attention first.
    fn rank(self) -> u8 {
        match self {
            StateKind::ActiveUnacked => 0,
            StateKind::ClearedUnacked => 1,
            StateKind::ActiveAcked => 2,
            StateKind::Shelved => 3,
            StateKind::Normal => 4,
            StateKind::Disabled => 5,
        }
    }
}

/// `AlarmState` JSON from docs/PROTOCOL.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlarmState {
    pub name: String,
    pub tag: String,
    pub message: String,
    pub severity: AlarmSeverity,
    pub area: Option<String>,
    pub state: StateKind,
    /// Tag value at the last state transition (the value that raised or cleared it).
    pub value: Value,
    /// When the alarm last activated or cleared (ms since the Unix epoch).
    pub since: Option<i64>,
    pub acked_by: Option<String>,
    pub shelved_until: Option<i64>,
}

/// Orders states for display: unacknowledged first, then severity, newest first.
pub fn display_order(a: &AlarmState, b: &AlarmState) -> std::cmp::Ordering {
    a.state
        .rank()
        .cmp(&b.state.rank())
        .then(b.severity.cmp(&a.severity))
        .then(b.since.cmp(&a.since))
        .then(a.name.cmp(&b.name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventKind {
    Activated,
    Cleared,
    Acked,
    Shelved,
    Unshelved,
}

/// `AlarmEvent` JSON from docs/PROTOCOL.md.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlarmEvent {
    pub ts: i64,
    pub name: String,
    pub event: EventKind,
    pub value: Value,
    pub user: Option<String>,
}

/// What survives a restart (see `persist`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub active: bool,
    pub unacked: bool,
    pub since: Option<i64>,
    pub value: Value,
    pub acked_by: Option<String>,
    pub shelved_until: Option<i64>,
}

#[derive(Debug)]
pub(crate) struct Slot {
    pub def: Alarm,
    pub tag: Option<TagId>,
    /// Last condition result after deadband, before the activation delay.
    cond: bool,
    /// The condition holds and the alarm activates at this instant.
    pending: Option<Instant>,
    active: bool,
    /// An activation the operator has not acknowledged yet.
    unacked: bool,
    since: Option<i64>,
    value: Value,
    acked_by: Option<String>,
    shelved: Option<(i64, Instant)>,
}

/// Time as both clocks see it: tokio's for timers, wall ms for the journal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Now {
    pub at: Instant,
    pub ms: i64,
}

impl Slot {
    pub fn new(def: Alarm, tag: Option<TagId>) -> Self {
        Slot {
            def,
            tag,
            cond: false,
            pending: None,
            active: false,
            unacked: false,
            since: None,
            value: Value::Null,
            acked_by: None,
            shelved: None,
        }
    }

    pub fn state(&self) -> StateKind {
        if !self.def.enabled {
            StateKind::Disabled
        } else if self.shelved.is_some() {
            StateKind::Shelved
        } else if self.active && self.unacked {
            StateKind::ActiveUnacked
        } else if self.active {
            StateKind::ActiveAcked
        } else if self.unacked {
            StateKind::ClearedUnacked
        } else {
            StateKind::Normal
        }
    }

    pub fn view(&self) -> AlarmState {
        AlarmState {
            name: self.def.name.clone(),
            tag: self.def.tag.clone(),
            message: self.def.message.clone(),
            severity: self.def.severity,
            area: self.def.area.clone(),
            state: self.state(),
            value: self.value.clone(),
            since: self.since,
            acked_by: self.acked_by.clone(),
            shelved_until: self.shelved.map(|(ms, _)| ms),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            active: self.active,
            unacked: self.unacked,
            since: self.since,
            value: self.value.clone(),
            acked_by: self.acked_by.clone(),
            shelved_until: self.shelved.map(|(ms, _)| ms),
        }
    }

    /// Restores a snapshot from before a restart. The condition is assumed to
    /// still hold until a fresh sample says otherwise, so an alarm that was
    /// active does not clear and re-raise just because the runtime restarted.
    /// An expired shelve is dropped (the next deadline check would unshelve it
    /// anyway, but there is nothing to announce).
    pub fn restore(&mut self, snap: Snapshot, now: Now) {
        if !self.def.enabled {
            return;
        }
        self.active = snap.active;
        self.cond = snap.active;
        self.unacked = snap.unacked;
        self.since = snap.since;
        self.value = snap.value;
        self.acked_by = snap.acked_by;
        self.shelved = snap.shelved_until.filter(|until| *until > now.ms).map(|until| {
            let remaining = Duration::from_millis((until - now.ms) as u64);
            (until, now.at + remaining)
        });
    }

    /// Feeds a new sample of the alarm's tag.
    pub fn on_sample(&mut self, sample: &Sample, now: Now) -> Vec<EventKind> {
        if !self.def.enabled {
            return Vec::new();
        }
        let Some(cond) = condition::evaluate(&self.def.condition, self.def.deadband, self.cond, sample) else {
            return Vec::new();
        };
        self.cond = cond;
        let mut events = Vec::new();
        if cond {
            if !self.active && self.pending.is_none() {
                if self.def.delay_s > 0.0 {
                    self.pending = Some(now.at + Duration::from_secs_f64(self.def.delay_s));
                } else {
                    self.activate(&sample.value, now, &mut events);
                }
            }
        } else {
            self.pending = None;
            if self.active {
                self.clear(&sample.value, now, &mut events);
            }
        }
        if !events.is_empty() || self.pending.is_some() {
            // Remember the value that matters: the one that raised (or will raise) or cleared the alarm.
            self.value = sample.value.clone();
        }
        events
    }

    /// Fires due activation delays and shelve expiries.
    pub fn on_time(&mut self, now: Now) -> Vec<EventKind> {
        let mut events = Vec::new();
        if self.pending.is_some_and(|at| at <= now.at) {
            self.pending = None;
            if self.cond && !self.active {
                let value = self.value.clone();
                self.activate(&value, now, &mut events);
            }
        }
        if self.shelved.is_some_and(|(_, at)| at <= now.at) {
            self.unshelve(now, &mut events);
        }
        events
    }

    /// Earliest instant at which [`Slot::on_time`] has something to do.
    pub fn next_deadline(&self) -> Option<Instant> {
        let shelve = self.shelved.map(|(_, at)| at);
        match (self.pending, shelve) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn ack(&mut self, user: Option<&str>) -> Vec<EventKind> {
        if !self.def.enabled || self.shelved.is_some() || !self.unacked {
            return Vec::new();
        }
        self.unacked = false;
        self.acked_by = user.map(str::to_string);
        vec![EventKind::Acked]
    }

    /// Shelving hides the alarm from operators and drops any pending
    /// acknowledgement; the condition is still tracked so that unshelving
    /// shows the alarm's real state.
    pub fn shelve(&mut self, duration: Duration, now: Now) -> Vec<EventKind> {
        if !self.def.enabled {
            return Vec::new();
        }
        let until_ms = now.ms + duration.as_millis() as i64;
        self.shelved = Some((until_ms, now.at + duration));
        self.unacked = false;
        vec![EventKind::Shelved]
    }

    pub fn manual_unshelve(&mut self, now: Now) -> Vec<EventKind> {
        let mut events = Vec::new();
        if self.shelved.is_some() {
            self.unshelve(now, &mut events);
        }
        events
    }

    fn unshelve(&mut self, now: Now, events: &mut Vec<EventKind>) {
        self.shelved = None;
        events.push(EventKind::Unshelved);
        if self.active {
            // Still active after the shelve: the operator has to see and acknowledge it.
            self.unacked = true;
            self.acked_by = None;
            self.since = Some(now.ms);
        }
    }

    fn activate(&mut self, value: &Value, now: Now, events: &mut Vec<EventKind>) {
        self.active = true;
        self.since = Some(now.ms);
        self.value = value.clone();
        if self.shelved.is_none() {
            self.unacked = true;
            self.acked_by = None;
            events.push(EventKind::Activated);
        }
    }

    fn clear(&mut self, value: &Value, now: Now, events: &mut Vec<EventKind>) {
        self.active = false;
        self.since = Some(now.ms);
        self.value = value.clone();
        if !self.def.ack_required {
            self.unacked = false;
        }
        if self.shelved.is_none() {
            events.push(EventKind::Cleared);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use moviclone_model::AlarmCondition;
    use moviclone_tagdb::Quality;

    fn alarm(delay_s: f64, ack_required: bool) -> Alarm {
        Alarm {
            name: "High".into(),
            tag: "T".into(),
            condition: AlarmCondition::Above { limit: 10.0 },
            severity: AlarmSeverity::High,
            message: "high".into(),
            area: None,
            deadband: 1.0,
            delay_s,
            ack_required,
            enabled: true,
        }
    }

    fn sample(v: f64) -> Sample {
        Sample { value: Value::Float(v), quality: Quality::Good, ts: 0 }
    }

    fn now(at: Instant, ms: i64) -> Now {
        Now { at, ms }
    }

    #[test]
    fn full_isa_cycle() {
        let t0 = Instant::now();
        let mut s = Slot::new(alarm(0.0, true), Some(0));
        assert_eq!(s.state(), StateKind::Normal);
        assert_eq!(s.on_sample(&sample(11.0), now(t0, 1)), vec![EventKind::Activated]);
        assert_eq!(s.state(), StateKind::ActiveUnacked);
        assert_eq!(s.ack(Some("op")), vec![EventKind::Acked]);
        assert_eq!(s.state(), StateKind::ActiveAcked);
        assert_eq!(s.view().acked_by.as_deref(), Some("op"));
        // Inside the deadband: still active.
        assert!(s.on_sample(&sample(9.5), now(t0, 2)).is_empty());
        assert_eq!(s.on_sample(&sample(8.0), now(t0, 3)), vec![EventKind::Cleared]);
        assert_eq!(s.state(), StateKind::Normal);
    }

    #[test]
    fn cleared_unacked_then_ack() {
        let t0 = Instant::now();
        let mut s = Slot::new(alarm(0.0, true), Some(0));
        s.on_sample(&sample(11.0), now(t0, 1));
        s.on_sample(&sample(0.0), now(t0, 2));
        assert_eq!(s.state(), StateKind::ClearedUnacked);
        assert_eq!(s.view().since, Some(2));
        assert_eq!(s.ack(None), vec![EventKind::Acked]);
        assert_eq!(s.state(), StateKind::Normal);
        assert!(s.ack(None).is_empty(), "second ack is a no-op");
    }

    #[test]
    fn no_ack_required_returns_to_normal_on_clear() {
        let t0 = Instant::now();
        let mut s = Slot::new(alarm(0.0, false), Some(0));
        s.on_sample(&sample(11.0), now(t0, 1));
        s.on_sample(&sample(0.0), now(t0, 2));
        assert_eq!(s.state(), StateKind::Normal);
    }

    #[test]
    fn delay_cancelled_when_condition_drops() {
        let t0 = Instant::now();
        let mut s = Slot::new(alarm(5.0, true), Some(0));
        assert!(s.on_sample(&sample(11.0), now(t0, 0)).is_empty());
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_secs(5)));
        s.on_sample(&sample(1.0), now(t0, 1));
        assert_eq!(s.next_deadline(), None);
        assert!(s.on_time(now(t0 + Duration::from_secs(6), 6000)).is_empty());
        assert_eq!(s.state(), StateKind::Normal);
    }

    #[test]
    fn shelve_suppresses_and_unshelve_reannounces() {
        let t0 = Instant::now();
        let mut s = Slot::new(alarm(0.0, true), Some(0));
        s.on_sample(&sample(11.0), now(t0, 0));
        assert_eq!(s.shelve(Duration::from_secs(60), now(t0, 0)), vec![EventKind::Shelved]);
        assert_eq!(s.state(), StateKind::Shelved);
        assert_eq!(s.view().shelved_until, Some(60_000));
        // Ack is refused while shelved; clears and re-activations are silent.
        assert!(s.ack(None).is_empty());
        assert!(s.on_sample(&sample(0.0), now(t0, 1)).is_empty());
        assert!(s.on_sample(&sample(12.0), now(t0, 2)).is_empty());
        let later = now(t0 + Duration::from_secs(60), 60_000);
        assert_eq!(s.on_time(later), vec![EventKind::Unshelved]);
        assert_eq!(s.state(), StateKind::ActiveUnacked);
    }

    #[test]
    fn disabled_alarm_ignores_everything() {
        let mut def = alarm(0.0, true);
        def.enabled = false;
        let mut s = Slot::new(def, Some(0));
        let t0 = Instant::now();
        assert!(s.on_sample(&sample(100.0), now(t0, 0)).is_empty());
        assert!(s.shelve(Duration::from_secs(1), now(t0, 0)).is_empty());
        assert_eq!(s.state(), StateKind::Disabled);
    }

    #[test]
    fn restore_keeps_active_and_unexpired_shelve() {
        let t0 = Instant::now();
        let mut s = Slot::new(alarm(0.0, true), Some(0));
        let snap = Snapshot { active: true, unacked: true, since: Some(5), value: Value::Float(11.0), acked_by: None, shelved_until: None };
        s.restore(snap, now(t0, 100));
        assert_eq!(s.state(), StateKind::ActiveUnacked);
        // Still above limit - deadband: no spurious clear/activate after restart.
        assert!(s.on_sample(&sample(9.5), now(t0, 101)).is_empty());

        let mut s = Slot::new(alarm(0.0, true), Some(0));
        let snap = Snapshot { shelved_until: Some(10_100), ..Snapshot::default() };
        s.restore(snap, now(t0, 100));
        assert_eq!(s.next_deadline(), Some(t0 + Duration::from_secs(10)));
        let mut s = Slot::new(alarm(0.0, true), Some(0));
        s.restore(Snapshot { shelved_until: Some(50), ..Snapshot::default() }, now(t0, 100));
        assert_eq!(s.state(), StateKind::Normal);
    }
}
