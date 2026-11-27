//! A depot's SCADA from a template project, with only the flow meters the
//! depot has, and the depot's name in `project.toml` (`site`, shown in the
//! HMI's top bar).
//!
//! The template is a normal project plus a manifest (`site-template.toml`)
//! that says which tags, screens, historians, loggers, Modbus stations and
//! screen elements belong to each meter. For every meter not chosen, those are
//! removed, and so is anything else that still refers to a removed tag or
//! screen. Meters keep their numbers, so Modbus unit ids still match.
//!
//! ```ignore
//! let manifest = Manifest::load(Path::new("demo/site-template.toml"))?;
//! let (template, _) = moviclone_model::load_project(Path::new("demo/oxoafrica"))?;
//! let project = build(&template, &manifest, &Choice { depot: "Kurasini".into(), meters: vec!["gasoline".into(), "gasoil".into()] })?;
//! moviclone_model::save_project(out, &project)?;
//! ```

use anyhow::{bail, Context, Result};
use moviclone_model::{Address, DriverConfig, Element, Project};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// `site-template.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Name of the template project folder next to the manifest.
    pub template: String,
    #[serde(rename = "meter")]
    pub meters: Vec<Meter>,
    #[serde(default)]
    pub restack: Vec<Restack>,
}

/// One flow meter of the template and what belongs to it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Meter {
    /// Stable id used to choose it, e.g. `gasoline`.
    pub key: String,
    /// Product name shown to the user, e.g. `Gasoline`.
    pub product: String,
    /// Product colour (CSS), for the setup screen.
    #[serde(default)]
    pub color: String,
    /// Every tag whose name starts with this belongs to the meter.
    pub tag_prefix: String,
    #[serde(default)]
    pub screens: Vec<String>,
    #[serde(default)]
    pub historians: Vec<String>,
    #[serde(default)]
    pub loggers: Vec<String>,
    #[serde(default)]
    pub stations: Vec<String>,
    /// Screen name -> element ids that draw this meter's line but refer to no tag.
    #[serde(default)]
    pub elements: BTreeMap<String, Vec<String>>,
}

/// Elements stacked at fixed places (e.g. navigation buttons): the ones that
/// remain move into the last places, keeping their order.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Restack {
    pub screen: String,
    pub elements: Vec<String>,
}

/// What the user chose.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Choice {
    /// Depot (site) name, shown in the HMI's top bar.
    pub depot: String,
    /// Meter keys to keep.
    pub meters: Vec<String>,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let m: Manifest = toml::from_str(&text).with_context(|| format!("{} is not a valid site template", path.display()))?;
        let mut keys = HashSet::new();
        for meter in &m.meters {
            if !keys.insert(meter.key.as_str()) {
                bail!("{}: meter key {:?} is used twice", path.display(), meter.key);
            }
        }
        Ok(m)
    }

    /// The template project folder: `<manifest folder>/<template>`.
    pub fn template_dir(&self, manifest_path: &Path) -> std::path::PathBuf {
        manifest_path.parent().unwrap_or(Path::new(".")).join(&self.template)
    }
}

