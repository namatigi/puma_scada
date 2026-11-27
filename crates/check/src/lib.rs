//! Cross-file validation and Modbus poll planning.
//!
//! [`check`] resolves every reference in a loaded project (tags, stations,
//! drivers, databases, historians, screens, reports) and reports what does not
//! resolve, with the file it was found in. A project with errors must not be
//! deployed; warnings and notes are advisory.

use moviclone_model::{
    Access, Address, Command, DataType, DriverConfig, ElementKind, ModbusArea, Project, SourceMap,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

mod plan;
pub use plan::{plan_polls, PollJob};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable identifier, e.g. `E011`.
    pub code: &'static str,
    pub file: PathBuf,
    /// Path to the item inside the file, e.g. `tag "FM_01_Temp"`.
    pub item: String,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}] {}: {}: {}",
            self.severity,
            self.code,
            self.file.display(),
            self.item,
            self.message
        )
    }
}

struct Ctx<'a> {
    p: &'a Project,
    src: &'a SourceMap,
    out: Vec<Diagnostic>,
}

impl Ctx<'_> {
    fn push(&mut self, severity: Severity, code: &'static str, file: PathBuf, item: String, message: String) {
        self.out.push(Diagnostic { severity, code, file, item, message });
    }
}

/// Validates a project. Diagnostics come back sorted: errors first, then by file.
pub fn check(p: &Project, src: &SourceMap) -> Vec<Diagnostic> {
    let mut cx = Ctx { p, src, out: Vec::new() };
    check_tags(&mut cx);
    check_drivers(&mut cx);
    check_data(&mut cx);
    check_screens(&mut cx);
    check_alarms(&mut cx);
    check_usage(&mut cx);
    let mut out = cx.out;
    out.sort_by(|a, b| (a.severity, &a.file, &a.item).cmp(&(b.severity, &b.file, &b.item)));
    out
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn check_tags(cx: &mut Ctx) {
    let file = cx.src.tags();
    let mut seen = HashSet::new();
    let p = cx.p;
    for t in &p.tags {
        let item = format!("tag {:?}", t.name);
        if !is_identifier(&t.name) {
            cx.push(Severity::Error, "E001", file.clone(), item.clone(),
                "tag names must start with a letter or '_' and contain only letters, digits and '_'".into());
        }
        if !seen.insert(t.name.as_str()) {
            cx.push(Severity::Error, "E002", file.clone(), item.clone(), "tag name is defined more than once".into());
        }
        if let Some(h) = &t.historian {
            if !p.data.historians.iter().any(|x| &x.name == h) {
                cx.push(Severity::Error, "E020", file.clone(), item.clone(),
                    format!("historian {h:?} is not defined in data.toml"));
            }
        }
        let Some(Address::Modbus(a)) = &t.address else {
            if t.access != Access::Read {
                cx.push(Severity::Warning, "W003", file.clone(), item.clone(),
                    "tag has no device address, so writes stay in memory".into());
            }
            continue;
        };
        let Some(driver) = p.driver(&a.driver) else {
            cx.push(Severity::Error, "E010", file.clone(), item.clone(),
                format!("driver {:?} does not exist (expected drivers/{}.toml)", a.driver, a.driver));
            continue;
        };
        let DriverConfig::ModbusTcp(m) = &driver.config;
        if !m.stations.iter().any(|s| s.name == a.station) {
            let known: Vec<_> = m.stations.iter().map(|s| s.name.as_str()).collect();
            cx.push(Severity::Error, "E011", file.clone(), item.clone(),
                format!("station {:?} does not exist in driver {:?} (stations: {})", a.station, a.driver, known.join(", ")));
        }
        if a.area.is_bit() != (t.data_type == DataType::Bool) {
            cx.push(Severity::Error, "E012", file.clone(), item.clone(),
                format!("a {:?} tag cannot live in the {} area", t.data_type, a.area));
        }
        if t.access.writes() && !a.area.is_writable() {
            cx.push(Severity::Error, "E013", file.clone(), item.clone(),
                format!("the {} area is read-only, but the tag is {:?}", a.area, t.access));
        }
        if a.register == 0 && m.stations.iter().any(|s| s.name == a.station && s.addressing == moviclone_model::Addressing::OneBased) {
            cx.push(Severity::Error, "E014", file.clone(), item.clone(),
                "register 0 is invalid on a station with one-based addressing".into());
        }
    }
    check_overlaps(cx);
}

/// (driver, station, area) → (start, end, tag name) register spans.
type Spans<'a> = BTreeMap<(&'a str, &'a str, ModbusArea), Vec<(u32, u32, &'a str)>>;

