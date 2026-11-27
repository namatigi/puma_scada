mod common;

use common::{alarm, project, settle, Running, TempDir};
use moviclone_alarms::{AlarmEngine, Clock, EventKind, StateKind};
use moviclone_model::AlarmCondition;
use moviclone_tagdb::{Quality, Value};
use std::time::Duration;

fn state(engine: &AlarmEngine, name: &str) -> StateKind {
    engine.state(name).expect("alarm exists").state
}

#[tokio::test(start_paused = true)]
async fn activation_delay_waits_for_condition_to_hold() {
    let mut a = alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 });
    a.delay_s = 5.0;
    let p = project(vec![a]);
    let dir = TempDir::new("delay");
    let rt = Running::start(&p, &dir).await;

    rt.set("Flow", Value::Float(120.0));
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Normal);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::ActiveUnacked);
    assert_eq!(rt.engine.state("HighFlow").map(|s| s.value), Some(Value::Float(120.0)));

    // A blip shorter than the delay never activates.
    rt.set("Flow", Value::Float(0.0));
    settle().await;
    rt.engine.ack("HighFlow", None).expect("ack");
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Normal);
    rt.set("Flow", Value::Float(150.0));
    tokio::time::sleep(Duration::from_secs(2)).await;
    rt.set("Flow", Value::Float(10.0));
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Normal);
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn deadband_ack_and_journal() {
    let mut a = alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 });
    a.deadband = 5.0;
    let p = project(vec![a, alarm("PumpOff", "Pump", AlarmCondition::IsFalse)]);
    let dir = TempDir::new("deadband");
    let rt = Running::start(&p, &dir).await;
    // Pump starts false (memory tag default) -> active immediately.
    assert_eq!(state(&rt.engine, "PumpOff"), StateKind::ActiveUnacked);

    rt.set("Flow", Value::Float(101.0));
    settle().await;
    rt.set("Flow", Value::Float(97.0));
    settle().await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::ActiveUnacked, "inside deadband");
    rt.set("Flow", Value::Float(94.0));
    settle().await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::ClearedUnacked);

    assert_eq!(rt.engine.ack_all(Some("op1")), 2);
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Normal);
    assert_eq!(state(&rt.engine, "PumpOff"), StateKind::ActiveAcked);
    assert_eq!(rt.engine.state("PumpOff").and_then(|s| s.acked_by).as_deref(), Some("op1"));

    let journal = rt.engine.journal(10);
    let kinds: Vec<_> = journal.iter().map(|e| (e.name.as_str(), e.event)).collect();
    assert_eq!(kinds.len(), 5);
    assert_eq!(kinds[4], ("PumpOff", EventKind::Activated), "oldest last");
    assert!(kinds.contains(&("HighFlow", EventKind::Cleared)));
    assert!(journal.iter().filter(|e| e.event == EventKind::Acked).all(|e| e.user.as_deref() == Some("op1")));
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn shelve_expires_automatically() {
    let p = project(vec![alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 })]);
    let dir = TempDir::new("shelve");
    let rt = Running::start(&p, &dir).await;
    rt.set("Flow", Value::Float(200.0));
    settle().await;
    rt.engine.shelve("HighFlow", Duration::from_secs(30 * 60), Some("op")).expect("shelve");
    let st = rt.engine.state("HighFlow").expect("state");
    assert_eq!(st.state, StateKind::Shelved);
    assert!(st.shelved_until.is_some());
    assert!(rt.engine.active().iter().any(|s| s.name == "HighFlow"));

    tokio::time::sleep(Duration::from_secs(29 * 60)).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Shelved);
    tokio::time::sleep(Duration::from_secs(61)).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::ActiveUnacked, "still above limit after the shelve");
    let last = rt.engine.journal(1);
    assert_eq!(last[0].event, EventKind::Unshelved);
    assert_eq!(last[0].user, None, "automatic unshelve has no user");

    assert!(rt.engine.shelve("HighFlow", Duration::from_secs(0), None).is_err());
    assert!(rt.engine.shelve("Nope", Duration::from_secs(60), None).is_err());
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn bad_quality_alarm_and_value_alarm_hold_state() {
    let p = project(vec![
        alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 }),
        alarm("FlowComms", "Flow", AlarmCondition::BadQuality),
    ]);
    let dir = TempDir::new("quality");
    let rt = Running::start(&p, &dir).await;
    rt.set("Flow", Value::Float(200.0));
    settle().await;
    let id = rt.store.id("Flow").expect("tag");
    rt.store.set_quality(id, Quality::Bad);
    settle().await;
    assert_eq!(state(&rt.engine, "FlowComms"), StateKind::ActiveUnacked);
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::ActiveUnacked, "bad quality does not clear a value alarm");
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn ws_feed_carries_full_message() {
    let p = project(vec![alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 })]);
    let dir = TempDir::new("feed");
    let rt = Running::start(&p, &dir).await;
    let mut feed = rt.engine.ws_feed();
    {
        let msg = feed.borrow_and_update();
        assert_eq!(msg["type"], "alarms");
        assert_eq!(msg["alarms"][0]["state"], "normal");
    }
    rt.set("Flow", Value::Float(130.5));
    tokio::time::timeout(Duration::from_secs(1), feed.changed()).await.expect("feed changed").expect("sender alive");
    let msg = feed.borrow().clone();
    let a = &msg["alarms"][0];
    assert_eq!(a["name"], "HighFlow");
    assert_eq!(a["tag"], "Flow");
    assert_eq!(a["state"], "active-unacked");
    assert_eq!(a["severity"], "high");
    assert_eq!(a["area"], "Manifold");
    assert_eq!(a["value"], 130.5);
    assert!(a["since"].is_i64());
    assert!(a["acked_by"].is_null() && a["shelved_until"].is_null());
    // A value change without a state change does not publish.
    rt.set("Flow", Value::Float(140.0));
    settle().await;
    assert!(!feed.has_changed().expect("sender alive"));
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn recovers_from_broadcast_lag() {
    let p = project(vec![alarm("HighLevel", "Level", AlarmCondition::Above { limit: 50.0 })]);
    let dir = TempDir::new("lag");
    let rt = Running::start(&p, &dir).await;
    let id = rt.store.id("Level").expect("tag");
    // Overflow the 8192-slot bus without yielding; the engine must re-read.
    for i in 0..10_000 {
        rt.store.update(id, Value::Float(f64::from(i % 40)), Quality::Good);
    }
    rt.store.update(id, Value::Float(99.0), Quality::Good);
    settle().await;
    settle().await;
    assert_eq!(state(&rt.engine, "HighLevel"), StateKind::ActiveUnacked);
    rt.stop().await;
}

