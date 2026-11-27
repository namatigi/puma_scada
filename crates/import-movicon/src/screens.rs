//! `Screen/`: one Moviclone screen from each Movicon screen's file family.
//!
//! | File                 | Used for                                         |
//! |----------------------|--------------------------------------------------|
//! | `.xaml`              | element list, positions, sizes, rotation, text   |
//! | `.xaml.Settings`     | screen size, document id, button captions        |
//! | `.svg`               | element kind (`class` of each foreignObject)     |
//! | `.controls`          | display units/format, tag reference ids          |
//! | `.references`        | reference id → tag NodeId and name               |
//! | `.commands`          | click commands                                   |
//!
//! The compiled copies, binary settings, `.ios`, `.styles`, `.entities` and the
//! PNG thumbnail are derived data and are ignored.

use crate::report::{Level, Report};
use crate::util::{child, child_text, css_color, guid_of, parse_xml, read_text};
use crate::TagIndex;
use anyhow::{Context, Result};
use moviclone_model::{Command, Element, ElementKind, Screen, WriteMode};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Returns the screens and a map from Movicon document GUID to screen name.
pub fn import(dir: &Path, index: &TagIndex, rep: &mut Report) -> Result<(Vec<Screen>, HashMap<String, String>)> {
    let mut xamls: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(e) => e.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "xaml")).collect(),
        Err(_) => return Ok((Vec::new(), HashMap::new())),
    };
    xamls.sort();

    // First pass: document ids, so open-screen commands can refer by GUID.
    let mut settings = Vec::new();
    let mut ids = HashMap::new();
    for x in &xamls {
        let name = stem(x);
        let s = ScreenSettings::read(&dir.join(format!("{name}.xaml.Settings")))?;
        if let Some(id) = &s.id {
            ids.insert(id.clone(), name.clone());
        }
        settings.push(s);
    }

    let mut screens = Vec::new();
    for (x, s) in xamls.iter().zip(settings) {
        let screen = import_screen(dir, x, s, &ids, index, rep).with_context(|| format!("importing screen {}", x.display()))?;
        screens.push(screen);
    }
    Ok((screens, ids))
}

fn stem(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

#[derive(Default)]
struct ScreenSettings {
    id: Option<String>,
    width: Option<f64>,
    height: Option<f64>,
    /// XamlName → `Content` string (button captions).
    content: HashMap<String, String>,
}

impl ScreenSettings {
    fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = read_text(path)?;
        let doc = parse_xml(&text, path)?;
        let root = doc.root_element();
        let mut s = Self {
            id: child_text(root, "idDocument").and_then(guid_of),
            width: child_text(root, "width").and_then(|v| v.parse().ok()),
            height: child_text(root, "height").and_then(|v| v.parse().ok()),
            content: HashMap::new(),
        };
        let Some(entities) = child(root, "mapScreenEntities") else { return Ok(s) };
        for entry in entities.children().filter(|n| n.is_element()) {
            let Some(name) = child_text(entry, "XamlName") else { continue };
            let props = entry.descendants().find(|n| n.has_tag_name(("http://progea.com", "mapProblematicXamlWriterProperties"))
                || n.tag_name().name() == "mapProblematicXamlWriterProperties");
            let Some(props) = props else { continue };
            for kv in props.children().filter(|n| n.is_element()) {
                if child_text(kv, "Key") != Some("Content") {
                    continue;
                }
                // The value is itself XAML: <String xmlns="clr-namespace:System;…">HFO Trend &amp; Log</String>
                if let Some(inner) = child(kv, "Value").and_then(|v| v.text()) {
                    if let Ok(d) = roxmltree::Document::parse(inner) {
                        if let Some(t) = d.root_element().text() {
                            s.content.insert(name.to_string(), t.trim().to_string());
                        }
                    }
                }
            }
        }
        Ok(s)
    }
}

/// `SVGReferenceId` → (NodeId GUID, tag name from the browse path).
type References = HashMap<i64, (Option<String>, Option<String>)>;

