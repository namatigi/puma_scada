//! moviclone-rt: loads a project and runs drivers, historian, alarm engine,
//! studio API and the HTTP/WebSocket gateway in one process.
//!
//! ```text
//! moviclone-rt <project-dir> [--listen 127.0.0.1:8080] [--simulate] [--studio]
//!              [--web ./web] [--data <dir>] [--allow-errors]
//! ```
//!
//! - Refuses to start when `moviclone-check` reports errors, unless `--allow-errors`.
//! - `--simulate` starts the Modbus simulator on `127.0.0.1:<free port>` and points
//!   every modbus-tcp driver at it (the project files are not changed).
//! - `--studio` mounts the studio API; saves are applied to the running project
//!   (see `moviclone_rt::reload`).
//! - `--data` holds the historian database and the alarm journal (default
//!   `<project>/data`).
//! - Ctrl+C shuts down gracefully: WebSocket sessions close, the historian writes
//!   its last batch, the alarm engine its state, drivers disconnect.
//! - Logging via `RUST_LOG` (default `info,tokio_modbus=warn`).

use clap::Parser;
use moviclone_rt::Args;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tokio_modbus=warn")),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("moviclone-rt: cannot start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(async {
        let runtime = moviclone_rt::start(args).await?;
        runtime
            .run_until(async {
                if let Err(e) = tokio::signal::ctrl_c().await {
                    tracing::error!("cannot listen for Ctrl+C: {e}");
                    std::future::pending::<()>().await;
                }
            })
            .await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("moviclone-rt: {e:#}");
            ExitCode::FAILURE
        }
    }
}
