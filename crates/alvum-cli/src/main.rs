//! CLI entry point for alvum.
//!
//! Subcommands:
//! - `alvum capture` — start capture sources (audio + screen)
//! - `alvum devices` — list available audio devices
//! - `alvum extract` — extract decisions from data sources
//! - `alvum config-init` — initialize a default config file
//! - `alvum config-show` — show current configuration
//! - `alvum connectors` — list connectors and their status

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::{info, warn};

fn connectors_from_config(
    config: &alvum_core::config::AlvumConfig,
    provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider>,
) -> Vec<Box<dyn alvum_core::connector::Connector>> {
    let mut connectors: Vec<Box<dyn alvum_core::connector::Connector>> = Vec::new();

    for (name, cfg) in &config.connectors {
        if !cfg.enabled {
            continue;
        }

        match name.as_str() {
            "audio" => {
                match alvum_connector_audio::AudioConnector::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "screen" => {
                match alvum_connector_screen::ScreenConnector::from_config(&cfg.settings, Some(provider.clone())) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "claude-code" => {
                match alvum_connector_claude::ClaudeCodeConnector::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            other => {
                tracing::warn!(name = %other, "unknown connector type, skipping");
            }
        }
    }

    connectors
}

#[derive(Parser)]
#[command(name = "alvum", about = "Life decision tracking and alignment engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start capture sources (audio + screen). Reads [capture.*] from config.
    Capture {
        /// Capture directory (default: ./capture/<today>)
        #[arg(long)]
        capture_dir: Option<PathBuf>,
        /// Only start these sources (comma-separated: audio-mic,audio-system,screen)
        #[arg(long)]
        only: Option<String>,
        /// Disable these sources (comma-separated)
        #[arg(long)]
        disable: Option<String>,
    },

    /// List available audio devices
    Devices,

    /// Initialize a default config file
    #[command(name = "config-init")]
    ConfigInit,

    /// Show current configuration
    #[command(name = "config-show")]
    ConfigShow,

    /// Set a config value (e.g., alvum config-set capture.screen.enabled false)
    #[command(name = "config-set")]
    ConfigSet {
        /// Dotted key path (e.g., capture.audio-mic.device, processors.screen.vision)
        key: String,
        /// Value to set
        value: String,
    },

    /// List connectors and their status
    Connectors,

    /// Extract decisions from a data source
    Extract {
        /// Data source: "claude" or "audio". Omit for cross-source threading.
        #[arg(long)]
        source: Option<String>,

        /// Path to a Claude Code JSONL session file (for --source claude)
        #[arg(long)]
        session: Option<PathBuf>,

        /// Output directory for decisions.jsonl and briefing.md
        #[arg(long, default_value = ".")]
        output: PathBuf,

        /// LLM provider: cli, api, ollama
        #[arg(long, default_value = "cli")]
        provider: String,

        /// Model to use
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,

        /// Only include observations before this timestamp (ISO 8601)
        #[arg(long)]
        before: Option<String>,

        /// Capture directory for audio files (for --source audio)
        #[arg(long)]
        capture_dir: Option<PathBuf>,

        /// Path to Whisper model file (reads from [processors.audio] config if omitted)
        #[arg(long)]
        whisper_model: Option<PathBuf>,

        /// Minimum relevance score for threads sent to decision extraction (0.0-1.0)
        #[arg(long, default_value = "0.5")]
        relevance_threshold: f32,

        /// Vision processing mode: local, api, ocr, off (reads from [processors.screen] config if omitted)
        #[arg(long)]
        vision: Option<String>,

        /// Resume from existing per-stage checkpoint files in --output. Each stage
        /// whose output file already exists is skipped (loaded from disk). Turns a
        /// 10-minute recovery after a transient LLM flake into ~2 minutes. Idempotent
        /// on a fresh output dir.
        #[arg(long)]
        resume: bool,
    },
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

    match cli.command {
        Commands::Capture { capture_dir, only, disable } => {
            cmd_capture(capture_dir, only, disable).await
        }
        Commands::Devices => {
            cmd_devices()
        }
        Commands::ConfigInit => cmd_config_init(),
        Commands::ConfigShow => cmd_config_show(),
        Commands::ConfigSet { key, value } => cmd_config_set(&key, &value),
        Commands::Connectors => cmd_connectors(),
        Commands::Extract { source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold, vision, resume } => {
            cmd_extract(source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold, vision, resume).await
        }
    }
}

async fn cmd_capture(
    capture_dir: Option<PathBuf>,
    only: Option<String>,
    disable: Option<String>,
) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let capture_dir = capture_dir
        .unwrap_or_else(|| PathBuf::from("capture").join(&today));

    // Get enabled sources from config
    let mut sources: Vec<(&str, &alvum_core::config::CaptureSourceConfig)> =
        config.enabled_capture_sources();

    // Apply --only filter
    if let Some(ref only_str) = only {
        let only_set: Vec<&str> = only_str.split(',').map(|s| s.trim()).collect();
        sources.retain(|(name, _)| only_set.contains(name));
    }

    // Apply --disable filter
    if let Some(ref disable_str) = disable {
        let disable_set: Vec<&str> = disable_str.split(',').map(|s| s.trim()).collect();
        sources.retain(|(name, _)| !disable_set.contains(name));
    }

    if sources.is_empty() {
        println!("No capture sources enabled. Check config or --only/--disable flags.");
        return Ok(());
    }

    // Create source implementations
    let mut source_impls: Vec<Box<dyn alvum_core::capture::CaptureSource>> = Vec::new();
    for (name, cfg) in &sources {
        match create_source(name, cfg) {
            Ok(src) => {
                info!(source = name, "created capture source");
                source_impls.push(src);
            }
            Err(e) => {
                warn!(source = name, error = %e, "failed to create capture source, skipping");
            }
        }
    }

    if source_impls.is_empty() {
        println!("No capture sources could be initialized.");
        return Ok(());
    }

    // Shared shutdown channel
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    // Print status
    let source_names: Vec<&str> = source_impls.iter().map(|s| s.name()).collect();
    println!("Capturing: {} — Press Ctrl-C to stop.", source_names.join(", "));

    // Spawn all sources
    let mut handles = Vec::new();
    for source in source_impls {
        let dir = capture_dir.clone();
        let rx = shutdown_tx.subscribe();
        handles.push(tokio::spawn(async move {
            if let Err(e) = source.run(&dir, rx).await {
                tracing::error!(source = source.name(), error = %e, "capture source failed");
            }
        }));
    }

    // Wait for Ctrl-C
    tokio::signal::ctrl_c().await?;
    println!("\nStopping...");

    // Send shutdown signal
    let _ = shutdown_tx.send(true);

    // Wait for all sources to stop (with timeout)
    for handle in handles {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            handle,
        ).await;
    }

    println!("Done.");
    Ok(())
}

