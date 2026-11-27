mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{alarm, project, settle, Running, TempDir};
use http_body_util::BodyExt;
use moviclone_alarms::routes;
use moviclone_model::AlarmCondition;
use moviclone_tagdb::Value;
use serde_json::{json, Value as Json};
use tower::ServiceExt;

async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<Json>) -> (StatusCode, Json) {
    let req = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json body"))
}

#[tokio::test(start_paused = true)]
async fn protocol_routes() {
    let p = project(vec![
        alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 }),
        alarm("HighLevel", "Level", AlarmCondition::Above { limit: 50.0 }),
    ]);
    let dir = TempDir::new("routes");
    let rt = Running::start(&p, &dir).await;
    let app = routes(rt.engine.clone());
    rt.set("Flow", Value::Float(120.0));
    rt.set("Level", Value::Float(60.0));
    settle().await;

    let (s, body) = call(&app, "GET", "/api/alarms", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["alarms"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["alarms"][0]["state"], "active-unacked");

    let (s, body) = call(&app, "POST", "/api/alarms/HighFlow/ack", None).await;
    assert_eq!((s, body), (StatusCode::OK, json!({"ok": true})));
    assert_eq!(rt.engine.state("HighFlow").map(|s| s.state), Some(moviclone_alarms::StateKind::ActiveAcked));

    let (s, body) = call(&app, "POST", "/api/alarms/Missing/ack", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], false);

    let (s, body) = call(&app, "POST", "/api/alarms/ack-all", Some(json!({"user": "op2"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["acked"], 1);

    let (s, _) = call(&app, "POST", "/api/alarms/HighLevel/shelve", Some(json!({"minutes": 30}))).await;
    assert_eq!(s, StatusCode::OK);
    let st = rt.engine.state("HighLevel").expect("state");
    assert_eq!(st.state, moviclone_alarms::StateKind::Shelved);

    let (s, body) = call(&app, "POST", "/api/alarms/HighLevel/shelve", Some(json!({"minutes": -1}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    let (s, _) = call(&app, "POST", "/api/alarms/HighLevel/shelve", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, _) = call(&app, "POST", "/api/alarms/HighLevel/unshelve", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(rt.engine.state("HighLevel").map(|s| s.state), Some(moviclone_alarms::StateKind::ActiveUnacked));

    let (s, body) = call(&app, "GET", "/api/alarms/journal?limit=3", None).await;
    assert_eq!(s, StatusCode::OK);
    let events = body["events"].as_array().expect("events");
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["event"], "unshelved");
    assert_eq!(events[1]["event"], "shelved");
    assert_eq!(events[2]["event"], "acked");
    assert_eq!(events[2]["user"], "op2");
    assert!(events[0]["ts"].is_i64());
    rt.stop().await;
}