/// The template with only the chosen meters, and the depot name set.
pub fn build(template: &Project, manifest: &Manifest, choice: &Choice) -> Result<Project> {
    let depot = choice.depot.trim();
    if depot.is_empty() {
        bail!("enter the depot name");
    }
    if choice.meters.is_empty() {
        bail!("choose at least one meter");
    }
    for key in &choice.meters {
        if !manifest.meters.iter().any(|m| &m.key == key) {
            bail!("unknown meter {key:?} (the template has {})", manifest.meters.iter().map(|m| m.key.as_str()).collect::<Vec<_>>().join(", "));
        }
    }
    let removed: Vec<&Meter> = manifest.meters.iter().filter(|m| !choice.meters.contains(&m.key)).collect();
    let mut p = template.clone();
    p.meta.site = Some(depot.to_string());

    let set = |f: fn(&Meter) -> &Vec<String>| -> HashSet<String> { removed.iter().flat_map(|m| f(m).iter().cloned()).collect() };
    let removed_historians = set(|m| &m.historians);
    let removed_loggers = set(|m| &m.loggers);
    let removed_stations = set(|m| &m.stations);
    let removed_screens = set(|m| &m.screens);
    // A removed meter's tags: by name, and anything else read from its device
    // (e.g. a shared register that lives on that meter's transmitter).
    let removed_tags: HashSet<String> = p
        .tags
        .iter()
        .filter(|t| {
            removed.iter().any(|m| t.name.starts_with(&m.tag_prefix))
                || matches!(&t.address, Some(Address::Modbus(a)) if removed_stations.contains(&a.station))
        })
        .map(|t| t.name.clone())
        .collect();

    p.tags.retain(|t| !removed_tags.contains(&t.name));
    p.alarms.retain(|a| !removed_tags.contains(&a.tag));
    p.screens.retain(|s| !removed_screens.contains(&s.name));
    p.data.historians.retain(|h| !removed_historians.contains(&h.name));
    p.data.loggers.retain(|l| !removed_loggers.contains(&l.name));
    for logger in &mut p.data.loggers {
        logger.columns.retain(|c| !removed_tags.contains(&c.tag));
    }
    p.data.loggers.retain(|l| !l.columns.is_empty());
    for driver in &mut p.drivers {
        match &mut driver.config {
            DriverConfig::ModbusTcp(cfg) => cfg.stations.retain(|s| !removed_stations.contains(&s.name)),
        }
    }

    for screen in &mut p.screens {
        let listed: HashSet<&String> = removed.iter().filter_map(|m| m.elements.get(&screen.name)).flatten().collect();
        let mut kept = Vec::with_capacity(screen.elements.len());
        for el in screen.elements.drain(..) {
            if listed.contains(&el.id) {
                continue;
            }
            if let Some(el) = without_refs(el, &removed_tags, &removed_screens)? {
                kept.push(el);
            }
        }
        screen.elements = kept;
    }
    for r in &manifest.restack {
        restack(template, &mut p, r);
    }
    if p.meta.main_screen.as_ref().is_some_and(|m| removed_screens.contains(m)) {
        p.meta.main_screen = p.screens.first().map(|s| s.name.clone());
    }
    Ok(p)
}

/// `el` without references to removed tags or screens: tag lists (trend pens)
/// lose those tags; an element that still names one is dropped (`None`).
fn without_refs(el: Element, tags: &HashSet<String>, screens: &HashSet<String>) -> Result<Option<Element>> {
    let mut v = serde_json::to_value(&el)?;
    if let Some(list) = v.get_mut("tags").and_then(Value::as_array_mut) {
        let before = list.len();
        list.retain(|t| t.as_str().map_or(true, |t| !tags.contains(t)));
        if before > 0 && list.is_empty() {
            return Ok(None);
        }
    }
    if refers(&v, tags, screens) {
        return Ok(None);
    }
    Ok(Some(serde_json::from_value(v)?))
}

fn refers(v: &Value, tags: &HashSet<String>, screens: &HashSet<String>) -> bool {
    match v {
        Value::String(s) => tags.contains(s) || screens.contains(s),
        Value::Array(a) => a.iter().any(|x| refers(x, tags, screens)),
        Value::Object(o) => o.iter().any(|(k, x)| k != "id" && refers(x, tags, screens)),
        _ => false,
    }
}

/// Moves the remaining elements of `r` into the lowest of the places all of
/// them had in `template` (sorted top to bottom), keeping the manifest's order.
fn restack(template: &Project, p: &mut Project, r: &Restack) {
    let Some(before) = template.screens.iter().find(|s| s.name == r.screen) else { return };
    let Some(after) = p.screens.iter_mut().find(|s| s.name == r.screen) else { return };
    let mut places: Vec<(f64, f64)> =
        r.elements.iter().filter_map(|id| before.elements.iter().find(|e| &e.id == id)).map(|e| (e.x, e.y)).collect();
    places.sort_by(|a, b| a.1.total_cmp(&b.1));
    let present: Vec<&String> = r.elements.iter().filter(|id| after.elements.iter().any(|e| &e.id == *id)).collect();
    let first = places.len().saturating_sub(present.len());
    for (id, &(x, y)) in present.iter().zip(&places[first..]) {
        if let Some(e) = after.elements.iter_mut().find(|e| &e.id == *id) {
            e.x = x;
            e.y = y;
        }
    }
}

/// Builds the depot project from `manifest_path` and writes it to the empty
/// folder `out`. Returns the project.
pub fn build_to(manifest_path: &Path, choice: &Choice, out: &Path) -> Result<Project> {
    let manifest = Manifest::load(manifest_path)?;
    let dir = manifest.template_dir(manifest_path);
    let (template, _) = moviclone_model::load_project(&dir).with_context(|| format!("loading the template {}", dir.display()))?;
    let project = build(&template, &manifest, choice)?;
    moviclone_model::save_project(out, &project).with_context(|| format!("writing {}", out.display()))?;
    Ok(project)
}