fn create_source(
    name: &str,
    config: &alvum_core::config::CaptureSourceConfig,
) -> anyhow::Result<Box<dyn alvum_core::capture::CaptureSource>> {
    match name {
        "audio-mic" => Ok(Box::new(
            alvum_capture_audio::source::AudioMicSource::from_config(config)
        )),
        "audio-system" => Ok(Box::new(
            alvum_capture_audio::source::AudioSystemSource::from_config(config)
        )),
        "screen" => Ok(Box::new(
            alvum_capture_screen::source::ScreenSource::from_config(&config.settings)
        )),
        other => anyhow::bail!("unknown capture source: {other}"),
    }
}

fn cmd_config_init() -> Result<()> {
    let path = alvum_core::config::config_path();
    if path.exists() {
        println!("Config already exists: {}", path.display());
        println!("Edit it directly or delete it to re-initialize.");
        return Ok(());
    }
    let config = alvum_core::config::AlvumConfig::default();
    config.save()?;
    println!("Created default config: {}", path.display());
    Ok(())
}

fn cmd_config_show() -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;
    let toml_str = toml::to_string_pretty(&config)?;
    println!("{toml_str}");
    Ok(())
}

fn cmd_config_set(key: &str, value: &str) -> Result<()> {
    let config_path = alvum_core::config::config_path();

    // Load existing config as raw TOML table (preserves structure)
    let mut doc: toml::Table = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse().context("failed to parse config")?
    } else {
        // Start from defaults
        let config = alvum_core::config::AlvumConfig::default();
        let toml_str = toml::to_string(&config)?;
        toml_str.parse().context("failed to serialize defaults")?
    };

    // Parse the dotted key path (e.g., "capture.audio-mic.device")
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() < 2 {
        bail!("key must be dotted path (e.g., capture.screen.enabled)");
    }

    // Navigate to the parent table, creating intermediate tables as needed
    let mut current = &mut doc;
    for part in &parts[..parts.len() - 1] {
        current = current
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("{part} is not a table"))?;
    }

    let leaf = parts.last().unwrap();

    // Infer type from value string
    let toml_value = if value == "true" {
        toml::Value::Boolean(true)
    } else if value == "false" {
        toml::Value::Boolean(false)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if let Ok(f) = value.parse::<f64>() {
        toml::Value::Float(f)
    } else {
        toml::Value::String(value.to_string())
    };

    current.insert(leaf.to_string(), toml_value.clone());

    // Save
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, toml::to_string_pretty(&doc)?)?;

    println!("{key} = {toml_value}");
    println!("Saved to {}", config_path.display());
    Ok(())
}

