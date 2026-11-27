//! Converts a Movicon.NExT project (`.UFProject` + its folder) into the
//! Moviclone model.
//!
//! The importer is deliberately faithful: a reference that is broken in the
//! source stays broken in the output, so `movi build` reports it instead of the
//! importer silently dropping it. Everything it changes, assumes or leaves out
//! is recorded in the [`Report`]. Plain-text database passwords are never
//! written; they become `secret://` references.

use anyhow::{bail, Context, Result};
use moviclone_model::{Origin, Project, ProjectMeta, Sampling};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

mod report;
mod screens;
mod server;
mod util;

pub use report::{Finding, Level, Report, SecretNeeded};
use util::{child_text, guid_of, parse_xml, read_dataset, read_text, rows};

pub struct Imported {
    pub project: Project,
    pub report: Report,
}

/// Finds the `.UFProject` file: either the path itself, or the only one in a folder.
pub fn find_project_file(path: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    let found: Vec<PathBuf> = std::fs::read_dir(path)
        .with_context(|| format!("reading {}", path.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("UFProject")))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no .UFProject file in {}", path.display()),
        _ => bail!("several .UFProject files in {}; pass one explicitly", path.display()),
    }
}

pub fn import(path: &Path) -> Result<Imported> {
    let file = find_project_file(path)?;
    let name = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let dir = file.with_file_name(&name);
    if !dir.is_dir() {
        bail!("expected the project folder {} next to {}", dir.display(), file.display());
    }
    let mut rep = Report::default();

    let text = read_text(&file)?;
    let doc = parse_xml(&text, &file)?;
    let root = doc.root_element();
    let num = |k: &str, d: u32| child_text(root, k).and_then(|v| v.parse().ok()).unwrap_or(d);
    let mut meta = ProjectMeta {
        name: name.clone(),
        site: None,
        description: child_text(root, "description").map(str::to_string),
        culture: child_text(root, "cultureName").unwrap_or("en-us").to_string(),
        main_screen: None,
        sampling: Sampling {
            fast_ms: num("fastSamplingInterval", 500),
            slow_ms: num("slowSamplingInterval", 5000),
            publish_ms: num("publishingInterval", 250),
        },
        reports: Vec::new(),
        origin: Some(Origin {
            tool: "Movicon.NExT".into(),
            version: child_text(root, "version").unwrap_or("?").to_string(),
            project_id: child_text(root, "projectId").unwrap_or("").to_string(),
        }),
    };
    if let Some(t) = child_text(root, "projectType") {
        rep.add(Level::Info, file_name(&file), format!("Movicon project type: {t}"));
    }

    let (server, index) = server::import(&dir, &mut rep)?;

    let report_dir = dir.join("Report");
    if let Ok(entries) = std::fs::read_dir(&report_dir) {
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "report"))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .collect();
        names.sort();
        if !names.is_empty() {
            rep.add(Level::NotImported, "Report/", format!(
                "{} report layouts ({}) are DevExpress XtraReports; only their names were kept", names.len(), names.join(", ")));
        }
        meta.reports = names;
    }

    let (screens, screen_ids) = screens::import(&dir.join("Screen"), &index, &mut rep)?;
    if let Some(main) = child_text(root, "mainScreen").and_then(guid_of) {
        match screen_ids.get(&main) {
            Some(n) => meta.main_screen = Some(n.clone()),
            None => rep.add(Level::Problem, file_name(&file), format!("main screen {main} is not one of the project's screens")),
        }
    }

    summarize_other_modules(&dir, &mut rep);

    let project = Project { meta, tags: server.tags, drivers: server.drivers, data: server.data, alarms: Vec::new(), screens };
    let elements: usize = project.screens.iter().map(|s| s.elements.len()).sum();
    rep.summary = vec![
        ("Source".into(), format!("{} (Movicon.NExT {})", file.display(),
            project.meta.origin.as_ref().map(|o| o.version.as_str()).unwrap_or("?"))),
        ("Tags".into(), project.tags.len().to_string()),
        ("Drivers".into(), project.drivers.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")),
        ("Databases / historians / loggers".into(), format!("{} / {} / {}",
            project.data.databases.len(), project.data.historians.len(), project.data.loggers.len())),
        ("Screens".into(), format!("{} ({elements} elements)", project.screens.len())),
        ("Findings".into(), format!("{} secrets removed, {} problems, {} assumptions, {} items not imported",
            rep.count(Level::Security), rep.count(Level::Problem), rep.count(Level::Warning), rep.count(Level::NotImported))),
    ];
    Ok(Imported { project, report: rep })
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Modules phase 1 does not convert: count what is there so nothing is lost silently.
fn summarize_other_modules(dir: &Path, rep: &mut Report) {
    let modules: [(&str, &[(&str, &str)]); 5] = [
        ("Events/Events.UFEvent", &[("UFEventObject", "events")]),
        ("SchedulerServer/SchedulerServer.MSServer", &[("MSScheduledAction", "scheduled actions")]),
        ("AlarmDispatcher/AlarmDispatcher.ADServer", &[("ADNotification", "alarm notifications"), ("ADPlugin", "dispatcher plug-ins")]),
        ("UFUser/User.UFUser", &[("UFUser", "users"), ("UFRole", "roles")]),
        ("Recipe/RecipeSettings.UAServer", &[("RecipeUAConfiguration", "recipe configurations")]),
    ];
    for (path, tables) in modules {
        let full = dir.join(path);
        if !full.exists() {
            continue;
        }
        match read_dataset(&full) {
            Ok(t) => {
                let parts: Vec<String> = tables
                    .iter()
                    .map(|(table, what)| (rows(&t, table).len(), what))
                    .filter(|(n, _)| *n > 0)
                    .map(|(n, what)| format!("{n} {what}"))
                    .collect();
                if !parts.is_empty() {
                    let extra = if path.starts_with("UFUser") { " (password hashes are never imported)" } else { "" };
                    rep.add(Level::NotImported, path, format!("{}{extra}", parts.join(", ")));
                }
            }
            Err(e) => rep.add(Level::Warning, path, format!("could not read: {e:#}")),
        }
    }
    if let Ok(entries) = std::fs::read_dir(dir.join("Logic")) {
        for p in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if let Ok(text) = read_text(&p) {
                let empty = text.contains("&lt;LogicCircuit /&gt;");
                if !empty {
                    rep.add(Level::NotImported, format!("Logic/{}", file_name(&p)), "logic circuit (phase 4: logic engine)");
                }
            }
        }
    }
    if dir.join("Symbols").is_dir() {
        rep.add(Level::NotImported, "Symbols/", "project symbol library is encrypted by Movicon; symbols are imported as unlinked placeholders");
    }
}

