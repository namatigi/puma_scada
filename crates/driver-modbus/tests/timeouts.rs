//! A device that accepts connections but never answers.

use moviclone_model::{
    Access, Address, Addressing, DataType, Driver, DriverConfig, ModbusAddress, ModbusArea, ModbusStation, ModbusTcp,
    ModbusWrite, Project, Tag,
};
use moviclone_tagdb::{Quality, TagStore, Value};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::watch;

#[tokio::test]
async fn silent_device_goes_uncertain_then_bad() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            // Swallow requests, never reply.
            tokio::spawn(async move {
                let mut buf = [0u8; 256];
                while matches!(sock.read(&mut buf).await, Ok(n) if n > 0) {}
            });
        }
    });

    let project = Project {
        drivers: vec![Driver {
            name: "d".into(),
            config: DriverConfig::ModbusTcp(ModbusTcp {
                description: None,
                host: "127.0.0.1".into(),
                port,
                poll_ms: 20,
                timeout_ms: 100,
                keep_open: true,
                stations: vec![ModbusStation {
                    name: "S".into(),
                    unit_id: 1,
                    addressing: Addressing::OneBased,
                    max_retries: 3,
                }],
            }),
        }],
        tags: vec![Tag {
            name: "T".into(),
            description: None,
            data_type: DataType::U16,
            access: Access::ReadWrite,
            address: Some(Address::Modbus(ModbusAddress {
                driver: "d".into(),
                station: "S".into(),
                area: ModbusArea::HoldingRegister,
                register: 1,
                write: ModbusWrite::Single,
            })),
            historian: None,
            legacy_id: None,
        }],
        ..Project::default()
    };
    let store = TagStore::new(&project);
    let id = store.id("T").unwrap();
    // Start from a known good value so the uncertain phase is visible.
    store.update(id, Value::Int(5), Quality::Good);
    let (_tx, rx) = watch::channel(false);
    let (stats, _task) = moviclone_driver_modbus::spawn(project.drivers[0].clone(), store.clone(), rx);

    let mut seen_uncertain = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let s = store.get(id);
        seen_uncertain |= s.quality == Quality::Uncertain;
        if s.quality == Quality::Bad {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "never went bad");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(seen_uncertain, "a single timeout should first mark the value uncertain");
    assert_eq!(store.get(id).value, Value::Int(5), "the last value is kept");
    let snap = stats.snapshot();
    assert!(snap.errors >= 3, "{snap:?}");
    assert!(snap.last_error.unwrap_or_default().contains("did not answer"));

    // A write while the device is silent fails with a timeout, not a hang.
    let started = tokio::time::Instant::now();
    let err = store.write(id, Value::Int(1)).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5), "{err}");
}
