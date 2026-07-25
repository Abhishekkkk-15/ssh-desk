//! ssh-desk — remote OS shell in the terminal.

mod app;
mod apps;
mod diagnostics;
mod files;
mod hit;
mod hostform;
mod term;
mod transfers;
mod ui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let log_file = std::fs::File::create("ssh-desk.log").unwrap_or_else(|_| {
        std::fs::File::open("/dev/null").unwrap()
    });
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(log_file)
        .init();

    app::run().await
}
