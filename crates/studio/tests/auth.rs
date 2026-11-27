//! Studio sign-in, roles and user management.

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use moviclone_studio::users::{self, Role, User};
use serde_json::{json, Value};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::sync::watch;
use tower::ServiceExt;

const TAGS: &str = "[[tag]]\nname = \"Flow\"\ntype = \"f32\"\n";

struct Rig {
    project: tempfile::TempDir,
    _data: tempfile::TempDir,
    users_file: PathBuf,
    /// The routes with no client address (like a test client or a proxy that hides it).
    app: Router,
}

impl Rig {
    fn new(initial: &[(&str, Role, &str)]) -> Self {
        let project = tempfile::tempdir().expect("tempdir");
        let root = project.path();
        fs::create_dir_all(root.join("screens")).expect("mkdir");
        fs::write(root.join("project.toml"), "name = \"T\"\nmain_screen = \"Main\"\n").expect("write");
        fs::write(root.join("tags.toml"), TAGS).expect("write");
        fs::write(root.join("screens/Main.toml"), "name = \"Main\"\nwidth = 100.0\nheight = 100.0\n").expect("write");
        let data = tempfile::tempdir().expect("tempdir");
        let users_file = users::users_file(data.path());
        if !initial.is_empty() {
            let list: Vec<User> = initial
                .iter()
                .map(|(n, r, p)| User { name: (*n).into(), role: *r, password: users::hash_password(p).expect("hash") })
                .collect();
            users::save(&users_file, &list).expect("users");
        }
        let auth = moviclone_studio::Auth::open(users_file.clone()).expect("auth");
        let app = moviclone_studio::routes(root.to_path_buf(), auth, watch::channel(0u64).0);
        Self { project, _data: data, users_file, app }
    }

    /// The same routes as seen from a client at `ip`.
    fn from(&self, ip: [u8; 4]) -> Router {
        self.app.clone().layer(MockConnectInfo(SocketAddr::from((ip, 50000))))
    }
}

struct Answer {
    status: StatusCode,
    body: Value,
    set_cookie: Option<String>,
}

async fn send(app: &Router, method: Method, uri: &str, cookie: Option<&str>, body: Option<Value>) -> Answer {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    if body.is_some() {
        req = req.header("content-type", "application/json");
    }
    let req = req.body(body.map(|b| Body::from(b.to_string())).unwrap_or_else(Body::empty)).expect("request");
    let res = app.clone().oneshot(req).await.expect("response");
    let status = res.status();
    let set_cookie = res.headers().get("set-cookie").and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX).await.expect("body");
    let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
    Answer { status, body, set_cookie }
}

async fn sign_in(app: &Router, name: &str, password: &str) -> String {
    let a = send(app, Method::POST, "/api/studio/session", None, Some(json!({ "name": name, "password": password }))).await;
    assert_eq!(a.status, StatusCode::OK, "{name}: {}", a.body);
    let c = a.set_cookie.expect("cookie");
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict"), "{c}");
    c.split(';').next().expect("pair").to_string()
}

fn tags_body() -> Value {
    json!([{ "name": "Flow", "type": "f32" }, { "name": "Level", "type": "f32" }])
}

#[tokio::test]
async fn nothing_works_without_signing_in() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let a = send(&rig.app, Method::GET, "/api/studio/project", None, None).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(a.body["auth"], "required");
    let a = send(&rig.app, Method::PUT, "/api/studio/tags", None, Some(tags_body())).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    let a = send(&rig.app, Method::PUT, "/api/studio/tags", Some("mc_studio=00"), Some(tags_body())).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "malformed token");
    assert!(!fs::read_to_string(rig.project.path().join("tags.toml")).unwrap().contains("Level"));

    let a = send(&rig.app, Method::GET, "/api/studio/session", None, None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.body["user"], Value::Null);
    assert_eq!(a.body["setup"], false);
}