/// Two tags reading the same registers usually means a copy-paste slip.
fn check_overlaps(cx: &mut Ctx) {
    let mut by_area: Spans = BTreeMap::new();
    for t in &cx.p.tags {
        if let Some(Address::Modbus(a)) = &t.address {
            let len = t.data_type.registers().unwrap_or(1) as u32;
            by_area
                .entry((a.driver.as_str(), a.station.as_str(), a.area))
                .or_default()
                .push((a.register as u32, a.register as u32 + len, t.name.as_str()));
        }
    }
    let file = cx.src.tags();
    let mut found = Vec::new();
    for ((_, station, area), mut spans) in by_area {
        spans.sort();
        for w in spans.windows(2) {
            let (s0, e0, n0) = w[0];
            let (s1, _, n1) = w[1];
            if s1 < e0 {
                found.push((n1.to_string(), format!(
                    "{area} {s1} on {station} overlaps {n0:?} ({area} {s0}..{})", e0 - 1)));
            }
        }
    }
    for (name, msg) in found {
        cx.push(Severity::Warning, "W015", file.clone(), format!("tag {name:?}"), msg);
    }
}

fn check_drivers(cx: &mut Ctx) {
    for d in &cx.p.drivers {
        let file = cx.src.driver(&d.name);
        let DriverConfig::ModbusTcp(m) = &d.config;
        let mut names = HashSet::new();
        let mut ids = HashMap::new();
        for s in &m.stations {
            let item = format!("station {:?}", s.name);
            if !names.insert(s.name.as_str()) {
                cx.push(Severity::Error, "E016", file.clone(), item.clone(), "station name is defined more than once".into());
            }
            if let Some(other) = ids.insert(s.unit_id, s.name.as_str()) {
                cx.push(Severity::Warning, "W017", file.clone(), item,
                    format!("unit id {} is also used by station {other:?} on the same connection", s.unit_id));
            }
        }
        if m.poll_ms < 50 {
            cx.push(Severity::Warning, "W018", file, "poll_ms".into(), format!("{} ms is faster than most devices answer", m.poll_ms));
        }
    }
}

fn check_data(cx: &mut Ctx) {
    let file = cx.src.data();
    let p = cx.p;
    let dbs: HashSet<_> = p.data.databases.iter().map(|d| d.name.as_str()).collect();
    for h in &p.data.historians {
        if !dbs.contains(h.database.as_str()) {
            cx.push(Severity::Error, "E022", file.clone(), format!("historian {:?}", h.name),
                format!("database {:?} is not defined", h.database));
        }
    }
    for l in &p.data.loggers {
        let item = format!("logger {:?}", l.name);
        if !dbs.contains(l.database.as_str()) {
            cx.push(Severity::Error, "E022", file.clone(), item.clone(), format!("database {:?} is not defined", l.database));
        }
        if l.columns.is_empty() {
            cx.push(Severity::Warning, "W023", file.clone(), item.clone(), "logger has no columns".into());
        }
        for c in &l.columns {
            if p.tag(&c.tag).is_none() {
                cx.push(Severity::Error, "E021", file.clone(), format!("{item} column {:?}", c.name),
                    format!("tag {:?} does not exist", c.tag));
            }
        }
    }
    for d in &p.data.databases {
        if d.user.is_some() && d.password.is_none() && !d.integrated_auth {
            cx.push(Severity::Warning, "W024", file.clone(), format!("database {:?}", d.name),
                "a user is set but no password secret".into());
        }
    }
}

