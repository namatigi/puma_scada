//! `movi`: the Moviclone command line.
//!
//! ```text
//! movi import <movicon project> -o <folder>   convert a Movicon.NExT project
//! movi build  <folder>                        validate a project and plan polling
//! movi bundle <folder> -o project.json        the project as one JSON document
//! ```

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use moviclone_check::{check, plan_polls, Severity};
use moviclone_model::{load_project, save_project, DriverConfig};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "movi", version, about = "Moviclone project tools")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert a Movicon.NExT project into a Moviclone project folder.
    Import {
        /// The .UFProject file, or the folder that contains it.
        source: PathBuf,
        /// Output folder.
        #[arg(short, long)]
        out: PathBuf,
        /// Write into an output folder that already contains a project.
        #[arg(long)]
        force: bool,
        /// Display name for the project (default: the Movicon project's file name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Validate a project: resolve every reference and plan device polling.
    Build {
        /// Project folder (contains project.toml).
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Print every poll job, not just the totals.
        #[arg(long)]
        plan: bool,
        /// Registers that may be read and discarded to merge two requests.
        #[arg(long, default_value_t = 0)]
        max_gap: u16,
        /// Treat warnings as errors.
        #[arg(long)]
        deny_warnings: bool,
    },
    /// Write the whole project as one JSON file (the shape of GET /api/bundle).
    Bundle {
        /// Project folder (contains project.toml).
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Output file; `-` for stdout.
        #[arg(short, long, default_value = "-")]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Import { source, out, force, name } => import(&source, &out, force, name),
        Cmd::Build { dir, plan, max_gap, deny_warnings } => build(&dir, plan, max_gap, deny_warnings),
        Cmd::Bundle { dir, out } => bundle(&dir, &out),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn import(source: &Path, out: &Path, force: bool, name: Option<String>) -> Result<ExitCode> {
    if out.join(moviclone_model::io::PROJECT_FILE).exists() && !force {
        bail!("{} already contains a project; pass --force to overwrite it", out.display());
    }
    let mut imported = moviclone_import_movicon::import(source)?;
    if let Some(name) = name {
        imported.project.meta.name = name;
    }
    save_project(out, &imported.project).with_context(|| format!("writing {}", out.display()))?;

    let report = imported.report.to_markdown(&imported.project.meta.name);
    std::fs::write(out.join("IMPORT-REPORT.md"), report)?;
    if !imported.report.secrets.is_empty() {
        std::fs::write(out.join("secrets.example.toml"), imported.report.secrets_example())?;
    }

    println!("Imported {} into {}", imported.project.meta.name, out.display());
    for (k, v) in &imported.report.summary {
        println!("  {k}: {v}");
    }
    println!("  Report: {}", out.join("IMPORT-REPORT.md").display());
    println!("\nNext: movi build {}", out.display());
    Ok(ExitCode::SUCCESS)
}

fn bundle(dir: &Path, out: &Path) -> Result<ExitCode> {
    let (project, _) = load_project(dir)?;
    let json = serde_json::to_string_pretty(&project)?;
    if out == Path::new("-") {
        println!("{json}");
    } else {
        std::fs::write(out, json).with_context(|| format!("writing {}", out.display()))?;
        eprintln!("Wrote {}", out.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn build(dir: &Path, show_plan: bool, max_gap: u16, deny_warnings: bool) -> Result<ExitCode> {
    let (project, sources) = match load_project(dir) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error[E000] {e}");
            return Ok(ExitCode::FAILURE);
        }
    };
    let diags = check(&project, &sources);
    for d in &diags {
        let file = d.file.strip_prefix(dir).unwrap_or(&d.file);
        println!("{}[{}] {}: {}: {}", d.severity, d.code, file.display(), d.item, d.message);
    }
    let count = |s| diags.iter().filter(|d| d.severity == s).count();
    let (errors, warnings, notes) = (count(Severity::Error), count(Severity::Warning), count(Severity::Note));

    let jobs = plan_polls(&project, max_gap);
    if !diags.is_empty() {
        println!();
    }
    println!("Poll plan (max gap {max_gap}):");
    for d in &project.drivers {
        let DriverConfig::ModbusTcp(m) = &d.config;
        let mine: Vec<_> = jobs.iter().filter(|j| j.driver == d.name).collect();
        let tags: usize = mine.iter().map(|j| j.tags.len()).sum();
        println!("  {} ({}:{}, every {} ms): {} requests for {} tags", d.name, m.host, m.port, m.poll_ms, mine.len(), tags);
        if show_plan {
            for j in mine {
                println!("    {:<14} {:<16} {:>5} +{:<3} {}", j.station, j.area.to_string(), j.start, j.count, j.tags.join(", "));
            }
        }
    }

    println!(
        "\n{}: {} tags, {} screens, {errors} errors, {warnings} warnings, {notes} notes",
        project.meta.name,
        project.tags.len(),
        project.screens.len()
    );
    let failed = errors > 0 || (deny_warnings && warnings > 0);
    println!("{}", if failed { "Build failed." } else { "Build passed." });
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}
