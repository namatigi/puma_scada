//! Sign-in, setup, roles and user management through `/api/auth/…`.

use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use moviclone_auth::users::{self, Role, User};
use moviclone_auth::{Auth, AuthError, Signed};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceExt;

struct Rig {
    _data: tempfile::TempDir,
    users_file: PathBuf,
    /// The routes with no client address (like a client behind a proxy that hides it).
    app: Router,
}

/// A route that needs `role`, standing in for an HMI or studio action.
async fn needs(State(_): State<Arc<Auth>>, me: Signed, role: Role) -> Result<Json<Value>, AuthError> {
    me.require(role, "this")?;
    Ok(Json(json!({ "ok": true, "user": me.name })))
}

impl Rig {
    fn new(initial: &[(&str, Role, &str)]) -> Self {
        Self::with(initial, false)
    }

    fn with(initial: &[(&str, Role, &str)], guest_view: bool) -> Self {
        let data = tempfile::tempdir().expect("tempdir");
        let users_file = users::users_file(data.path());
        if !initial.is_empty() {
            let list: Vec<User> = initial
                .iter()
                .map(|(n, r, p)| User { name: (*n).into(), role: *r, password: users::hash_password(p).expect("hash") })
                .collect();
            users::save(&users_file, &list).expect("users");
        }
        let auth = Auth::open(users_file.clone(), guest_view).expect("auth");
        let probes = Router::new()
            .route("/probe/viewer", get(|s, m| needs(s, m, Role::Viewer)))
            .route("/probe/operator", get(|s, m| needs(s, m, Role::Operator)))
            .route("/probe/engineer", get(|s, m| needs(s, m, Role::Engineer)))
            .with_state(auth.clone());
        let app = moviclone_auth::routes(auth).merge(probes);
        Self { _data: data, users_file, app }
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
    let a = send(app, Method::POST, "/api/auth/session", None, Some(json!({ "name": name, "password": password }))).await;
    assert_eq!(a.status, StatusCode::OK, "{name}: {}", a.body);
    let c = a.set_cookie.expect("cookie");
    assert!(c.starts_with("mc_session=") && c.contains("HttpOnly") && c.contains("SameSite=Strict"), "{c}");
    c.split(';').next().expect("pair").to_string()
}

async fn probe(app: &Router, role: &str, cookie: Option<&str>) -> StatusCode {
    send(app, Method::GET, &format!("/probe/{role}"), cookie, None).await.status
}

#[tokio::test]
async fn nothing_works_without_signing_in() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let a = send(&rig.app, Method::GET, "/probe/viewer", None, None).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(a.body["auth"], "required");
    assert_eq!(probe(&rig.app, "viewer", Some("mc_session=00")).await, StatusCode::UNAUTHORIZED, "malformed token");

