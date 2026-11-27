//! The `Historian` handle: opening the journal and answering queries.
//! Recording lives in `recorder`, SQL forwarding in `forward`.

use crate::clock::Clock;
use crate::codec;
use crate::db::Db;
use crate::downsample::{Downsampler, Point};
use crate::plan::Plan;
use moviclone_model::Project;
use moviclone_tagdb::{Quality, TagStore, Value};
use serde::Serialize;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// File name of the journal inside the data directory.
pub const DB_FILE: &str = "historian.redb";

#[derive(Debug, Clone)]
pub struct HistorianConfig {
    /// A sample of every historized tag at least this often, so flat signals
    /// still have points in any time window.
    pub heartbeat: Duration,
    /// Recorded samples are committed in batches this often.
    pub flush_every: Duration,
    /// Retention check interval.
    pub prune_every: Duration,
    /// A batch is committed early once it holds this many records.
    pub max_batch: usize,
    pub clock: Clock,
}

impl Default for HistorianConfig {
    fn default() -> Self {
        HistorianConfig {
            heartbeat: Duration::from_secs(60),
            flush_every: Duration::from_secs(1),
            prune_every: Duration::from_secs(600),
            max_batch: 5_000,
            clock: Clock::system(),
        }
    }
}

/// `{"tag":"A","points":[[ts,v],…]}`
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Series {
    pub tag: String,
    pub points: Vec<Point>,
}

/// `{"columns":[…],"rows":[[…],…]}`
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

/// Which kind of source a history-table query names. Historians and data
/// loggers often share names (Petrogas has both an `FM01` historian and an
/// `FM01` logger); without an explicit kind the historian wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Historian,
    Logger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    UnknownSource(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryError::UnknownSource(s) => write!(f, "no enabled historian or data logger named {s:?}"),
        }
    }
}

impl std::error::Error for QueryError {}

pub struct Historian {
    pub(crate) db: Db,
    pub(crate) plan: Plan,
    pub(crate) store: Arc<TagStore>,
    pub(crate) cfg: HistorianConfig,
    /// Last committed sequence number; forwarders wake on changes.
    pub(crate) committed: watch::Sender<u64>,
}

impl Historian {
    /// Opens (or creates) `<dir>/historian.redb` with default settings.
    pub fn open(project: &Project, store: Arc<TagStore>, dir: &Path) -> anyhow::Result<Arc<Self>> {
        Self::open_with(project, store, dir, HistorianConfig::default())
    }

    pub fn open_with(project: &Project, store: Arc<TagStore>, dir: &Path, cfg: HistorianConfig) -> anyhow::Result<Arc<Self>> {
        let db = Db::open(&dir.join(DB_FILE))?;
        let plan = Plan::build(project, &store, &db)?;
        let (committed, _) = watch::channel(db.last_seq());
        tracing::info!(
            historians = plan.historians.len(),
            tags = plan.historized_tags().count(),
            loggers = plan.loggers.len(),
            "historian opened"
        );
        Ok(Arc::new(Historian { db, plan, store, cfg, committed }))
    }

    pub fn now_ms(&self) -> i64 {
        self.cfg.clock.now_ms()
    }

    /// Series for `tags` in `[from_ms, to_ms]`, each downsampled to at most
    /// `max_points` (min 2) with per-bucket min/max. Bad-quality and
    /// non-numeric samples are gaps (`null`). Tags that are not historized
    /// return an empty series.
    pub fn query(&self, tags: &[String], from_ms: i64, to_ms: i64, max_points: usize) -> anyhow::Result<Vec<Series>> {
        tags.iter()
            .map(|tag| {
                let mut points = Vec::new();
                if let Some(series) = self.store.id(tag).and_then(|id| self.plan.tag_series(id)) {
                    let mut ds = Downsampler::new(from_ms, to_ms, max_points);
                    self.db.scan(series, from_ms, to_ms, |ts, payload| {
                        let ((value, quality), _) = codec::decode_sample(payload)?;
                        ds.push(ts, trend_value(&value, quality));
                        Ok(())
                    })?;
                    points = ds.finish();
                }
                Ok(Series { tag: tag.clone(), points })
            })
            .collect()
    }

