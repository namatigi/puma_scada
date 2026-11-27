//! Alarm persistence in the data directory:
//!
//! - `alarm-journal.jsonl`: append-only, one `AlarmEvent` JSON per line. A
//!   torn last line (power loss mid-write) is skipped on load. When the file
//!   grows past [`COMPACT_AT`] lines it is rewritten with the newest
//!   [`JOURNAL_KEEP`] events.
//! - `alarm-state.json`: snapshot of every alarm's state, replaced atomically
//!   (write to a temp file, then rename) so a crash leaves the old or the new
//!   snapshot, never half of one.

use crate::state::{AlarmEvent, Snapshot};
use anyhow::Context;
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

pub const JOURNAL_FILE: &str = "alarm-journal.jsonl";
pub const STATE_FILE: &str = "alarm-state.json";
/// Events kept in memory (and in the file after compaction).
pub const JOURNAL_KEEP: usize = 10_000;
const COMPACT_AT: usize = 2 * JOURNAL_KEEP;

pub(crate) struct Persist {
    dir: PathBuf,
    journal: BufWriter<File>,
    lines: usize,
}

impl Persist {
    /// Opens (creating) the data directory and loads the journal tail and the
    /// last state snapshot.
    pub fn open(dir: &Path) -> anyhow::Result<(Self, VecDeque<AlarmEvent>, BTreeMap<String, Snapshot>)> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let journal_path = dir.join(JOURNAL_FILE);
        let (events, lines) = read_journal(&journal_path)?;
        let snapshot = read_snapshot(&dir.join(STATE_FILE))?;
        let mut persist = Persist { dir: dir.to_path_buf(), journal: open_append(&journal_path)?, lines };
        if lines > COMPACT_AT {
            persist.compact(&events)?;
        }
        Ok((persist, events, snapshot))
    }

    pub fn append(&mut self, events: &[AlarmEvent]) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        for event in events {
            serde_json::to_writer(&mut self.journal, event)?;
            self.journal.write_all(b"\n")?;
        }
        self.journal.flush().context("writing the alarm journal")?;
        self.lines += events.len();
        Ok(())
    }

    /// Rewrites the journal with just `tail` once it has grown too long.
    pub fn maybe_compact(&mut self, tail: &VecDeque<AlarmEvent>) -> anyhow::Result<()> {
        if self.lines > COMPACT_AT {
            self.compact(tail)?;
        }
        Ok(())
    }

    fn compact(&mut self, tail: &VecDeque<AlarmEvent>) -> anyhow::Result<()> {
        let path = self.dir.join(JOURNAL_FILE);
        let tmp = self.dir.join(format!("{JOURNAL_FILE}.tmp"));
        {
            let mut w = BufWriter::new(File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?);
            for event in tail {
                serde_json::to_writer(&mut w, event)?;
                w.write_all(b"\n")?;
            }
            w.flush()?;
            w.get_ref().sync_all()?;
        }
        replace(&tmp, &path)?;
        self.journal = open_append(&path)?;
        self.lines = tail.len();
        Ok(())
    }

    pub fn write_snapshot(&self, snapshot: &BTreeMap<String, Snapshot>) -> anyhow::Result<()> {
        let path = self.dir.join(STATE_FILE);
        let tmp = self.dir.join(format!("{STATE_FILE}.tmp"));
        let bytes = serde_json::to_vec_pretty(snapshot)?;
        {
            let mut f = File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        replace(&tmp, &path)
    }
}

fn open_append(path: &Path) -> anyhow::Result<BufWriter<File>> {
    let f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    Ok(BufWriter::new(f))
}

fn replace(tmp: &Path, path: &Path) -> anyhow::Result<()> {
    // `rename` replaces an existing file on Windows too (MoveFileEx with REPLACE_EXISTING).
    fs::rename(tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn read_journal(path: &Path) -> anyhow::Result<(VecDeque<AlarmEvent>, usize)> {
    let mut events = VecDeque::new();
    let mut lines = 0;
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((events, 0)),
        Err(e) => return Err(e).with_context(|| format!("opening {}", path.display())),
    };
    for (n, line) in BufReader::new(file).lines().enumerate() {
        let line = line.with_context(|| format!("reading {}", path.display()))?;
        lines += 1;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AlarmEvent>(&line) {
            Ok(event) => {
                if events.len() == JOURNAL_KEEP {
                    events.pop_front();
                }
                events.push_back(event);
            }
            Err(e) => tracing::warn!(line = n + 1, error = %e, "skipping unreadable alarm journal line"),
        }
    }
    Ok((events, lines))
}

fn read_snapshot(path: &Path) -> anyhow::Result<BTreeMap<String, Snapshot>> {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(map) => Ok(map),
            Err(e) => {
                // A corrupt snapshot must not stop the plant's alarm system from starting.
                tracing::warn!(error = %e, path = %path.display(), "ignoring unreadable alarm state snapshot");
                Ok(BTreeMap::new())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::EventKind;
    use moviclone_tagdb::Value;

    fn event(ts: i64) -> AlarmEvent {
        AlarmEvent { ts, name: "A".into(), event: EventKind::Activated, value: Value::Int(ts), user: None }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moviclone-alarms-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn journal_round_trip_skips_torn_line() {
        let dir = temp_dir("torn");
        {
            let (mut p, events, _) = Persist::open(&dir).unwrap();
            assert!(events.is_empty());
            p.append(&[event(1), event(2)]).unwrap();
        }
        let mut f = OpenOptions::new().append(true).open(dir.join(JOURNAL_FILE)).unwrap();
        f.write_all(b"{\"ts\":3,\"na").unwrap();
        drop(f);
        let (_, events, _) = Persist::open(&dir).unwrap();
        assert_eq!(events.iter().map(|e| e.ts).collect::<Vec<_>>(), vec![1, 2]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn compaction_keeps_newest() {
        let dir = temp_dir("compact");
        {
            let (mut p, _, _) = Persist::open(&dir).unwrap();
            let batch: Vec<_> = (0..(COMPACT_AT as i64 + 10)).map(event).collect();
            p.append(&batch).unwrap();
        }
        let (p, events, _) = Persist::open(&dir).unwrap();
        assert_eq!(events.len(), JOURNAL_KEEP);
        assert_eq!(events.back().map(|e| e.ts), Some(COMPACT_AT as i64 + 9));
        assert_eq!(p.lines, JOURNAL_KEEP);
        drop(p);
        let text = fs::read_to_string(dir.join(JOURNAL_FILE)).unwrap();
        assert_eq!(text.lines().count(), JOURNAL_KEEP);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn snapshot_round_trip_and_corrupt_snapshot_ignored() {
        let dir = temp_dir("snap");
        let (p, _, _) = Persist::open(&dir).unwrap();
        let mut map = BTreeMap::new();
        map.insert("A".to_string(), Snapshot { active: true, ..Snapshot::default() });
        p.write_snapshot(&map).unwrap();
        drop(p);
        let (_, _, snap) = Persist::open(&dir).unwrap();
        assert_eq!(snap, map);
        fs::write(dir.join(STATE_FILE), b"not json").unwrap();
        let (_, _, snap) = Persist::open(&dir).unwrap();
        assert!(snap.is_empty());
        fs::remove_dir_all(&dir).ok();
    }
}
