//! The project being served, swappable at runtime (studio saves, reloads).

use axum::body::Bytes;
use moviclone_model::Project;
use std::sync::{Arc, RwLock};
use tokio::sync::watch;

struct Snapshot {
    project: Arc<Project>,
    /// `GET /api/bundle` body, serialized once per project version.
    json: Bytes,
}

/// A shared, replaceable project. Cloning is cheap; every clone sees
/// [`SharedProject::replace`], and connected WebSocket clients get
/// `{"type":"reload"}` when it is called.
#[derive(Clone)]
pub struct SharedProject {
    inner: Arc<RwLock<Snapshot>>,
    generation: Arc<watch::Sender<u64>>,
}

fn serialize(p: &Project) -> Bytes {
    match serde_json::to_vec(p) {
        Ok(v) => Bytes::from(v),
        Err(e) => {
            tracing::error!("cannot serialize the project bundle: {e}");
            Bytes::from_static(b"{}")
        }
    }
}

impl SharedProject {
    pub fn new(project: Project) -> Self {
        let json = serialize(&project);
        Self {
            inner: Arc::new(RwLock::new(Snapshot { project: Arc::new(project), json })),
            generation: Arc::new(watch::channel(0).0),
        }
    }

    /// The current project.
    pub fn get(&self) -> Arc<Project> {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).project.clone()
    }

    /// Serves `project` from now on and tells clients to reload.
    pub fn replace(&self, project: Project) {
        let json = serialize(&project);
        {
            let mut s = self.inner.write().unwrap_or_else(|e| e.into_inner());
            s.project = Arc::new(project);
            s.json = json;
        }
        self.generation.send_modify(|g| *g += 1);
    }

    pub(crate) fn json(&self) -> Bytes {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).json.clone()
    }

    /// Changes whenever the project is replaced.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }
}

impl From<Project> for SharedProject {
    fn from(p: Project) -> Self {
        Self::new(p)
    }
}
