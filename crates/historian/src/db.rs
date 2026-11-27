//! The local journal on redb: the source of truth for history and for SQL
//! forwarding.
//!
//! Tables:
//! - `journal`: `seq -> series:u32 ts:i64 payload`, in recording order. SQL
//!   forwarders walk it from their watermark.
//! - `index`: `(series, ts, seq) -> payload`, for time-range queries per series.
//! - `series`: stable key (`tag:<name>`, `logger:<name>:<columns>`) -> id.
//! - `meta`: `next_seq`, `next_series`.
//! - `watermark`: forwarder name -> last forwarded seq.
//!
//! `next_seq` is stored, not derived from the journal, so that pruning the
//! whole journal never makes sequence numbers go backwards under a watermark.

use anyhow::Context;
use redb::{Database, ReadableTable, TableDefinition};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

const JOURNAL: TableDefinition<u64, &[u8]> = TableDefinition::new("journal");
const INDEX: TableDefinition<(u32, i64, u64), &[u8]> = TableDefinition::new("index");
const SERIES: TableDefinition<&str, u32> = TableDefinition::new("series");
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");
const WATERMARK: TableDefinition<&str, u64> = TableDefinition::new("watermark");

const NEXT_SEQ: &str = "next_seq";
const NEXT_SERIES: &str = "next_series";
/// Entries removed per write transaction while pruning, so a large prune
/// does not hold the write lock (and block recording) for long.
const PRUNE_CHUNK: usize = 20_000;

/// One record waiting to be written.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub series: u32,
    pub ts: i64,
    pub payload: Vec<u8>,
}

/// One journal entry as read back.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub seq: u64,
    pub series: u32,
    pub ts: i64,
    pub payload: Vec<u8>,
}

