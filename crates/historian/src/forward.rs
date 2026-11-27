//! Store-and-forward to external databases.
//!
//! The local journal is the source of truth. Each [`Forwarder`] has a durable
//! watermark (last journal sequence it confirmed); the forward loop sends
//! everything after it, in order, and advances the watermark only after the
//! target confirms. After an outage the target catches up from where it left
//! off. Delivery is at-least-once (a crash between "sent" and "watermark
//! saved" resends one batch), so targets must ignore duplicate `seq`s.

use crate::codec;
use crate::historian::Historian;
use crate::plan::SeriesInfo;
use moviclone_tagdb::{Quality, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One journal record, decoded for a target database.
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardEntry {
    /// Journal sequence number: unique and increasing; use it to de-duplicate.
    pub seq: u64,
    pub ts: i64,
    pub data: ForwardData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ForwardData {
    Sample { historian: String, tag: String, value: Value, quality: Quality },
    Row { logger: String, table: String, columns: Vec<LoggedCell> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoggedCell {
    pub column: String,
    pub value: Value,
    pub quality: Quality,
}

/// A target for journal records (SQL Server, a test sink, …).
pub trait Forwarder: Send + Sync + 'static {
    /// Unique and stable: the watermark is stored under this name.
    fn name(&self) -> &str;
    /// Only records of historians and loggers configured with this
    /// `database` are sent.
    fn database(&self) -> &str;
    /// Stores the batch. An error leaves the watermark where it was; the
    /// batch is retried with backoff.
    fn send<'a>(&'a self, batch: &'a [ForwardEntry]) -> BoxFuture<'a, anyhow::Result<()>>;
}

#[derive(Debug, Clone)]
pub struct ForwardOptions {
    /// Journal records read per round (records for other databases count too).
    pub batch: usize,
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    /// A send that takes longer counts as failed.
    pub send_timeout: Duration,
}

impl Default for ForwardOptions {
    fn default() -> Self {
        ForwardOptions {
            batch: 500,
            min_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            send_timeout: Duration::from_secs(30),
        }
    }
}

struct Batch {
    entries: Vec<ForwardEntry>,
    last_seq: u64,
}

impl Historian {
    /// Forwards journal records to `fwd` until `shutdown` turns true.
    pub async fn run_forwarder(self: Arc<Self>, fwd: Arc<dyn Forwarder>, opts: ForwardOptions, mut shutdown: watch::Receiver<bool>) {
        let mut committed = self.committed.subscribe();
        let mut backoff = opts.min_backoff;
        tracing::info!(forwarder = fwd.name(), database = fwd.database(), "forwarder started");
        while !*shutdown.borrow() {
            committed.borrow_and_update();
            let h = self.clone();
            let f = fwd.clone();
            let batch_size = opts.batch.max(1);
            let step = tokio::task::spawn_blocking(move || h.next_batch(&*f, batch_size)).await;
            let batch = match step {
                Ok(Ok(Some(b))) => b,
                Ok(Ok(None)) => {
                    // Caught up: wait for the next commit.
                    tokio::select! {
                        res = committed.changed() => if res.is_err() { break },
                        res = shutdown.changed() => if res.is_err() { break },
                    }
                    continue;
                }
                Ok(Err(e)) => {
                    tracing::error!(forwarder = fwd.name(), error = %e, "reading the journal failed");
                    if !pause(&mut shutdown, backoff).await {
                        break;
                    }
                    backoff = (backoff * 2).min(opts.max_backoff);
                    continue;
                }
                Err(e) => {
                    tracing::error!(forwarder = fwd.name(), error = %e, "journal reader task failed");
                    break;
                }
            };
            let sent = if batch.entries.is_empty() {
                Ok(())
            } else {
                match tokio::time::timeout(opts.send_timeout, fwd.send(&batch.entries)).await {
                    Ok(r) => r,
                    Err(_) => Err(anyhow::anyhow!("no answer within {:?}", opts.send_timeout)),
                }
            };
            match sent {
                Ok(()) => {
                    if let Err(e) = self.save_watermark(fwd.name().to_string(), batch.last_seq).await {
                        tracing::error!(forwarder = fwd.name(), error = %e, "saving the watermark failed");
                    }
                    if backoff > opts.min_backoff {
                        tracing::info!(forwarder = fwd.name(), "forwarder reconnected");
                    }
                    backoff = opts.min_backoff;
                }
                Err(e) => {
                    tracing::warn!(forwarder = fwd.name(), error = %e, retry_in = ?backoff, "forwarding failed; records stay in the local journal");
                    if !pause(&mut shutdown, backoff).await {
                        break;
                    }
                    backoff = (backoff * 2).min(opts.max_backoff);
                }
            }
        }
        tracing::info!(forwarder = fwd.name(), "forwarder stopped");
    }

    /// The next records after the forwarder's watermark, decoded and filtered
    /// to its database; `None` when there is nothing new.
    fn next_batch(&self, fwd: &dyn Forwarder, limit: usize) -> anyhow::Result<Option<Batch>> {
        let wm = self.db.watermark(fwd.name())?;
        let stored = self.db.journal_after(wm, limit)?;
        let Some(first) = stored.first() else { return Ok(None) };
        if wm > 0 && first.seq > wm + 1 {
            tracing::warn!(
                forwarder = fwd.name(),
                lost = first.seq - wm - 1,
                "records were pruned by retention before they could be forwarded"
            );
        }
        let last_seq = stored.last().map(|s| s.seq).unwrap_or(wm);
        let mut entries = Vec::with_capacity(stored.len());
        for s in stored {
            if self.plan.database_of(s.series) != Some(fwd.database()) {
                continue;
            }
            let data = match self.plan.series.get(&s.series) {
                Some(SeriesInfo::Tag { tag, historian }) => {
                    let ((value, quality), _) = codec::decode_sample(&s.payload)?;
                    ForwardData::Sample { historian: self.plan.historians[*historian].name.clone(), tag: tag.clone(), value, quality }
                }
                Some(SeriesInfo::Logger { logger }) => {
                    let l = &self.plan.loggers[*logger];
                    let cells = codec::decode_row(&s.payload)?;
                    let columns = l
                        .columns
                        .iter()
                        .zip(cells)
                        .map(|((column, _), (value, quality))| LoggedCell { column: column.clone(), value, quality })
                        .collect();
                    ForwardData::Row { logger: l.name.clone(), table: l.table.clone(), columns }
                }
                None => continue,
            };
            entries.push(ForwardEntry { seq: s.seq, ts: s.ts, data });
        }
        Ok(Some(Batch { entries, last_seq }))
    }

    async fn save_watermark(self: &Arc<Self>, name: String, seq: u64) -> anyhow::Result<()> {
        let h = self.clone();
        tokio::task::spawn_blocking(move || h.db.set_watermark(&name, seq)).await?
    }
}

/// Sleeps for `d`; false if shutdown was requested meanwhile.
async fn pause(shutdown: &mut watch::Receiver<bool>, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => !*shutdown.borrow(),
        res = shutdown.changed() => res.is_ok() && !*shutdown.borrow(),
    }
}

/// In-memory target for tests and diagnostics. `set_failing(true)` simulates
/// an outage.
pub struct MemoryForwarder {
    name: String,
    database: String,
    received: Mutex<Vec<ForwardEntry>>,
    failing: AtomicBool,
}

impl MemoryForwarder {
    pub fn new(name: &str, database: &str) -> Arc<Self> {
        Arc::new(MemoryForwarder {
            name: name.to_string(),
            database: database.to_string(),
            received: Mutex::new(Vec::new()),
            failing: AtomicBool::new(false),
        })
    }

    pub fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    pub fn received(&self) -> Vec<ForwardEntry> {
        self.received.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl Forwarder for MemoryForwarder {
    fn name(&self) -> &str {
        &self.name
    }
    fn database(&self) -> &str {
        &self.database
    }
    fn send<'a>(&'a self, batch: &'a [ForwardEntry]) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if self.failing.load(Ordering::SeqCst) {
                anyhow::bail!("{} is unreachable (simulated)", self.name);
            }
            let mut received = self.received.lock().unwrap_or_else(|e| e.into_inner());
            // De-duplicate like a real target would (at-least-once delivery).
            let last = received.last().map(|e| e.seq).unwrap_or(0);
            received.extend(batch.iter().filter(|e| e.seq > last).cloned());
            Ok(())
        })
    }
}
