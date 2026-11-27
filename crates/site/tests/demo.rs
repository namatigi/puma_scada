//! Depot projects built from the real template (demo/oxoafrica + demo/site-template.toml).

use moviclone_check::{check, Severity};
use moviclone_model::{load_project, DriverConfig, Project};
use moviclone_site::{build, build_to, Choice, Manifest};
use std::path::{Path, PathBuf};

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/site-template.toml")
}

fn template() -> (Project, Manifest) {
    let manifest = Manifest::load(&manifest_path()).expect("manifest");
    let (p, _) = load_project(&manifest.template_dir(&manifest_path())).expect("template");
    (p, manifest)
}

fn choose(meters: &[&str]) -> Choice {
    Choice { depot: "Kurasini Depot".into(), meters: meters.iter().map(|m| m.to_string()).collect() }
}

/// Written to disk, loaded back and checked: no errors.
fn written_and_checked(choice: &Choice) -> Project {
    let dir = tempfile::tempdir().unwrap();
    build_to(&manifest_path(), choice, dir.path()).expect("build");
    let (p, src) = load_project(dir.path()).expect("the built project loads");
    let errors: Vec<_> = check(&p, &src).into_iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errors.is_empty(), "{:?}: {errors:#?}", choice.meters);
    p
}

fn names<T>(items: &[T], f: impl Fn(&T) -> &str) -> Vec<String> {
    let mut v: Vec<String> = items.iter().map(|x| f(x).to_string()).collect();
    v.sort();
    v
}

fn stations(p: &Project) -> Vec<String> {
    p.drivers
        .iter()
        .flat_map(|d| match &d.config {
            DriverConfig::ModbusTcp(c) => c.stations.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
        })
        .collect()
}

#[test]
fn two_meters_gasoline_and_gasoil() {
    let p = written_and_checked(&choose(&["gasoline", "gasoil"]));
    assert_eq!(p.meta.site.as_deref(), Some("Kurasini Depot"));
    assert_eq!(p.meta.main_screen.as_deref(), Some("Manifold"));
    assert_eq!(names(&p.screens, |s| &s.name), ["FM02_Trend", "FM04_Trend", "FM_02", "FM_04", "Manifold"]);
    assert!(p.tags.iter().all(|t| !t.name.starts_with("FM_01_") && !t.name.starts_with("FM_03_")));
    assert!(p.tags.iter().any(|t| t.name == "FM_02_MassFlow") && p.tags.iter().any(|t| t.name == "FM_04_MassFlow"));
    assert_eq!(p.alarms.len(), 12, "six alarms per meter");
    assert!(p.alarms.iter().all(|a| a.tag.starts_with("FM_02_") || a.tag.starts_with("FM_04_")));
    assert_eq!(names(&p.data.historians, |h| &h.name), ["FM02", "FM04"]);
    assert_eq!(names(&p.data.loggers, |l| &l.name), ["FM02", "FM04"]);
    assert_eq!(stations(&p), ["Flow_Meter_2", "Flow_Meter_4"]);

    let manifold = p.screen("Manifold").unwrap();
    let text = serde_json::to_string(manifold).unwrap();
    for gone in ["FM_01_", "FM_03_", "\"FM_01\"", "\"FM_03\"", "FM01_Trend", "FM03_Trend"] {
        assert!(!text.contains(gone), "Manifold still refers to {gone}");
    }
    for id in ["ContentControl24", "ContentControl17", "Pipeline10", "Pipeline5", "Pipeline2"] {
        assert!(manifold.elements.iter().all(|e| e.id != id), "{id} (HFO or JET line) is still drawn");
    }
    for id in ["ContentControl33", "ContentControl21", "Pipeline3", "Pipeline4", "ContentControl8", "ContentControl28"] {
        assert!(manifold.elements.iter().any(|e| e.id == id), "{id} (Gasoline or Gasoil line) is missing");
    }

    // The two remaining "Trend & Log" buttons move down into the lowest places.
    let (t, _) = template();
    let y = |p: &Project, id: &str| p.screen("Manifold").unwrap().elements.iter().find(|e| e.id == id).map(|e| e.y);
    assert_eq!(y(&p, "ContentControl4"), y(&t, "ContentControl5"), "Gasoil Trend & Log");
    assert_eq!(y(&p, "ContentControl6"), y(&t, "ContentControl6"), "Gasoline Trend & Log");
}

#[test]
fn three_meters_drop_only_the_fourth() {
    let p = written_and_checked(&choose(&["gasoline", "gasoil", "jet"]));
    assert_eq!(names(&p.screens, |s| &s.name).len(), 7);
    assert!(p.tags.iter().all(|t| !t.name.starts_with("FM_01_")));
    assert_eq!(p.alarms.len(), 18);
    assert_eq!(stations(&p), ["Flow_Meter_2", "Flow_Meter_3", "Flow_Meter_4"]);
    let manifold = p.screen("Manifold").unwrap();
    assert!(manifold.elements.iter().any(|e| e.id == "Pipeline10"), "the JET A-1 bypass stays");
    assert!(manifold.elements.iter().all(|e| e.id != "ContentControl24"), "the HFO meter goes");
}

#[test]
fn all_four_meters_keep_the_whole_template() {
    let (t, _) = template();
    let p = written_and_checked(&choose(&["hfo", "gasoil", "jet", "gasoline"]));
    assert_eq!(p.tags.len(), t.tags.len());
    assert_eq!(p.alarms.len(), t.alarms.len());
    assert_eq!(p.screens.len(), t.screens.len());
    assert_eq!(p.screen("Manifold").unwrap().elements, t.screen("Manifold").unwrap().elements, "nothing moved");
}

#[test]
fn every_single_meter_builds_cleanly() {
    let (t, manifest) = template();
    for m in &manifest.meters {
        let p = written_and_checked(&choose(&[&m.key]));
        assert!(p.tags.iter().filter(|x| x.name.starts_with("FM_")).all(|x| x.name.starts_with(&m.tag_prefix)), "{}", m.key);
        let left = p.screen("Manifold").unwrap().elements.len();
        assert!(left < t.screen("Manifold").unwrap().elements.len(), "{}", m.key);
    }
}

#[test]
fn bad_choices_are_refused() {
    let (t, manifest) = template();
    let err = |c: Choice| build(&t, &manifest, &c).unwrap_err().to_string();
    assert!(err(Choice { depot: " ".into(), meters: vec!["jet".into()] }).contains("depot name"));
    assert!(err(Choice { depot: "D".into(), meters: vec![] }).contains("at least one meter"));
    assert!(err(choose(&["diesel"])).contains("unknown meter"));
}
