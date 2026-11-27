//! Imports the synthetic `fixtures/Mini` project, which contains one example of
//! each problem seen in real projects: a plain-text password, a station that
//! does not exist, a stale NodeId, a dangling tag name, bindings and commands on
//! elements that are gone.

use moviclone_check::{check, plan_polls, Severity};
use moviclone_import_movicon::{import, Level};
use moviclone_model::*;
use std::path::{Path, PathBuf};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Mini")
}

fn messages(r: &moviclone_import_movicon::Report, level: Level) -> Vec<String> {
    r.findings.iter().filter(|f| f.level == level).map(|f| f.message.clone()).collect()
}

#[test]
fn imports_project_settings() {
    let p = import(&fixture()).unwrap().project;
    assert_eq!(p.meta.name, "Mini");
    assert_eq!(p.meta.main_screen.as_deref(), Some("Main"));
    assert_eq!(p.meta.sampling.fast_ms, 400);
    assert_eq!(p.meta.reports, ["Daily"]);
}

#[test]
fn imports_tags_with_typed_addresses() {
    let p = import(&fixture()).unwrap().project;
    let flow = p.tag("Flow").unwrap();
    assert_eq!(flow.data_type, DataType::F32);
    assert_eq!(flow.access, Access::Read);
    assert_eq!(flow.historian.as_deref(), Some("H1"));
    assert_eq!(
        flow.address,
        Some(Address::Modbus(ModbusAddress {
            driver: "field-bus".into(),
            station: "Meter_1".into(),
            area: ModbusArea::HoldingRegister,
            register: 247,
            write: ModbusWrite::Single,
        }))
    );
    let reset = p.tag("Reset").unwrap();
    assert_eq!(reset.data_type, DataType::Bool);
    assert_eq!(reset.access, Access::ReadWrite);
    assert!(matches!(&reset.address, Some(Address::Modbus(a)) if a.area == ModbusArea::Coil && a.register == 3));
}

#[test]
fn imports_driver_channel_and_stations() {
    let p = import(&fixture()).unwrap().project;
    let d = p.driver("field-bus").unwrap();
    let DriverConfig::ModbusTcp(m) = &d.config;
    assert_eq!((m.host.as_str(), m.port, m.timeout_ms), ("10.0.0.5", 502, 3000));
    assert_eq!(m.stations.len(), 1);
    assert_eq!((m.stations[0].name.as_str(), m.stations[0].unit_id), ("Meter_1", 7));
    assert_eq!(m.stations[0].addressing, Addressing::OneBased);
}

#[test]
fn passwords_become_secret_references() {
    let imported = import(&fixture()).unwrap();
    let p = &imported.project;
    assert_eq!(p.data.databases.len(), 1, "historian and logger share one database");
    let db = &p.data.databases[0];
    assert_eq!(db.password.as_ref().map(|s| s.name()), Some("mini-io"));
    assert_eq!(imported.report.secrets.len(), 1);

    let dir = std::env::temp_dir().join(format!("moviclone-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    save_project(&dir, p).unwrap();
    for entry in walk(&dir) {
        let text = std::fs::read_to_string(&entry).unwrap();
        assert!(!text.contains("Hunter2"), "password leaked into {}", entry.display());
    }
    assert!(!imported.report.to_markdown("Mini").contains("Hunter2"));
    assert!(!imported.report.secrets_example().contains("Hunter2"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn imports_screen_elements() {
    let p = import(&fixture()).unwrap().project;
    let s = p.screen("Main").unwrap();
    assert_eq!((s.width, s.height), (320.0, 680.0), "settings size wins over the XAML canvas");
    assert_eq!(s.background.as_deref(), Some("#3F3F3F"));

    let el = |id: &str| s.elements.iter().find(|e| e.id == id).unwrap();
    assert_eq!(
        el("EditDisplay").kind,
        ElementKind::Display { tag: Some("Flow".into()), unit: Some("kg/h".into()), format: Some("#0.00".into()), editable: false }
    );
    assert_eq!(el("TextBox").kind, ElementKind::Label { text: "Mass flow".into() });
    assert_eq!(el("EditDisplay").rotation, 0.0);
    assert_eq!(el("EditDisplay2").rotation, 315.0, "control rotation comes from the SVG export when the XAML has none");
    assert_eq!(el("Valve1").kind, ElementKind::Symbol { symbol: None, tag: None });

    let button = el("ButtonControl");
    assert_eq!(button.kind, ElementKind::Button { text: "Reset & Report".into() });
    assert_eq!(button.rotation, 90.0, "450 degrees normalizes to 90");
    assert_eq!(button.on_click[0], Command::WriteTag { tag: "Reset".into(), value: "1".into(), mode: WriteMode::Pulse });
    assert_eq!(
        button.on_click[1],
        Command::Report { report: "Daily".into(), tags: vec!["Flow".into(), "Press".into()], output_dir: Some("C:\\Reports".into()) }
    );
}

#[test]
fn reports_source_problems() {
    let r = import(&fixture()).unwrap().report;
    let problems = messages(&r, Level::Problem).join("\n");
    for needle in [
        "station \"Meters\", which no channel defines",
        "stale NodeId cccccccc-0000-0000-0000-000000000009",
        "tag \"Press\" does not exist",
        "element \"Gone\", which is not on the screen",
        "binding is stored for element \"Ghost\"",
    ] {
        assert!(problems.contains(needle), "missing {needle:?} in:\n{problems}");
    }
    assert_eq!(r.count(Level::Security), 1);
}

#[test]
fn build_reports_the_dangling_references() {
    let dir = std::env::temp_dir().join(format!("moviclone-build-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    save_project(&dir, &import(&fixture()).unwrap().project).unwrap();
    let (p, src) = load_project(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    let diags = check(&p, &src);
    let errors: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).map(|d| d.code).collect();
    assert!(errors.contains(&"E011"), "missing station: {diags:#?}");
    assert!(errors.contains(&"E044"), "missing report tag: {diags:#?}");
    assert_eq!(errors.len(), 2, "{diags:#?}");

    let jobs = plan_polls(&p, 0);
    assert_eq!(jobs.iter().map(|j| j.tags.len()).sum::<usize>(), 3);
}

#[test]
fn import_is_deterministic_and_round_trips() {
    let a = import(&fixture()).unwrap().project;
    let b = import(&fixture()).unwrap().project;
    assert_eq!(a, b);

    let dir = std::env::temp_dir().join(format!("moviclone-det-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    save_project(&dir, &a).unwrap();
    let (back, _) = load_project(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(back, a);
}

/// Optional: import the real project when `MOVICON_PROJECT` points at it.
#[test]
fn real_project_if_available() {
    let Ok(path) = std::env::var("MOVICON_PROJECT") else { return };
    let imported = import(Path::new(&path)).unwrap();
    assert!(!imported.project.tags.is_empty());
    assert!(!imported.project.screens.is_empty());
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
    }
    out
}