    /// Recorded rows of a historian (`time, tag, value, quality`, one row per
    /// sample) or data logger (`time` plus one column per logger column),
    /// newest first.
    pub fn table(&self, source: &str, kind: Option<SourceKind>, from_ms: i64, to_ms: i64, limit: usize) -> anyhow::Result<Table> {
        let historian = self.plan.historian(source).filter(|_| kind != Some(SourceKind::Logger));
        let logger = self.plan.logger(source).filter(|_| kind != Some(SourceKind::Historian));
        if let Some(hi) = historian {
            return self.historian_table(hi, from_ms, to_ms, limit);
        }
        if let Some(li) = logger {
            return self.logger_table(li, from_ms, to_ms, limit);
        }
        Err(QueryError::UnknownSource(source.to_string()).into())
    }

    fn historian_table(&self, hi: usize, from_ms: i64, to_ms: i64, limit: usize) -> anyhow::Result<Table> {
        let mut rows = Vec::new();
        for &tag in &self.plan.historians[hi].tags {
            let Some(series) = self.plan.tag_series(tag) else { continue };
            for (ts, seq, payload) in self.db.newest(series, from_ms, to_ms, limit)? {
                let ((value, quality), _) = codec::decode_sample(&payload)?;
                rows.push((ts, seq, tag, value, quality));
            }
        }
        rows.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
        rows.truncate(limit);
        Ok(Table {
            columns: ["time", "tag", "value", "quality"].map(String::from).to_vec(),
            rows: rows
                .into_iter()
                .map(|(ts, _, tag, value, quality)| {
                    vec![ts.into(), self.store.name(tag).into(), json(&value), json(&quality)]
                })
                .collect(),
        })
    }

    fn logger_table(&self, li: usize, from_ms: i64, to_ms: i64, limit: usize) -> anyhow::Result<Table> {
        let logger = &self.plan.loggers[li];
        let mut columns = vec!["time".to_string()];
        columns.extend(logger.columns.iter().map(|(name, _)| name.clone()));
        let mut rows = Vec::new();
        for (ts, _, payload) in self.db.newest(logger.series, from_ms, to_ms, limit)? {
            let mut row = vec![serde_json::Value::from(ts)];
            for (value, quality) in codec::decode_row(&payload)? {
                // A wide row has no room for per-cell quality: bad cells show as empty.
                row.push(if quality == Quality::Bad { serde_json::Value::Null } else { json(&value) });
            }
            rows.push(row);
        }
        Ok(Table { columns, rows })
    }

    /// Records not yet confirmed by the named forwarder (an upper bound: it
    /// includes records for other databases).
    pub fn forward_backlog(&self, forwarder: &str) -> anyhow::Result<u64> {
        Ok(self.db.last_seq().saturating_sub(self.db.watermark(forwarder)?))
    }

    /// Deletes records past each historian's / logger's `max_age_s`; returns
    /// how many were removed. Called periodically by [`Historian::run`].
    pub fn prune(&self) -> anyhow::Result<usize> {
        let now = self.now_ms();
        let mut removed = 0;
        for &(series, max_age) in &self.plan.retention {
            let cutoff = now.saturating_sub(max_age.as_millis() as i64);
            removed += self.db.prune(series, cutoff)?;
        }
        if removed > 0 {
            tracing::info!(removed, "historian retention pruned old records");
        }
        Ok(removed)
    }
}

fn trend_value(value: &Value, quality: Quality) -> Option<f64> {
    if quality == Quality::Bad {
        return None;
    }
    value.as_f64()
}

fn json<T: Serialize>(v: &T) -> serde_json::Value {
    serde_json::to_value(v).unwrap_or(serde_json::Value::Null)
}
