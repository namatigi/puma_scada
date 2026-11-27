//! The recording loop: tag changes, heartbeats and data-logger rows go into a
//! buffer that is committed to the journal in batches.
//!
//! Timestamps come from the historian's clock at the moment of recording
//! (within microseconds of the tag store's own timestamp in production), so
//! change samples, heartbeats and logger rows share one timeline.

use crate::codec;
use crate::db::Pending;
use crate::historian::Historian;
use moviclone_model::Recording;
use moviclone_tagdb::{Quality, Sample, TagId, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tokio::time::Instant;

/// Records kept in memory while the disk refuses writes; beyond this the
/// oldest are dropped so a full disk cannot exhaust memory.
const MAX_UNCOMMITTED: usize = 500_000;

impl Historian {
    /// Records until `shutdown` turns true (or its sender is dropped), then
    /// commits what is buffered.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut rx = self.store.subscribe();
        let mut rec = Recorder::new(&self);
        rec.record_all_tags(&self, false);
        rec.flush(&self).await;
        if let Err(e) = self.clone().prune_blocking().await {
            tracing::error!(error = %e, "historian retention failed");
        }
        while !*shutdown.borrow() {
            let deadline = rec.next_deadline();
            tokio::select! {
                biased;
                res = shutdown.changed() => if res.is_err() { break },
                msg = rx.recv() => match msg {
                    Ok(change) => rec.on_change(&self, change.id, &change.sample),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(missed = n, "historian fell behind the tag bus; re-reading current values");
                        rec.recover_from_lag(&self);
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = tokio::time::sleep_until(deadline) => rec.on_time(&self).await,
            }
            if rec.buffer.len() >= self.cfg.max_batch && !rec.commit_failing {
                rec.flush(&self).await;
            }
        }
        rec.flush(&self).await;
    }

    async fn prune_blocking(self: Arc<Self>) -> anyhow::Result<usize> {
        tokio::task::spawn_blocking(move || self.prune()).await?
    }
}

struct Recorder {
    buffer: Vec<Pending>,
    /// Last recorded value and quality per tag (for lag recovery).
    last: Vec<Option<(Value, Quality)>>,
    /// When each tag was last recorded (clock ms), for heartbeats.
    last_at: Vec<i64>,
    next_flush: Instant,
    /// The last commit failed: retry on the flush timer only, not on every record.
    commit_failing: bool,
    next_heartbeat: Instant,
    next_prune: Instant,
    /// Next row time per `every-s` logger.
    logger_due: Vec<Option<(Instant, Duration)>>,
}

impl Recorder {
    fn new(h: &Historian) -> Self {
        let now = Instant::now();
        let logger_due = h
            .plan
            .loggers
            .iter()
            .map(|l| match l.record {
                Recording::EveryS(s) => {
                    let every = Duration::from_secs(u64::from(s.max(1)));
                    Some((now + every, every))
                }
                Recording::OnChange => None,
            })
            .collect();
        Recorder {
            buffer: Vec::new(),
            last: vec![None; h.store.len()],
            last_at: vec![i64::MIN; h.store.len()],
            next_flush: now + h.cfg.flush_every,
            commit_failing: false,
            next_heartbeat: now + heartbeat_tick(h),
            next_prune: now + h.cfg.prune_every,
            logger_due,
        }
    }

    fn next_deadline(&self) -> Instant {
        let mut at = self.next_heartbeat.min(self.next_prune);
        // No idle wake-ups: the flush timer only runs while something is buffered.
        if !self.buffer.is_empty() {
            at = at.min(self.next_flush);
        }
        for (due, _) in self.logger_due.iter().flatten() {
            at = at.min(*due);
        }
        at
    }

    fn on_change(&mut self, h: &Historian, id: TagId, sample: &Sample) {
        let now = h.now_ms();
        self.record_sample(h, id, &sample.value, sample.quality, now);
        for &li in &h.plan.tag_loggers[id as usize] {
            self.record_row(h, li, Some((id, sample)), now);
        }
    }

    /// Samples every historized tag. With `only_changed`, only tags whose
    /// value or quality differ from what was last recorded (after a lag).
    fn record_all_tags(&mut self, h: &Historian, only_changed: bool) {
        let now = h.now_ms();
        for id in h.plan.historized_tags() {
            let s = h.store.get(id);
            let current = Some((s.value.clone(), s.quality));
            if only_changed && self.last[id as usize] == current {
                continue;
            }
            self.record_sample(h, id, &s.value, s.quality, now);
        }
    }

    fn recover_from_lag(&mut self, h: &Historian) {
        self.record_all_tags(h, true);
        // On-change loggers: one row for any logger whose columns may have changed.
        let now = h.now_ms();
        for (li, logger) in h.plan.loggers.iter().enumerate() {
            if logger.record == Recording::OnChange {
                self.record_row(h, li, None, now);
            }
        }
    }

    async fn on_time(&mut self, h: &Arc<Historian>) {
        let now = Instant::now();
        if self.next_heartbeat <= now {
            self.next_heartbeat = now + heartbeat_tick(h);
            let now_ms = h.now_ms();
            let beat = h.cfg.heartbeat.as_millis() as i64;
            let due: Vec<_> = h.plan.historized_tags().filter(|&id| now_ms.saturating_sub(self.last_at[id as usize]) >= beat).collect();
            for id in due {
                let s = h.store.get(id);
                self.record_sample(h, id, &s.value, s.quality, now_ms);
            }
        }
        for li in 0..self.logger_due.len() {
            if let Some((due, every)) = self.logger_due[li] {
                if due <= now {
                    // Skip missed periods rather than writing a burst of identical rows.
                    let mut next = due + every;
                    while next <= now {
                        next += every;
                    }
                    self.logger_due[li] = Some((next, every));
                    self.record_row(h, li, None, h.now_ms());
                }
            }
        }
        if !self.buffer.is_empty() && self.next_flush <= now {
            self.flush(h).await;
        }
        if self.next_prune <= now {
            self.next_prune = now + h.cfg.prune_every;
            if let Err(e) = h.clone().prune_blocking().await {
                tracing::error!(error = %e, "historian retention failed");
            }
        }
    }

    fn record_sample(&mut self, h: &Historian, id: TagId, value: &Value, quality: Quality, ts: i64) {
        let Some(series) = h.plan.tag_series(id) else { return };
        let mut payload = Vec::with_capacity(10);
        codec::encode_sample(value, quality, &mut payload);
        self.push(h, Pending { series, ts, payload });
        self.last[id as usize] = Some((value.clone(), quality));
        self.last_at[id as usize] = ts;
    }

    /// A row with the current value of every column; `changed` overrides the
    /// store for the tag that triggered the row, which may have moved on since.
    fn record_row(&mut self, h: &Historian, li: usize, changed: Option<(TagId, &Sample)>, ts: i64) {
        let logger = &h.plan.loggers[li];
        let cells: Vec<_> = logger
            .columns
            .iter()
            .map(|&(_, id)| match changed {
                Some((cid, s)) if cid == id => (s.value.clone(), s.quality),
                _ => {
                    let s = h.store.get(id);
                    (s.value, s.quality)
                }
            })
            .collect();
        let mut payload = Vec::new();
        codec::encode_row(&cells, &mut payload);
        self.push(h, Pending { series: logger.series, ts, payload });
    }

    fn push(&mut self, h: &Historian, p: Pending) {
        if self.buffer.is_empty() {
            self.next_flush = Instant::now() + h.cfg.flush_every;
        }
        self.buffer.push(p);
    }

    async fn flush(&mut self, h: &Arc<Historian>) {
        if self.buffer.is_empty() {
            return;
        }
        let batch = std::mem::take(&mut self.buffer);
        let hh = h.clone();
        let result = tokio::task::spawn_blocking(move || {
            let r = hh.db.append(&batch);
            (r, batch)
        })
        .await;
        match result {
            Ok((Ok(last), _)) => {
                if self.commit_failing {
                    tracing::info!("historian commits are working again");
                }
                self.commit_failing = false;
                h.committed.send_replace(last);
            }
            Ok((Err(e), mut batch)) => {
                tracing::error!(error = %e, records = batch.len(), "historian could not commit; will retry");
                batch.append(&mut self.buffer);
                if batch.len() > MAX_UNCOMMITTED {
                    let drop = batch.len() - MAX_UNCOMMITTED;
                    tracing::error!(dropped = drop, "historian buffer full; dropping oldest records");
                    batch.drain(..drop);
                }
                self.buffer = batch;
                self.commit_failing = true;
                self.next_flush = Instant::now() + h.cfg.flush_every;
            }
            Err(e) => tracing::error!(error = %e, "historian commit task failed; batch lost"),
        }
    }
}

/// Heartbeats are checked four times per period, so a flat signal gets a
/// point at most 1.25 heartbeats after its last one.
fn heartbeat_tick(h: &Historian) -> Duration {
    (h.cfg.heartbeat / 4).max(Duration::from_millis(10))
}
