//! Driver against simulator over real TCP on an ephemeral port.

use moviclone_driver_modbus as driver;
use moviclone_model::{
    Access, Address, Addressing, DataType, Driver, DriverConfig, ModbusAddress, ModbusArea, ModbusStation, ModbusTcp,
    ModbusWrite, Project, Tag, WriteMode,
};
use moviclone_tagdb::{Quality, TagStore, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

fn tag(name: &str, ty: DataType, station: &str, area: ModbusArea, register: u16, write: ModbusWrite) -> Tag {
    Tag {
        name: name.into(),
        description: None,
        data_type: ty,
        access: if area.is_writable() { Access::ReadWrite } else { Access::Read },
        address: Some(Address::Modbus(ModbusAddress { driver: "fm".into(), station: station.into(), area, register, write })),
        historian: None,
        legacy_id: None,
    }
}

fn project(port: u16) -> Project {
    use DataType::*;
    use ModbusArea::*;
    let s = ModbusWrite::Single;
    Project {
        drivers: vec![Driver {
            name: "fm".into(),
            config: DriverConfig::ModbusTcp(ModbusTcp {
                description: None,
                host: "127.0.0.1".into(),
                port,
                poll_ms: 50,
                timeout_ms: 500,
                keep_open: true,
                stations: vec![
                    ModbusStation { name: "Flow_Meter_1".into(), unit_id: 1, addressing: Addressing::OneBased, max_retries: 2 },
                    ModbusStation { name: "Aux".into(), unit_id: 7, addressing: Addressing::ZeroBased, max_retries: 2 },
                ],
            }),
        }],
        tags: vec![
            tag("FM_01_MassFlow", F32, "Flow_Meter_1", HoldingRegister, 247, s),
            tag("FM_01_Density", F32, "Flow_Meter_1", HoldingRegister, 249, s),
            tag("FM_01_Mass_Total", F32, "Flow_Meter_1", HoldingRegister, 259, s),
            tag("FM_01_External_Pressure", F32, "Flow_Meter_1", HoldingRegister, 451, s),
            tag("FM_01_START", Bool, "Flow_Meter_1", Coil, 2, ModbusWrite::Multiple),
            tag("FM_01_Reset_Total", Bool, "Flow_Meter_1", Coil, 3, s),
            tag("Aux_Level", U16, "Aux", HoldingRegister, 100, s),
            tag("Aux_Count", I32, "Aux", HoldingRegister, 300, s),
            tag("Aux_Big", F64, "Aux", InputRegister, 400, s),
            tag("Aux_Name", String, "Aux", HoldingRegister, 200, s),
            tag("Aux_Alarm", Bool, "Aux", DiscreteInput, 5, s),
        ],
        ..Project::default()
    }
}

struct Rig {
    project: Project,
    store: Arc<TagStore>,
    stats: driver::DriverStats,
    sim: Option<moviclone_sim::Simulator>,
    addr: SocketAddr,
    _shutdown: watch::Sender<bool>,
}

async fn rig() -> Rig {
    // The simulator only needs the station layout; the port is patched in after binding.
    let sim = moviclone_sim::start(&project(0), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = sim.local_addr();
    let project = project(addr.port());
    let store = TagStore::new(&project);
    let (shutdown, rx) = watch::channel(false);
    let (stats, _task) = driver::spawn(project.drivers[0].clone(), store.clone(), rx);
    Rig { project, store, stats, sim: Some(sim), addr, _shutdown: shutdown }
}

impl Rig {
    fn sim(&self) -> &moviclone_sim::Simulator {
        self.sim.as_ref().unwrap()
    }
    fn get(&self, name: &str) -> moviclone_tagdb::Sample {
        self.store.get_by_name(name).unwrap()
    }
    fn all(&self, q: Quality) -> bool {
        self.project.tags.iter().all(|t| self.get(&t.name).quality == q)
    }
}

async fn eventually(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn values_arrive_with_good_quality_and_exact_scaling() {
    let rig = rig().await;
    let sim = rig.sim();
    sim.write_tag("FM_01_MassFlow", &Value::Float(123.5)).unwrap();
    sim.write_tag("Aux_Level", &Value::Int(4321)).unwrap();
    sim.write_tag("Aux_Count", &Value::Int(-70_000)).unwrap();
    sim.write_tag("Aux_Name", &Value::Text("PETRO".into())).unwrap();

    eventually("all tags good", || rig.all(Quality::Good)).await;
    eventually("pinned values", || rig.get("FM_01_MassFlow").value == Value::Float(123.5)).await;
    assert_eq!(rig.get("Aux_Level").value, Value::Int(4321));
    assert_eq!(rig.get("Aux_Count").value, Value::Int(-70_000));
    assert_eq!(rig.get("Aux_Name").value, Value::Text("PETRO".into()));

    // Simulated values: HFO density on unit 1, decoded from a big-endian f32.
    let density = rig.get("FM_01_Density").value.as_f64().unwrap();
    assert!((950.0..1010.0).contains(&density), "density {density}");
    let sim_density = sim.read_tag("FM_01_Density").unwrap().as_f64().unwrap();
    assert!((density - sim_density).abs() < 2.0, "driver {density} vs sim {sim_density}");
    assert_eq!(rig.get("FM_01_START").value, Value::Bool(true));

    let s = rig.stats.snapshot();
    assert!(s.connected && s.requests > 0 && s.last_ok_ts.is_some(), "{s:?}");
    assert!(s.stations.iter().all(|st| st.ok), "{s:?}");
}

#[tokio::test]
async fn writes_reach_the_device_and_persist() {
    let rig = rig().await;
    eventually("connected", || rig.all(Quality::Good)).await;
    let id = |n: &str| rig.store.id(n).unwrap();

    // Coil via FC15 (write = multiple), read back by the simulator and the driver.
    rig.store.write(id("FM_01_START"), Value::Bool(false)).await.unwrap();
    assert_eq!(rig.sim().read_tag("FM_01_START"), Some(Value::Bool(false)));
    assert_eq!(rig.get("FM_01_START").value, Value::Bool(false));

    // A float needs FC16 even though the tag says write = "single".
    rig.store.write(id("FM_01_External_Pressure"), Value::Float(6.5)).await.unwrap();
    assert_eq!(rig.sim().read_tag("FM_01_External_Pressure"), Some(Value::Float(6.5)));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.get("FM_01_External_Pressure").value, Value::Float(6.5), "written value must persist");

    // 16-bit register via FC6.
    rig.store.write(id("Aux_Level"), Value::Int(17)).await.unwrap();
    assert_eq!(rig.sim().read_tag("Aux_Level"), Some(Value::Int(17)));

    // Pulse on the reset coil zeroes the totals and the coil returns to false.
    rig.store.write_mode(id("FM_01_Reset_Total"), Value::Bool(true), WriteMode::Pulse).await.unwrap();
    assert_eq!(rig.sim().read_tag("FM_01_Reset_Total"), Some(Value::Bool(false)));
    // START is off, so the totals stay at zero after the reset.
    assert_eq!(rig.sim().read_tag("FM_01_Mass_Total"), Some(Value::Float(0.0)));
    eventually("total reset seen by the driver", || rig.get("FM_01_Mass_Total").value == Value::Float(0.0)).await;

    // Input registers cannot be written.
    let err = rig.store.write(id("Aux_Big"), Value::Float(1.0)).await.unwrap_err();
    assert!(matches!(err, moviclone_tagdb::WriteError::ReadOnly(_)), "{err}");
}

#[tokio::test]
async fn losing_the_device_turns_quality_bad_and_reconnect_restores_it() {
    let mut rig = rig().await;
    eventually("good before the outage", || rig.all(Quality::Good)).await;

    rig.sim.take().unwrap().stop().await;
    eventually("all bad after the device disappears", || rig.all(Quality::Bad)).await;
    assert!(!rig.stats.snapshot().connected);
    let err = rig.store.write(rig.store.id("FM_01_START").unwrap(), Value::Bool(true)).await.unwrap_err();
    assert!(err.to_string().contains("not connected"), "{err}");

    // Same port again (retry briefly in case the OS still holds it).
    let mut restarted = None;
    for _ in 0..50 {
        match moviclone_sim::start(&rig.project, rig.addr).await {
            Ok(sim) => {
                restarted = Some(sim);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    rig.sim = Some(restarted.expect("simulator restarts on the same port"));
    eventually("good again after reconnect", || rig.all(Quality::Good)).await;
    let s = rig.stats.snapshot();
    assert!(s.connected && s.connects >= 2 && s.errors >= 1, "{s:?}");
}

#[tokio::test]
async fn silent_station_degrades_alone() {
    // Station 9 does not exist in the simulator: it answers with an exception
    // (gateway target failed), the other station stays good.
    let sim = moviclone_sim::start(&project(0), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let mut p = project(sim.local_addr().port());
    let DriverConfig::ModbusTcp(cfg) = &mut p.drivers[0].config;
    cfg.stations[1].unit_id = 9;
    let store = TagStore::new(&p);
    let (_tx, rx) = watch::channel(false);
    let (stats, _task) = driver::spawn(p.drivers[0].clone(), store.clone(), rx);
    eventually("station 1 good", || store.get_by_name("FM_01_Density").unwrap().quality == Quality::Good).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(store.get_by_name("Aux_Level").unwrap().quality, Quality::Bad);
    assert_eq!(store.get_by_name("FM_01_Density").unwrap().quality, Quality::Good);
    let s = stats.snapshot();
    assert!(s.connected);
    assert!(s.stations.iter().any(|st| st.name == "Aux" && !st.ok), "{s:?}");
    sim.stop().await;
}

#[tokio::test]
async fn petrogas_sample_project_is_fully_simulated() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/oxoafrica");
    if !dir.exists() {
        eprintln!("skipping: {} not found", dir.display());
        return;
    }
    let (mut p, _) = moviclone_model::load_project(&dir).unwrap();
    let sim = moviclone_sim::start(&p, "127.0.0.1:0".parse().unwrap()).await.unwrap();
    for d in &mut p.drivers {
        let DriverConfig::ModbusTcp(cfg) = &mut d.config;
        cfg.host = "127.0.0.1".into();
        cfg.port = sim.local_addr().port();
    }
    let store = TagStore::new(&p);
    let (_tx, rx) = watch::channel(false);
    for d in &p.drivers {
        driver::spawn(d.clone(), store.clone(), rx.clone());
    }
    eventually("every Petrogas tag good", || store.ids().all(|id| store.get(id).quality == Quality::Good)).await;
    let rho = |n: &str| store.get_by_name(n).unwrap().value.as_f64().unwrap();
    // FM_01 HFO, FM_02 gasoil, FM_03 JET A-1, FM_04 gasoline (as the screens name them).
    assert!((rho("FM_01_Density") - 980.0).abs() < 30.0);
    assert!((rho("FM_02_Density") - 840.0).abs() < 30.0);
    assert!((rho("FM_03_Density") - 800.0).abs() < 30.0);
    assert!((rho("FM_04_Density") - 720.0).abs() < 30.0);
    let flow = rho("FM_01_MassFlow");
    let vr = rho("FM_01_Volume_Rate");
    assert!((0.0..=150.0).contains(&flow));
    assert!((vr - flow * 1000.0 / rho("FM_01_Density")).abs() < 5.0, "volume rate {vr} for flow {flow}");
    sim.stop().await;
}

#[tokio::test]
async fn writes_work_with_keep_open_false() {
    let sim = moviclone_sim::start(&project(0), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    let mut p = project(sim.local_addr().port());
    let DriverConfig::ModbusTcp(cfg) = &mut p.drivers[0].config;
    cfg.keep_open = false;
    let store = TagStore::new(&p);
    let (_tx, rx) = watch::channel(false);
    let (_stats, _task) = driver::spawn(p.drivers[0].clone(), store.clone(), rx);
    eventually("polled", || store.ids().all(|id| store.get(id).quality == Quality::Good)).await;

    let start = store.id("FM_01_START").unwrap();
    for value in [false, true, false] {
        store.write(start, Value::Bool(value)).await.unwrap_or_else(|e| panic!("write {value}: {e}"));
        assert_eq!(sim.read_tag("FM_01_START"), Some(Value::Bool(value)));
    }
    store.write(store.id("Aux_Level").unwrap(), Value::Int(99)).await.unwrap();
    eventually("written value read back by polling", || store.get_by_name("Aux_Level").unwrap().value == Value::Int(99)).await;
    assert_eq!(store.get(start).value, Value::Bool(false));
    sim.stop().await;
}

/// Forwards Modbus TCP frames to `target`, dropping every request for `silent_unit`.
async fn proxy_with_silent_unit(target: SocketAddr, silent_unit: u8) -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let Ok(device) = tokio::net::TcpStream::connect(target).await else { return };
            let (mut client_rd, mut client_wr) = client.into_split();
            let (mut device_rd, mut device_wr) = device.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut device_rd, &mut client_wr).await;
            });
            tokio::spawn(async move {
                let mut head = [0u8; 7];
                while client_rd.read_exact(&mut head).await.is_ok() {
                    let len = usize::from(u16::from_be_bytes([head[4], head[5]])).saturating_sub(1);
                    let mut pdu = vec![0u8; len];
                    if client_rd.read_exact(&mut pdu).await.is_err() {
                        return;
                    }
                    if head[6] == silent_unit {
                        continue;
                    }
                    if device_wr.write_all(&head).await.is_err() || device_wr.write_all(&pdu).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test]
async fn a_silent_station_does_not_stall_the_others() {
    let sim = moviclone_sim::start(&project(0), "127.0.0.1:0".parse().unwrap()).await.unwrap();
    // "Aux" (unit 7) sorts before "Flow_Meter_1", so it is polled first in every cycle.
    let proxy = proxy_with_silent_unit(sim.local_addr(), 7).await;
    let mut p = project(proxy.port());
    let DriverConfig::ModbusTcp(cfg) = &mut p.drivers[0].config;
    cfg.timeout_ms = 200;
    // Many retries: before the fix, no other station was read until Aux was given up on.
    cfg.stations[1].max_retries = 10;
    let store = TagStore::new(&p);
    let (_tx, rx) = watch::channel(false);
    let started = tokio::time::Instant::now();
    let (stats, _task) = driver::spawn(p.drivers[0].clone(), store.clone(), rx);

    eventually("Flow_Meter_1 polled despite the silent station", || {
        store.get_by_name("FM_01_Density").unwrap().quality == Quality::Good
    })
    .await;
    assert!(started.elapsed() < Duration::from_millis(1500), "Flow_Meter_1 waited {:?}", started.elapsed());
    assert_eq!(store.get_by_name("Aux_Level").unwrap().quality, Quality::Uncertain, "Aux is still being retried");
    eventually("silent station goes bad", || store.get_by_name("Aux_Level").unwrap().quality == Quality::Bad).await;
    // Flow_Meter_1 keeps being read while Aux is retried.
    sim.write_tag("FM_01_MassFlow", &Value::Float(77.25)).unwrap();
    eventually("fresh values from Flow_Meter_1", || store.get_by_name("FM_01_MassFlow").unwrap().value == Value::Float(77.25)).await;
    assert_eq!(store.get_by_name("FM_01_MassFlow").unwrap().quality, Quality::Good);
    // Writes to the healthy station still go through.
    store.write(store.id("FM_01_START").unwrap(), Value::Bool(false)).await.unwrap();
    assert_eq!(sim.read_tag("FM_01_START"), Some(Value::Bool(false)));
    let s = stats.snapshot();
    assert!(s.stations.iter().any(|st| st.name == "Aux" && !st.ok), "{s:?}");
    assert!(s.stations.iter().any(|st| st.name == "Flow_Meter_1" && st.ok), "{s:?}");
    sim.stop().await;
}
