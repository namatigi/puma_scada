//! WebSocket `/ws`: subscriptions, batched value pushes, writes, feeds, reload.

use crate::api::{execute_write, sample_json, values_json, Failure};
use crate::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, State};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use moviclone_model::WriteMode;
use moviclone_tagdb::{now_ms, Sample, TagId};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, watch, Semaphore};
use tokio::task::JoinSet;

/// Largest client message accepted (a subscribe for thousands of tags fits easily).
const MAX_MESSAGE: usize = 256 * 1024;
/// Outgoing messages buffered per client before the session waits for it.
const OUT_QUEUE: usize = 256;
/// Writes one session may have waiting for an answer; more are refused at once
/// so one client cannot flood the drivers' write queues.
pub(crate) const MAX_PENDING_WRITES: usize = 16;
/// How long queued messages and the close frame may take to go out at the end.
const WRITER_GRACE: Duration = Duration::from_secs(2);
/// How often an open socket checks that its session is still valid.
const SESSION_CHECK: Duration = Duration::from_secs(20);

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
enum ClientMsg {
    Subscribe {
        tags: Vec<String>,
    },
    Unsubscribe {
        tags: Vec<String>,
    },
    SubscribeAll,
    Write {
        #[serde(default)]
        id: Json,
        tag: String,
        value: Json,
        #[serde(default)]
        mode: WriteMode,
    },
    Ping,
}

pub(crate) async fn upgrade(ws: WebSocketUpgrade, State(st): State<AppState>, who: Option<Extension<moviclone_auth::Signed>>) -> Response {
    let who = who.map(|Extension(s)| s);
    ws.max_message_size(MAX_MESSAGE).on_upgrade(move |socket| session(socket, st, who))
}

struct Session {
    st: AppState,
    out: mpsc::Sender<String>,
    all: bool,
    subscribed: HashSet<TagId>,
    pending: HashMap<TagId, Sample>,
    /// In-flight writes of this session (each holds a permit until it is answered).
    writes: Arc<Semaphore>,
    /// The signed-in user (with sign-in enabled; `None` for a guest viewer).
    who: Option<moviclone_auth::Signed>,
}

impl Session {
    async fn send(&self, msg: Json) -> bool {
        self.out.send(msg.to_string()).await.is_ok()
    }

    fn wants(&self, id: TagId) -> bool {
        self.all || self.subscribed.contains(&id)
    }

    async fn send_values(&self, ids: impl Iterator<Item = TagId>) -> bool {
        let values = values_json(&self.st.store, ids);
        if values.is_empty() {
            return true;
        }
        self.send(json!({"type": "values", "values": values})).await
    }

    fn resolve(&self, names: &[String]) -> Vec<TagId> {
        names.iter().filter_map(|n| self.st.store.id(n)).collect()
    }