fn check_screens(cx: &mut Ctx) {
    let p = cx.p;
    if let Some(main) = &p.meta.main_screen {
        if p.screen(main).is_none() {
            cx.push(Severity::Error, "E030", cx.src.project(), "main_screen".into(), format!("screen {main:?} does not exist"));
        }
    }
    let mut names = HashSet::new();
    for s in &p.screens {
        let file = cx.src.screen(&s.name);
        if !names.insert(s.name.as_str()) {
            cx.push(Severity::Error, "E031", file.clone(), format!("screen {:?}", s.name), "screen name is used twice".into());
        }
        let mut ids = HashSet::new();
        let mut unlinked = 0usize;
        let mut editable_read_only = Vec::new();
        for e in &s.elements {
            let item = format!("element {:?}", e.id);
            if !ids.insert(e.id.as_str()) {
                cx.push(Severity::Error, "E032", file.clone(), item.clone(), "element id is used twice on this screen".into());
            }
            if e.x + e.w < 0.0 || e.y + e.h < 0.0 || e.x > s.width || e.y > s.height {
                cx.push(Severity::Warning, "W033", file.clone(), item.clone(),
                    format!("element lies outside the {}x{} screen", s.width, s.height));
            }
            match &e.kind {
                ElementKind::Display { tag: Some(tag), editable, .. } => match p.tag(tag) {
                    None => cx.push(Severity::Error, "E034", file.clone(), item.clone(), format!("tag {tag:?} does not exist")),
                    Some(t) if *editable && !t.access.writes() => editable_read_only.push(e.id.as_str()),
                    _ => {}
                },
                ElementKind::Pipeline { tag: Some(tag), .. } if p.tag(tag).is_none() => {
                    cx.push(Severity::Error, "E034", file.clone(), item.clone(), format!("tag {tag:?} does not exist"));
                }
                ElementKind::Symbol { symbol, tag } => {
                    if !symbol.as_deref().is_some_and(|s| !s.starts_with("movicon:")) {
                        unlinked += 1;
                    }
                    if let Some(tag) = tag.as_ref().filter(|t| p.tag(t).is_none()) {
                        cx.push(Severity::Error, "E034", file.clone(), item.clone(), format!("tag {tag:?} does not exist"));
                    }
                }
                ElementKind::Trend { tags, historian, .. } => {
                    for tag in tags.iter().filter(|t| p.tag(t).is_none()) {
                        cx.push(Severity::Error, "E034", file.clone(), item.clone(), format!("trend tag {tag:?} does not exist"));
                    }
                    if let Some(h) = historian.as_ref().filter(|h| !p.data.historians.iter().any(|x| &x.name == *h)) {
                        cx.push(Severity::Error, "E037", file.clone(), item.clone(), format!("historian {h:?} is not defined in data.toml"));
                    }
                }
                ElementKind::HistoryTable { source } => {
                    let known = p.data.historians.iter().any(|x| &x.name == source) || p.data.loggers.iter().any(|x| &x.name == source);
                    if !known {
                        cx.push(Severity::Error, "E037", file.clone(), item.clone(), format!("no historian or logger is named {source:?}"));
                    }
                }
                _ => {}
            }
            for c in &e.on_click {
                check_command(cx, &file, &item, c);
            }
        }
        if !editable_read_only.is_empty() {
            cx.push(Severity::Warning, "W035", file.clone(), format!("screen {:?}", s.name), format!(
                "{} display(s) accept operator input but their tags are read-only, so typed values never reach the device: {}",
                editable_read_only.len(), editable_read_only.join(", ")));
        }
        if unlinked > 0 {
            cx.push(Severity::Note, "N036", file, format!("screen {:?}", s.name),
                format!("{unlinked} symbol(s) still use an imported Movicon symbol name or none; map them to Moviclone symbols"));
        }
    }
}

