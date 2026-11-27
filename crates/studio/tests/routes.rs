//! Route tests against a copy of a small project written to a temp folder.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use tokio::sync::watch;
use tower::ServiceExt;

const PROJECT: &str = r##"name = "Test plant"
culture = "en-us"
main_screen = "Main"
"##;

const TAGS: &str = r#"[[tag]]
name = "Flow"
type = "f32"

[[tag]]
name = "Pump_On"
type = "bool"
access = "read-write"
"#;

const MAIN: &str = r##"name = "Main"
width = 800.0
height = 600.0
background = "#3F3F3F"

[[element]]
id = "Display1"
kind = "display"
tag = "Flow"
unit = "m3/h"
format = "#0.0"
x = 10.0
y = 20.0
w = 120.0
h = 40.0

[[element]]
id = "Button1"
kind = "button"
text = "Faceplate"
x = 10.0
y = 80.0
w = 120.0
h = 40.0

[[element.on_click]]
action = "open-screen"
screen = "Popup"
"##;

const POPUP: &str = r#"name = "Popup"
width = 300.0
height = 200.0
"#;

struct Fixture {
    dir: tempfile::TempDir,
    app: Router,
    reload: watch::Receiver<u64>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("screens")).expect("mkdir");
        fs::write(root.join("project.toml"), PROJECT).expect("write");
        fs::write(root.join("tags.toml"), TAGS).expect("write");
        fs::write(root.join("screens/Main.toml"), MAIN).expect("write");
        fs::write(root.join("screens/Popup.toml"), POPUP).expect("write");
        let (tx, rx) = watch::channel(0u64);
        let app = moviclone_studio::routes(root.to_path_buf(), tx);
        Self { dir, app, reload: rx }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root().join(rel)).expect("read project file")
    }

    async fn call(&self, method: Method, uri: &str, body: Option<String>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(body.map(Body::from).unwrap_or_else(Body::empty)).expect("request");
        let res = self.app.clone().oneshot(req).await.expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.expect("body");
        let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
        (status, value)
    }

    async fn project(&self) -> Value {
        let (status, v) = self.call(Method::GET, "/api/studio/project", None).await;
        assert_eq!(status, StatusCode::OK);
        v
    }

    fn reloads(&self) -> u64 {
        *self.reload.borrow()
    }
}