    let a = send(&rig.app, Method::GET, "/api/auth/session", None, None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert_eq!(a.body["user"], Value::Null);
    assert_eq!(a.body["setup"], false);
    assert_eq!(a.body["guest_view"], false);
    let guest = Rig::with(&[("eng", Role::Engineer, "engineer-pass")], true);
    assert_eq!(send(&guest.app, Method::GET, "/api/auth/session", None, None).await.body["guest_view"], true);
}

#[tokio::test]
async fn first_administrator_only_from_this_machine_and_only_once() {
    let rig = Rig::new(&[]);
    let local = rig.from([127, 0, 0, 1]);
    let remote = rig.from([10, 0, 0, 7]);
    let creds = json!({ "name": "boss", "password": "first-admin-pass" });

    let a = send(&remote, Method::GET, "/api/auth/session", None, None).await;
    assert_eq!((a.body["setup"].clone(), a.body["can_setup"].clone()), (json!(true), json!(false)));
    let a = send(&remote, Method::POST, "/api/auth/setup", None, Some(creds.clone())).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN);
    let a = send(&rig.app, Method::POST, "/api/auth/setup", None, Some(creds.clone())).await;
    assert_eq!(a.status, StatusCode::FORBIDDEN, "no client address: refused");
    let a = send(&local, Method::POST, "/api/auth/setup", None, Some(json!({ "name": "boss", "password": "short" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);

    let a = send(&local, Method::GET, "/api/auth/session", None, None).await;
    assert_eq!(a.body["can_setup"], true);
    let a = send(&local, Method::POST, "/api/auth/setup", None, Some(creds.clone())).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["user"], json!({ "name": "boss", "role": "admin" }));
    let cookie = a.set_cookie.expect("signed in").split(';').next().unwrap().to_string();
    let a = send(&local, Method::GET, "/api/auth/session", Some(&cookie), None).await;
    assert_eq!(a.body["user"]["name"], "boss");
    assert_eq!(a.body["setup"], false);

    let a = send(&local, Method::POST, "/api/auth/setup", None, Some(json!({ "name": "evil", "password": "evil-password" }))).await;
    assert_eq!(a.status, StatusCode::CONFLICT);
    let stored = std::fs::read_to_string(&rig.users_file).unwrap();
    assert!(stored.contains("boss") && !stored.contains("evil") && !stored.contains("first-admin-pass"));
}

#[tokio::test]
async fn roles_include_the_ones_below() {
    let rig = Rig::new(&[("val", Role::Viewer, "viewer-pass"), ("op", Role::Operator, "operator-pass"), ("eng", Role::Engineer, "engineer-pass")]);
    let viewer = sign_in(&rig.app, "val", "viewer-pass").await;
    let operator = sign_in(&rig.app, "OP", "operator-pass").await; // names ignore case
    let engineer = sign_in(&rig.app, "eng", "engineer-pass").await;
    let grid = [
        (&viewer, [StatusCode::OK, StatusCode::FORBIDDEN, StatusCode::FORBIDDEN]),
        (&operator, [StatusCode::OK, StatusCode::OK, StatusCode::FORBIDDEN]),
        (&engineer, [StatusCode::OK, StatusCode::OK, StatusCode::OK]),
    ];
    for (cookie, want) in grid {
        let got = [probe(&rig.app, "viewer", Some(cookie)).await, probe(&rig.app, "operator", Some(cookie)).await, probe(&rig.app, "engineer", Some(cookie)).await];
        assert_eq!(got, want, "{cookie}");
    }
    let a = send(&rig.app, Method::GET, "/probe/engineer", Some(&operator), None).await;
    assert_eq!(a.body["auth"], "forbidden");
    assert!(a.body["error"].as_str().unwrap().contains("engineer"));
    assert_eq!(send(&rig.app, Method::GET, "/api/auth/users", Some(&engineer), None).await.status, StatusCode::FORBIDDEN, "only admins manage users");
}

#[tokio::test]
async fn wrong_passwords_are_refused_and_lock_the_name() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let wrong = json!({ "name": "eng", "password": "nope-nope" });
    let a = send(&rig.app, Method::POST, "/api/auth/session", None, Some(wrong.clone())).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert!(a.set_cookie.is_none());
    let a = send(&rig.app, Method::POST, "/api/auth/session", None, Some(json!({ "name": "ghost", "password": "whatever1" }))).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED);
    assert_eq!(a.body["error"], "wrong user name or password", "same answer for unknown names");
    for _ in 1..moviclone_auth::MAX_FAILURES {
        send(&rig.app, Method::POST, "/api/auth/session", None, Some(wrong.clone())).await;
    }
    let a = send(&rig.app, Method::POST, "/api/auth/session", None, Some(json!({ "name": "eng", "password": "engineer-pass" }))).await;
    assert_eq!(a.status, StatusCode::TOO_MANY_REQUESTS, "locked even with the right password");
}

