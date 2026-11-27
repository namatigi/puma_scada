//! `movi`: the Moviclone command line.
//!
//! ```text
//! movi import <movicon project> -o <folder>   convert a Movicon.NExT project
//! movi build  <folder>                        validate a project and plan polling
//! movi bundle <folder> -o project.json        the project as one JSON document
//! movi users  list|add|passwd|role|remove     studio sign-ins (<data>/users.toml)
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
    /// Manage studio users (who may sign in to the studio, and their roles).
    Users {
        /// The runtime's data folder (its --data; default `<project>/data`).
        #[arg(long, default_value = "data", global = true)]
        data: PathBuf,
        #[command(subcommand)]
        action: UsersCmd,
    },
}

#[derive(Subcommand)]
enum UsersCmd {
    /// List users and roles.
    List,
    /// Add a user; asks for the password.
    Add {
        name: String,
        /// viewer, engineer or admin.
        #[arg(long, default_value = "engineer")]
        role: String,
        /// Read the password from the first line of stdin instead of asking.
        #[arg(long)]
        password_stdin: bool,
    },
    /// Set a user's password (e.g. a forgotten administrator password).
    Passwd {
        name: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Change a user's role.
    Role { name: String, role: String },
    /// Remove a user.
    Remove { name: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Import { source, out, force, name } => import(&source, &out, force, name),
        Cmd::Build { dir, plan, max_gap, deny_warnings } => build(&dir, plan, max_gap, deny_warnings),
        Cmd::Bundle { dir, out } => bundle(&dir, &out),
        Cmd::Users { data, action } => users_cmd(&data, action),
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

// ---------------------------------------------------------------- users

fn read_new_password(from_stdin: bool) -> Result<String> {
    use moviclone_studio::users::check_password;
    let password = if from_stdin {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).context("cannot read the password from stdin")?;
        line.trim_end_matches(['\r', '\n']).to_string()
    } else {
        let first = rpassword::prompt_password("New password: ").context("cannot read the password")?;
        let again = rpassword::prompt_password("Repeat it: ").context("cannot read the password")?;
        if first != again {
            bail!("the passwords differ");
        }
        first
    };
    check_password(&password).map_err(anyhow::Error::msg)?;
    Ok(password)
}

fn users_cmd(data: &Path, action: UsersCmd) -> Result<ExitCode> {
    use moviclone_studio::users::{self, Role, User};
    let file = users::users_file(data);
    let mut list = users::load(&file).map_err(anyhow::Error::msg)?;
    let role_of = |r: &str| Role::parse(r).with_context(|| format!("unknown role {r:?} (viewer, engineer or admin)"));
    let find = |list: &[User], name: &str| users::find(list, name).with_context(|| format!("no user {name:?} in {}", file.display()));
    match action {
        UsersCmd::List => {
            if list.is_empty() {
                println!("no users in {} (the studio offers to create the first administrator on this machine)", file.display());
            }
            for u in &list {
                println!("{:<24} {}", u.name, u.role);
            }
            return Ok(ExitCode::SUCCESS);
        }
        UsersCmd::Add { name, role, password_stdin } => {
            users::check_name(&name).map_err(anyhow::Error::msg)?;
            let role = role_of(&role)?;
            if users::find(&list, &name).is_some() {
                bail!("user {name:?} already exists; use `movi users passwd` or `movi users role`");
            }
            let hash = users::hash_password(&read_new_password(password_stdin)?).map_err(anyhow::Error::msg)?;
            list.push(User { name: name.clone(), role, password: hash });
            println!("added {name} ({role})");
        }
        UsersCmd::Passwd { name, password_stdin } => {
            let i = find(&list, &name)?;
            list[i].password = users::hash_password(&read_new_password(password_stdin)?).map_err(anyhow::Error::msg)?;
            println!("password of {} changed (their open studio sessions stay signed in until the runtime restarts or they sign out)", list[i].name);
        }
        UsersCmd::Role { name, role } => {
            let i = find(&list, &name)?;
            let role = role_of(&role)?;
            if list[i].role == Role::Admin && role != Role::Admin && users::admins(&list) == 1 {
                bail!("{} is the only administrator", list[i].name);
            }
            list[i].role = role;
            println!("{} is now {role}", list[i].name);
        }
        UsersCmd::Remove { name } => {
            let i = find(&list, &name)?;
            let u = list.remove(i);
            println!("removed {}", u.name);
        }
    }
    users::save(&file, &list).map_err(anyhow::Error::msg)?;
    Ok(ExitCode::SUCCESS)
}