#[tokio::test]
async fn first_administrator_only_from_this_machine_and_only_once() {
    let rig = Rig::new(&[]);
    let local = rig.from([127, 0, 0, 1]);
    let remote = rig.from([10, 0, 0, 7]);
    let creds = json!({ "name": "boss", "password": "first-admin-pass" });

    let a = send(&remote, Method::GET, "/api/studio/session", None, None).await;
    assert_eq!((a.body["setup"].clone(), a.body["can_setup"].clone()), (json!(true), json!(false)));
    let a = send(&remote, Method::POST, "/api/studio/setup", None, Some(creds.clone())).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let a = send(&rig.app, Method::POST, "/api/studio/setup", None, Some(creds.clone())).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "no client address: refused");
    let a = send(&local, Method::POST, "/api/studio/setup", None, Some(json!({ "name": "boss", "password": "short" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);

    let a = send(&local, Method::GET, "/api/studio/session", None, None).await;
    assert_eq!(a.body["can_setup"], true);
    let a = send(&local, Method::POST, "/api/studio/setup", None, Some(creds.clone())).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["user"], json!({ "name": "boss", "role": "admin" }));
    let cookie = a.set_cookie.expect("signed in").split(';').next().unwrap().to_string();
    let a = send(&local, Method::GET, "/api/studio/session", Some(&cookie), None).await;
    assert_eq!(a.body["user"]["name"], "boss");
    assert_eq!(a.body["setup"], false);

    let a = send(&local, Method::POST, "/api/studio/setup", None, Some(json!({ "name": "evil", "password": "evil-password" }))).await;
    assert_eq!(a.status, StatusCode::CONFLICT);
    let stored = fs::read_to_string(&rig.users_file).unwrap();
    assert!(stored.contains("boss") && !stored.contains("evil") && !stored.contains("first-admin-pass"));
}

#[tokio::test]
async fn roles_decide_what_a_user_may_do() {
    let rig = Rig::new(&[("val", Role::Viewer, "viewer-pass"), ("eng", Role::Engineer, "engineer-pass")]);
    let viewer = sign_in(&rig.app, "val", "viewer-pass").await;
    let a = send(&rig.app, Method::GET, "/api/studio/project", Some(&viewer), None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.body["project"]["meta"]["name"], "T");
    let a = send(&rig.app, Method::PUT, "/api/studio/tags", Some(&viewer), Some(tags_body())).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    assert_eq!(a.body["auth"], "forbidden");
    assert!(a.body["error"].as_str().unwrap().contains("engineer"));
    let a = send(&rig.app, Method::DELETE, "/api/studio/screens/Main", Some(&viewer), None).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    assert!(rig.project.path().join("screens/Main.toml").exists());

    // Names are matched without regard to case.
    let eng = sign_in(&rig.app, "ENG", "engineer-pass").await;
    let a = send(&rig.app, Method::PUT, "/api/studio/tags", Some(&eng), Some(tags_body())).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert!(fs::read_to_string(rig.project.path().join("tags.toml")).unwrap().contains("Level"));
    let a = send(&rig.app, Method::GET, "/api/studio/users", Some(&eng), None).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "only admins manage users");
}

#[tokio::test]
async fn wrong_passwords_are_refused_and_lock_the_name() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let wrong = json!({ "name": "eng", "password": "nope-nope" });
    let a = send(&rig.app, Method::POST, "/api/studio/session", None, Some(wrong.clone())).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert!(a.set_cookie.is_none());
    let a = send(&rig.app, Method::POST, "/api/studio/session", None, Some(json!({ "name": "ghost", "password": "whatever1" }))).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(a.body["error"], "wrong user name or password", "same answer for unknown names");
    for _ in 1..moviclone_studio::auth::MAX_FAILURES {
        send(&rig.app, Method::POST, "/api/studio/session", None, Some(wrong.clone())).await;
    }
    let a = send(&rig.app, Method::POST, "/api/studio/session", None, Some(json!({ "name": "eng", "password": "engineer-pass" }))).await;
    assert_eq!(a.status, StatusCode::TOO_MANY_REQUESTS, "locked even with the right password");
}