    /// Handles one client message; `false` ends the session.
    async fn on_message(&mut self, text: &str) -> bool {
        let msg: ClientMsg = match serde_json::from_str(text) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("ignoring websocket message: {e}");
                return true;
            }
        };
        match msg {
            ClientMsg::Subscribe { tags } => {
                let ids = self.resolve(&tags);
                for id in &ids {
                    self.subscribed.insert(*id);
                    self.pending.remove(id);
                }
                self.send_values(ids.into_iter()).await
            }
            ClientMsg::Unsubscribe { tags } => {
                let ids = self.resolve(&tags);
                if self.all {
                    self.all = false;
                    self.subscribed = self.st.store.ids().collect();
                }
                for id in ids {
                    self.subscribed.remove(&id);
                    self.pending.remove(&id);
                }
                true
            }
            ClientMsg::SubscribeAll => {
                self.all = true;
                self.pending.clear();
                self.send_values(self.st.store.ids()).await
            }
            ClientMsg::Write { id, tag, value, mode } => {
                let user = match self.may_write() {
                    Ok(user) => user,
                    Err(error) => {
                        tracing::warn!(%tag, error, "refusing websocket write");
                        return self.send(json!({"type": "write-result", "id": id, "ok": false, "error": error, "auth": true})).await;
                    }
                };
                let Ok(permit) = self.writes.clone().try_acquire_owned() else {
                    tracing::debug!(%tag, "refusing websocket write: too many pending writes");
                    return self.send(json!({"type": "write-result", "id": id, "ok": false, "error": "too many pending writes"})).await;
                };
                tracing::info!(%tag, ?mode, user = user.as_deref().unwrap_or("-"), "write via websocket");
                let store = self.st.store.clone();
                let locks = self.st.write_locks.clone();
                let out = self.out.clone();
                // Detached: a pulse must restore its value even if the client leaves.
                tokio::spawn(async move {
                    let reply = match execute_write(&store, &locks, &tag, value, mode).await {
                        Ok(()) => json!({"type": "write-result", "id": id, "ok": true}),
                        Err(Failure(_, error)) => json!({"type": "write-result", "id": id, "ok": false, "error": error}),
                    };
                    drop(permit);
                    let _ = out.send(reply.to_string()).await;
                });
                true
            }
            ClientMsg::Ping => self.send(json!({"type": "pong"})).await,
        }
    }

    /// With sign-in, a write needs a valid session with the operator role,
    /// checked on every write (it counts as activity). Answers the user name.
    fn may_write(&mut self) -> Result<Option<String>, String> {
        let Some(auth) = &self.st.auth else { return Ok(None) };
        let Some(who) = &self.who else {
            return Err("sign in as an operator to write".into());
        };
        let me = auth.check(who.token(), true).map_err(|_| "your session ended; sign in again".to_string())?;
        me.require(moviclone_auth::Role::Operator, "writing").map_err(|_| format!("{} is signed in as {}; writing needs the operator role", me.name, me.role))?;
        let name = me.name.clone();
        self.who = Some(me);
        Ok(Some(name))
    }

    /// `false` when sign-in is on and this socket's session has ended.
    fn session_valid(&self) -> bool {
        match (&self.st.auth, &self.who) {
            (Some(auth), Some(who)) => auth.check(who.token(), false).is_ok(),
            _ => true,
        }
    }

    async fn flush(&mut self) -> bool {
        if self.pending.is_empty() {
            return true;
        }
        let values: serde_json::Map<String, Json> = self
            .pending
            .drain()
            .map(|(id, s)| (self.st.store.name(id).to_string(), sample_json(&s)))
            .collect();
        self.send(json!({"type": "values", "values": values})).await
    }

    /// Moves every change already waiting on the bus into `pending`;
    /// `false` when the bus is closed.
    fn drain_changes(&mut self, changes: &mut broadcast::Receiver<moviclone_tagdb::Change>) -> bool {
        // Bounded so a busy bus cannot keep the session from reading client messages.
        for _ in 0..4096 {
            match changes.try_recv() {
                Ok(c) => {
                    if self.wants(c.id) {
                        self.pending.insert(c.id, c.sample);
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => return true,
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::debug!("websocket client lagged by {n} changes; resending current values");
                    self.resync();
                }
                Err(broadcast::error::TryRecvError::Closed) => return false,
            }
        }
        true
    }

    /// After falling behind the change bus, re-read everything the client watches.
    fn resync(&mut self) {
        let ids: Vec<TagId> = if self.all { self.st.store.ids().collect() } else { self.subscribed.iter().copied().collect() };
        for id in ids {
            self.pending.insert(id, self.st.store.get(id));
        }
    }
}

fn hello(st: &AppState) -> Json {
    let p = st.project.get();
    json!({
        "type": "hello",
        "project": p.meta.name,
        "main_screen": p.meta.main_screen,
        "server_time": now_ms(),
        "version": &*st.version,
    })
}

fn publish_interval(st: &AppState) -> tokio::time::Interval {
    let ms = st.project.get().meta.sampling.publish_ms.clamp(20, 10_000);
    let mut i = tokio::time::interval(Duration::from_millis(u64::from(ms)));
    i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    i
}

