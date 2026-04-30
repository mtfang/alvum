//! CLI entry point for alvum.
//!
use anyhow::Result;
use clap::Parser;

mod capture;
mod cli;
mod config_cmd;
mod config_doc;
mod extensions;
mod extract;
mod models;
mod profile;
mod providers;
mod tail;

#[tokio::main]
async fn main() -> Result<()> {
    // Send tracing to stderr so stdout stays clean for structured
    // output. `alvum providers list` / `test` print JSON for the tray
    // popover to parse — any ANSI-colored INFO log on the same stream
    // breaks the parse.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    cli::run(cli::Cli::parse()).await
}