fn codes(v: &Value) -> Vec<String> {
    v["diagnostics"]
        .as_array()
        .map(|a| a.iter().filter_map(|d| d["code"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn get_project_returns_project_and_diagnostics() {
    let fx = Fixture::new();
    let v = fx.project().await;
    assert_eq!(v["project"]["meta"]["name"], "Test plant");
    assert_eq!(v["project"]["screens"].as_array().map(Vec::len), Some(2));
    assert_eq!(v["project"]["tags"][1]["access"], "read-write");
    assert!(v["diagnostics"].is_array());
    // Pump_On is unused: a note, with the file relative to the project folder.
    let n050 = v["diagnostics"].as_array().and_then(|a| a.iter().find(|d| d["code"] == "N050")).cloned();
    let n050 = n050.expect("N050 note");
    assert_eq!(n050["severity"], "note");
    assert_eq!(n050["file"], "tags.toml");
}

#[tokio::test]
async fn put_screen_persists_and_bumps_reload() {
    let fx = Fixture::new();
    let mut screen = fx.project().await["project"]["screens"][0].clone();
    assert_eq!(screen["name"], "Main");
    screen["element"][0]["x"] = json!(42.5);
    screen["element"][0]["rotation"] = json!(90.0);
    screen["element"].as_array_mut().expect("elements").push(json!({
        "id": "Label7", "kind": "label", "text": "Hello", "x": 1.0, "y": 2.0, "w": 50.0, "h": 20.0
    }));
    let (status, v) = fx.call(Method::PUT, "/api/studio/screens/Main", Some(screen.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["ok"], true);
    assert_eq!(v["file"], "screens/Main.toml");
    assert!(v["diagnostics"].is_array());
    assert_eq!(fx.reloads(), 1);

    let text = fx.read("screens/Main.toml");
    assert!(text.contains("x = 42.5"), "{text}");
    assert!(text.contains("rotation = 90.0"), "{text}");
    assert!(text.contains("[[element.on_click]]"), "{text}");
    let back = fx.project().await;
    assert_eq!(back["project"]["screens"][0]["element"][2]["text"], "Hello");
    assert_eq!(back["project"]["screens"][0]["element"][1]["on_click"][0]["screen"], "Popup");
}

#[tokio::test]
async fn put_new_screen_creates_file_and_reports_broken_references() {
    let fx = Fixture::new();
    let body = json!({
        "name": "Extra", "width": 400.0, "height": 300.0,
        "element": [{ "id": "D", "kind": "display", "tag": "Missing", "x": 0, "y": 0, "w": 10, "h": 10 }]
    });
    let (status, v) = fx.call(Method::PUT, "/api/studio/screens/Extra", Some(body.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(fx.root().join("screens/Extra.toml").exists());
    assert!(codes(&v).contains(&"E034".to_string()), "{v}");
    let d = v["diagnostics"].as_array().and_then(|a| a.iter().find(|d| d["code"] == "E034")).cloned();
    assert_eq!(d.expect("E034")["file"], "screens/Extra.toml");
}

#[tokio::test]
async fn screen_name_must_match_url() {
    let fx = Fixture::new();
    let body = json!({ "name": "Other", "width": 10.0, "height": 10.0 });
    let (status, v) = fx.call(Method::PUT, "/api/studio/screens/Main", Some(body.to_string())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(v["ok"], false);
    assert!(fx.read("screens/Main.toml").contains("Display1"));
    assert_eq!(fx.reloads(), 0);
}

#[tokio::test]
async fn traversal_attempts_are_refused_and_write_nothing() {
    let fx = Fixture::new();
    let before: Vec<_> = list_all(fx.root());
    let outside = fx.root().parent().expect("parent").join("evil.toml");
    for name in ["..", "..%2Fevil", "..%5Cevil", "%2e%2e", "a%2Fb", "C%3Aevil", ".hidden", "CON", "x%00", "with%20space"] {
        let decoded = name.replace("%2F", "/").replace("%5C", "\\").replace("%2e", ".").replace("%3A", ":").replace("%00", "\0").replace("%20", " ");
        let body = json!({ "name": decoded, "width": 10.0, "height": 10.0 }).to_string();
        let uri = format!("/api/studio/screens/{name}");
        let (status, _) = fx.call(Method::PUT, &uri, Some(body)).await;
        assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND, "{name}: {status}");
        let (status, _) = fx.call(Method::DELETE, &uri, None).await;
        assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::NOT_FOUND, "{name}: {status}");
    }
    // Unencoded separators do not even reach the handler.
    let (status, _) = fx.call(Method::PUT, "/api/studio/screens/../evil", Some("{}".into())).await;
    assert_ne!(status, StatusCode::OK);
    assert!(!outside.exists());
    assert_eq!(list_all(fx.root()), before);
    assert_eq!(fx.reloads(), 0);
}

#[tokio::test]
async fn invalid_screen_bodies_are_refused() {
    let fx = Fixture::new();
    let original = fx.read("screens/Main.toml");
    let bad = [
        "not json".to_string(),
        json!({ "name": "Main" }).to_string(),
        json!({ "name": "Main", "width": "wide", "height": 10.0 }).to_string(),
        json!({ "name": "Main", "width": 10.0, "height": 10.0, "colour": "red" }).to_string(),
        json!({ "name": "Main", "width": 10.0, "height": 10.0, "element": [{ "id": "a", "kind": "sprocket", "x": 0, "y": 0, "w": 1, "h": 1 }] }).to_string(),
        json!({ "name": "Main", "width": 10.0, "height": 10.0, "element": [{ "id": "a", "kind": "label", "x": 0, "y": 0, "w": 1, "h": 1 }] }).to_string(),
        json!({ "name": "Main", "width": 10.0, "height": 10.0, "element": [{ "id": "a", "kind": "button", "text": "x", "x": 0, "y": 0, "w": 1, "h": 1, "on_click": [{ "action": "launch-rocket" }] }] }).to_string(),
        json!({ "name": "Main", "width": 0.0, "height": 10.0 }).to_string(),
        json!({ "name": "Main", "width": -5.0, "height": 10.0 }).to_string(),
    ];
    for body in bad {
        let (status, v) = fx.call(Method::PUT, "/api/studio/screens/Main", Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(v["ok"], false, "{body}");
        assert!(v["error"].as_str().is_some_and(|e| !e.is_empty()), "{body}");
    }
    assert_eq!(fx.read("screens/Main.toml"), original);
    assert_eq!(fx.reloads(), 0);
}

#[tokio::test]
async fn delete_screen_removes_file() {
    let fx = Fixture::new();
    let (status, v) = fx.call(Method::DELETE, "/api/studio/screens/Popup", None).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(!fx.root().join("screens/Popup.toml").exists());
    // Main's button now points nowhere.
    assert!(codes(&v).contains(&"E040".to_string()), "{v}");
    assert_eq!(fx.reloads(), 1);
    let (status, v) = fx.call(Method::DELETE, "/api/studio/screens/Popup", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["ok"], false);
}

#[tokio::test]
async fn screen_loaded_from_differently_named_file_is_saved_in_place() {
    let fx = Fixture::new();
    fs::rename(fx.root().join("screens/Popup.toml"), fx.root().join("screens/popup-old.toml")).expect("rename");
    let body = json!({ "name": "Popup", "width": 333.0, "height": 200.0 });
    let (status, v) = fx.call(Method::PUT, "/api/studio/screens/Popup", Some(body.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["file"], "screens/popup-old.toml");
    assert!(!fx.root().join("screens/Popup.toml").exists(), "a second file would load as a duplicate screen");
    assert!(fx.read("screens/popup-old.toml").contains("width = 333.0"));
}

#[tokio::test]
async fn put_tags_persists_and_validates() {
    let fx = Fixture::new();
    let mut tags = fx.project().await["project"]["tags"].clone();
    tags[0]["description"] = json!("Line flow");
    tags.as_array_mut().expect("tags").push(json!({
        "name": "Level", "type": "f64", "access": "read",
        "address": { "kind": "modbus", "driver": "plc", "station": "S1", "area": "holding-register", "register": 10 }
    }));
    let (status, v) = fx.call(Method::PUT, "/api/studio/tags", Some(tags.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["file"], "tags.toml");
    // Driver "plc" does not exist.
    assert!(codes(&v).contains(&"E010".to_string()), "{v}");
    let text = fx.read("tags.toml");
    assert!(text.contains("description = \"Line flow\""), "{text}");
    assert!(text.contains("[tag.address]"), "{text}");
    assert_eq!(fx.reloads(), 1);

    let dup = json!([{ "name": "A", "type": "bool" }, { "name": "A", "type": "bool" }]);
    let (status, v) = fx.call(Method::PUT, "/api/studio/tags", Some(dup.to_string())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(codes(&v).contains(&"E002".to_string()), "{v}");
}

#[tokio::test]
async fn invalid_tag_bodies_are_refused() {
    let fx = Fixture::new();
    let original = fx.read("tags.toml");
    for body in [
        json!({ "name": "A", "type": "bool" }).to_string(),
        json!([{ "name": "A", "type": "float" }]).to_string(),
        json!([{ "name": "A", "type": "bool", "access": "sometimes" }]).to_string(),
        json!([{ "name": "A", "type": "bool", "colour": "red" }]).to_string(),
        json!([{ "name": "A", "type": "u16", "address": { "kind": "modbus", "driver": "d", "station": "s", "area": "holding-register", "register": 70000 } }]).to_string(),
        "[".to_string(),
    ] {
        let (status, v) = fx.call(Method::PUT, "/api/studio/tags", Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(v["ok"], false);
    }
    assert_eq!(fx.read("tags.toml"), original);
}

#[tokio::test]
async fn put_alarms_writes_and_empty_list_removes_file() {
    let fx = Fixture::new();
    let alarms = json!([{
        "name": "HighFlow", "tag": "Flow", "when": "above", "limit": 120.0, "deadband": 2.0,
        "severity": "high", "message": "Flow above 120", "area": "Main"
    }, {
        "name": "PumpRunning", "tag": "Pump_On", "when": "is-true", "message": "Pump running", "ack_required": false
    }]);
    let (status, v) = fx.call(Method::PUT, "/api/studio/alarms", Some(alarms.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["file"], "alarms.toml");
    let text = fx.read("alarms.toml");
    assert!(text.contains("when = \"above\""), "{text}");
    assert!(text.contains("ack_required = false"), "{text}");
    let back = fx.project().await;
    assert_eq!(back["project"]["alarms"][0]["limit"], 120.0);

    // A boolean condition on an analog tag is a check error, not a refusal.
    let wrong = json!([{ "name": "X", "tag": "Flow", "when": "is-true", "message": "m" }]);
    let (status, v) = fx.call(Method::PUT, "/api/studio/alarms", Some(wrong.to_string())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(codes(&v).contains(&"E062".to_string()), "{v}");

    let (status, _) = fx.call(Method::PUT, "/api/studio/alarms", Some("[]".into())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!fx.root().join("alarms.toml").exists());

    for bad in [
        json!([{ "name": "X", "tag": "Flow", "when": "above", "message": "m" }]),
        json!([{ "name": "X", "tag": "Flow", "when": "sideways", "message": "m" }]),
        json!([{ "name": "X", "tag": "Flow", "when": "is-true", "message": "m", "severity": "apocalyptic" }]),
    ] {
        let (status, _) = fx.call(Method::PUT, "/api/studio/alarms", Some(bad.to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn put_meta_persists_and_refuses_bad_bodies() {
    let fx = Fixture::new();
    let mut meta = fx.project().await["project"]["meta"].clone();
    meta["description"] = json!("Engineering copy");
    meta["main_screen"] = json!("Nowhere");
    let (status, v) = fx.call(Method::PUT, "/api/studio/meta", Some(meta.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(codes(&v).contains(&"E030".to_string()), "{v}");
    assert!(fx.read("project.toml").contains("description = \"Engineering copy\""));

    let original = fx.read("project.toml");
    for bad in [
        json!({ "name": "" }),
        json!({ "name": "x", "password": "hunter2" }),
        json!({ "name": "x", "sampling": { "fast_ms": "fast" } }),
        json!({ "culture": "en-us" }),
    ] {
        let (status, _) = fx.call(Method::PUT, "/api/studio/meta", Some(bad.to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(fx.read("project.toml"), original);
}

#[tokio::test]
async fn broken_file_is_reported_not_fatal() {
    let fx = Fixture::new();
    fs::write(fx.root().join("tags.toml"), "[[tag]]\nname = \"X\"\ntype = \"nope\"\n").expect("write");
    let v = fx.project().await;
    assert!(v["project"].is_null());
    assert_eq!(codes(&v), vec!["E000".to_string()]);
    assert_eq!(v["diagnostics"][0]["file"], "tags.toml");

    // Saving a valid tag list repairs the project.
    let (status, v) = fx.call(Method::PUT, "/api/studio/tags", Some(json!([{ "name": "Flow", "type": "f32" }]).to_string())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!codes(&v).contains(&"E000".to_string()), "{v}");
    assert!(!fx.project().await["project"].is_null());
}

#[tokio::test]
async fn unknown_routes_and_methods_are_rejected() {
    let fx = Fixture::new();
    let (status, _) = fx.call(Method::GET, "/api/studio/screens/Main", None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (status, _) = fx.call(Method::POST, "/api/studio/tags", Some("[]".into())).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}

fn list_all(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).expect("list").filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p.clone());
            }
            out.push(p.strip_prefix(root).expect("prefix").to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}

#[tokio::test]
async fn screen_names_differing_only_in_case_never_touch_each_others_file() {
    let fx = Fixture::new();
    let original = fx.read("screens/Main.toml");
    let body = json!({ "name": "main", "width": 100.0, "height": 100.0 });
    let (status, v) = fx.call(Method::PUT, "/api/studio/screens/main", Some(body.to_string())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{v}");
    assert!(v["error"].as_str().is_some_and(|e| e.contains("\"Main\"") && e.contains("case-insensitive")), "{v}");
    assert_eq!(fx.read("screens/Main.toml"), original, "Main.toml was overwritten");
    assert!(!list_all(fx.root()).iter().any(|f| f.ends_with("main.toml")), "no second file either");

    let (status, v) = fx.call(Method::DELETE, "/api/studio/screens/main", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    assert_eq!(fx.read("screens/Main.toml"), original, "Main.toml was deleted");
    assert_eq!(fx.reloads(), 0);

    // A case-only rename done the way the studio does it (temporary name first)
    // keeps the screen's content.
    let mut screen = fx.project().await["project"]["screens"][0].clone();
    assert_eq!(screen["name"], "Main");
    screen["name"] = json!("Main.renaming");
    assert_eq!(fx.call(Method::PUT, "/api/studio/screens/Main.renaming", Some(screen.to_string())).await.0, StatusCode::OK);
    assert_eq!(fx.call(Method::DELETE, "/api/studio/screens/Main", None).await.0, StatusCode::OK);
    screen["name"] = json!("main");
    assert_eq!(fx.call(Method::PUT, "/api/studio/screens/main", Some(screen.to_string())).await.0, StatusCode::OK);
    assert_eq!(fx.call(Method::DELETE, "/api/studio/screens/Main.renaming", None).await.0, StatusCode::OK);
    let names: Vec<String> = fx.project().await["project"]["screens"]
        .as_array()
        .expect("screens")
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    assert!(names.contains(&"main".to_string()) && !names.contains(&"Main".to_string()), "{names:?}");
    assert!(fx.read("screens/main.toml").contains("Display1"), "content survived the rename");
}

// ---------------------------------------------------------------- versions

impl Fixture {
    async fn call_if_match(&self, method: Method, uri: &str, version: &str, body: Option<String>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri).header("if-match", format!("\"{version}\""));
        if body.is_some() {
            req = req.header("content-type", "application/json");
        }
        let req = req.body(body.map(Body::from).unwrap_or_else(Body::empty)).expect("request");
        let res = self.app.clone().oneshot(req).await.expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.expect("body");
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

/// A studio window opened before another save must not overwrite it: the save
/// that wiped out a fix in the demo (a stale Manifold) is refused with 409.
#[tokio::test]
async fn stale_screen_save_is_refused_and_writes_nothing() {
    let fx = Fixture::new();
    let opened = fx.project().await;
    let old_version = opened["versions"]["screen:Main"].as_str().expect("version of Main").to_string();
    let mut stale = opened["project"]["screens"][0].clone();

    // Someone else saves Main (another window, or a hand edit).
    let mut newer = stale.clone();
    newer["element"][0]["x"] = json!(500.0);
    let (status, v) = fx.call_if_match(Method::PUT, "/api/studio/screens/Main", &old_version, Some(newer.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let new_version = v["version"].as_str().expect("new version").to_string();
    assert_ne!(new_version, old_version);
    assert_eq!(fx.project().await["versions"]["screen:Main"], new_version.as_str());

    // The first window saves its old copy, based on the old version: refused.
    stale["element"][0]["y"] = json!(1.0);
    let before = fx.read("screens/Main.toml");
    let reloads = fx.reloads();
    let (status, v) = fx.call_if_match(Method::PUT, "/api/studio/screens/Main", &old_version, Some(stale.to_string())).await;
    assert_eq!(status, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["conflict"], true);
    assert_eq!(v["version"], new_version.as_str(), "tells the client what is on disk now");
    assert!(v["error"].as_str().unwrap_or_default().contains("screens/Main.toml"), "{v}");
    assert_eq!(fx.read("screens/Main.toml"), before, "nothing written");
    assert_eq!(fx.reloads(), reloads, "no reload for a refused save");

    // Overwriting on purpose: based on the current version, it goes through.
    let (status, _) = fx.call_if_match(Method::PUT, "/api/studio/screens/Main", &new_version, Some(stale.to_string())).await;
    assert_eq!(status, StatusCode::OK);
    assert!(fx.read("screens/Main.toml").contains("y = 1.0"));
}

#[tokio::test]
async fn stale_delete_and_list_saves_are_refused_unchecked_saves_still_work() {
    let fx = Fixture::new();
    let opened = fx.project().await;
    let versions = opened["versions"].clone();
    assert_eq!(versions["alarms"], moviclone_studio::versions::ABSENT);

    // Popup changes on disk; deleting it from the old version is refused.
    fs::write(fx.root().join("screens/Popup.toml"), "name = \"Popup\"\nwidth = 320.0\nheight = 200.0\n").expect("write");
    let old_popup = versions["screen:Popup"].as_str().expect("version");
    let (status, _) = fx.call_if_match(Method::DELETE, "/api/studio/screens/Popup", old_popup, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(fx.root().join("screens/Popup.toml").exists());

    // tags.toml changes by hand; a tag list based on the old version is refused.
    let tags = opened["project"]["tags"].clone();
    fs::write(fx.root().join("tags.toml"), format!("{TAGS}\n[[tag]]\nname = \"Level\"\ntype = \"f32\"\n")).expect("write");
    let (status, _) = fx.call_if_match(Method::PUT, "/api/studio/tags", versions["tags"].as_str().expect("v"), Some(tags.to_string())).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(fx.read("tags.toml").contains("Level"));

    // A new screen expects "absent"; a file that appeared meanwhile is a conflict.
    let new_screen = json!({ "name": "Extra", "width": 100.0, "height": 100.0 });
    fs::write(fx.root().join("screens/Extra.toml"), "name = \"Extra\"\nwidth = 5.0\nheight = 5.0\n").expect("write");
    let (status, _) = fx.call_if_match(Method::PUT, "/api/studio/screens/Extra", "absent", Some(new_screen.to_string())).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Without If-Match (scripts, curl) nothing is checked.
    let (status, v) = fx.call(Method::PUT, "/api/studio/tags", Some(tags.to_string())).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert!(v["version"].is_string());
}