/// Forwards every value of a watch channel as a message, starting with the current one.
fn forward_feed(tasks: &mut JoinSet<()>, mut feed: watch::Receiver<Json>, out: mpsc::Sender<String>) {
    tasks.spawn(async move {
        loop {
            let msg = feed.borrow_and_update().to_string();
            if !msg.is_empty() && msg != "null" && out.send(msg).await.is_err() {
                return;
            }
            if feed.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Sends `{"type":"reload"}` whenever `rx` changes (not for its current value).
fn forward_reload(tasks: &mut JoinSet<()>, mut rx: watch::Receiver<u64>, out: mpsc::Sender<String>, reloaded: mpsc::Sender<()>) {
    rx.mark_unchanged();
    tasks.spawn(async move {
        while rx.changed().await.is_ok() {
            let _ = reloaded.try_send(());
            if out.send(json!({"type": "reload"}).to_string()).await.is_err() {
                return;
            }
        }
    });
}

async fn session(socket: WebSocket, st: AppState, who: Option<moviclone_auth::Signed>) {
    st.ws_clients.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (out, mut out_rx) = mpsc::channel::<String>(OUT_QUEUE);
    let mut writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sink.send(Message::Text(text)).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });

    // Subscribe to changes before the first snapshot so nothing falls in between.
    let mut changes = st.store.subscribe();
    let mut tasks = JoinSet::new();
    let (reloaded_tx, mut reloaded) = mpsc::channel(1);
    let mut s = Session {
        st: st.clone(),
        out: out.clone(),
        all: false,
        subscribed: HashSet::new(),
        pending: HashMap::new(),
        writes: Arc::new(Semaphore::new(MAX_PENDING_WRITES)),
        who,
    };

    if s.send(hello(&st)).await {
        for feed in st.ws_feeds.iter() {
            forward_feed(&mut tasks, feed.clone(), out.clone());
        }
        forward_reload(&mut tasks, st.project.changes(), out.clone(), reloaded_tx.clone());
        if let Some(r) = &st.reload {
            forward_reload(&mut tasks, r.clone(), out.clone(), reloaded_tx.clone());
        }
        drop(reloaded_tx);

        let mut publish = publish_interval(&st);
        let mut shutdown = st.shutdown.clone();
        let mut session_check = tokio::time::interval(SESSION_CHECK);
        session_check.tick().await;
        loop {
            tokio::select! {
                _ = session_check.tick() => if !s.session_valid() {
                    tracing::info!("closing a websocket: its session ended");
                    let _ = s.send(json!({"type": "signed-out"})).await;
                    break;
                },
                msg = stream.next() => match msg {
                    Some(Ok(Message::Text(text))) => if !s.on_message(&text).await { break },
                    Some(Ok(Message::Binary(_))) => tracing::debug!("ignoring binary websocket message"),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        tracing::debug!("websocket error: {e}");
                        break;
                    }
                },
                change = changes.recv() => match change {
                    Ok(c) => {
                        if s.wants(c.id) { s.pending.insert(c.id, c.sample); }
                        // Take everything already queued, so a publish tick that is due
                        // at the same time never sends half of a burst of changes.
                        if !s.drain_changes(&mut changes) { break }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("websocket client lagged by {n} changes; resending current values");
                        s.resync();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = publish.tick() => if !s.flush().await { break },
                Some(()) = reloaded.recv() => publish = publish_interval(&st),
                _ = wait_shutdown(&mut shutdown) => break,
            }
        }
    }

    tasks.abort_all();
    drop(s);
    drop(out);
    // Let queued messages (and the close frame) go out, but never hang on a dead
    // peer: a peer that stopped reading would keep the task, socket and queue alive
    // until TCP gives up, possibly hours later.
    if tokio::time::timeout(WRITER_GRACE, &mut writer).await.is_err() {
        tracing::debug!("websocket peer did not take the last messages in time; dropping the connection");
        writer.abort();
    }
    st.ws_clients.fetch_sub(1, Ordering::Relaxed);
}

async fn wait_shutdown(rx: &mut Option<watch::Receiver<bool>>) {
    match rx {
        Some(rx) => {
            let _ = rx.wait_for(|stop| *stop).await;
        }
        None => std::future::pending().await,
    }
}
