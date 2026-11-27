//! File versions, so a save based on an old copy of a file cannot overwrite a
//! newer one (another studio window, a hand edit, a re-import).
//!
//! `GET /api/studio/project` returns the version of every file the studio
//! edits, keyed like the studio's document parts (`meta`, `tags`, `alarms`,
//! `screen:<name>`). A save or delete may send the version it was based on in
//! an `If-Match` header; when the file on disk has another version the request
//! is refused with 409 and nothing is written. Requests without `If-Match`
//! (scripts, curl) are not checked.
//!
//! A version is a hash of the file's bytes, so it is the same across runtime
//! restarts and changes whenever the content does. A missing file has version
//! [`ABSENT`].

use crate::files;
use moviclone_model::SourceMap;
use std::collections::BTreeMap;
use std::path::Path;

/// Version of a file that does not exist (a new screen, no `alarms.toml`).
pub const ABSENT: &str = "absent";

/// The version of the file at `path`. A file that cannot be read gets a version
/// no client holds, so a checked save is refused rather than guessed.
pub fn of_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => format!("{:016x}", fnv1a(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ABSENT.to_string(),
        Err(e) => format!("unreadable-{}", e.kind()),
    }
}

/// Versions of every edited file, keyed by studio document part.
pub fn of_project(root: &Path, map: Option<&SourceMap>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("meta".to_string(), of_file(&files::project_file(root)));
    out.insert("tags".to_string(), of_file(&files::tags_file(root)));
    out.insert("alarms".to_string(), of_file(&files::alarms_file(root)));
    if let Some(map) = map {
        for (name, path) in &map.screens {
            out.insert(format!("screen:{name}"), of_file(path));
        }
    }
    out
}

/// The `If-Match` value without the quotes HTTP puts around entity tags.
pub fn parse_if_match(value: &str) -> Option<String> {
    let v = value.trim().trim_start_matches("W/").trim_matches('"').trim();
    (!v.is_empty() && v != "*").then(|| v.to_string())
}

/// 64-bit FNV-1a: tiny, stable across Rust versions and platforms, and plenty
/// to tell two versions of one project file apart (this is not a security check).
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_follows_content_not_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.toml");
        assert_eq!(of_file(&path), ABSENT);
        std::fs::write(&path, "a = 1\n").expect("write");
        let v1 = of_file(&path);
        std::fs::write(&path, "a = 2\n").expect("write");
        let v2 = of_file(&path);
        std::fs::write(&path, "a = 1\n").expect("write");
        assert_ne!(v1, v2);
        assert_eq!(of_file(&path), v1, "same bytes, same version");
    }

    #[test]
    fn if_match_quotes_and_wildcards() {
        assert_eq!(parse_if_match("\"abc\""), Some("abc".into()));
        assert_eq!(parse_if_match("W/\"abc\""), Some("abc".into()));
        assert_eq!(parse_if_match("abc"), Some("abc".into()));
        assert_eq!(parse_if_match("*"), None);
        assert_eq!(parse_if_match("  "), None);
    }
}