#[tokio::test]
async fn administrators_manage_users() {
    let rig = Rig::new(&[("boss", Role::Admin, "admin-pass1")]);
    let admin = sign_in(&rig.app, "boss", "admin-pass1").await;

    let a = send(&rig.app, Method::PUT, "/api/studio/users/kim", Some(&admin), Some(json!({ "role": "engineer" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "a new user needs a password");
    let a = send(&rig.app, Method::PUT, "/api/studio/users/kim", Some(&admin), Some(json!({ "role": "engineer", "password": "kim-password" }))).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["created"], true);
    let a = send(&rig.app, Method::PUT, "/api/studio/users/kim", Some(&admin), Some(json!({ "role": "root" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);

    let a = send(&rig.app, Method::GET, "/api/studio/users", Some(&admin), None).await;
    assert_eq!(a.body["users"], json!([{ "name": "boss", "role": "admin" }, { "name": "kim", "role": "engineer" }]));
    assert!(!a.body.to_string().contains("argon2"), "hashes never leave the server");

    // A role change applies to an open session at once.
    let kim = sign_in(&rig.app, "kim", "kim-password").await;
    assert_eq!(send(&rig.app, Method::PUT, "/api/studio/tags", Some(&kim), Some(tags_body())).await.status, StatusCode::OK);
    send(&rig.app, Method::PUT, "/api/studio/users/kim", Some(&admin), Some(json!({ "role": "viewer" }))).await;
    assert_eq!(send(&rig.app, Method::PUT, "/api/studio/tags", Some(&kim), Some(tags_body())).await.status, StatusCode::FORBIDDEN);

    // The last administrator stays an administrator and cannot remove themself.
    let a = send(&rig.app, Method::PUT, "/api/studio/users/boss", Some(&admin), Some(json!({ "role": "engineer" }))).await;
    assert_eq!(a.status, StatusCode::CONFLICT);
    let a = send(&rig.app, Method::DELETE, "/api/studio/users/boss", Some(&admin), None).await;
    assert_eq!(a.status, StatusCode::CONFLICT);

    // Removing a user ends their sessions.
    let a = send(&rig.app, Method::DELETE, "/api/studio/users/kim", Some(&admin), None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(send(&rig.app, Method::GET, "/api/studio/project", Some(&kim), None).await.status, StatusCode::UNAUTHORIZED);
    let a = send(&rig.app, Method::DELETE, "/api/studio/users/kim", Some(&admin), None).await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn own_password_change_and_sign_out() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let first = sign_in(&rig.app, "eng", "engineer-pass").await;
    let second = sign_in(&rig.app, "eng", "engineer-pass").await;

    let a = send(&rig.app, Method::PUT, "/api/studio/password", Some(&first), Some(json!({ "current": "wrong-one", "new": "brand-new-pass" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);
    let a = send(&rig.app, Method::PUT, "/api/studio/password", Some(&first), Some(json!({ "current": "engineer-pass", "new": "brand-new-pass" }))).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    // This window stays signed in; the other session ends.
    assert_eq!(send(&rig.app, Method::GET, "/api/studio/project", Some(&first), None).await.status, StatusCode::OK);
    assert_eq!(send(&rig.app, Method::GET, "/api/studio/project", Some(&second), None).await.status, StatusCode::UNAUTHORIZED);
    let a = send(&rig.app, Method::POST, "/api/studio/session", None, Some(json!({ "name": "eng", "password": "engineer-pass" }))).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "old password");
    sign_in(&rig.app, "eng", "brand-new-pass").await;

    let a = send(&rig.app, Method::DELETE, "/api/studio/session", Some(&first), None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.set_cookie.unwrap().contains("Max-Age=0"));
    assert_eq!(send(&rig.app, Method::GET, "/api/studio/project", Some(&first), None).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_hand_edited_users_file_applies_without_restart() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let eng = sign_in(&rig.app, "eng", "engineer-pass").await;
    // What `movi users role eng viewer` does while the runtime runs.
    let mut list = users::load(&rig.users_file).unwrap();
    list[0].role = Role::Viewer;
    std::thread::sleep(std::time::Duration::from_millis(20));
    users::save(&rig.users_file, &list).unwrap();
    assert_eq!(send(&rig.app, Method::PUT, "/api/studio/tags", Some(&eng), Some(tags_body())).await.status, StatusCode::FORBIDDEN);
}