fn cmd_connectors() -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;

    println!("Connectors:\n");
    for (name, connector) in &config.connectors {
        let status = if connector.enabled { "enabled" } else { "disabled" };
        println!("  {} ({})", name, status);
        for (key, value) in &connector.settings {
            println!("    {}: {}", key, value);
        }
    }

    if config.connectors.is_empty() {
        println!("  (none configured)");
    }

    println!("\nEdit config: {}", alvum_core::config::config_path().display());
    Ok(())
}

fn cmd_devices() -> Result<()> {
    let devices = alvum_capture_audio::devices::list_devices()?;

    println!("Audio devices:\n");
    for d in &devices {
        let caps = match (d.is_input, d.is_output) {
            (true, true) => "input + output",
            (true, false) => "input",
            (false, true) => "output",
            _ => "unknown",
        };
        println!("  {} ({})", d.name, caps);
    }

    if devices.is_empty() {
        println!("  (no devices found)");
    }

    println!("\nConfigure device in [capture.audio-mic] or [capture.audio-system] in config.");
    Ok(())
}

async fn cmd_extract(
    _source: Option<String>,       // legacy, ignored
    _session: Option<PathBuf>,     // legacy, ignored
    output: PathBuf,
    provider_name: String,
    model: String,
    _before: Option<String>,       // legacy
    capture_dir: Option<PathBuf>,
    _whisper_model: Option<PathBuf>, // now read from connector config
    relevance_threshold: f32,
    _vision: Option<String>,       // now read from connector config
    resume: bool,
) -> Result<()> {
    let capture_dir = capture_dir.context("--capture-dir required")?;

    // Provider built from flags — convert Box to Arc for sharing across connectors
    let provider_box = alvum_pipeline::llm::create_provider(&provider_name, &model)?;
    let provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider> = provider_box.into();

    let config = alvum_core::config::AlvumConfig::load()?;
    let connectors = connectors_from_config(&config, provider.clone());

    if connectors.is_empty() {
        println!("No connectors enabled. Check config.");
        return Ok(());
    }

    let names: Vec<&str> = connectors.iter().map(|c| c.name()).collect();
    println!("Running connectors: {}", names.join(", "));

    let extract_config = alvum_pipeline::extract::ExtractConfig {
        capture_dir,
        output_dir: output.clone(),
        relevance_threshold,
        resume,
    };

    let result = alvum_pipeline::extract::extract_and_pipeline(
        connectors,
        provider,
        extract_config,
    ).await?;

    println!("\nExtracted {} decisions from {} observations across {} threads.",
        result.result.decisions.len(),
        result.observations.len(),
        result.threading.threads.len(),
    );
    println!("\nOutput: {}", output.display());
    println!("\n{}", "=".repeat(60));
    println!("{}", result.result.briefing);

    Ok(())
}
