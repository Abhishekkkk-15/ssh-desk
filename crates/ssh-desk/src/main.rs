//! ssh-desk — remote OS shell in the terminal.

mod app;
mod apps;
mod diagnostics;
mod files;
mod files_prompt;
mod hit;
mod hostform;
mod session;
mod term;
mod theme;
mod transfers;
mod ui;

use std::fs::OpenOptions;
use std::process::ExitCode;

use anyhow::Result;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NAME: &str = env!("CARGO_PKG_NAME");

fn print_version() {
    println!("{NAME} {VERSION}");
}

fn print_help() {
    println!(
        "\
{NAME} {VERSION}
Remote OS shell in the terminal — tiled SSH desktop.

Usage:
  {NAME} [OPTIONS]

Options:
  -h, --help       Print help
  -V, --version    Print version

Config:  ~/.config/ssh-desk/
State:   ~/.local/state/ssh-desk/ (logs)
Session: ~/.config/ssh-desk/session.json (restored on start)
"
    );
}

fn setup_logging() -> Result<()> {
    let path = session::log_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log_file = OpenOptions::new().create(true).append(true).open(&path)?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(log_file)
        .init();
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => {
            print_help();
            return ExitCode::SUCCESS;
        }
        Some("-V" | "--version") => {
            print_version();
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("{NAME}: unknown argument '{other}'");
            eprintln!("Try '{NAME} --help' for more information.");
            return ExitCode::from(2);
        }
    }

    if let Err(e) = setup_logging() {
        eprintln!("ssh-desk: failed to set up logging ({e}); continuing without file log");
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .try_init();
    }

    match app::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ssh-desk error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
