mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{flush, project, Running, TempDir, BASE_MS};
use http_body_util::BodyExt;
use moviclone_historian::routes;
use serde_json::Value as Json;
use std::time::Duration;
use tokio::time::Instant;
use tower::ServiceExt;

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Json) {
    let req = Request::builder().uri(uri).body(Body::empty()).expect("request");
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}

#[tokio::test(start_paused = true)]
async fn protocol_routes() {
    let p = project();
    let dir = TempDir::new("routes");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    for i in 0..50 {
        rt.set("Flow", f64::from(i));
        rt.set("Temp", f64::from(i) / 2.0);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    flush().await;
    let app = routes(rt.hist.clone());
    let to = rt.hist.now_ms();

    let (s, body) = get(&app, &format!("/api/history?tags=Flow,Temp&from={BASE_MS}&to={to}&points=10")).await;
    assert_eq!(s, StatusCode::OK);
    let series = body["series"].as_array().expect("series");
    assert_eq!(series.len(), 2);
    assert_eq!(series[0]["tag"], "Flow");
    let pts = series[0]["points"].as_array().expect("points");
    assert!(!pts.is_empty() && pts.len() <= 10);
    assert!(pts.iter().any(|p| p[1] == 49.0), "max kept");

    // Defaults: last hour up to now.
    let (s, body) = get(&app, "/api/history?tags=Flow").await;
    assert_eq!(s, StatusCode::OK);
    assert!(body["series"][0]["points"].as_array().map(|a| a.len()).unwrap_or(0) > 10);

    let (s, body) = get(&app, "/api/history").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());
    let (s, _) = get(&app, "/api/history?tags=Flow&from=10&to=5").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, body) = get(&app, &format!("/api/history/table?source=H1&from={BASE_MS}&to={to}&limit=5")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["columns"], serde_json::json!(["time", "tag", "value", "quality"]));
    let rows = body["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 5);
    assert!(rows[0][0].as_i64() >= rows[4][0].as_i64(), "newest first");
    assert_eq!(rows[0][3], "good");

    let (s, body) = get(&app, "/api/history/table?source=L1&kind=logger&limit=3").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["columns"][0], "time");
    assert_eq!(body["rows"].as_array().map(Vec::len), Some(3));

    let (s, _) = get(&app, "/api/history/table?source=Nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = get(&app, "/api/history/table").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = get(&app, "/api/history/table?source=H1&kind=bogus").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    rt.stop().await;
}