fn check_alarms(cx: &mut Ctx) {
    use moviclone_model::AlarmCondition as C;
    let p = cx.p;
    let file = cx.src.alarms();
    let mut names = HashSet::new();
    for a in &p.alarms {
        let item = format!("alarm {:?}", a.name);
        if !names.insert(a.name.as_str()) {
            cx.push(Severity::Error, "E060", file.clone(), item.clone(), "alarm name is defined more than once".into());
        }
        let Some(t) = p.tag(&a.tag) else {
            cx.push(Severity::Error, "E061", file.clone(), item, format!("tag {:?} does not exist", a.tag));
            continue;
        };
        let boolean = t.data_type == DataType::Bool;
        let fits = match a.condition {
            C::IsTrue | C::IsFalse => boolean,
            C::Above { .. } | C::Below { .. } | C::Equals { .. } => !boolean && t.data_type != DataType::String,
            C::BadQuality => true,
        };
        if !fits {
            cx.push(Severity::Error, "E062", file.clone(), item.clone(),
                format!("condition {:?} does not apply to a {:?} tag", a.condition, t.data_type));
        }
        if a.deadband < 0.0 || a.delay_s < 0.0 {
            cx.push(Severity::Error, "E063", file.clone(), item, "deadband and delay_s must not be negative".into());
        }
    }
}

fn check_command(cx: &mut Ctx, file: &Path, item: &str, c: &Command) {
    let p = cx.p;
    match c {
        Command::OpenScreen { screen } if p.screen(screen).is_none() => {
            cx.push(Severity::Error, "E040", file.to_path_buf(), item.into(), format!("open-screen target {screen:?} does not exist"));
        }
        Command::WriteTag { tag, .. } => match p.tag(tag) {
            None => cx.push(Severity::Error, "E041", file.to_path_buf(), item.into(), format!("write-tag target {tag:?} does not exist")),
            Some(t) if !t.access.writes() => cx.push(Severity::Warning, "W042", file.to_path_buf(), item.into(),
                format!("write-tag targets {tag:?}, which is read-only, so the device is never written")),
            _ => {}
        },
        Command::Report { report, tags, .. } => {
            if !p.meta.reports.contains(report) {
                cx.push(Severity::Error, "E043", file.to_path_buf(), item.into(), format!("report {report:?} is not listed in project.toml"));
            }
            for t in tags.iter().filter(|t| p.tag(t).is_none()) {
                cx.push(Severity::Error, "E044", file.to_path_buf(), item.into(), format!("report parameter tag {t:?} does not exist"));
            }
        }
        _ => {}
    }
}

fn check_usage(cx: &mut Ctx) {
    let p = cx.p;
    let mut used: HashSet<&str> = HashSet::new();
    for s in &p.screens {
        for e in &s.elements {
            match &e.kind {
                ElementKind::Display { tag: Some(t), .. }
                | ElementKind::Pipeline { tag: Some(t), .. }
                | ElementKind::Symbol { tag: Some(t), .. } => {
                    used.insert(t);
                }
                ElementKind::Trend { tags, .. } => used.extend(tags.iter().map(String::as_str)),
                _ => {}
            }
            for c in &e.on_click {
                match c {
                    Command::WriteTag { tag, .. } => {
                        used.insert(tag);
                    }
                    Command::Report { tags, .. } => used.extend(tags.iter().map(String::as_str)),
                    Command::OpenScreen { .. } => {}
                }
            }
        }
    }
    for l in &p.data.loggers {
        used.extend(l.columns.iter().map(|c| c.tag.as_str()));
    }
    used.extend(p.alarms.iter().map(|a| a.tag.as_str()));
    let unused: Vec<_> = p
        .tags
        .iter()
        .filter(|t| t.historian.is_none() && !used.contains(t.name.as_str()))
        .map(|t| t.name.as_str())
        .collect();
    if !unused.is_empty() {
        cx.push(Severity::Note, "N050", cx.src.tags(), "tags".into(), format!(
            "{} tag(s) are not shown, logged or historized anywhere: {}", unused.len(), unused.join(", ")));
    }
    let used_drivers: HashSet<&str> = p.tags.iter().filter_map(|t| t.address.as_ref().map(Address::driver)).collect();
    for d in p.drivers.iter().filter(|d| !used_drivers.contains(d.name.as_str())) {
        cx.push(Severity::Note, "N051", cx.src.driver(&d.name), format!("driver {:?}", d.name), "no tag uses this driver".into());
    }
}

#[cfg(test)]
mod tests;
