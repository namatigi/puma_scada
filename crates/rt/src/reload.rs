//! Applying a changed project to the running runtime (studio saves).
//!
//! After a studio save the project is re-read from disk (saves arriving within
//! [`DEBOUNCE`] are applied together, e.g. "Save All"), checked, and applied
//! with the smallest restart that is still correct:
//!
//! | What changed | What happens |
//! |---|---|
//! | screens, project settings | the served project is swapped; drivers, historian and alarms keep running |
//! | alarms, historians, data loggers, databases | historian and alarm engine restart on the same tag store (their journals are kept); drivers keep running |
//! | tags or drivers | a new tag store and new drivers (and simulator) start, the gateway switches to them, then the old drivers stop; historian and alarm engine restart |
//!
//! In every case WebSocket clients receive `{"type":"reload"}`. On a tag or
//! driver change their WebSocket is also closed (it belongs to the old tag
//! store); the HMI and studio reconnect on their own.
//!
//! A project that does not load, or that has check errors while the runtime
//! was started without `--allow-errors`, is not applied: the runtime keeps
//! running the previous version and logs why. The studio shows the same
//! diagnostics to the engineer.

use moviclone_check::{check, Diagnostic, Severity};
use moviclone_model::Project;
use std::path::Path;
use std::time::Duration;

/// Saves arriving this close together are applied as one reload.
pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// The smallest restart that applies a project change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// Nothing the runtime uses changed.
    None,
    /// Swap the served project only.
    Project,
    /// Restart the historian and the alarm engine.
    Services,
    /// New tag store, drivers and simulator, then the services.
    Core,
}

pub fn classify(old: &Project, new: &Project) -> Change {
    if old.tags != new.tags || old.drivers != new.drivers {
        Change::Core
    } else if old.alarms != new.alarms || old.data != new.data {
        Change::Services
    } else if old != new {
        Change::Project
    } else {
        Change::None
    }
}

/// Loads and checks the project in `dir`. Errors are refused unless
/// `allow_errors`; every diagnostic is logged.
pub fn load_checked(dir: &Path, allow_errors: bool) -> anyhow::Result<Project> {
    let (project, src) =
        moviclone_model::load_project(dir).map_err(|e| anyhow::anyhow!("cannot load project {}: {e}", dir.display()))?;
    let diags = check(&project, &src);
    log_diagnostics(&project, &diags);
    let errors = diags.iter().filter(|d| d.severity == Severity::Error).count();
    if errors > 0 {
        if !allow_errors {
            anyhow::bail!("the project has {errors} error(s); fix them or start with --allow-errors");
        }
        tracing::warn!("using the project despite {errors} error(s) (--allow-errors)");
    }
    Ok(project)
}

fn log_diagnostics(project: &Project, diags: &[Diagnostic]) {
    let count = |s| diags.iter().filter(|d| d.severity == s).count();
    for d in diags {
        match d.severity {
            Severity::Error => tracing::error!("{d}"),
            Severity::Warning => tracing::warn!("{d}"),
            Severity::Note => tracing::info!("{d}"),
        }
    }
    tracing::info!(
        "{}: {} tags, {} screens, {} alarms, {} errors, {} warnings, {} notes",
        project.meta.name,
        project.tags.len(),
        project.screens.len(),
        project.alarms.len(),
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Note)
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use moviclone_model::{Alarm, AlarmCondition, AlarmSeverity};

    fn demo() -> Project {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/oxoafrica");
        moviclone_model::load_project(&dir).expect("demo project loads").0
    }

    #[test]
    fn classifies_changes_by_the_smallest_restart() {
        let base = demo();
        assert_eq!(classify(&base, &base.clone()), Change::None);

        let mut p = base.clone();
        p.screens[0].background = Some("#123456".into());
        assert_eq!(classify(&base, &p), Change::Project);
        let mut p = base.clone();
        p.meta.description = Some("changed".into());
        assert_eq!(classify(&base, &p), Change::Project);

        let mut p = base.clone();
        p.alarms.push(Alarm {
            name: "X".into(),
            tag: "FM_01_Temp".into(),
            condition: AlarmCondition::Above { limit: 1.0 },
            severity: AlarmSeverity::Low,
            message: "x".into(),
            area: None,
            deadband: 0.0,
            delay_s: 0.0,
            ack_required: true,
            enabled: true,
        });
        assert_eq!(classify(&base, &p), Change::Services);
        let mut p = base.clone();
        p.data.historians[0].max_age_s = 60;
        assert_eq!(classify(&base, &p), Change::Services);

        let mut p = base.clone();
        p.tags[0].description = Some("changed".into());
        assert_eq!(classify(&base, &p), Change::Core);
        let mut p = base.clone();
        p.drivers[0].name.push('x');
        assert_eq!(classify(&base, &p), Change::Core);
    }

    #[test]
    fn demo_project_passes_the_check() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo/oxoafrica");
        let p = load_checked(&dir, false).expect("demo/oxoafrica has no check errors");
        assert!(p.alarms.len() >= 20, "demo alarms present");
    }
}