fn read_references(path: &Path) -> Result<References> {
    let mut out = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let text = read_text(path)?;
    let doc = parse_xml(&text, path)?;
    for h in doc.descendants().filter(|n| n.tag_name().name() == "SVGHelper") {
        let Some(id) = child_text(h, "SVGReferenceId").and_then(|v| v.parse().ok()) else { continue };
        let tag = child(h, "Tag");
        let guid = tag
            .and_then(|t| child(t, "resolvedNodeId"))
            .and_then(|n| n.children().find(|c| c.tag_name().name() == "Identifier"))
            .and_then(|n| n.text())
            .and_then(guid_of);
        let name = tag
            .and_then(|t| child_text(t, "relativePath"))
            .and_then(|p| p.rsplit(':').next())
            .map(str::to_string);
        out.insert(id, (guid, name));
    }
    Ok(out)
}

fn read_json_array(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = read_text(path)?;
    let v: Value = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    Ok(v.as_array().cloned().unwrap_or_default())
}

/// `class` of each `<foreignObject id=…>` in the screen's SVG: the control type.
fn read_svg_classes(path: &Path) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let text = read_text(path)?;
    let doc = parse_xml(&text, path)?;
    for n in doc.descendants().filter(|n| n.tag_name().name() == "foreignObject") {
        if let (Some(id), Some(class)) = (n.attribute("id"), n.attribute("class")) {
            out.insert(id.to_string(), class.to_string());
        }
    }
    Ok(out)
}

/// Rotation of each element in the screen's SVG export: `transform="rotate(a cx cy)"`
/// on the element's `<g>` or `<foreignObject>`. Movicon keeps the rotation of
/// controls (displays, buttons) in `.xaml.Settings` rather than in `.xaml`; the SVG
/// export always carries it in this plain form. Value: (angle, centre x, centre y).
fn read_svg_rotations(path: &Path) -> Result<HashMap<String, (f64, f64, f64)>> {
    let mut out = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let text = read_text(path)?;
    let doc = parse_xml(&text, path)?;
    for n in doc.descendants().filter(|n| matches!(n.tag_name().name(), "g" | "foreignObject")) {
        let (Some(id), Some(t)) = (n.attribute("id"), n.attribute("transform")) else { continue };
        let Some(args) = t.trim().strip_prefix("rotate(").and_then(|r| r.split(')').next()) else { continue };
        let v: Vec<f64> = args.split([' ', ',']).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        if let [a, cx, cy] = v[..] {
            out.insert(id.to_string(), (a, cx, cy));
        }
    }
    Ok(out)
}

/// Library symbol name per element, from `<Label x:Key="SymbolName">` in the
/// compiled XAML. Only the name is read; the symbol artwork is not copied.
fn read_symbol_names(path: &Path) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let text = read_text(path)?;
    let doc = parse_xml(&text, path)?;
    for el in doc.root_element().children().filter(|n| n.is_element()) {
        let Some(id) = el.attribute("Name") else { continue };
        let name = el
            .descendants()
            .find(|n| n.tag_name().name() == "Label" && n.attributes().any(|a| a.name() == "Key" && a.value() == "SymbolName"))
            .and_then(|n| n.text());
        if let Some(name) = name {
            out.insert(id.to_string(), name.trim().to_string());
        }
    }
    Ok(out)
}

