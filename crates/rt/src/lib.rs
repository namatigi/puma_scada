//! moviclone-rt: loads a project and runs drivers, historian, alarm engine,
//! studio API and the HTTP/WebSocket gateway in one process.
//!
//! The process is layered so a studio save can be applied without a restart:
//!
//! ```text
//! listener ── RouterSwitch ──► gateway of the current core (drivers + TagStore)
//!                                 ├─ /api/studio/…   studio routes (stable)
//!                                 └─ /api/history…, /api/alarms…  ──► RouterSwitch ──► services
//! ```
//!
//! - **core** ([`core`]): tag store, Modbus drivers, optional simulator and the
//!   gateway router built on them. Rebuilt only when tags or drivers change.
//! - **services** ([`services`]): historian and alarm engine on the core's tag
//!   store. Rebuilt when alarms, the data config, tags or drivers change.
//! - Everything else (screens, project settings) is a plain project swap.
//!
//! See [`reload`] for the exact rules. Embedders and tests use [`start`]; the
//! binary in `main.rs` adds Ctrl+C handling.

pub mod core;
pub mod reload;
pub mod runtime;
pub mod services;
pub mod switch;

pub use runtime::{start, Runtime};

use clap::Parser;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Parser)]
#[command(name = "moviclone-rt", version, about = "Moviclone runtime: drivers, historian, alarms, gateway, HMI and studio")]
pub struct Args {
    /// Project folder (contains project.toml).
    pub project: PathBuf,
    /// Address the HTTP/WebSocket gateway listens on (port 0 picks a free port).
    #[arg(long, default_value = "127.0.0.1:8080")]
    pub listen: SocketAddr,
    /// Run the built-in Modbus simulator and connect every modbus-tcp driver to it.
    #[arg(long)]
    pub simulate: bool,
    /// Folder with hmi/, studio/ and shared/.
    #[arg(long, default_value = "web")]
    pub web: PathBuf,
    /// Where the historian database and the alarm journal are kept
    /// (default: `<project>/data`).
    #[arg(long)]
    pub data: Option<PathBuf>,
    /// Start (and apply studio saves) even if the project check reports errors.
    #[arg(long)]
    pub allow_errors: bool,
    /// Mount the studio API (/api/studio/…); saves are applied to the running project.
    #[arg(long)]
    pub studio: bool,
}

impl Args {
    /// Arguments for `project` with every option at its default.
    pub fn new(project: impl Into<PathBuf>) -> Self {
        Self {
            project: project.into(),
            listen: SocketAddr::from(([127, 0, 0, 1], 8080)),
            simulate: false,
            web: PathBuf::from("web"),
            data: None,
            allow_errors: false,
            studio: false,
        }
    }

    /// The data folder: `--data`, or `data/` inside the project folder.
    pub fn data_dir(&self) -> PathBuf {
        self.data.clone().unwrap_or_else(|| default_data_dir(&self.project))
    }
}

pub fn default_data_dir(project: &Path) -> PathBuf {
    project.join("data")
}
