#![allow(dead_code)]

use moviclone_alarms::{AlarmEngine, Clock};
use moviclone_model::{Access, Alarm, AlarmCondition, AlarmSeverity, DataType, Project, Tag};
use moviclone_tagdb::TagStore;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub fn tag(name: &str, ty: DataType) -> Tag {
    Tag { name: name.into(), description: None, data_type: ty, access: Access::ReadWrite, address: None, historian: None, legacy_id: None }
}

pub fn alarm(name: &str, tag: &str, condition: AlarmCondition) -> Alarm {
    Alarm {
        name: name.into(),
        tag: tag.into(),
        condition,
        severity: AlarmSeverity::High,
        message: format!("{name} message"),
        area: Some("Manifold".into()),
        deadband: 0.0,
        delay_s: 0.0,
        ack_required: true,
        enabled: true,
    }
}

pub fn project(alarms: Vec<Alarm>) -> Project {
    Project {
        tags: vec![tag("Flow", DataType::F64), tag("Pump", DataType::Bool), tag("Level", DataType::F64)],
        alarms,
        ..Project::default()
    }
}

pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("moviclone-alarms-it-{name}-{}-{n}", std::process::id()));
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct Running {
    pub engine: Arc<AlarmEngine>,
    pub store: Arc<TagStore>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Running {
    pub async fn start(project: &Project, dir: &TempDir) -> Self {
        Self::start_with_clock(project, dir, Clock::tokio()).await
    }

    pub async fn start_with_clock(project: &Project, dir: &TempDir, clock: Clock) -> Self {
        let store = TagStore::new(project);
        let engine = AlarmEngine::open_with_clock(project, store.clone(), &dir.0, clock).expect("open engine");
        let (shutdown, rx) = watch::channel(false);
        let task = tokio::spawn(engine.clone().run(rx));
        settle().await;
        Running { engine, store, shutdown, task }
    }

    pub fn set(&self, tag: &str, v: moviclone_tagdb::Value) {
        let id = self.store.id(tag).expect("tag");
        self.store.update(id, v, moviclone_tagdb::Quality::Good);
    }

    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        self.task.await.expect("engine task");
    }
}

/// Lets spawned tasks process what is queued (time is paused, so a short
/// sleep auto-advances once everything is idle).
pub async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
}
