//! Writing project files: same TOML layout as `moviclone_model::save_project`,
//! written atomically so a crash or a concurrent reader never sees half a file.

use moviclone_model::io::{ALARMS_FILE, PROJECT_FILE, SCREENS_DIR, TAGS_FILE};
use moviclone_model::{Alarm, ProjectMeta, Screen, Tag};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// `tags.toml`, the same shape the model loads (`[[tag]]` …).
#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct TagsFile {
    #[serde(rename = "tag", default)]
    pub tags: Vec<Tag>,
}

/// `alarms.toml` (`[[alarm]]` …).
#[derive(Serialize, Deserialize, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct AlarmsFile {
    #[serde(rename = "alarm", default)]
    pub alarms: Vec<Alarm>,
}

/// Serializes `value` and parses the text back, so a file is only written when
/// the model will load exactly what was sent.
pub fn to_toml_checked<T>(value: &T) -> Result<String, String>
where
    T: Serialize + DeserializeOwned + PartialEq,
{
    let text = toml::to_string(value).map_err(|e| format!("cannot be written as TOML: {e}"))?;
    let back: T = toml::from_str(&text).map_err(|e| format!("written TOML does not load back: {e}"))?;
    if &back != value {
        return Err("written TOML does not load back to the same value".into());
    }
    Ok(text)
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `text` to `path` through a temporary file in the same folder and a
/// rename, so readers see the old file or the new one, never a mix.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let dir = path.parent().ok_or_else(|| std::io::Error::other("file has no parent folder"))?;
    fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.{}-{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Windows refuses to replace a file another process has open without
/// FILE_SHARE_DELETE (editors, virus scanners, the runtime reading it), usually
/// for a few milliseconds; retry briefly before giving up.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut last = None;
    for attempt in 0..8u64 {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(15 * (attempt + 1)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

pub fn project_file(root: &Path) -> PathBuf {
    root.join(PROJECT_FILE)
}
pub fn tags_file(root: &Path) -> PathBuf {
    root.join(TAGS_FILE)
}
pub fn alarms_file(root: &Path) -> PathBuf {
    root.join(ALARMS_FILE)
}
pub fn default_screen_file(root: &Path, name: &str) -> PathBuf {
    root.join(SCREENS_DIR).join(format!("{name}.toml"))
}

pub fn write_screen(path: &Path, screen: &Screen) -> Result<(), String> {
    let text = to_toml_checked(screen)?;
    write_atomic(path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn write_tags(root: &Path, tags: Vec<Tag>) -> Result<(), String> {
    let text = to_toml_checked(&TagsFile { tags })?;
    let path = tags_file(root);
    write_atomic(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// An empty alarm list removes `alarms.toml`, as `save_project` does.
pub fn write_alarms(root: &Path, alarms: Vec<Alarm>) -> Result<(), String> {
    let path = alarms_file(root);
    if alarms.is_empty() {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove {}: {e}", path.display())),
        };
    }
    let text = to_toml_checked(&AlarmsFile { alarms })?;
    write_atomic(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn write_meta(root: &Path, meta: &ProjectMeta) -> Result<(), String> {
    let text = to_toml_checked(meta)?;
    let path = project_file(root);
    write_atomic(&path, &text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.toml");
        write_atomic(&path, "one = 1\n").expect("first write");
        write_atomic(&path, "two = 2\n").expect("second write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "two = 2\n");
        let names: Vec<_> = fs::read_dir(dir.path()).expect("list").filter_map(|e| e.ok()).collect();
        assert_eq!(names.len(), 1, "temporary file left behind");
    }

    #[test]
    fn removing_missing_alarms_file_is_fine() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_alarms(dir.path(), Vec::new()).expect("no file is not an error");
    }
}
