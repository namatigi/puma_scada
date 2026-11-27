use super::*;
use moviclone_model::*;

fn project() -> Project {
    let tag = |name: &str, ty, access, addr: bool| Tag {
        name: name.into(),
        description: None,
        data_type: ty,
        access,
        address: addr.then(|| {
            Address::Modbus(ModbusAddress {
                driver: "bus".into(),
                station: "S1".into(),
                area: if ty == DataType::Bool { ModbusArea::Coil } else { ModbusArea::HoldingRegister },
                register: 1,
                write: ModbusWrite::Single,
            })
        }),
        historian: None,
        legacy_id: None,
    };
    Project {
        tags: vec![
            tag("Flow", DataType::F32, Access::Read, true),
            tag("Reset", DataType::Bool, Access::ReadWrite, true),
            tag("Setpoint", DataType::I16, Access::ReadWrite, false),
        ],
        ..Default::default()
    }
}

#[test]
fn initial_quality_depends_on_address() {
    let s = TagStore::new(&project());
    assert_eq!(s.get_by_name("Flow").unwrap().quality, Quality::Bad);
    assert_eq!(s.get_by_name("Setpoint").unwrap(), Sample { value: Value::Int(0), quality: Quality::Good, ts: s.get(2).ts });
}

#[test]
fn update_coerces_and_publishes_only_changes() {
    let s = TagStore::new(&project());
    let mut rx = s.subscribe();
    let flow = s.id("Flow").unwrap();
    assert!(s.update(flow, Value::Int(12), Quality::Good));
    assert!(!s.update(flow, Value::Float(12.0), Quality::Good), "same value is not a change");
    assert!(s.set_quality(flow, Quality::Bad));
    let first = rx.try_recv().unwrap();
    assert_eq!(first.sample.value, Value::Float(12.0));
    assert_eq!(rx.try_recv().unwrap().sample.quality, Quality::Bad);
    assert!(rx.try_recv().is_err());
}

#[test]
fn value_coercion() {
    assert_eq!(Value::Text("1".into()).coerce(DataType::Bool), Ok(Value::Bool(true)));
    assert_eq!(Value::Float(3.0).coerce(DataType::U16), Ok(Value::Int(3)));
    assert!(Value::Int(70000).coerce(DataType::U16).is_err());
    assert!(Value::Float(1.5).coerce(DataType::I32).is_err());
    assert_eq!(Value::Int(2).coerce(DataType::String), Ok(Value::Text("2".into())));
}

#[test]
fn value_json_is_plain() {
    let s = Sample { value: Value::Float(1.5), quality: Quality::Good, ts: 7 };
    assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"v":1.5,"q":"good","ts":7}"#);
    let v: Value = serde_json::from_str("true").unwrap();
    assert_eq!(v, Value::Bool(true));
    let v: Value = serde_json::from_str("null").unwrap();
    assert_eq!(v, Value::Null);
}

#[tokio::test]
async fn writes_route_to_driver_and_respect_access() {
    let s = TagStore::new(&project());
    let (tx, mut rx) = mpsc::channel(4);
    s.register_writer("bus", tx);
    let store = s.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            store.update(req.id, req.value.clone(), Quality::Good);
            let _ = req.reply.send(Ok(()));
        }
    });

    let reset = s.id("Reset").unwrap();
    s.write(reset, Value::Text("on".into())).await.unwrap();
    assert_eq!(s.get(reset).value, Value::Bool(true));

    let flow = s.id("Flow").unwrap();
    assert_eq!(s.write(flow, Value::Float(1.0)).await, Err(WriteError::ReadOnly("Flow".into())));

    let sp = s.id("Setpoint").unwrap();
    s.write_mode(sp, Value::Int(5), WriteMode::Increase).await.unwrap();
    s.write_mode(sp, Value::Int(2), WriteMode::Decrease).await.unwrap();
    assert_eq!(s.get(sp).value, Value::Int(3));

    s.write_mode(reset, Value::Bool(true), WriteMode::Toggle).await.unwrap();
    assert_eq!(s.get(reset).value, Value::Bool(false));
}

#[tokio::test(start_paused = true)]
async fn pulse_returns_to_the_resting_value() {
    let s = TagStore::new(&project());
    let sp = s.id("Setpoint").unwrap();
    s.write(sp, Value::Int(4)).await.unwrap();
    let mut rx = s.subscribe();
    s.write_mode(sp, Value::Int(1), WriteMode::Pulse).await.unwrap();
    assert_eq!(rx.try_recv().unwrap().sample.value, Value::Int(1));
    assert_eq!(rx.try_recv().unwrap().sample.value, Value::Int(0), "numbers rest at zero, not at the previous value");
}

#[tokio::test(start_paused = true)]
async fn pulse_clears_a_coil_that_was_left_on() {
    let s = TagStore::new(&project());
    let (tx, mut rx) = mpsc::channel(4);
    s.register_writer("bus", tx);
    let store = s.clone();
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            store.update(req.id, req.value.clone(), Quality::Good);
            let _ = req.reply.send(Ok(()));
        }
    });
    let reset = s.id("Reset").unwrap();
    s.write(reset, Value::Bool(true)).await.unwrap();
    s.write_mode(reset, Value::Int(1), WriteMode::Pulse).await.unwrap();
    assert_eq!(s.get(reset).value, Value::Bool(false));
}

#[tokio::test]
async fn write_without_driver_fails_cleanly() {
    let s = TagStore::new(&project());
    let reset = s.id("Reset").unwrap();
    assert_eq!(s.write(reset, Value::Bool(true)).await, Err(WriteError::NoDriver("bus".into())));
}