pub struct Db {
    db: Database,
    next_seq: AtomicU64,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let db = Database::create(path).with_context(|| format!("opening historian database {}", path.display()))?;
        let tx = db.begin_write()?;
        let next_seq = {
            tx.open_table(JOURNAL)?;
            tx.open_table(INDEX)?;
            tx.open_table(SERIES)?;
            tx.open_table(WATERMARK)?;
            let meta = tx.open_table(META)?;
            let stored = meta.get(NEXT_SEQ)?.map(|v| v.value()).unwrap_or(1);
            stored.max(1)
        };
        tx.commit()?;
        Ok(Db { db, next_seq: AtomicU64::new(next_seq) })
    }

    /// Ids for the given series keys, allocating new ids as needed.
    pub fn series_ids(&self, keys: &[String]) -> anyhow::Result<HashMap<String, u32>> {
        let tx = self.db.begin_write()?;
        let mut out = HashMap::with_capacity(keys.len());
        {
            let mut series = tx.open_table(SERIES)?;
            let mut meta = tx.open_table(META)?;
            let mut next = meta.get(NEXT_SERIES)?.map(|v| v.value()).unwrap_or(0);
            for key in keys {
                let existing = series.get(key.as_str())?.map(|v| v.value());
                let id = match existing {
                    Some(id) => id,
                    None => {
                        let id = u32::try_from(next).context("too many series")?;
                        series.insert(key.as_str(), id)?;
                        next += 1;
                        id
                    }
                };
                out.insert(key.clone(), id);
            }
            meta.insert(NEXT_SERIES, next)?;
        }
        tx.commit()?;
        Ok(out)
    }

    /// Every series id ever allocated (including ones the project no longer uses).
    pub fn all_series(&self) -> anyhow::Result<Vec<(String, u32)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(SERIES)?;
        let mut out = Vec::new();
        for item in table.iter()? {
            let (k, v) = item?;
            out.push((k.value().to_string(), v.value()));
        }
        Ok(out)
    }

    /// Appends a batch in one transaction; returns the last sequence number used.
    pub fn append(&self, batch: &[Pending]) -> anyhow::Result<u64> {
        let first = self.next_seq.load(Ordering::SeqCst);
        if batch.is_empty() {
            return Ok(first - 1);
        }
        let tx = self.db.begin_write()?;
        {
            let mut journal = tx.open_table(JOURNAL)?;
            let mut index = tx.open_table(INDEX)?;
            let mut meta = tx.open_table(META)?;
            let mut buf = Vec::new();
            for (i, p) in batch.iter().enumerate() {
                let seq = first + i as u64;
                buf.clear();
                buf.extend_from_slice(&p.series.to_le_bytes());
                buf.extend_from_slice(&p.ts.to_le_bytes());
                buf.extend_from_slice(&p.payload);
                journal.insert(seq, buf.as_slice())?;
                index.insert((p.series, p.ts, seq), p.payload.as_slice())?;
            }
            meta.insert(NEXT_SEQ, first + batch.len() as u64)?;
        }
        tx.commit().context("committing historian batch")?;
        let last = first + batch.len() as u64 - 1;
        self.next_seq.store(last + 1, Ordering::SeqCst);
        Ok(last)
    }

    /// Last sequence number written (0 when nothing was ever written).
    pub fn last_seq(&self) -> u64 {
        self.next_seq.load(Ordering::SeqCst) - 1
    }

    /// Calls `f(ts, payload)` for each record of `series` in `[from, to]`, oldest first.
    pub fn scan(&self, series: u32, from: i64, to: i64, mut f: impl FnMut(i64, &[u8]) -> anyhow::Result<()>) -> anyhow::Result<()> {
        if from > to {
            return Ok(());
        }
        let tx = self.db.begin_read()?;
        let index = tx.open_table(INDEX)?;
        for item in index.range((series, from, 0)..=(series, to, u64::MAX))? {
            let (k, v) = item?;
            f(k.value().1, v.value())?;
        }
        Ok(())
    }

    /// Up to `limit` records of `series` in `[from, to]`, newest first, as `(ts, seq, payload)`.
    pub fn newest(&self, series: u32, from: i64, to: i64, limit: usize) -> anyhow::Result<Vec<(i64, u64, Vec<u8>)>> {
        let mut out = Vec::new();
        if from > to || limit == 0 {
            return Ok(out);
        }
        let tx = self.db.begin_read()?;
        let index = tx.open_table(INDEX)?;
        for item in index.range((series, from, 0)..=(series, to, u64::MAX))?.rev().take(limit) {
            let (k, v) = item?;
            let (_, ts, seq) = k.value();
            out.push((ts, seq, v.value().to_vec()));
        }
        Ok(out)
    }

    /// Deletes records of `series` older than `cutoff` (exclusive); returns how many.
    pub fn prune(&self, series: u32, cutoff: i64) -> anyhow::Result<usize> {
        let mut total = 0;
        loop {
            let tx = self.db.begin_write()?;
            let removed = {
                let mut index = tx.open_table(INDEX)?;
                let mut journal = tx.open_table(JOURNAL)?;
                let mut keys = Vec::new();
                for item in index.range((series, i64::MIN, 0)..(series, cutoff, 0))?.take(PRUNE_CHUNK) {
                    let (k, _) = item?;
                    keys.push(k.value());
                }
                for key in &keys {
                    index.remove(key)?;
                    journal.remove(key.2)?;
                }
                keys.len()
            };
            tx.commit().context("committing historian prune")?;
            total += removed;
            if removed < PRUNE_CHUNK {
                return Ok(total);
            }
        }
    }

    /// Journal entries after `seq`, oldest first, at most `limit`.
    pub fn journal_after(&self, seq: u64, limit: usize) -> anyhow::Result<Vec<Stored>> {
        let tx = self.db.begin_read()?;
        let journal = tx.open_table(JOURNAL)?;
        let mut out = Vec::new();
        let Some(start) = seq.checked_add(1) else { return Ok(out) };
        for item in journal.range(start..)?.take(limit) {
            let (k, v) = item?;
            let raw = v.value();
            if raw.len() < 12 {
                anyhow::bail!("journal entry {} is truncated", k.value());
            }
            let series = u32::from_le_bytes(raw[0..4].try_into()?);
            let ts = i64::from_le_bytes(raw[4..12].try_into()?);
            out.push(Stored { seq: k.value(), series, ts, payload: raw[12..].to_vec() });
        }
        Ok(out)
    }

    pub fn watermark(&self, name: &str) -> anyhow::Result<u64> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(WATERMARK)?;
        let wm = table.get(name)?.map(|v| v.value()).unwrap_or(0);
        Ok(wm)
    }

    pub fn set_watermark(&self, name: &str, seq: u64) -> anyhow::Result<()> {
        let tx = self.db.begin_write()?;
        tx.open_table(WATERMARK)?.insert(name, seq)?;
        tx.commit().context("saving forwarder watermark")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("moviclone-hist-db-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn append_scan_prune_and_sequence_survive_reopen() {
        let dir = temp("basic");
        let path = dir.join("h.redb");
        {
            let db = Db::open(&path).unwrap();
            let ids = db.series_ids(&["tag:A".into(), "tag:B".into()]).unwrap();
            let (a, b) = (ids["tag:A"], ids["tag:B"]);
            let batch: Vec<_> = (0..10).map(|i| Pending { series: if i % 2 == 0 { a } else { b }, ts: i, payload: vec![i as u8] }).collect();
            assert_eq!(db.append(&batch).unwrap(), 10);
            let mut seen = Vec::new();
            db.scan(a, 2, 6, |ts, p| {
                seen.push((ts, p[0]));
                Ok(())
            })
            .unwrap();
            assert_eq!(seen, vec![(2, 2), (4, 4), (6, 6)]);
            let newest = db.newest(b, 0, 100, 2).unwrap();
            assert_eq!(newest.iter().map(|n| n.0).collect::<Vec<_>>(), vec![9, 7]);
            assert_eq!(db.prune(a, 5).unwrap(), 3);
            assert_eq!(db.journal_after(0, 100).unwrap().len(), 7);
            db.set_watermark("sql", 4).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.last_seq(), 10);
        assert_eq!(db.watermark("sql").unwrap(), 4);
        let ids = db.series_ids(&["tag:B".into(), "tag:C".into()]).unwrap();
        assert_eq!(ids["tag:B"], 1, "ids are stable");
        assert_eq!(ids["tag:C"], 2);
        let after = db.journal_after(4, 2).unwrap();
        assert_eq!(after.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![6, 7], "seq 5 was pruned");
        assert_eq!(after[0].ts, 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sequence_never_goes_back_after_full_prune() {
        let dir = temp("seq");
        let path = dir.join("h.redb");
        {
            let db = Db::open(&path).unwrap();
            db.append(&[Pending { series: 0, ts: 1, payload: vec![] }]).unwrap();
            db.prune(0, i64::MAX).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.append(&[Pending { series: 0, ts: 2, payload: vec![] }]).unwrap(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }
}
