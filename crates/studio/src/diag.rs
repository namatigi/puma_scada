//! Loading the project from disk and validating it, as the studio shows it.

use moviclone_check::{check, Diagnostic, Severity};
use moviclone_model::{load_project, LoadError, Project, SourceMap};
use serde::Serialize;
use std::path::Path;

/// `Diagnostic` as JSON (docs/PROTOCOL.md). `file` is relative to the project
/// folder with `/` separators, so it means the same thing on every machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticJson {
    pub severity: &'static str,
    pub code: String,
    pub file: String,
    pub item: String,
    pub message: String,
}

/// Code used when a file does not load at all (syntax or type error).
pub const LOAD_ERROR_CODE: &str = "E000";

pub fn relative(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn severity(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

pub fn to_json(root: &Path, d: &Diagnostic) -> DiagnosticJson {
    DiagnosticJson {
        severity: severity(d.severity),
        code: d.code.to_string(),
        file: relative(root, &d.file),
        item: d.item.clone(),
        message: d.message.clone(),
    }
}

fn load_error(root: &Path, e: &LoadError) -> DiagnosticJson {
    let (path, message) = match e {
        LoadError::Io { path, source } => (path, source.to_string()),
        LoadError::Parse { path, message } => (path, message.clone()),
    };
    DiagnosticJson {
        severity: "error",
        code: LOAD_ERROR_CODE.into(),
        file: relative(root, path),
        item: "file".into(),
        message: message.trim().to_string(),
    }
}

/// What the studio needs after any read or save.
pub struct Snapshot {
    /// `None` when a file does not load; the diagnostics then say which.
    pub loaded: Option<(Project, SourceMap)>,
    pub diagnostics: Vec<DiagnosticJson>,
}

/// Loads and validates the project folder. Never fails: problems become diagnostics.
pub fn snapshot(root: &Path) -> Snapshot {
    match load_project(root) {
        Ok((project, map)) => {
            let diagnostics = check(&project, &map).iter().map(|d| to_json(root, d)).collect();
            Snapshot { loaded: Some((project, map)), diagnostics }
        }
        Err(e) => Snapshot { loaded: None, diagnostics: vec![load_error(root, &e)] },
    }
}

#[cfg(test)]
mod tests {
    use super::relative;
    use std::path::Path;

    #[test]
    fn relative_paths_use_forward_slashes() {
        let root = Path::new("proj");
        assert_eq!(relative(root, &root.join("screens").join("A.toml")), "screens/A.toml");
        assert_eq!(relative(root, &root.join("tags.toml")), "tags.toml");
    }
}