fn num(node: roxmltree::Node, attr: &str) -> Option<f64> {
    node.attribute(attr).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

fn rotation(node: roxmltree::Node) -> f64 {
    let angle = node
        .descendants()
        .filter(|n| n.tag_name().name() == "RotateTransform")
        .filter_map(|n| num(n, "Angle"))
        .sum::<f64>();
    let a = angle.rem_euclid(360.0);
    if a.abs() < 1e-9 { 0.0 } else { a }
}

fn import_screen(
    dir: &Path,
    xaml_path: &Path,
    settings: ScreenSettings,
    ids: &HashMap<String, String>,
    index: &TagIndex,
    rep: &mut Report,
) -> Result<Screen> {
    let name = stem(xaml_path);
    let file = |ext: &str| dir.join(format!("{name}.{ext}"));
    let src = format!("Screen/{name}");

    let xaml_text = read_text(xaml_path)?;
    let xaml = parse_xml(&xaml_text, xaml_path)?;
    let root = xaml.root_element();
    let mut classes = read_svg_classes(&file("svg"))?;
    let svg_rotations = read_svg_rotations(&file("svg"))?;
    let symbol_names = read_symbol_names(&file("xaml.compiled"))?;
    let controls: HashMap<String, Value> = read_json_array(&file("controls"))?
        .into_iter()
        .filter_map(|c| Some((c.get("SVGItemId")?.as_str()?.to_string(), c)))
        .collect();
    let refs = read_references(&file("references"))?;
    let commands = read_json_array(&file("commands"))?;

    let binder = Binder { refs, index, src: src.clone(), refs_file: format!("{name}.references") };

    let width = settings.width.or_else(|| num(root, "Width")).unwrap_or(1920.0);
    let height = settings.height.or_else(|| num(root, "Height")).unwrap_or(1080.0);
    if let (Some(w), Some(h)) = (num(root, "Width"), num(root, "Height")) {
        if (w, h) != (width, height) {
            rep.add(Level::Info, &src, format!("XAML canvas says {w}x{h}; screen settings say {width}x{height}; using the settings"));
        }
    }
    let background = root.attribute("Background").and_then(css_color).or_else(|| {
        let stops: Vec<String> = root
            .descendants()
            .filter(|n| n.tag_name().name() == "GradientStop")
            .filter_map(|n| n.attribute("Color").and_then(css_color))
            .collect();
        (!stops.is_empty()).then(|| format!("linear-gradient({})", stops.join(", ")))
    });

    let mut elements = Vec::new();
    let mut seen = HashSet::new();
    for (i, node) in root.children().filter(|n| n.is_element()).enumerate() {
        let local = node.tag_name().name();
        if local.contains('.') {
            continue; // property element such as <Canvas.Background>
        }
        let id = node.attribute("Name").map(str::to_string).unwrap_or_else(|| format!("{local}{i}"));
        seen.insert(id.clone());
        let ctl = controls.get(&id);
        let ctl_str = |k: &str| ctl.and_then(|c| c.get(k)).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        let ctl_ref = ctl.and_then(|c| c.get("SVGReferenceId")).and_then(Value::as_i64);
        let context = format!("element {id:?}");

        let mut x = num(node, "Canvas.Left").unwrap_or(0.0);
        let mut y = num(node, "Canvas.Top").unwrap_or(0.0);
        let mut w = num(node, "Width").unwrap_or(0.0);
        let mut h = num(node, "Height").unwrap_or(0.0);

        // Trend and history controls have no foreignObject in the SVG export.
        if matches!(id.as_str(), "DataAnalisysRT" | "HistoricalViewer") {
            classes.entry(id.clone()).or_insert_with(|| id.clone());
        }
        let kind = match (local, classes.get(&id).map(String::as_str)) {
            ("TextBox" | "TextBlock" | "Label", _) => ElementKind::Label {
                text: node.attribute("Text").or_else(|| node.text()).unwrap_or("").trim().to_string(),
            },
            ("Pipeline", _) => {
                let points: Vec<[f64; 2]> = node
                    .attribute("Points")
                    .unwrap_or("")
                    .split_whitespace()
                    .filter_map(|p| {
                        let (a, b) = p.split_once(',')?;
                        let r = |v: &str| v.parse::<f64>().ok().map(|v| (v * 1000.0).round() / 1000.0);
                        Some([r(a)?, r(b)?])
                    })
                    .collect();
                let thickness = num(node, "StrokeThickness").unwrap_or(1.0);
                if w == 0.0 && h == 0.0 && !points.is_empty() {
                    let max_x = points.iter().map(|p| p[0]).fold(f64::MIN, f64::max);
                    let max_y = points.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
                    w = (max_x + thickness / 2.0).max(thickness);
                    h = (max_y + thickness / 2.0).max(thickness);
                }
                ElementKind::Pipeline {
                    points,
                    thickness: (thickness * 1000.0).round() / 1000.0,
                    fill: node.attribute("Fill").and_then(css_color),
                    stroke: node.attribute("Stroke").and_then(css_color),
                    tag: binder.by_ref(ctl_ref, &context, rep),
                }
            }
            (_, Some("EditDisplay" | "Display")) => ElementKind::Display {
                tag: binder.by_ref(ctl_ref, &context, rep),
                unit: ctl_str("MeasUnit"),
                format: ctl_str("DisplayStringFormat"),
                editable: ctl.and_then(|c| c.get("IsReadOnly")).and_then(Value::as_bool) == Some(false),
            },
            (_, Some("ButtonControl" | "Button")) => ElementKind::Button {
                text: settings.content.get(&id).cloned().unwrap_or_default(),
            },
            ("ContentControl" | "Viewbox", None) => ElementKind::Symbol {
                symbol: symbol_names.get(&id).map(|n| format!("movicon:{n}")),
                tag: None,
            },
            (_, Some("DataAnalisysRT" | "DataAnalysisRT")) => ElementKind::Trend { tags: Vec::new(), historian: None, span_s: 3600 },
            (_, Some("HistoricalViewer")) => match ctl_str("HistoricalName") {
                Some(source) => ElementKind::HistoryTable { source },
                None => {
                    rep.add(Level::Problem, &src, format!("{context}: history viewer has no source; imported as a placeholder"));
                    ElementKind::Symbol { symbol: None, tag: None }
                }
            },
            (_, other) => {
                rep.add(Level::Warning, &src, format!(
                    "{context}: {local}{} has no equivalent yet; imported as a placeholder symbol",
                    other.map(|c| format!(" ({c})")).unwrap_or_default()));
                ElementKind::Symbol { symbol: None, tag: None }
            }
        };
        // Keep coordinates readable in diffs.
        for v in [&mut x, &mut y, &mut w, &mut h] {
            *v = (*v * 1000.0).round() / 1000.0;
        }

        let mut on_click = Vec::new();
        for c in commands.iter().filter(|c| c.get("SVGItemId").and_then(Value::as_str) == Some(id.as_str())) {
            if let Some(cmd) = convert_command(c, &context, ids, &binder, rep) {
                on_click.push(cmd);
            }
        }
        let mut angle = rotation(node);
        if angle == 0.0 {
            if let Some(&(a, cx, cy)) = svg_rotations.get(&id) {
                angle = a.rem_euclid(360.0);
                // Our renderer rotates about the element centre; say so when Movicon used another origin.
                if angle != 0.0 && ((cx - (x + w / 2.0)).abs() > 1.0 || (cy - (y + h / 2.0)).abs() > 1.0) {
                    rep.add(Level::Warning, &src, format!(
                        "{context}: rotated about ({cx}, {cy}), not its centre; it may sit slightly off"));
                }
            }
        }
        elements.push(Element { id, kind, x, y, w, h, rotation: angle, on_click });
    }

    // References to elements that no longer exist in the XAML.
    for c in &commands {
        if let Some(id) = c.get("SVGItemId").and_then(Value::as_str).filter(|id| !seen.contains(*id)) {
            rep.add(Level::Problem, &src, format!(
                "a {} command is attached to element {id:?}, which is not on the screen; dropped",
                c.get("__type").and_then(Value::as_str).unwrap_or("?")));
        }
    }
    let mut stale: Vec<&String> = controls
        .iter()
        .filter(|(id, c)| !seen.contains(*id) && c.get("SVGReferenceId").and_then(Value::as_i64).is_some())
        .map(|(id, _)| id)
        .collect();
    stale.sort();
    for id in stale {
        rep.add(Level::Problem, &src, format!("a tag binding is stored for element {id:?}, which is not on the screen; dropped"));
    }

    // A trend on the same screen as a history table plots that historian's tags.
    let table_source = elements.iter().find_map(|e| match &e.kind {
        ElementKind::HistoryTable { source } => Some(source.clone()),
        _ => None,
    });
    for e in &mut elements {
        if let ElementKind::Trend { tags, historian, .. } = &mut e.kind {
            if let Some(h) = &table_source {
                *historian = Some(h.clone());
                *tags = index.tags_of_historian(h);
                rep.add(Level::Warning, &src, format!(
                    "element {:?}: trend pens were not readable; plotting the {} tags of historian {h:?}", e.id, tags.len()));
            }
        }
    }

    Ok(Screen { name, width, height, background, legacy_id: settings.id, elements })
}

fn convert_command(
    c: &Value,
    context: &str,
    ids: &HashMap<String, String>,
    binder: &Binder,
    rep: &mut Report,
) -> Option<Command> {
    let src = binder.src.as_str();
    let ty = c.get("__type").and_then(Value::as_str).unwrap_or("?");
    let s = |k: &str| c.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    match ty {
        "OpenScreenCommand" => {
            let target = s("screenName")?;
            let screen = guid_of(target).and_then(|g| ids.get(&g).cloned()).unwrap_or_else(|| {
                let file = target.rsplit('/').next().unwrap_or(target);
                file.strip_suffix(".xaml").unwrap_or(file).to_string()
            });
            if c.get("executionMode").and_then(Value::as_i64).unwrap_or(0) != 0 {
                rep.add(Level::Warning, src, format!("{context}: open-screen as popup/frame is opened as a normal screen"));
            }
            Some(Command::OpenScreen { screen })
        }
        "ValueCommand" => {
            let mode = match c.get("type").and_then(Value::as_i64) {
                Some(0) => WriteMode::Set,
                Some(1) => WriteMode::Increase,
                Some(2) => WriteMode::Decrease,
                Some(3) => WriteMode::Toggle,
                Some(4) => WriteMode::Pulse,
                other => {
                    rep.add(Level::NotImported, src, format!("{context}: value command type {other:?} is not supported yet; dropped"));
                    return None;
                }
            };
            let tag = binder.by_ref(c.get("SVGReferenceId").and_then(Value::as_i64), context, rep)?;
            let value = match c.get("_value") {
                Some(Value::String(v)) => v.clone(),
                Some(v) if !v.is_null() => v.to_string(),
                _ => "1".into(),
            };
            Some(Command::WriteTag { tag, value, mode })
        }
        "ReportCommand" => {
            let report = s("reportName")?.rsplit('/').next()?.trim_end_matches(".report").to_string();
            let mut tags = Vec::new();
            for p in c.get("parameters").and_then(Value::as_array).into_iter().flatten() {
                let guid = p.get("NodeId").and_then(Value::as_str).and_then(guid_of);
                let name = p.get("StringRef").and_then(Value::as_str);
                let pctx = format!("{context} report parameter {:?}", p.get("Name").and_then(Value::as_str).unwrap_or("?"));
                if let Some(t) = binder.index.resolve(guid.as_deref(), name, src, &pctx, rep) {
                    tags.push(t);
                }
            }
            let output_dir = s("folderpath").map(str::to_string);
            if output_dir.is_some() {
                rep.add(Level::Warning, src, format!("{context}: report output folder is a path on the original PC"));
            }
            Some(Command::Report { report, tags, output_dir })
        }
        other => {
            rep.add(Level::NotImported, src, format!("{context}: {other} is not supported yet; dropped"));
            None
        }
    }
}

/// Resolves a screen's tag reference ids through its `.references` file.
struct Binder<'a> {
    refs: References,
    index: &'a TagIndex,
    src: String,
    refs_file: String,
}

impl Binder<'_> {
    fn by_ref(&self, ref_id: Option<i64>, context: &str, rep: &mut Report) -> Option<String> {
        let id = ref_id?;
        match self.refs.get(&id) {
            Some((guid, tag)) => self.index.resolve(guid.as_deref(), tag.as_deref(), &self.src, context, rep),
            None => {
                rep.add(Level::Problem, &self.src, format!("{context}: tag reference #{id} is missing from {}", self.refs_file));
                None
            }
        }
    }
}
