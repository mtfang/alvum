//! CLI entry point for alvum decision extraction.
//!
//! Parses a Claude Code session log, extracts decisions with causal links,
//! and generates a morning briefing. Supports multiple LLM backends via `--provider`.

use anyhow::{bail, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "alvum", about = "Extract decisions from conversation logs")]
struct Cli {
    /// Path to a Claude Code JSONL session file
    #[arg(long)]
    session: PathBuf,

    /// Output directory for decisions.jsonl and briefing.md
    #[arg(long, default_value = ".")]
    output: PathBuf,

    /// LLM provider: cli (Claude Code headless), api (Anthropic API), ollama (local)
    #[arg(long, default_value = "cli")]
    provider: String,

    /// Model to use (provider-specific)
    #[arg(long, default_value = "claude-sonnet-4-6")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    if !cli.session.exists() {
        bail!("session file not found: {}", cli.session.display());
    }

    std::fs::create_dir_all(&cli.output)?;
    let decisions_path = cli.output.join("decisions.jsonl");
    let briefing_path = cli.output.join("briefing.md");
    let extraction_path = cli.output.join("extraction.json");

    // Create LLM provider
    let provider = alvum_pipeline::llm::create_provider(&cli.provider, &cli.model)?;

    // Step 1: Parse Claude Code logs -> Observations
    info!("parsing session: {}", cli.session.display());
    let observations = alvum_connector_claude::parser::parse_session(&cli.session)?;
    info!(observations = observations.len(), "parsed observations");

    if observations.is_empty() {
        bail!("no observations found in session file");
    }

    // Step 2: Extract decisions from observations
    info!("extracting decisions...");
    let mut decisions =
        alvum_pipeline::distill::extract_decisions(provider.as_ref(), &observations).await?;
    info!(decisions = decisions.len(), "extracted");

    // Step 3: Analyze causal links
    info!("analyzing causal links...");
    alvum_pipeline::causal::link_decisions(provider.as_ref(), &mut decisions).await?;

    let link_count: usize = decisions.iter().map(|d| d.causes.len()).sum();
    info!(links = link_count, "linked");

    // Step 4: Generate briefing
    info!("generating briefing...");
    let briefing =
        alvum_pipeline::briefing::generate_briefing(provider.as_ref(), &decisions).await?;

    // Step 5: Write outputs
    for dec in &decisions {
        alvum_core::storage::append_jsonl(&decisions_path, dec)?;
    }
    info!(path = %decisions_path.display(), "wrote decisions");

    std::fs::write(&briefing_path, &briefing)?;
    info!(path = %briefing_path.display(), "wrote briefing");

    let result = alvum_core::decision::ExtractionResult {
        session_id: cli
            .session
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        decisions: decisions.clone(),
        briefing: briefing.clone(),
    };
    std::fs::write(&extraction_path, serde_json::to_string_pretty(&result)?)?;

    println!(
        "\n✓ Extracted {} decisions with {} causal links",
        decisions.len(),
        link_count
    );
    println!("  decisions: {}", decisions_path.display());
    println!("  briefing:  {}", briefing_path.display());

    println!("\n{}", "=".repeat(60));
    println!("{briefing}");

    Ok(())
}
