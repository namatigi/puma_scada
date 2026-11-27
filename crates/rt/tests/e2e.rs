//! End-to-end: the whole runtime (simulator, Modbus driver, tag store,
//! historian, alarm engine, studio API, gateway) on ephemeral ports, driven
//! over real HTTP and WebSocket connections.
//!
//! Each test runs on its own copy of `demo/oxoafrica` with one extra alarm,
//! `Test_Start` (`FM_01_START` is true), that the tests can raise on demand.

use futures_util::{SinkExt, StreamExt};
use moviclone_rt::{Args, Runtime};
use serde_json::{json, Value as Json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const TEST_ALARM: &str = r#"
[[alarm]]
name = "Test_Start"
tag = "FM_01_START"
when = "is-true"
severity = "low"
message = "FM_01 totalizer running (test alarm)"
area = "Test"
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "data" {
            continue; // runtime data of a demo run, not part of the project
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Rig {
    rt: Option<Runtime>,
    addr: SocketAddr,
    dir: tempfile::TempDir,
}

impl Rig {
    async fn start() -> Rig {
        // RUST_LOG=info shows the runtime's log next to a failing test.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("oxoafrica");
        copy_dir(&repo().join("demo/oxoafrica"), &project);
        let alarms = project.join("alarms.toml");
        let mut text = std::fs::read_to_string(&alarms).unwrap();
        text.push_str(TEST_ALARM);
        std::fs::write(&alarms, text).unwrap();

        let mut args = Args::new(&project);
        args.listen = "127.0.0.1:0".parse().unwrap();
        args.simulate = true;
        args.studio = true;
        args.web = repo().join("web");
        args.data = Some(dir.path().join("data"));
        let rt = moviclone_rt::start(args).await.expect("runtime starts");
        let addr = rt.local_addr();
        Rig { rt: Some(rt), addr, dir }
    }

    async fn stop(mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown().await.expect("clean shutdown");
        }
    }
}

// ------------------------------------------------------------------ HTTP

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    /// The `name=value` of the reply's Set-Cookie header.
    fn cookie(&self) -> Option<String> {
        self.head
            .lines()
            .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v.trim().to_string()))
            .and_then(|v| v.split(';').next().map(str::to_string))
    }

    fn json(&self) -> Json {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.body))
    }
}

/// Minimal HTTP/1.1 client (no extra dependencies), with chunked decoding.
async fn http(addr: SocketAddr, method: &str, path: &str, body: Option<&Json>) -> Reply {
    http_as(addr, None, method, path, body).await
}

/// `http` with a session cookie (studio routes).
async fn http_as(addr: SocketAddr, cookie: Option<&str>, method: &str, path: &str, body: Option<&Json>) -> Reply {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let cookie = cookie.map(|c| format!("Cookie: {c}\r\n")).unwrap_or_default();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{cookie}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), sock.read_to_end(&mut buf)).await.expect("reply in time").unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, raw) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = if head.to_ascii_lowercase().contains("transfer-encoding: chunked") { dechunk(raw) } else { raw.to_string() };
    Reply { status, head: head.to_string(), body }
}

fn dechunk(mut raw: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = raw.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        raw = rest[n..].trim_start_matches("\r\n");
    }
    out
}

async fn get(addr: SocketAddr, path: &str) -> Json {
    let r = http(addr, "GET", path, None).await;
    assert_eq!(r.status, 200, "GET {path}: {}", r.body);
    r.json()
}

/// Retries `f` until it returns `Some` or `secs` pass.
async fn eventually<T, F, Fut>(secs: u64, what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ------------------------------------------------------------------ WebSocket

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: SocketAddr) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("websocket connects").0
}

async fn send(ws: &mut Ws, msg: Json) {
    ws.send(Message::Text(msg.to_string())).await.unwrap();
}

enum Next {
    Msg(Json),
    Closed,
    TimedOut,
}

