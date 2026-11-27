//! Gateway over real sockets on an ephemeral port.

use futures_util::{SinkExt, StreamExt};
use moviclone_gateway::{GatewayConfig, SharedProject};
use moviclone_model::{
    Access, Address, DataType, ModbusAddress, ModbusArea, ModbusWrite, Project, ProjectMeta, Sampling, Tag,
};
use moviclone_tagdb::{Quality, TagStore, Value};
use serde_json::{json, Value as Json};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;

fn mem(name: &str, ty: DataType) -> Tag {
    Tag { name: name.into(), description: None, data_type: ty, access: Access::ReadWrite, address: None, historian: None, legacy_id: None }
}

fn project(name: &str) -> Project {
    let mut ro = mem("RO", DataType::F32);
    ro.access = Access::Read;
    ro.address = Some(Address::Modbus(ModbusAddress {
        driver: "fm".into(),
        station: "S".into(),
        area: ModbusArea::HoldingRegister,
        register: 1,
        write: ModbusWrite::Multiple,
    }));
    Project {
        meta: ProjectMeta {
            name: name.into(),
            main_screen: Some("Main".into()),
            sampling: Sampling { fast_ms: 500, slow_ms: 5000, publish_ms: 100 },
            ..ProjectMeta::default()
        },
        tags: vec![mem("A", DataType::F64), mem("B", DataType::I32), mem("Flag", DataType::Bool), ro],
        ..Project::default()
    }
}

struct Server {
    addr: SocketAddr,
    store: Arc<TagStore>,
    project: SharedProject,
    feed: watch::Sender<Json>,
    reload: watch::Sender<u64>,
    _shutdown: watch::Sender<bool>,
    _web: TempDir,
}

/// A throwaway folder (no tempfile crate needed).
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn web_root() -> TempDir {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "moviclone-gw-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let web = dir.join("web");
    for sub in ["hmi", "studio", "shared"] {
        std::fs::create_dir_all(web.join(sub)).unwrap();
    }
    std::fs::write(web.join("hmi/index.html"), "<!doctype html><title>hmi</title>").unwrap();
    std::fs::write(web.join("hmi/app.js"), "export const x = 1;").unwrap();
    std::fs::write(web.join("shared/style.css"), "body{}").unwrap();
    std::fs::write(web.join("studio/index.html"), "studio").unwrap();
    std::fs::write(dir.join("secret.txt"), "TOP-SECRET").unwrap();
    TempDir(dir)
}

async fn start() -> Server {
    start_with(project("Test"), Vec::new()).await
}

async fn start_with(p: Project, allowed_origins: Vec<String>) -> Server {
    let store = TagStore::new(&p);
    let shared = SharedProject::new(p);
    let web = web_root();
    let (feed, feed_rx) = watch::channel(json!({"type": "alarms", "alarms": []}));
    let (reload, reload_rx) = watch::channel(0u64);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let mut cfg = GatewayConfig::new(shared.clone(), store.clone(), web.0.join("web"));
    cfg.ws_feeds.push(feed_rx);
    cfg.reload = Some(reload_rx);
    cfg.shutdown = Some(shutdown_rx);
    cfg.diagnostics = Some(Arc::new(|| vec![json!({"name": "fm", "connected": true, "requests": 3, "errors": 0, "last_error": null})]));
    cfg.extra = axum::Router::new()
        .route("/api/extra", axum::routing::get(|| async { "extra" }).post(|| async { "posted" }));
    cfg.allowed_origins = allowed_origins;
    let app = moviclone_gateway::router(cfg);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut stop = shutdown.subscribe();
    tokio::spawn(moviclone_gateway::serve(listener, app, async move {
        let _ = stop.wait_for(|s| *s).await;
    }));
    Server { addr, store, project: shared, feed, reload, _shutdown: shutdown, _web: web }
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

/// Minimal HTTP/1.1 client over raw TCP, so paths reach the server unnormalized.
async fn http(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> Reply {
    http_with(addr, method, path, body, &[("Content-Type", "application/json")]).await
}

/// Like [`http`] with the given headers instead of the JSON content type.
async fn http_with(addr: SocketAddr, method: &str, path: &str, body: Option<&str>, headers: &[(&str, &str)]) -> Reply {
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let body = body.unwrap_or("");
    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    Reply { status, head: head.to_ascii_lowercase(), body: body.to_string() }
}

fn parse(r: &Reply) -> Json {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body))
}