#[tokio::test]
async fn administrators_manage_users() {
    let rig = Rig::new(&[("boss", Role::Admin, "admin-pass1")]);
    let admin = sign_in(&rig.app, "boss", "admin-pass1").await;

    let a = send(&rig.app, Method::PUT, "/api/auth/users/kim", Some(&admin), Some(json!({ "role": "operator" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "a new user needs a password");
    let a = send(&rig.app, Method::PUT, "/api/auth/users/kim", Some(&admin), Some(json!({ "role": "operator", "password": "kim-password" }))).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    assert_eq!(a.body["created"], true);
    let a = send(&rig.app, Method::PUT, "/api/auth/users/kim", Some(&admin), Some(json!({ "role": "root" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);

    let a = send(&rig.app, Method::GET, "/api/auth/users", Some(&admin), None).await;
    assert_eq!(a.body["users"], json!([{ "name": "boss", "role": "admin" }, { "name": "kim", "role": "operator" }]));
    assert_eq!(a.body["roles"], json!(["viewer", "operator", "engineer", "admin"]));
    assert!(!a.body.to_string().contains("argon2"), "hashes never leave the server");

    // A role change applies to an open session at once.
    let kim = sign_in(&rig.app, "kim", "kim-password").await;
    assert_eq!(probe(&rig.app, "operator", Some(&kim)).await, StatusCode::OK);
    send(&rig.app, Method::PUT, "/api/auth/users/kim", Some(&admin), Some(json!({ "role": "viewer" }))).await;
    assert_eq!(probe(&rig.app, "operator", Some(&kim)).await, StatusCode::FORBIDDEN);

    // The last administrator stays an administrator and cannot remove themself.
    let a = send(&rig.app, Method::PUT, "/api/auth/users/boss", Some(&admin), Some(json!({ "role": "engineer" }))).await;
    assert_eq!(a.status, StatusCode::CONFLICT);
    let a = send(&rig.app, Method::DELETE, "/api/auth/users/boss", Some(&admin), None).await;
    assert_eq!(a.status, StatusCode::CONFLICT);

    // Removing a user ends their sessions.
    assert_eq!(send(&rig.app, Method::DELETE, "/api/auth/users/kim", Some(&admin), None).await.status, StatusCode::OK);
    assert_eq!(probe(&rig.app, "viewer", Some(&kim)).await, StatusCode::UNAUTHORIZED);
    assert_eq!(send(&rig.app, Method::DELETE, "/api/auth/users/kim", Some(&admin), None).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn own_password_change_and_sign_out() {
    let rig = Rig::new(&[("eng", Role::Engineer, "engineer-pass")]);
    let first = sign_in(&rig.app, "eng", "engineer-pass").await;
    let second = sign_in(&rig.app, "eng", "engineer-pass").await;

    let a = send(&rig.app, Method::PUT, "/api/auth/password", Some(&first), Some(json!({ "current": "wrong-one", "new": "brand-new-pass" }))).await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST);
    let a = send(&rig.app, Method::PUT, "/api/auth/password", Some(&first), Some(json!({ "current": "engineer-pass", "new": "brand-new-pass" }))).await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.body);
    // This window stays signed in; the other session ends.
    assert_eq!(probe(&rig.app, "viewer", Some(&first)).await, StatusCode::OK);
    assert_eq!(probe(&rig.app, "viewer", Some(&second)).await, StatusCode::UNAUTHORIZED);
    let a = send(&rig.app, Method::POST, "/api/auth/session", None, Some(json!({ "name": "eng", "password": "engineer-pass" }))).await;
    assert_eq!(a.status, StatusCode::UNAUTHORIZED, "old password");
    sign_in(&rig.app, "eng", "brand-new-pass").await;

    let a = send(&rig.app, Method::DELETE, "/api/auth/session", Some(&first), None).await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.set_cookie.unwrap().contains("Max-Age=0"));
    assert_eq!(probe(&rig.app, "viewer", Some(&first)).await, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_hand_edited_users_file_applies_without_restart() {
    let rig = Rig::new(&[("op", Role::Operator, "operator-pass")]);
    let op = sign_in(&rig.app, "op", "operator-pass").await;
    // What `movi users role op viewer` does while the runtime runs.
    let mut list = users::load(&rig.users_file).unwrap();
    list[0].role = Role::Viewer;
    std::thread::sleep(std::time::Duration::from_millis(20));
    users::save(&rig.users_file, &list).unwrap();
    assert_eq!(probe(&rig.app, "operator", Some(&op)).await, StatusCode::FORBIDDEN);
}
