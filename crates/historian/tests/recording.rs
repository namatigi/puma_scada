mod common;

use common::{flush, project, settle, Running, TempDir, BASE_MS};
use moviclone_historian::{SourceKind, Table};
use moviclone_tagdb::{Quality, Value};
use std::time::Duration;
use tokio::time::Instant;

#[tokio::test(start_paused = true)]
async fn records_startup_sample_and_changes() {
    let p = project();
    let dir = TempDir::new("changes");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    rt.set("Flow", 10.0);
    tokio::time::sleep(Duration::from_millis(100)).await;
    rt.set("Flow", 12.5);
    rt.set("Note", 1.0); // not historized: ignored
    flush().await;
    // Startup sample (memory tag starts at 0.0), then both changes.
    assert_eq!(rt.values("Flow"), vec![Some(0.0), Some(10.0), Some(12.5)]);
    let s = rt.hist.query(&["Note".into(), "Missing".into()], 0, i64::MAX, 600).expect("query");
    assert!(s.iter().all(|s| s.points.is_empty()));
    let flow = &rt.hist.query(&["Flow".into()], 0, i64::MAX, 600).expect("query")[0];
    assert!(flow.points[0].0 >= BASE_MS, "timestamps follow the historian clock");
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn heartbeat_gives_flat_signals_points() {
    let p = project();
    let dir = TempDir::new("heartbeat");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    rt.set("Temp", 21.0);
    tokio::time::sleep(Duration::from_secs(10 * 60)).await;
    flush().await;
    let pts = rt.hist.query(&["Temp".into()], 0, i64::MAX, 10_000).expect("query").remove(0).points;
    // start-up + change + ~one heartbeat per minute (checked every 15 s).
    assert!((9..=13).contains(&pts.len()), "{} points", pts.len());
    assert!(pts.iter().skip(1).all(|p| p.1 == Some(21.0)));
    let gaps: Vec<_> = pts.windows(2).map(|w| w[1].0 - w[0].0).collect();
    assert!(gaps.iter().skip(1).all(|g| *g <= 75_000), "no gap above 1.25 heartbeats: {gaps:?}");
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn downsampling_keeps_spikes() {
    let p = project();
    let dir = TempDir::new("downsample");
    let t0 = Instant::now();
    let rt = Running::start(&p, &dir, t0).await;
    for i in 0..3_000 {
        let v = if i == 1_777 { 999.0 } else if i == 2_222 { -999.0 } else { f64::from(i % 50) };
        rt.set("Flow", v);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    flush().await;
    let all = rt.values("Flow");
    assert_eq!(all.len(), 3_000, "start-up sample + 2999 changes (the first equals it)");
    let series = rt.hist.query(&["Flow".into()], BASE_MS, rt.hist.now_ms(), 200).expect("query").remove(0);
    assert!(series.points.len() <= 200, "{} points", series.points.len());
    let vals: Vec<_> = series.points.iter().filter_map(|p| p.1).collect();
    assert!(vals.contains(&999.0) && vals.contains(&-999.0), "extremes preserved");
    assert!(series.points.windows(2).all(|w| w[0].0 <= w[1].0));
    // JSON shape: [[ts, v], …]
    let json = serde_json::to_value(&series).expect("json");
    assert!(json["points"][0][0].is_i64() && json["points"][0][1].is_number());
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn recovers_from_broadcast_lag() {
    let p = project();
    let dir = TempDir::new("lag");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    let id = rt.store.id("Temp").expect("tag");
    // More than the bus holds, without yielding to the recorder.
    for i in 0..10_000 {
        rt.store.update(id, Value::Float(f64::from(i)), Quality::Good);
    }
    rt.store.update(id, Value::Float(-1.0), Quality::Good);
    flush().await;
    let vals = rt.values("Temp");
    assert_eq!(vals.last(), Some(&Some(-1.0)), "latest value recorded after lag: {:?}", &vals[vals.len().saturating_sub(3)..]);
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn bad_quality_is_a_gap() {
    let p = project();
    let dir = TempDir::new("quality");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    rt.set("Flow", 5.0);
    let id = rt.store.id("Flow").expect("tag");
    rt.store.set_quality(id, Quality::Bad);
    flush().await;
    assert_eq!(rt.values("Flow"), vec![Some(0.0), Some(5.0), None]);
    let table = rt.hist.table("H1", None, 0, i64::MAX, 10).expect("table");
    assert_eq!(table.rows[0][3], "bad");
    assert_eq!(table.rows[0][2], 5.0, "the table still shows the last value");
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn retention_prunes_per_historian() {
    let p = project();
    let dir = TempDir::new("retention");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    rt.set("Level", 1.0); // H2 keeps 1 h
    rt.set("Flow", 1.0); // H1 keeps 24 h
    flush().await;
    let start = BASE_MS;
    tokio::time::sleep(Duration::from_secs(2 * 3600)).await;
    flush().await;
    let level = rt.hist.query(&["Level".into()], 0, i64::MAX, 100_000).expect("query").remove(0).points;
    let flow = rt.hist.query(&["Flow".into()], 0, i64::MAX, 100_000).expect("query").remove(0).points;
    let cutoff = rt.hist.now_ms() - 3_600_000 - 600_000; // one prune interval of slack
    assert!(level.iter().all(|p| p.0 >= cutoff), "old Level points pruned");
    assert!(flow.iter().any(|p| p.0 < start + 1_000), "Flow keeps its first points");
    assert!(rt.hist.prune().expect("prune") <= 1);
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn history_survives_restart() {
    let p = project();
    let dir = TempDir::new("restart");
    let t0 = Instant::now();
    {
        let rt = Running::start(&p, &dir, t0).await;
        rt.set("Flow", 42.0);
        settle().await;
        rt.stop().await; // final flush on shutdown
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    let rt = Running::start(&p, &dir, t0).await;
    flush().await;
    let vals = rt.values("Flow");
    assert_eq!(vals[..2], [Some(0.0), Some(42.0)], "first run's samples are kept: {vals:?}");
    assert_eq!(vals.last(), Some(&Some(0.0)), "second start-up sample (memory tag restarted at 0)");
    rt.stop().await;
}

fn col(t: &Table, name: &str) -> usize {
    t.columns.iter().position(|c| c == name).expect("column")
}

#[tokio::test(start_paused = true)]
async fn data_logger_rows_and_tables() {
    let p = project();
    let dir = TempDir::new("logger");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    rt.set("Temp", 20.0);
    tokio::time::sleep(Duration::from_millis(10)).await;
    rt.set("Flow", 7.0);
    rt.set("Level", 3.0);
    tokio::time::sleep(Duration::from_secs(35)).await;
    flush().await;

    // On-change logger: one row per change of a column tag, every column filled.
    let l1 = rt.hist.table("L1", None, 0, i64::MAX, 100).expect("L1");
    assert_eq!(l1.columns, vec!["time", "col_Flow", "col_Temp"]);
    assert_eq!(l1.rows.len(), 2);
    assert_eq!(l1.rows[0][col(&l1, "col_Flow")], 7.0, "newest first");
    assert_eq!(l1.rows[0][col(&l1, "col_Temp")], 20.0);
    assert_eq!(l1.rows[1][col(&l1, "col_Flow")], 0.0);
    // Every-10 s logger: rows at 10, 20, 30 s.
    let l2 = rt.hist.table("L2", Some(SourceKind::Logger), 0, i64::MAX, 100).expect("L2");
    assert_eq!(l2.rows.len(), 3);
    assert!(l2.rows.iter().all(|r| r[1] == 3.0));
    let limited = rt.hist.table("L2", None, 0, i64::MAX, 2).expect("L2");
    assert_eq!(limited.rows.len(), 2);

    // Historian table merges its tags newest first.
    let h1 = rt.hist.table("H1", None, 0, i64::MAX, 3).expect("H1");
    assert_eq!(h1.columns, vec!["time", "tag", "value", "quality"]);
    assert_eq!(h1.rows.len(), 3);
    assert_eq!(h1.rows[0][1], "Flow");
    assert_eq!(h1.rows[0][2], 7.0);
    assert_eq!(h1.rows[1][1], "Temp");
    assert!(h1.rows.windows(2).all(|w| w[0][0].as_i64() >= w[1][0].as_i64()));
    assert!(rt.hist.table("Nope", None, 0, i64::MAX, 10).is_err());
    rt.stop().await;
}