/// The next JSON message within `wait`.
async fn next_msg(ws: &mut Ws, wait: Duration) -> Next {
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return Next::TimedOut,
            Ok(Some(Ok(Message::Text(t)))) => return Next::Msg(serde_json::from_str(&t).unwrap()),
            Ok(Some(Ok(Message::Close(_))) | None | Some(Err(_))) => return Next::Closed,
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// The next JSON message, or `None` when the server closed the socket.
async fn recv(ws: &mut Ws, secs: u64) -> Option<Json> {
    match next_msg(ws, Duration::from_secs(secs)).await {
        Next::Msg(m) => Some(m),
        Next::Closed => None,
        Next::TimedOut => panic!("no websocket message within {secs} s"),
    }
}

/// Reads messages until `pred` accepts one.
async fn recv_until(ws: &mut Ws, secs: u64, what: &str, mut pred: impl FnMut(&Json) -> bool) -> Json {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match next_msg(ws, deadline.saturating_duration_since(tokio::time::Instant::now())).await {
            Next::Msg(m) if pred(&m) => return m,
            Next::Msg(_) => {}
            Next::Closed => panic!("websocket closed while waiting for {what}"),
            Next::TimedOut => panic!("timed out after {secs} s waiting for {what}"),
        }
    }
}

/// Subscribes to `tags` and waits until each has been reported with good quality.
async fn wait_good(ws: &mut Ws, tags: &[&str]) -> serde_json::Map<String, Json> {
    send(ws, json!({"op": "subscribe", "tags": tags})).await;
    let mut seen = serde_json::Map::new();
    recv_until(ws, 30, "good values", |m| {
        if m["type"] == "values" {
            for (k, v) in m["values"].as_object().unwrap() {
                if v["q"] == "good" {
                    seen.insert(k.clone(), v.clone());
                }
            }
        }
        tags.iter().all(|t| seen.contains_key(*t))
    })
    .await;
    seen
}