#[tokio::test(start_paused = true)]
async fn state_and_journal_survive_restart() {
    let p = project(vec![
        alarm("HighFlow", "Flow", AlarmCondition::Above { limit: 100.0 }),
        alarm("HighLevel", "Level", AlarmCondition::Above { limit: 50.0 }),
        alarm("PumpOn", "Pump", AlarmCondition::IsTrue),
    ]);
    let dir = TempDir::new("restart");
    // One wall clock across both runs, driven by tokio's paused time.
    const BASE_MS: i64 = 1_795_740_000_000;
    let t0 = tokio::time::Instant::now();
    let clock = || Clock::tokio_at(BASE_MS + t0.elapsed().as_millis() as i64);
    {
        let rt = Running::start_with_clock(&p, &dir, clock()).await;
        rt.set("Flow", Value::Float(150.0));
        rt.set("Level", Value::Float(60.0));
        rt.set("Pump", Value::Bool(true));
        settle().await;
        rt.set("Level", Value::Float(10.0));
        settle().await;
        rt.engine.ack("PumpOn", Some("op")).expect("ack");
        rt.engine.shelve("HighFlow", Duration::from_secs(600), None).expect("shelve");
        rt.stop().await;
    }
    tokio::time::sleep(Duration::from_secs(60)).await;

    // New store: device values not read yet are bad/null, memory tags are back at zero.
    let rt = Running::start_with_clock(&p, &dir, clock()).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Shelved);
    assert_eq!(state(&rt.engine, "HighLevel"), StateKind::ClearedUnacked);
    // Pump restarted as `false` (memory tag), so this alarm cleared; ack is kept.
    assert_eq!(state(&rt.engine, "PumpOn"), StateKind::Normal);
    let journal = rt.engine.journal(100);
    assert!(journal.len() >= 6, "journal restored: {journal:?}");
    // Remaining shelve time continues after restart (600 s - 60 s elapsed).
    tokio::time::sleep(Duration::from_secs(530)).await;
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Shelved);
    tokio::time::sleep(Duration::from_secs(20)).await;
    // Flow restarted at 0.0: shelve ends with the alarm cleared.
    assert_eq!(state(&rt.engine, "HighFlow"), StateKind::Normal);
    rt.stop().await;
}
