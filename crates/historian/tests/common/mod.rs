#![allow(dead_code)]

use moviclone_historian::{Clock, Historian, HistorianConfig};
use moviclone_model::{Access, DataConfig, DataLogger, DataType, Historian as HistorianDef, LoggerColumn, Project, Recording, Tag};
use moviclone_tagdb::{Quality, TagStore, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Wall time at the start of every test (2026-11-17), advanced by tokio's paused clock.
pub const BASE_MS: i64 = 1_795_400_000_000;

pub fn tag(name: &str, ty: DataType, historian: Option<&str>) -> Tag {
    Tag {
        name: name.into(),
        description: None,
        data_type: ty,
        access: Access::ReadWrite,
        address: None,
        historian: historian.map(String::from),
        legacy_id: None,
    }
}

pub fn historian(name: &str, database: &str, max_age_s: u64) -> HistorianDef {
    HistorianDef { name: name.into(), enabled: true, database: database.into(), max_age_s }
}

pub fn logger(name: &str, record: Recording, columns: &[&str]) -> DataLogger {
    DataLogger {
        name: name.into(),
        enabled: true,
        database: "sql".into(),
        table: format!("{name}_table"),
        record,
        max_age_s: 86_400,
        columns: columns.iter().map(|c| LoggerColumn { name: format!("col_{c}"), tag: (*c).into() }).collect(),
    }
}

/// Flow and Temp on historian H1, Level on H2 (other database, 1 h
/// retention), Note not historized; loggers L1 (on-change: Flow, Temp) and
/// L2 (every 10 s: Level).
pub fn project() -> Project {
    Project {
        tags: vec![
            tag("Flow", DataType::F64, Some("H1")),
            tag("Temp", DataType::F64, Some("H1")),
            tag("Level", DataType::F64, Some("H2")),
            tag("Note", DataType::String, None),
        ],
        data: DataConfig {
            databases: vec![],
            historians: vec![historian("H1", "sql", 86_400), historian("H2", "other", 3_600)],
            loggers: vec![logger("L1", Recording::OnChange, &["Flow", "Temp"]), logger("L2", Recording::EveryS(10), &["Level"])],
        },
        ..Project::default()
    }
}

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        TempDir(std::env::temp_dir().join(format!("moviclone-hist-it-{name}-{}-{n}", std::process::id())))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn config(clock: Clock) -> HistorianConfig {
    HistorianConfig {
        heartbeat: Duration::from_secs(60),
        flush_every: Duration::from_secs(1),
        prune_every: Duration::from_secs(600),
        max_batch: 5_000,
        clock,
    }
}

pub struct Running {
    pub hist: Arc<Historian>,
    pub store: Arc<TagStore>,
    pub shutdown: watch::Sender<bool>,
    pub shutdown_rx: watch::Receiver<bool>,
    task: JoinHandle<()>,
}

impl Running {
    /// `t0` anchors the wall clock so restarts continue the same timeline.
    pub async fn start(project: &Project, dir: &TempDir, t0: tokio::time::Instant) -> Self {
        let store = TagStore::new(project);
        let clock = Clock::tokio_at(BASE_MS + t0.elapsed().as_millis() as i64);
        let hist = Historian::open_with(project, store.clone(), &dir.0, config(clock)).expect("open historian");
        let (shutdown, shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(hist.clone().run(shutdown_rx.clone()));
        settle().await;
        Running { hist, store, shutdown, shutdown_rx, task }
    }

    pub fn set(&self, tag: &str, v: f64) {
        let id = self.store.id(tag).expect("tag");
        self.store.update(id, Value::Float(v), Quality::Good);
    }

    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        self.task.await.expect("historian task");
    }

    pub fn values(&self, tag: &str) -> Vec<Option<f64>> {
        let now = self.hist.now_ms();
        let s = self.hist.query(&[tag.to_string()], 0, now + 1, 100_000).expect("query");
        s[0].points.iter().map(|p| p.1).collect()
    }
}

/// Lets spawned tasks run; blocking-pool commits need real time to finish.
pub async fn settle() {
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
    }
}

/// Advances past the next flush and waits for the commit to land.
pub async fn flush() {
    tokio::time::sleep(Duration::from_millis(1100)).await;
    settle().await;
    // spawn_blocking runs on real threads: give it a moment of real time.
    std::thread::sleep(Duration::from_millis(30));
    settle().await;
}
