//! Projects on disk: recent list and settings, new empty projects, Movicon imports.

use anyhow::{bail, Context, Result};
use moviclone_model::{io::PROJECT_FILE, save_project, Project, ProjectMeta, Screen};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 10;

/// `settings.json` in the app's config folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Most recent first.
    pub recent: Vec<PathBuf>,
    /// Run projects against the built-in Modbus simulator instead of the devices.
    pub simulate: bool,
}

impl Default for Settings {
    fn default() -> Self {
        // An engineering PC rarely reaches the plant's devices: simulate by default.
        Self { recent: Vec::new(), simulate: true }
    }
}

impl Settings {
    pub fn load(file: &Path) -> Self {
        std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, file: &Path) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(file, text) {
                    tracing::warn!("cannot save {}: {e}", file.display());
                }
            }
            Err(e) => tracing::warn!("cannot serialize settings: {e}"),
        }
    }

    pub fn remember(&mut self, dir: &Path) {
        self.recent.retain(|p| p != dir);
        self.recent.insert(0, dir.to_path_buf());
        self.recent.truncate(MAX_RECENT);
    }

    pub fn forget(&mut self, dir: &Path) {
        self.recent.retain(|p| p != dir);
    }
}

/// Whether `dir` holds a Moviclone project.
pub fn is_project(dir: &Path) -> bool {
    dir.join(PROJECT_FILE).is_file()
}

/// The project's display name (`project.toml` `name`), or the folder name.
pub fn project_name(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(PROJECT_FILE))
        .ok()
        .and_then(|t| toml_name(&t))
        .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Project".into())
}

/// `name = "…"` from project.toml without a full parse (the file may be mid-edit).
fn toml_name(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix("name")?.trim_start().strip_prefix('=')?.trim();
        let v = rest.strip_prefix('"')?.split('"').next()?;
        (!v.is_empty()).then(|| v.to_string())
    })
}

fn ensure_empty(dir: &Path) -> Result<()> {
    if is_project(dir) {
        bail!("{} already holds a project; open it instead", dir.display());
    }
    let busy = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .any(|e| !e.file_name().to_string_lossy().starts_with('.'));
    if busy {
        bail!("{} is not empty; choose or create an empty folder", dir.display());
    }
    Ok(())
}

/// Writes a new project with one empty 1920×1080 screen into the empty folder `dir`,
/// named after the folder.
pub fn create(dir: &Path) -> Result<String> {
    ensure_empty(dir)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "New project".into());
    let project = Project {
        meta: ProjectMeta { name: name.clone(), main_screen: Some("Main".into()), ..ProjectMeta::default() },
        screens: vec![Screen {
            name: "Main".into(),
            width: 1920.0,
            height: 1080.0,
            background: Some("#3F3F3F".into()),
            legacy_id: None,
            elements: Vec::new(),
        }],
        ..Project::default()
    };
    save_project(dir, &project).with_context(|| format!("writing {}", dir.display()))?;
    Ok(name)
}

/// Converts the Movicon.NExT project `source` (a .UFProject file or its folder)
/// into the empty folder `out`, with `IMPORT-REPORT.md` next to it.
pub fn import_movicon(source: &Path, out: &Path) -> Result<String> {
    ensure_empty(out)?;
    let imported = moviclone_import_movicon::import(source).with_context(|| format!("importing {}", source.display()))?;
    save_project(out, &imported.project).with_context(|| format!("writing {}", out.display()))?;
    std::fs::write(out.join("IMPORT-REPORT.md"), imported.report.to_markdown(&imported.project.meta.name))?;
    if !imported.report.secrets.is_empty() {
        std::fs::write(out.join("secrets.example.toml"), imported.report.secrets_example())?;
    }
    Ok(imported.project.meta.name)
}

/// A folder name for `dir`'s runtime data: readable, and unique per project path.
pub fn data_key(dir: &Path) -> String {
    let path = dir.to_string_lossy().to_lowercase();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    let name: String = project_name(dir)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .take(40)
        .collect();
    format!("{name}-{:08x}", h as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_project_loads_and_refuses_a_busy_folder() {
        let dir = tempdir("new");
        let target = dir.join("Plant A");
        std::fs::create_dir_all(&target).unwrap();
        assert_eq!(create(&target).unwrap(), "Plant A");
        let (p, _) = moviclone_model::load_project(&target).unwrap();
        assert_eq!(p.meta.main_screen.as_deref(), Some("Main"));
        assert_eq!(project_name(&target), "Plant A");
        assert!(create(&target).unwrap_err().to_string().contains("already holds a project"));
        let busy = dir.join("busy");
        std::fs::create_dir_all(&busy).unwrap();
        std::fs::write(busy.join("notes.txt"), "x").unwrap();
        assert!(create(&busy).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_list_is_most_recent_first_and_bounded() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.remember(Path::new(&format!("p{i}")));
        }
        s.remember(Path::new("p5"));
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], PathBuf::from("p5"));
        assert_eq!(s.recent.iter().filter(|p| **p == PathBuf::from("p5")).count(), 1);
    }

    #[test]
    fn data_keys_differ_per_path() {
        let a = data_key(Path::new("C:/x/Plant"));
        let b = data_key(Path::new("C:/y/Plant"));
        assert_ne!(a, b);
        assert!(toml_name("culture = \"en-us\"\nname = \"OXOAFRICA SCADA\"\n").as_deref() == Some("OXOAFRICA SCADA"));
    }

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("moviclone-desktop-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
