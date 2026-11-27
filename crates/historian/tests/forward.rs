mod common;

use common::{flush, project, Running, TempDir};
use moviclone_historian::{ForwardData, ForwardOptions, Forwarder, MemoryForwarder};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

fn opts() -> ForwardOptions {
    ForwardOptions { batch: 50, min_backoff: Duration::from_secs(1), max_backoff: Duration::from_secs(8), send_timeout: Duration::from_secs(5) }
}

fn seqs(f: &MemoryForwarder) -> Vec<u64> {
    f.received().iter().map(|e| e.seq).collect()
}

#[tokio::test(start_paused = true)]
async fn forwards_in_order_and_catches_up_after_outage() {
    let p = project();
    let dir = TempDir::new("fwd");
    let rt = Running::start(&p, &dir, Instant::now()).await;
    let sql = MemoryForwarder::new("sql-main", "sql");
    let task = tokio::spawn(rt.hist.clone().run_forwarder(sql.clone() as Arc<dyn Forwarder>, opts(), rt.shutdown_rx.clone()));

    rt.set("Flow", 1.0);
    rt.set("Level", 9.0); // H2 is on database "other": not forwarded here
    flush().await;
    flush().await;
    let got = sql.received();
    assert!(got.iter().all(|e| match &e.data {
        ForwardData::Sample { historian, .. } => historian == "H1",
        ForwardData::Row { logger, .. } => logger == "L1" || logger == "L2",
    }));
    assert!(got.iter().any(|e| matches!(&e.data, ForwardData::Sample { tag, value, .. } if tag == "Flow" && value.as_f64() == Some(1.0))));
    assert!(got.iter().any(|e| matches!(&e.data, ForwardData::Row { table, columns, .. } if table == "L1_table" && columns.len() == 2)));
    let before = got.len();

    // Outage: records pile up locally, then flow once the target is back.
    sql.set_failing(true);
    for i in 0..120 {
        rt.set("Flow", f64::from(i) + 2.0);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    flush().await;
    assert_eq!(sql.received().len(), before, "nothing delivered during the outage");
    assert!(rt.hist.forward_backlog("sql-main").expect("backlog") >= 240);
    sql.set_failing(false);
    tokio::time::sleep(Duration::from_secs(20)).await;
    flush().await;
    let s = seqs(&sql);
    assert!(s.windows(2).all(|w| w[0] < w[1]), "in order, no duplicates");
    let flows = sql.received().iter().filter(|e| matches!(&e.data, ForwardData::Sample { tag, .. } if tag == "Flow")).count();
    assert!(flows >= 122, "caught up: {flows} Flow samples");
    assert_eq!(rt.hist.forward_backlog("sql-main").expect("backlog"), 0);
    rt.stop().await;
    task.await.expect("forwarder task");
}

#[tokio::test(start_paused = true)]
async fn watermark_survives_restart() {
    let p = project();
    let dir = TempDir::new("fwd-restart");
    let t0 = Instant::now();
    let first_last;
    {
        let rt = Running::start(&p, &dir, t0).await;
        let sql = MemoryForwarder::new("sql-main", "sql");
        let task = tokio::spawn(rt.hist.clone().run_forwarder(sql.clone() as Arc<dyn Forwarder>, opts(), rt.shutdown_rx.clone()));
        rt.set("Flow", 5.0);
        flush().await;
        flush().await;
        first_last = *seqs(&sql).last().expect("received something");
        rt.stop().await;
        task.await.expect("forwarder task");
    }
    let rt = Running::start(&p, &dir, t0).await;
    let sql = MemoryForwarder::new("sql-main", "sql");
    let task = tokio::spawn(rt.hist.clone().run_forwarder(sql.clone() as Arc<dyn Forwarder>, opts(), rt.shutdown_rx.clone()));
    rt.set("Flow", 6.0);
    flush().await;
    flush().await;
    let s = seqs(&sql);
    assert!(!s.is_empty());
    assert!(s.iter().all(|&q| q > first_last), "only records after the saved watermark: {s:?} vs {first_last}");
    rt.stop().await;
    task.await.expect("forwarder task");
}