// ------------------------------------------------------------------ tests

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn serves_the_bundle_live_values_and_writes_coils() {
    let rig = Rig::start().await;
    let addr = rig.addr;

    let bundle = get(addr, "/api/bundle").await;
    assert_eq!(bundle["meta"]["name"], "OXOAFRICA SCADA");
    assert_eq!(bundle["tags"].as_array().unwrap().len(), 49);
    assert!(bundle["alarms"].as_array().unwrap().len() > 20);
    assert_eq!(http(addr, "GET", "/hmi/", None).await.status, 200);
    assert_eq!(http(addr, "GET", "/studio/", None).await.status, 200);

    let mut ws = connect(addr).await;
    let hello = recv(&mut ws, 5).await.unwrap();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["main_screen"], "Manifold");

    let values = wait_good(&mut ws, &["FM_01_MassFlow", "FM_01_Density", "FM_01_START"]).await;
    let density = values["FM_01_Density"]["v"].as_f64().unwrap();
    assert!((950.0..1010.0).contains(&density), "HFO density {density}");

    // Coil round trip: write over the WebSocket, then see the device's value come back.
    let before = values["FM_01_START"]["v"].as_bool().unwrap();
    send(&mut ws, json!({"op": "write", "id": 1, "tag": "FM_01_START", "value": !before})).await;
    let result = recv_until(&mut ws, 15, "write-result", |m| m["type"] == "write-result").await;
    assert_eq!(result, json!({"type": "write-result", "id": 1, "ok": true}));
    recv_until(&mut ws, 10, "the written coil value", |m| m["type"] == "values" && m["values"]["FM_01_START"]["v"] == json!(!before)).await;

    // The value survives later polls (it is the device's value, not just the write).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let v = get(addr, "/api/values").await;
    assert_eq!(v["FM_01_START"]["v"], json!(!before));
    assert_eq!(v["FM_01_START"]["q"], "good");

    // And back over REST.
    let r = http(addr, "POST", "/api/write", Some(&json!({"tag": "FM_01_START", "value": before}))).await;
    assert_eq!((r.status, r.json()), (200, json!({"ok": true})));
    let diag = get(addr, "/api/diagnostics").await;
    assert_eq!(diag["drivers"][0]["connected"], true);
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_returns_points_after_a_few_seconds() {
    let rig = Rig::start().await;
    let addr = rig.addr;
    let from = moviclone_tagdb::now_ms() - 60_000;

    let points = eventually(30, "trend points", || async move {
        let to = moviclone_tagdb::now_ms() + 1_000;
        // Density and temperature always move; a stopped meter's flow can sit at 0.
        let h = get(addr, &format!("/api/history?tags=FM_01_Density,FM_01_Temp&from={from}&to={to}&points=200")).await;
        let series = h["series"].as_array()?.clone();
        let numeric = |s: &Json| s["points"].as_array().map(|p| p.iter().filter(|pt| pt[1].is_number()).count()).unwrap_or(0);
        (series.len() == 2 && series.iter().all(|s| numeric(s) >= 3)).then_some(series)
    })
    .await;
    assert_eq!(points[0]["tag"], "FM_01_Density");

    let table = eventually(30, "history table rows", || async move {
        let to = moviclone_tagdb::now_ms() + 1_000;
        let t = get(addr, &format!("/api/history/table?source=FM01&from={from}&to={to}&limit=50")).await;
        (!t["rows"].as_array()?.is_empty()).then_some(t)
    })
    .await;
    assert_eq!(table["columns"], json!(["time", "tag", "value", "quality"]));
    let logger = get(addr, &format!("/api/history/table?source=FM01&kind=logger&from={from}&limit=5")).await;
    assert_eq!(logger["columns"][0], "time");
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alarm_activates_and_is_acknowledged() {
    let rig = Rig::start().await;
    let addr = rig.addr;
    let mut ws = connect(addr).await;
    recv_until(&mut ws, 15, "initial alarm list", |m| m["type"] == "alarms").await;
    wait_good(&mut ws, &["FM_01_START"]).await;

    // Off, then on: the alarm is active whatever the simulator started with.
    for v in [false, true] {
        let r = http(addr, "POST", "/api/write", Some(&json!({"tag": "FM_01_START", "value": v}))).await;
        assert_eq!(r.status, 200, "{}", r.body);
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    let find = |m: &Json| m["alarms"].as_array().and_then(|a| a.iter().find(|x| x["name"] == "Test_Start").cloned());
    recv_until(&mut ws, 30, "Test_Start active on the WebSocket", |m| {
        m["type"] == "alarms" && find(m).is_some_and(|a| a["state"] == "active-unacked")
    })
    .await;
    let listed = find(&get(addr, "/api/alarms").await).unwrap();
    assert_eq!(listed["state"], "active-unacked");
    assert_eq!(listed["severity"], "low");

    let r = http(addr, "POST", "/api/alarms/Test_Start/ack", Some(&json!({"user": "e2e"}))).await;
    assert_eq!((r.status, r.json()["ok"].clone()), (200, json!(true)), "{}", r.body);
    recv_until(&mut ws, 30, "Test_Start acknowledged", |m| {
        m["type"] == "alarms" && find(m).is_some_and(|a| a["state"] == "active-acked")
    })
    .await;
    let journal = get(addr, "/api/alarms/journal?limit=50").await;
    let events: Vec<_> = journal["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "Test_Start")
        .map(|e| e["event"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(events.contains(&"activated".into()) && events.contains(&"acked".into()), "{events:?}");
    assert_eq!(http(addr, "POST", "/api/alarms/Nope/ack", None).await.status, 404);

    // The journal is on disk in the data folder.
    assert!(rig.dir.path().join("data/alarms").join(moviclone_alarms::JOURNAL_FILE).exists());
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn studio_saves_reload_the_running_project() {
    let rig = Rig::start().await;
    let addr = rig.addr;
    let mut ws = connect(addr).await;
    wait_good(&mut ws, &["FM_01_MassFlow"]).await;

    // The studio needs a signed-in user. With no users yet, the first
    // administrator can be created from this machine (the test connects over loopback).
    assert_eq!(http(addr, "GET", "/api/studio/project", None).await.status, 401);
    let r = http(addr, "POST", "/api/studio/setup", Some(&json!({ "name": "admin", "password": "e2e-admin-pass" }))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let cookie = r.cookie().expect("session cookie");
    let me = Some(cookie.as_str());
    assert!(rig.dir.path().join("data/users.toml").exists());

    // 1. A screen edit: the project is swapped, the socket stays open.
    let studio = http_as(addr, me, "GET", "/api/studio/project", None).await.json();
    let mut screen = studio["project"]["screens"].as_array().unwrap().iter().find(|s| s["name"] == "Manifold").unwrap().clone();
    screen["background"] = json!("#204060");
    let r = http_as(addr, me, "PUT", "/api/studio/screens/Manifold", Some(&screen)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["ok"], true);
    recv_until(&mut ws, 10, "reload after the screen save", |m| m["type"] == "reload").await;
    let bundle = get(addr, "/api/bundle").await;
    let manifold = bundle["screens"].as_array().unwrap().iter().find(|s| s["name"] == "Manifold").unwrap().clone();
    assert_eq!(manifold["background"], "#204060");
    // Same socket, still live (a ping, since a stopped meter sends no value changes).
    send(&mut ws, json!({"op": "ping"})).await;
    recv_until(&mut ws, 10, "pong after the screen reload", |m| m["type"] == "pong").await;

    // 2. An alarm edit: historian and alarm engine restart, drivers keep running.
    let mut alarms = studio["project"]["alarms"].as_array().unwrap().clone();
    alarms.retain(|a| a["name"] != "FM01_HighFlow");
    let r = http_as(addr, me, "PUT", "/api/studio/alarms", Some(&Json::Array(alarms))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    recv_until(&mut ws, 10, "reload after the alarm save", |m| m["type"] == "reload").await;
    eventually(30, "the new alarm list", || async move {
        let r = http(addr, "GET", "/api/alarms", None).await;
        let names: Vec<Json> = (r.status == 200).then(|| r.json()["alarms"].as_array().unwrap().iter().map(|a| a["name"].clone()).collect())?;
        (!names.contains(&json!("FM01_HighFlow")) && names.contains(&json!("Test_Start"))).then_some(())
    })
    .await;

    // 3. A tag edit: new tag store and drivers; the old socket is closed after the reload.
    let mut tags = studio["project"]["tags"].as_array().unwrap().clone();
    tags[0]["description"] = json!("edited by the e2e test");
    let r = http_as(addr, me, "PUT", "/api/studio/tags", Some(&Json::Array(tags))).await;
    assert_eq!(r.status, 200, "{}", r.body);
    recv_until(&mut ws, 15, "reload after the tag save", |m| m["type"] == "reload").await;
    let closed = tokio::time::timeout(Duration::from_secs(15), async {
        while recv(&mut ws, 15).await.is_some() {}
    })
    .await;
    assert!(closed.is_ok(), "the old socket closes after a tag change");
    let mut ws = connect(addr).await;
    wait_good(&mut ws, &["FM_01_MassFlow", "FM_02_MassFlow"]).await;
    assert_eq!(get(addr, "/api/bundle").await["tags"][0]["description"], "edited by the e2e test");
    eventually(30, "alarm routes after the restart", || async move {
        (http(addr, "GET", "/api/alarms", None).await.status == 200).then_some(())
    })
    .await;

    // 4. A save that breaks the project is not applied.
    let mut broken = screen.clone();
    broken["element"] = json!([{"id": "X", "kind": "display", "tag": "No_Such_Tag", "x": 0.0, "y": 0.0, "w": 10.0, "h": 10.0}]);
    let r = http_as(addr, me, "PUT", "/api/studio/screens/Manifold", Some(&broken)).await;
    assert_eq!(r.status, 200, "{}", r.body);
    tokio::time::sleep(Duration::from_secs(2)).await;
    let bundle = get(addr, "/api/bundle").await;
    let manifold = bundle["screens"].as_array().unwrap().iter().find(|s| s["name"] == "Manifold").unwrap().clone();
    assert!(manifold["element"].as_array().unwrap().len() > 1, "a project with errors must not replace the running one");
    rig.stop().await;
}
