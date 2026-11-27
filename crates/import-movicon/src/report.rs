//! What the importer found, changed and left behind.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Credentials or other secrets removed from the project.
    Security,
    /// A broken or inconsistent reference in the source project.
    Problem,
    /// Something converted with an assumption, or partly.
    Warning,
    /// Present in the source but not imported in this version.
    NotImported,
    Info,
}

impl Level {
    fn heading(self) -> &'static str {
        match self {
            Level::Security => "Secrets removed",
            Level::Problem => "Problems in the source project",
            Level::Warning => "Converted with assumptions",
            Level::NotImported => "Not imported yet",
            Level::Info => "Other notes",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    /// Source file or module, relative to the Movicon project folder.
    pub source: String,
    pub message: String,
}

/// A secret the user must provide before the project can connect.
#[derive(Debug, Clone)]
pub struct SecretNeeded {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub secrets: Vec<SecretNeeded>,
    pub summary: Vec<(String, String)>,
}

impl Report {
    pub fn add(&mut self, level: Level, source: impl Into<String>, message: impl Into<String>) {
        self.findings.push(Finding { level, source: source.into(), message: message.into() });
    }

    pub fn count(&self, level: Level) -> usize {
        self.findings.iter().filter(|f| f.level == level).count()
    }

    pub fn to_markdown(&self, project: &str) -> String {
        let mut md = format!("# Import report: {project}\n\n");
        for (k, v) in &self.summary {
            let _ = writeln!(md, "- **{k}:** {v}");
        }
        let mut findings = self.findings.clone();
        findings.sort_by(|a, b| (a.level, &a.source).cmp(&(b.level, &b.source)));
        let mut current = None;
        for f in &findings {
            if current != Some(f.level) {
                let _ = write!(md, "\n## {}\n\n", f.level.heading());
                current = Some(f.level);
            }
            let _ = writeln!(md, "- `{}`: {}", f.source, f.message);
        }
        if !self.secrets.is_empty() {
            md.push_str("\n## Secrets to provide\n\nSet each one in the OS keychain, or as an environment variable `MOVICLONE_SECRET_<NAME>` (upper case, `-` → `_`).\n\n");
            for s in &self.secrets {
                let _ = writeln!(md, "- `secret://{}`: {}", s.name, s.description);
            }
        }
        md
    }

    pub fn secrets_example(&self) -> String {
        let mut out = String::from(
            "# Secrets referenced by this project. Values were removed during import and\n\
             # must be provided outside the project folder: OS keychain, or environment\n\
             # variables MOVICLONE_SECRET_<NAME>. Never commit real values.\n\n",
        );
        for s in &self.secrets {
            let _ = writeln!(out, "# {}\n{} = \"\"\n", s.description, s.name);
        }
        out
    }
}