/// Resolves Movicon tag references (NodeId GUID and/or name) to tag names.
#[derive(Default)]
pub(crate) struct TagIndex {
    by_guid: HashMap<String, String>,
    names: HashSet<String>,
    removed: HashMap<String, String>,
    /// historian name → tag names.
    historians: HashMap<String, Vec<String>>,
}

impl TagIndex {
    fn tags_of_historian(&self, h: &str) -> Vec<String> {
        self.historians.get(h).cloned().unwrap_or_default()
    }

    fn load_removed(&mut self, path: &Path) -> Result<()> {
        let text = read_text(path)?;
        let doc = parse_xml(&text, path)?;
        for e in doc.descendants().filter(|n| n.has_tag_name("entry")) {
            if let (Some(name), Some(g)) = (child_text(e, "TagName"), child_text(e, "NodeId").and_then(guid_of)) {
                self.removed.insert(g, name.to_string());
            }
        }
        Ok(())
    }

    /// The NodeId wins when it matches a tag. Otherwise the name is used, and a
    /// stale NodeId is reported. A name that matches nothing is kept, so the
    /// build reports the dangling reference.
    fn resolve(&self, guid: Option<&str>, name: Option<&str>, source: &str, context: &str, rep: &mut Report) -> Option<String> {
        if let Some(found) = guid.and_then(|g| self.by_guid.get(g)) {
            if let Some(n) = name.filter(|n| *n != found) {
                rep.add(Level::Problem, source, format!(
                    "{context}: labelled {n:?} but its NodeId belongs to {found:?}; using {found:?}"));
            }
            return Some(found.clone());
        }
        let removed = guid
            .and_then(|g| self.removed.get(g))
            .map(|r| format!(" (it belonged to a deleted tag {r:?})"))
            .unwrap_or_default();
        match (name, guid) {
            (Some(n), Some(g)) if self.names.contains(n) => {
                rep.add(Level::Problem, source, format!(
                    "{context}: stale NodeId {g}{removed}; re-linked to tag {n:?} by name"));
                Some(n.to_string())
            }
            (Some(n), _) if self.names.contains(n) => Some(n.to_string()),
            (Some(n), _) => {
                rep.add(Level::Problem, source, format!(
                    "{context}: tag {n:?} does not exist{removed}; kept so `movi build` reports it"));
                Some(n.to_string())
            }
            (None, Some(g)) => {
                rep.add(Level::Problem, source, format!("{context}: NodeId {g} matches no tag{removed}; binding dropped"));
                None
            }
            (None, None) => None,
        }
    }
}
