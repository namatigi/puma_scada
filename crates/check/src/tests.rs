use super::*;
use moviclone_model::*;

fn tag(name: &str, ty: DataType, station: &str, area: ModbusArea, register: u16) -> Tag {
    Tag {
        name: name.into(),
        description: None,
        data_type: ty,
        access: Access::Read,
        address: Some(Address::Modbus(ModbusAddress {
            driver: "meters".into(),
            station: station.into(),
            area,
            register,
            write: ModbusWrite::Multiple,
        })),
        historian: None,
        legacy_id: None,
    }
}

fn base() -> Project {
    Project {
        meta: ProjectMeta { main_screen: Some("Main".into()), ..Default::default() },
        tags: vec![
            tag("Flow", DataType::F32, "M1", ModbusArea::HoldingRegister, 247),
            tag("Density", DataType::F32, "M1", ModbusArea::HoldingRegister, 249),
            tag("Total", DataType::F32, "M1", ModbusArea::HoldingRegister, 259),
        ],
        drivers: vec![Driver {
            name: "meters".into(),
            config: DriverConfig::ModbusTcp(ModbusTcp {
                description: None,
                host: "10.0.0.5".into(),
                port: 502,
                poll_ms: 500,
                timeout_ms: 5000,
                keep_open: true,
                stations: vec![ModbusStation {
                    name: "M1".into(),
                    unit_id: 1,
                    addressing: Addressing::OneBased,
                    max_retries: 3,
                }],
            }),
        }],
        data: DataConfig::default(),
        alarms: vec![],
        screens: vec![Screen {
            name: "Main".into(),
            width: 800.0,
            height: 600.0,
            background: None,
            legacy_id: None,
            elements: vec![Element {
                id: "d1".into(),
                kind: ElementKind::Display { tag: Some("Flow".into()), unit: None, format: None, editable: false },
                x: 10.0,
                y: 10.0,
                w: 100.0,
                h: 30.0,
                rotation: 0.0,
                on_click: vec![],
            }],
        }],
    }
}

fn codes(p: &Project) -> Vec<&'static str> {
    check(p, &SourceMap::default()).into_iter().map(|d| d.code).collect()
}

#[test]
fn clean_project_has_no_errors_or_warnings() {
    let d = check(&base(), &SourceMap::default());
    assert!(d.iter().all(|d| d.severity == Severity::Note), "{d:#?}");
}

#[test]
fn missing_station_is_an_error() {
    let mut p = base();
    if let Some(Address::Modbus(a)) = &mut p.tags[0].address {
        a.station = "Flow_Meters".into();
    }
    assert!(codes(&p).contains(&"E011"));
}

#[test]
fn dangling_references_are_errors() {
    let mut p = base();
    p.screens[0].elements[0].on_click = vec![
        Command::OpenScreen { screen: "Nowhere".into() },
        Command::Report { report: "Report1".into(), tags: vec!["FM_01_Press".into()], output_dir: None },
    ];
    p.meta.main_screen = Some("Gone".into());
    let c = codes(&p);
    for code in ["E030", "E040", "E043", "E044"] {
        assert!(c.contains(&code), "missing {code} in {c:?}");
    }
}

#[test]
fn bool_in_register_area_is_rejected() {
    let mut p = base();
    p.tags.push(tag("Reset", DataType::Bool, "M1", ModbusArea::HoldingRegister, 3));
    assert!(codes(&p).contains(&"E012"));
}

#[test]
fn overlapping_floats_warn() {
    let mut p = base();
    p.tags.push(tag("Oops", DataType::F32, "M1", ModbusArea::HoldingRegister, 248));
    assert!(codes(&p).contains(&"W015"));
}

#[test]
fn planner_merges_adjacent_and_splits_far_registers() {
    let jobs = plan_polls(&base(), 0);
    assert_eq!(jobs.len(), 2, "{jobs:#?}");
    assert_eq!((jobs[0].start, jobs[0].count), (247, 4));
    assert_eq!(jobs[0].tags, ["Flow", "Density"]);
    assert_eq!((jobs[1].start, jobs[1].count), (259, 2));

    let jobs = plan_polls(&base(), 8);
    assert_eq!(jobs.len(), 1);
    assert_eq!((jobs[0].start, jobs[0].count), (247, 14));
}

#[test]
fn planner_respects_protocol_limit() {
    let mut p = base();
    p.tags = (0..100)
        .map(|i| tag(&format!("T{i}"), DataType::F32, "M1", ModbusArea::HoldingRegister, 1 + i * 2))
        .collect();
    let jobs = plan_polls(&p, 0);
    assert!(jobs.iter().all(|j| j.count <= 125));
    assert_eq!(jobs.iter().map(|j| j.tags.len()).sum::<usize>(), 100);
}

#[test]
fn plaintext_password_does_not_parse() {
    let ok: Result<Database, _> = toml::from_str(
        "name='m'\nprovider='mssql'\nserver='s'\ndatabase='d'\npassword='secret://main-sql'",
    );
    assert!(ok.is_ok());
    let bad: Result<Database, _> =
        toml::from_str("name='m'\nprovider='mssql'\nserver='s'\ndatabase='d'\npassword='plain-text-example'");
    assert!(bad.unwrap_err().to_string().contains("secret reference"));
}

#[test]
fn project_round_trips_through_files() {
    let dir = std::env::temp_dir().join(format!("moviclone-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = base();
    p.screens[0].elements.push(Element {
        id: "pipe".into(),
        kind: ElementKind::Pipeline {
            points: vec![[3.0, 0.0], [3.0, 120.5]],
            thickness: 26.4,
            fill: Some("#EFEFF0".into()),
            stroke: None,
            tag: None,
        },
        x: 1017.0,
        y: 160.0,
        w: 30.0,
        h: 121.0,
        rotation: 90.0,
        on_click: vec![Command::WriteTag { tag: "Flow".into(), value: "1".into(), mode: WriteMode::Pulse }],
    });
    save_project(&dir, &p).unwrap();
    let (back, _) = load_project(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(back, p);
}

fn alarm(name: &str, tag: &str, condition: AlarmCondition) -> Alarm {
    Alarm {
        name: name.into(),
        tag: tag.into(),
        condition,
        severity: AlarmSeverity::High,
        message: "x".into(),
        area: None,
        deadband: 0.0,
        delay_s: 0.0,
        ack_required: true,
        enabled: true,
    }
}

#[test]
fn alarm_on_missing_tag_or_wrong_type_is_an_error() {
    let mut p = base();
    p.alarms = vec![
        alarm("A1", "Nope", AlarmCondition::Above { limit: 1.0 }),
        alarm("A2", "Flow", AlarmCondition::IsTrue),
    ];
    let c = codes(&p);
    assert!(c.contains(&"E061") && c.contains(&"E062"), "{c:?}");
}

#[test]
fn bundle_json_round_trips() {
    let mut p = base();
    p.alarms = vec![alarm("A1", "Flow", AlarmCondition::Above { limit: 1.0 })];
    let json = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
}

#[test]
fn alarms_round_trip_through_files() {
    let dir = std::env::temp_dir().join(format!("moviclone-alarms-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut p = base();
    p.alarms = vec![alarm("A1", "Flow", AlarmCondition::Above { limit: 120.0 })];
    save_project(&dir, &p).unwrap();
    let (back, _) = load_project(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(back, p);
}
