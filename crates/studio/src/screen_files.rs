//! Which file a screen save or delete touches.
//!
//! Screen names are case-sensitive in the model, but NTFS (and the default
//! macOS file system) are not: `screens/manifold.toml` *is* `Manifold.toml`
//! there. Saving `manifold` next to `Manifold` would silently overwrite the
//! other screen, and deleting by the default path could remove the file of a
//! screen that was just saved under another case. So:
//!
//! - a save or delete is refused (409) when another loaded screen has the same
//!   name ignoring ASCII case;
//! - a save of a new screen is refused when its default file name is already
//!   taken, in any case, by a file holding another screen;
//! - a delete only removes the file the project loader read that exact screen
//!   from, never a path derived from the name.

use crate::diag::{relative, snapshot};
use crate::files;
use moviclone_model::io::SCREENS_DIR;
use moviclone_model::SourceMap;
use std::path::{Path, PathBuf};

/// Why a screen file cannot be resolved.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The request would clash with another screen (HTTP 409).
    Conflict(String),
    /// The screen does not exist (HTTP 404).
    NotFound(String),
}

fn case_twin<'a>(map: &'a SourceMap, name: &str) -> Option<&'a str> {
    map.screens.keys().map(String::as_str).find(|other| *other != name && other.eq_ignore_ascii_case(name))
}

fn twin_conflict(twin: &str) -> Refusal {
    Refusal::Conflict(format!("a screen named {twin:?} already exists (screen names are case-insensitive on disk)"))
}

/// The existing file in `dir` whose name equals `file_name` ignoring ASCII case.
fn existing_file_ignoring_case(dir: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(file_name))
        .map(|e| e.path())
}

fn same_file_ignoring_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// The file a save of screen `name` writes: where it was loaded from, or
/// `screens/<name>.toml` for a new screen.
pub fn file_for_save(root: &Path, name: &str) -> Result<PathBuf, Refusal> {
    let loaded = snapshot(root).loaded.map(|(_, map)| map);
    if let Some(map) = &loaded {
        if let Some(twin) = case_twin(map, name) {
            return Err(twin_conflict(twin));
        }
        if let Some(path) = map.screens.get(name) {
            return Ok(path.clone());
        }
    }
    let default = files::default_screen_file(root, name);
    let file_name = default.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(existing) = existing_file_ignoring_case(&root.join(SCREENS_DIR), &file_name) else {
        return Ok(default);
    };
    let owner = loaded.as_ref().and_then(|map| {
        map.screens.iter().find(|(_, p)| same_file_ignoring_case(p, &existing)).map(|(n, _)| n.clone())
    });
    match owner {
        Some(owner) => Err(Refusal::Conflict(format!(
            "{} already holds screen {owner:?}; choose another name",
            relative(root, &existing)
        ))),
        // The project does not load (so the file's screen is unknown): only an
        // exact-case file may be replaced, which is the engineer fixing it.
        None if existing.file_name() == default.file_name() => Ok(default),
        None => Err(Refusal::Conflict(format!(
            "{} exists with different letter case; rename or remove it first",
            relative(root, &existing)
        ))),
    }
}

/// The file a delete of screen `name` removes: only the one it was loaded from.
pub fn file_for_delete(root: &Path, name: &str) -> Result<PathBuf, Refusal> {
    let Some((_, map)) = snapshot(root).loaded else {
        return Err(Refusal::Conflict(
            "the project does not load, so it is not known which file holds this screen; fix the problems first".into(),
        ));
    };
    if let Some(path) = map.screens.get(name) {
        return Ok(path.clone());
    }
    match case_twin(&map, name) {
        Some(twin) => Err(Refusal::NotFound(format!("screen {name:?} does not exist (did you mean {twin:?}?)"))),
        None => Err(Refusal::NotFound(format!("screen {name:?} does not exist"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project(screens: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("project.toml"), "name = \"T\"\n").expect("write");
        fs::create_dir_all(dir.path().join("screens")).expect("mkdir");
        for (file, name) in screens {
            fs::write(dir.path().join("screens").join(file), format!("name = \"{name}\"\nwidth = 10.0\nheight = 10.0\n"))
                .expect("write");
        }
        dir
    }

    #[test]
    fn case_twins_are_refused() {
        let dir = project(&[("Manifold.toml", "Manifold")]);
        assert!(matches!(file_for_save(dir.path(), "manifold"), Err(Refusal::Conflict(m)) if m.contains("\"Manifold\"")));
        assert_eq!(file_for_save(dir.path(), "Manifold"), Ok(dir.path().join("screens/Manifold.toml")));
        assert!(matches!(file_for_delete(dir.path(), "manifold"), Err(Refusal::NotFound(m)) if m.contains("did you mean")));
    }

    #[test]
    fn default_file_of_another_screen_is_not_taken() {
        // File X.toml holds screen Y: a new screen X must not overwrite it.
        let dir = project(&[("X.toml", "Y")]);
        assert!(matches!(file_for_save(dir.path(), "X"), Err(Refusal::Conflict(m)) if m.contains("\"Y\"")));
        assert!(matches!(file_for_save(dir.path(), "x"), Err(Refusal::Conflict(_))));
        assert!(matches!(file_for_delete(dir.path(), "X"), Err(Refusal::NotFound(_))));
        assert_eq!(file_for_save(dir.path(), "Z"), Ok(dir.path().join("screens/Z.toml")));
    }

    #[test]
    fn broken_project_allows_saving_but_not_deleting() {
        let dir = project(&[("Main.toml", "Main")]);
        fs::write(dir.path().join("screens/Bad.toml"), "not = [valid").expect("write");
        assert_eq!(file_for_save(dir.path(), "Bad"), Ok(dir.path().join("screens/Bad.toml")));
        assert!(matches!(file_for_save(dir.path(), "bad"), Err(Refusal::Conflict(_))));
        assert!(matches!(file_for_delete(dir.path(), "Main"), Err(Refusal::Conflict(_))));
    }
}
