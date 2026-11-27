//! Reading and writing the project folder.
//!
//! ```text
//! project.toml          ProjectMeta
//! tags.toml             [[tag]] …
//! data.toml             [[database]] [[historian]] [[logger]]
//! alarms.toml           [[alarm]] …
//! drivers/<name>.toml   one DriverConfig per file
//! screens/<name>.toml   one Screen per file
//! ```
//!
//! Files are written in a fixed order with defaults left out, so a project diffs
//! cleanly in version control.

use crate::{Alarm, DataConfig, Driver, DriverConfig, Project, ProjectMeta, Screen, Tag};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "project.toml";
pub const TAGS_FILE: &str = "tags.toml";
pub const DATA_FILE: &str = "data.toml";
pub const ALARMS_FILE: &str = "alarms.toml";
pub const DRIVERS_DIR: &str = "drivers";
pub const SCREENS_DIR: &str = "screens";

/// Which file each part of a loaded project came from, for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct SourceMap {
    pub root: PathBuf,
    pub drivers: HashMap<String, PathBuf>,
    pub screens: HashMap<String, PathBuf>,
}

impl SourceMap {
    pub fn project(&self) -> PathBuf {
        self.root.join(PROJECT_FILE)
    }
    pub fn tags(&self) -> PathBuf {
        self.root.join(TAGS_FILE)
    }
    pub fn data(&self) -> PathBuf {
        self.root.join(DATA_FILE)
    }
    pub fn alarms(&self) -> PathBuf {
        self.root.join(ALARMS_FILE)
    }
    pub fn driver(&self, name: &str) -> PathBuf {
        self.drivers
            .get(name)
            .cloned()
            .unwrap_or_else(|| self.root.join(DRIVERS_DIR).join(format!("{name}.toml")))
    }
    pub fn screen(&self, name: &str) -> PathBuf {
        self.screens
            .get(name)
            .cloned()
            .unwrap_or_else(|| self.root.join(SCREENS_DIR).join(format!("{}.toml", file_stem_for(name))))
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io { path: PathBuf, source: std::io::Error },
    /// Syntax or type error; the message from `toml` includes line and column.
    Parse { path: PathBuf, message: String },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            LoadError::Parse { path, message } => write!(f, "{}: {message}", path.display()),
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AlarmsFile {
    #[serde(rename = "alarm", default)]
    alarms: Vec<Alarm>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TagsFile {
    #[serde(rename = "tag", default)]
    tags: Vec<Tag>,
}

/// Loads every file of a project folder. Stops at the first file that fails to
/// parse; reference checks are left to `moviclone-check`.
pub fn load_project(root: &Path) -> Result<(Project, SourceMap), LoadError> {
    let mut map = SourceMap { root: root.to_path_buf(), ..Default::default() };

    let meta: ProjectMeta = read_toml(&map.project())?;
    let tags = if map.tags().exists() { read_toml::<TagsFile>(&map.tags())?.tags } else { Vec::new() };
    let data = if map.data().exists() { read_toml::<DataConfig>(&map.data())? } else { DataConfig::default() };
    let alarms = if map.alarms().exists() { read_toml::<AlarmsFile>(&map.alarms())?.alarms } else { Vec::new() };

    let mut drivers = Vec::new();
    for path in toml_files(&root.join(DRIVERS_DIR))? {
        let name = stem(&path);
        let config: DriverConfig = read_toml(&path)?;
        map.drivers.insert(name.clone(), path);
        drivers.push(Driver { name, config });
    }

    let mut screens = Vec::new();
    for path in toml_files(&root.join(SCREENS_DIR))? {
        let screen: Screen = read_toml(&path)?;
        map.screens.insert(screen.name.clone(), path);
        screens.push(screen);
    }

    Ok((Project { meta, tags, drivers, data, alarms, screens }, map))
}

/// Writes a project folder, replacing the files it owns. Driver and screen files
/// that are no longer part of the project are removed.
pub fn save_project(root: &Path, project: &Project) -> std::io::Result<()> {
    fs::create_dir_all(root.join(DRIVERS_DIR))?;
    fs::create_dir_all(root.join(SCREENS_DIR))?;

    write_toml(&root.join(PROJECT_FILE), &project.meta)?;
    write_toml(&root.join(TAGS_FILE), &TagsFile { tags: project.tags.clone() })?;
    write_toml(&root.join(DATA_FILE), &project.data)?;
    let alarms = root.join(ALARMS_FILE);
    if project.alarms.is_empty() {
        if alarms.exists() {
            fs::remove_file(&alarms)?;
        }
    } else {
        write_toml(&alarms, &AlarmsFile { alarms: project.alarms.clone() })?;
    }

    let mut keep = Vec::new();
    for d in &project.drivers {
        let path = root.join(DRIVERS_DIR).join(format!("{}.toml", file_stem_for(&d.name)));
        write_toml(&path, &d.config)?;
        keep.push(path);
    }
    for s in &project.screens {
        let path = root.join(SCREENS_DIR).join(format!("{}.toml", file_stem_for(&s.name)));
        write_toml(&path, s)?;
        keep.push(path);
    }
    for dir in [DRIVERS_DIR, SCREENS_DIR] {
        for path in toml_files(&root.join(dir)).map_err(|e| std::io::Error::other(e.to_string()))? {
            if !keep.contains(&path) {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

/// File name for an item name: anything outside `[A-Za-z0-9_.-]` becomes `_`.
pub fn file_stem_for(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' })
        .collect()
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T, LoadError> {
    let text = fs::read_to_string(path).map_err(|source| LoadError::Io { path: path.into(), source })?;
    toml::from_str(&text).map_err(|e| LoadError::Parse { path: path.into(), message: e.to_string() })
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = toml::to_string(value).map_err(std::io::Error::other)?;
    fs::write(path, text)
}

fn toml_files(dir: &Path) -> Result<Vec<PathBuf>, LoadError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir).map_err(|source| LoadError::Io { path: dir.into(), source })?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    Ok(files)
}

fn stem(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