#[tokio::test]
async fn rest_api() {
    let s = start().await;
    let bundle = parse(&http(s.addr, "GET", "/api/bundle", None).await);
    assert_eq!(bundle["meta"]["name"], "Test");
    assert_eq!(bundle["tags"].as_array().unwrap().len(), 4);

    let values = parse(&http(s.addr, "GET", "/api/values", None).await);
    assert_eq!(values.as_object().unwrap().len(), 4);
    assert_eq!(values["A"]["q"], "good");
    assert_eq!(values["RO"]["q"], "bad");
    assert!(values["A"]["ts"].as_i64().unwrap() > 0);

    let ok = http(s.addr, "POST", "/api/write", Some(r#"{"tag":"A","value":12.5}"#)).await;
    assert_eq!((ok.status, parse(&ok)), (200, json!({"ok": true})));
    assert_eq!(s.store.get_by_name("A").unwrap().value, Value::Float(12.5));

    let inc = http(s.addr, "POST", "/api/write", Some(r#"{"tag":"A","value":2,"mode":"increase"}"#)).await;
    assert_eq!(inc.status, 200);
    assert_eq!(s.store.get_by_name("A").unwrap().value, Value::Float(14.5));

    let toggle = http(s.addr, "POST", "/api/write", Some(r#"{"tag":"Flag","value":null,"mode":"toggle"}"#)).await;
    assert_eq!(toggle.status, 200);
    assert_eq!(s.store.get_by_name("Flag").unwrap().value, Value::Bool(true));

    for (body, status, needle) in [
        (r#"{"tag":"Nope","value":1}"#, 404, "does not exist"),
        (r#"{"tag":"RO","value":1}"#, 403, "read-only"),
        (r#"{"tag":"B","value":1.5}"#, 400, "invalid value"),
        (r#"{"tag":"B","value":[1]}"#, 400, "value must be"),
        (r#"{"tag":"B""#, 400, "invalid request"),
        (r#"{"tag":"B","value":1,"mode":"explode"}"#, 400, "invalid request"),
    ] {
        let r = http(s.addr, "POST", "/api/write", Some(body)).await;
        let j = parse(&r);
        assert_eq!(r.status, status, "{body}: {}", r.body);
        assert_eq!(j["ok"], false);
        assert!(j["error"].as_str().unwrap().contains(needle), "{body}: {}", r.body);
    }

    let diag = parse(&http(s.addr, "GET", "/api/diagnostics", None).await);
    assert_eq!(diag["drivers"][0]["name"], "fm");
    assert!(diag["uptime_s"].is_u64());
    assert_eq!(diag["tags"]["total"], 4);
    assert_eq!(diag["tags"]["bad"], 1);

    assert_eq!(http(s.addr, "GET", "/api/extra", None).await.body, "extra");
}

#[tokio::test]
async fn static_files_and_traversal() {
    let s = start().await;
    let root = http(s.addr, "GET", "/", None).await;
    assert_eq!(root.status, 307);
    assert!(root.head.contains("location: /hmi/"), "{}", root.head);
    let bare = http(s.addr, "GET", "/hmi", None).await;
    assert!(bare.status == 308 && bare.head.contains("location: /hmi/"), "{}", bare.head);

    let index = http(s.addr, "GET", "/hmi/", None).await;
    assert_eq!(index.status, 200);
    assert!(index.body.contains("<title>hmi</title>"));
    let js = http(s.addr, "GET", "/hmi/app.js", None).await;
    assert!(js.status == 200 && js.head.contains("javascript"), "{}", js.head);
    assert_eq!(http(s.addr, "GET", "/shared/style.css", None).await.body, "body{}");
    assert_eq!(http(s.addr, "GET", "/studio/", None).await.body, "studio");
    assert_eq!(http(s.addr, "GET", "/hmi/missing.js", None).await.status, 404);

    for path in [
        "/hmi/../../secret.txt",
        "/hmi/%2e%2e/%2e%2e/secret.txt",
        "/hmi/..%2f..%2fsecret.txt",
        "/hmi/..%5c..%5csecret.txt",
        "/hmi/..\\..\\secret.txt",
        "/shared/%2e%2e%2f%2e%2e%2fsecret.txt",
        "/hmi/C:%2fWindows%2fwin.ini",
    ] {
        let r = http(s.addr, "GET", path, None).await;
        assert!(r.status == 404 || r.status == 400, "{path} -> {}", r.status);
        assert!(!r.body.contains("TOP-SECRET"), "{path} leaked the file");
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: SocketAddr) -> Ws {
    tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap().0
}

async fn send(ws: &mut Ws, msg: Json) {
    ws.send(Message::Text(msg.to_string())).await.unwrap();
}

/// Next JSON message, failing after a timeout.
async fn recv(ws: &mut Ws) -> Json {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("message in time").unwrap().unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

/// Next message of `ty`, skipping others.
async fn recv_type(ws: &mut Ws, ty: &str) -> Json {
    loop {
        let m = recv(ws).await;
        if m["type"] == ty {
            return m;
        }
    }
}

#[tokio::test]
async fn websocket_session() {
    let s = start().await;
    let mut ws = connect(s.addr).await;
    let hello = recv(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["project"], "Test");
    assert_eq!(hello["main_screen"], "Main");
    assert!(hello["server_time"].as_i64().unwrap() > 0);
    assert_eq!(recv(&mut ws).await, json!({"type": "alarms", "alarms": []}));

    // Immediate snapshot for the subscribed tags only.
    send(&mut ws, json!({"op": "subscribe", "tags": ["A", "Unknown"]})).await;
    let snap = recv(&mut ws).await;
    assert_eq!(snap["type"], "values");
    assert_eq!(snap["values"].as_object().unwrap().len(), 1);
    assert_eq!(snap["values"]["A"]["v"], 0.0);

    // Several changes within one batch: one message, latest value only. B is not subscribed.
    for v in [1.0, 2.0, 3.0] {
        s.store.update(s.store.id("A").unwrap(), Value::Float(v), Quality::Good);
    }
    s.store.update(s.store.id("B").unwrap(), Value::Int(9), Quality::Good);
    let batch = recv_type(&mut ws, "values").await;
    assert_eq!(batch["values"], json!({"A": batch["values"]["A"].clone()}));
    assert_eq!(batch["values"]["A"]["v"], 3.0);

    send(&mut ws, json!({"op": "ping"})).await;
    assert_eq!(recv_type(&mut ws, "pong").await, json!({"type": "pong"}));

    send(&mut ws, json!({"op": "write", "id": 7, "tag": "B", "value": 5})).await;
    assert_eq!(recv_type(&mut ws, "write-result").await, json!({"type": "write-result", "id": 7, "ok": true}));
    assert_eq!(s.store.get_by_name("B").unwrap().value, Value::Int(5));
    send(&mut ws, json!({"op": "write", "id": 8, "tag": "RO", "value": 1, "mode": "set"})).await;
    let fail = recv_type(&mut ws, "write-result").await;
    assert_eq!((fail["id"].clone(), fail["ok"].clone()), (json!(8), json!(false)));
    assert!(fail["error"].as_str().unwrap().contains("read-only"));

    // Feeds push their new value.
    s.feed.send(json!({"type": "alarms", "alarms": [{"name": "X"}]})).unwrap();
    assert_eq!(recv_type(&mut ws, "alarms").await["alarms"][0]["name"], "X");

    // Swapping the project: reload message and the new bundle.
    s.project.replace(project("Renamed"));
    assert_eq!(recv_type(&mut ws, "reload").await, json!({"type": "reload"}));
    assert_eq!(parse(&http(s.addr, "GET", "/api/bundle", None).await)["meta"]["name"], "Renamed");
    s.reload.send(1).unwrap();
    recv_type(&mut ws, "reload").await;

    // Unsubscribe: no more values for A.
    send(&mut ws, json!({"op": "unsubscribe", "tags": ["A"]})).await;
    send(&mut ws, json!({"op": "ping"})).await;
    recv_type(&mut ws, "pong").await;
    s.store.update(s.store.id("A").unwrap(), Value::Float(42.0), Quality::Good);
    send(&mut ws, json!({"op": "subscribe-all"})).await;
    let all = recv_type(&mut ws, "values").await;
    assert_eq!(all["values"].as_object().unwrap().len(), 4);
    assert_eq!(all["values"]["A"]["v"], 42.0);
}

#[tokio::test]
async fn lagging_client_gets_current_values_again() {
    let s = start().await;
    let mut ws = connect(s.addr).await;
    recv_type(&mut ws, "hello").await;
    send(&mut ws, json!({"op": "subscribe", "tags": ["A"]})).await;
    recv_type(&mut ws, "values").await;
    // More changes than the change bus holds, without yielding: the session lags.
    let id = s.store.id("A").unwrap();
    for i in 0..20_000 {
        s.store.update(id, Value::Float(f64::from(i)), Quality::Good);
    }
    loop {
        let m = recv_type(&mut ws, "values").await;
        if m["values"]["A"]["v"] == 19_999.0 {
            break;
        }
    }
}

#[tokio::test]
async fn shutdown_closes_sessions() {
    let s = start().await;
    let mut ws = connect(s.addr).await;
    recv_type(&mut ws, "hello").await;
    s._shutdown.send(true).unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(m)) = ws.next().await {
            if m.is_close() {
                break;
            }
        }
    })
    .await;
    assert!(end.is_ok(), "session did not close on shutdown");
}

#[tokio::test]
async fn cross_origin_writes_are_refused() {
    let s = start_with(project("Test"), vec!["https://scada.example.com".into()]).await;
    let write = r#"{"tag":"A","value":7}"#;
    let json_from = |origin: &'static str| [("Content-Type", "application/json"), ("Origin", origin)];

    // A page on another site, with or without a "simple" content type.
    for headers in [
        &json_from("http://evil.example")[..],
        &[("Content-Type", "text/plain"), ("Origin", "http://evil.example")][..],
        &json_from("null")[..],
        &json_from("http://localhost:9999")[..],
    ] {
        let r = http_with(s.addr, "POST", "/api/write", Some(write), headers).await;
        assert_eq!(r.status, 403, "{headers:?}: {}", r.body);
        assert_eq!(parse(&r)["ok"], false);
    }
    // Routes merged from other crates (alarms, studio) are covered too.
    assert_eq!(http_with(s.addr, "POST", "/api/extra", None, &[("Origin", "http://evil.example")]).await.status, 403);
    assert_eq!(s.store.get_by_name("A").unwrap().value, Value::Float(0.0), "nothing was written");

    // Form-encoded or text bodies are refused even without an Origin (old browsers, curl -d).
    for ct in ["application/x-www-form-urlencoded", "text/plain"] {
        let r = http_with(s.addr, "POST", "/api/write", Some(write), &[("Content-Type", ct)]).await;
        assert_eq!(r.status, 415, "{ct}: {}", r.body);
    }
    let r = http_with(s.addr, "POST", "/api/write", Some(write), &[]).await;
    assert_eq!(r.status, 415, "missing content type: {}", r.body);
    assert_eq!(s.store.get_by_name("A").unwrap().value, Value::Float(0.0));

    // The HMI's own pages, an allow-listed origin, and tools without Origin work.
    let ok = http_with(s.addr, "POST", "/api/write", Some(write), &json_from("http://localhost")).await;
    assert_eq!(ok.status, 200, "{}", ok.body);
    let ok = http_with(s.addr, "POST", "/api/write", Some(write), &json_from("https://scada.example.com")).await;
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert_eq!(http(s.addr, "POST", "/api/write", Some(write)).await.status, 200);
    // A body-less POST (alarm ack) needs no content type.
    assert_eq!(http_with(s.addr, "POST", "/api/extra", None, &[]).await.body, "posted");
    // Reading is not restricted (images, history in other dashboards).
    assert_eq!(http_with(s.addr, "GET", "/api/values", None, &[("Origin", "http://evil.example")]).await.status, 200);
}

#[tokio::test]
async fn cross_origin_websocket_is_refused() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let s = start().await;
    let mut req = format!("ws://{}/ws", s.addr).into_client_request().unwrap();
    req.headers_mut().insert("Origin", "http://evil.example".parse().unwrap());
    match tokio_tungstenite::connect_async(req).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(res)) => assert_eq!(res.status(), 403),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("cross-origin websocket was accepted"),
    }
    let mut req = format!("ws://{}/ws", s.addr).into_client_request().unwrap();
    req.headers_mut().insert("Origin", format!("http://{}", s.addr).parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("same-origin websocket");
    assert_eq!(recv(&mut ws).await["type"], "hello");
}

#[tokio::test]
async fn overlapping_pulses_do_not_leave_the_tag_set() {
    let s = start().await;
    let pulse = r#"{"tag":"Flag","value":true,"mode":"pulse"}"#;
    let (a, b) = tokio::join!(http(s.addr, "POST", "/api/write", Some(pulse)), async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        http(s.addr, "POST", "/api/write", Some(pulse)).await
    });
    assert_eq!(a.status, 200, "{}", a.body);
    assert_eq!(b.status, 409, "{}", b.body);
    assert!(parse(&b)["error"].as_str().unwrap().contains("in progress"));
    assert_eq!(s.store.get_by_name("Flag").unwrap().value, Value::Bool(false), "pulse restored");

    // Read-modify-write modes queue behind each other instead of racing.
    let inc = r#"{"tag":"A","value":1,"mode":"increase"}"#;
    let replies = futures_util::future::join_all((0..20).map(|_| http(s.addr, "POST", "/api/write", Some(inc)))).await;
    assert!(replies.iter().all(|r| r.status == 200));
    assert_eq!(s.store.get_by_name("A").unwrap().value, Value::Float(20.0));
}

#[tokio::test]
async fn websocket_limits_pending_writes() {
    let mut p = project("Test");
    // A device tag whose driver never answers: each write waits for the tagdb timeout.
    let mut dev = mem("Dev", DataType::I32);
    dev.address = p.tags[3].address.clone();
    p.tags.push(dev);
    let s = start_with(p, Vec::new()).await;
    let (tx, _rx) = tokio::sync::mpsc::channel(1024);
    s.store.register_writer("fm", tx);

    let mut ws = connect(s.addr).await;
    recv_type(&mut ws, "hello").await;
    for id in 0..=16 {
        send(&mut ws, json!({"op": "write", "id": id, "tag": "Dev", "value": 1})).await;
    }
    let refused = recv_type(&mut ws, "write-result").await;
    assert_eq!(refused["id"], 16, "{refused}");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"], "too many pending writes");
    // The session itself still answers.
    send(&mut ws, json!({"op": "ping"})).await;
    recv_type(&mut ws, "pong").await;
}
