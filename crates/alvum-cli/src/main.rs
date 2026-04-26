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
                match alvum_connector_claude::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "codex" => {
                match alvum_connector_codex::from_config(&cfg.settings) {
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
enum ProviderAction {
    /// Output JSON describing every provider's availability + active
    /// status. Cheap (no network) — only checks for binary on PATH and
    /// env-var / config-file presence.
    List,

    /// Make a tiny `Reply with OK` call against a provider and report
    /// whether auth + connectivity work end-to-end. When --model is
    /// omitted, picks a sensible default per provider (Anthropic
    /// models for claude/anthropic-api/bedrock, OpenAI gpt-5 for
    /// codex-cli, llama3.2 for ollama).
    Test {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: Option<String>,
    },

    /// Set the [pipeline] provider config key (same effect as
    /// `alvum config-set pipeline.provider <value>`, but accepts the
    /// shorter alias names like "claude" / "codex").
    SetActive {
        provider: String,
    },
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

    /// LLM provider status + test commands. Designed to be called from
    /// the menu-bar popover for the Provider settings section, but
    /// fine for direct CLI use too.
    Providers {
        #[command(subcommand)]
        action: ProviderAction,
    },

    /// Stream the live pipeline event log. Reads
    /// `~/.alvum/runtime/pipeline.events` (or `$ALVUM_PIPELINE_EVENTS_FILE`)
    /// and pretty-prints each event. Companion to the tray popover live
    /// panel; useful for SSH/terminal debugging without the GUI.
    Tail {
        /// Keep watching the file and print new events as they arrive.
        /// Without `--follow` the command prints what's there now and exits.
        #[arg(short, long)]
        follow: bool,

        /// Only show events whose `kind` matches this substring (e.g.
        /// `llm_call`, `stage`, `warning`). Without `--filter` everything
        /// is shown.
        #[arg(short = 'k', long)]
        filter: Option<String>,
    },

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

        /// LLM provider. Options:
        ///   auto         — pick the first authenticated backend (default)
        ///   claude-cli   — Claude Code subscription (`claude login`)
        ///   codex-cli    — Codex / ChatGPT subscription (`codex login`)
        ///   anthropic-api — direct Anthropic API (needs ANTHROPIC_API_KEY)
        ///   bedrock      — Anthropic-on-Bedrock (needs AWS credentials)
        ///   ollama       — local Ollama
        #[arg(long, default_value = "auto")]
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

        /// Re-process every DataRef even if it appears in
        /// `<output>/processed.jsonl`. Default: skip already-processed refs.
        #[arg(long)]
        no_skip_processed: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Send tracing to stderr so stdout stays clean for structured
    // output. `alvum providers list` / `test` print JSON for the tray
    // popover to parse — any ANSI-colored INFO log on the same stream
    // breaks the parse.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
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
        Commands::Providers { action } => cmd_providers(action).await,
        Commands::Tail { follow, filter } => cmd_tail(follow, filter).await,
        Commands::Extract { source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold, vision, resume, no_skip_processed } => {
            cmd_extract(source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold, vision, resume, no_skip_processed).await
        }
    }
}

async fn cmd_providers(action: ProviderAction) -> Result<()> {
    match action {
        ProviderAction::List => cmd_providers_list(),
        ProviderAction::Test { provider, model } => {
            let model = model.unwrap_or_else(|| default_model_for(&provider).into());
            cmd_providers_test(&provider, &model).await
        }
        ProviderAction::SetActive { provider } => cmd_providers_set_active(&provider),
    }
}

/// Sensible default model per provider for a Reply-with-OK probe. Each
/// CLI / API rejects models from the wrong family (sending an Anthropic
/// model to Codex returns a 400 invalid_request_error), so the test
/// command can't share a single default across providers.
///
/// Empty string is a valid return — for codex-cli we want to defer
/// entirely to the user's ~/.codex/config.toml default, since model
/// names there can be arbitrary (gpt-5, gpt-5.5, o3, etc.) and we
/// can't pick one that's guaranteed to exist.
fn default_model_for(provider: &str) -> &'static str {
    match provider {
        "codex" | "codex-cli" => "",   // let codex pick from its config
        "ollama" => "llama3.2",
        "bedrock" => "anthropic.claude-sonnet-4-20250514-v1:0",
        // claude-cli / anthropic-api / cli / api / auto / unknown
        _ => "claude-sonnet-4-6",
    }
}

/// Each entry the popover renders. `available` reflects the cheap
/// detection check; an entry that's `available` may still fail at call
/// time if the user hasn't actually completed `claude login` etc. —
/// the Test action proves end-to-end auth.
#[derive(serde::Serialize)]
struct ProviderInfo {
    name: &'static str,
    available: bool,
    auth_hint: &'static str,
    active: bool,
}

#[derive(serde::Serialize)]
struct ProviderListReport {
    /// Whatever's recorded in `[pipeline] provider` — may be "auto" or a
    /// concrete name. The popover shows this as the user's stated
    /// preference; if it's "auto", `auto_resolved` tells you which
    /// concrete provider auto would pick today.
    configured: String,
    /// Concrete provider auto would currently select. None if none
    /// authenticate.
    auto_resolved: Option<&'static str>,
    providers: Vec<ProviderInfo>,
}

fn cmd_providers_list() -> Result<()> {
    let configured_raw = alvum_core::config::AlvumConfig::load()
        .map(|c| c.pipeline.provider)
        .unwrap_or_else(|_| "auto".into());
    // Legacy aliases — old install.sh wrote "cli", we now use canonical
    // provider names everywhere. Treat the legacy short form as equivalent
    // for "is this provider currently active" comparisons.
    let configured = match configured_raw.as_str() {
        "cli" => "claude-cli".to_string(),
        "codex" => "codex-cli".to_string(),
        "api" => "anthropic-api".to_string(),
        _ => configured_raw,
    };

    let entries = provider_entries();
    let auto_resolved = entries.iter().find(|p| p.available).map(|p| p.name);

    let providers: Vec<ProviderInfo> = entries
        .into_iter()
        .map(|p| ProviderInfo {
            name: p.name,
            available: p.available,
            auth_hint: p.auth_hint,
            active: configured == p.name
                || (configured == "auto" && Some(p.name) == auto_resolved),
        })
        .collect();

    let report = ProviderListReport { configured, auto_resolved, providers };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

struct ProviderEntry {
    name: &'static str,
    available: bool,
    auth_hint: &'static str,
}

fn provider_entries() -> Vec<ProviderEntry> {
    vec![
        ProviderEntry {
            name: "claude-cli",
            available: cli_binary_on_path("claude"),
            auth_hint: "subscription via `claude login`",
        },
        ProviderEntry {
            name: "codex-cli",
            available: cli_binary_on_path("codex"),
            auth_hint: "subscription via `codex login`",
        },
        ProviderEntry {
            name: "anthropic-api",
            available: std::env::var("ANTHROPIC_API_KEY").is_ok(),
            auth_hint: "set ANTHROPIC_API_KEY env var",
        },
        ProviderEntry {
            name: "bedrock",
            available: aws_credentials_present(),
            auth_hint: "set AWS_PROFILE or AWS_ACCESS_KEY_ID",
        },
        ProviderEntry {
            name: "ollama",
            available: cli_binary_on_path("ollama"),
            auth_hint: "install from ollama.ai and `ollama run <model>`",
        },
    ]
}

fn cli_binary_on_path(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn aws_credentials_present() -> bool {
    std::env::var("AWS_PROFILE").is_ok()
        || std::env::var("AWS_ACCESS_KEY_ID").is_ok()
        || std::env::var("AWS_SESSION_TOKEN").is_ok()
        || dirs::home_dir()
            .map(|h| h.join(".aws/credentials").exists() || h.join(".aws/config").exists())
            .unwrap_or(false)
}

#[derive(serde::Serialize)]
struct ProviderTestReport {
    provider: String,
    ok: bool,
    elapsed_ms: u128,
    response_preview: Option<String>,
    error: Option<String>,
}

async fn cmd_providers_test(provider_name: &str, model: &str) -> Result<()> {
    // Tiny prompt. The expected response is "OK" — anything containing
    // it counts as success. Some providers may include leading
    // whitespace or quote marks, hence the contains() check.
    const TEST_SYSTEM: &str = "You are a connectivity probe. Reply with the exact word OK and nothing else.";
    const TEST_USER: &str = "ping";
    let started = std::time::Instant::now();

    let report = match alvum_pipeline::llm::create_provider_async(provider_name, model).await {
        Ok(provider) => match provider.complete(TEST_SYSTEM, TEST_USER).await {
            Ok(text) => {
                let preview: String = text.chars().take(80).collect();
                let ok = text.to_uppercase().contains("OK");
                ProviderTestReport {
                    provider: provider_name.into(),
                    ok,
                    elapsed_ms: started.elapsed().as_millis(),
                    response_preview: Some(preview),
                    error: if ok {
                        None
                    } else {
                        Some(format!("response did not contain 'OK': {text:?}"))
                    },
                }
            }
            Err(e) => ProviderTestReport {
                provider: provider_name.into(),
                ok: false,
                elapsed_ms: started.elapsed().as_millis(),
                response_preview: None,
                error: Some(format!("{e:#}")),
            },
        },
        Err(e) => ProviderTestReport {
            provider: provider_name.into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("provider construction failed: {e:#}")),
        },
    };

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn cmd_providers_set_active(provider: &str) -> Result<()> {
    // Accept short aliases — "claude" → "claude-cli", "codex" → "codex-cli".
    let normalized = match provider {
        "claude" => "claude-cli",
        "codex" => "codex-cli",
        "api" => "anthropic-api",
        other => other,
    };
    cmd_config_set("pipeline.provider", normalized)?;
    println!("active provider set to {normalized}");
    Ok(())
}

async fn cmd_capture(
    capture_dir: Option<PathBuf>,
    only: Option<String>,
    disable: Option<String>,
) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;

    // capture_dir is the ROOT that holds per-day subdirs. Sources resolve
    // today's dir at each flush so the process rolls over local midnight
    // without needing a restart.
    let capture_dir = capture_dir.unwrap_or_else(|| PathBuf::from("capture"));

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
    no_skip_processed: bool,
) -> Result<()> {
    let capture_dir = capture_dir.context("--capture-dir required")?;

    // Provider built from flags — convert Box to Arc for sharing across connectors.
    // Use the async builder so `auto` and `bedrock` work; non-async providers
    // fall through unchanged.
    let provider_box = alvum_pipeline::llm::create_provider_async(&provider_name, &model).await?;
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
        no_skip_processed,
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

// === alvum tail =======================================================
//
// Streams `~/.alvum/runtime/pipeline.events` to stdout for live-debug
// during a briefing run. Reads existing content, optionally tails for
// new lines, optionally filters by event `kind` substring.
//
// Same file the tray popover live panel reads; the two are independent
// consumers of the JSONL append-only stream.

fn pipeline_events_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("ALVUM_PIPELINE_EVENTS_FILE") {
        return Ok(p.into());
    }
    let home = dirs::home_dir().context("could not resolve $HOME for pipeline events file")?;
    Ok(home.join(".alvum/runtime/pipeline.events"))
}

async fn cmd_tail(follow: bool, filter: Option<String>) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncSeekExt, BufReader};

    let path = pipeline_events_path()?;
    if !path.exists() {
        // Touch the parent so a freshly-installed system tails cleanly
        // before the first run has created the file.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        eprintln!(
            "(no events file yet at {} — start a briefing to populate it)",
            path.display()
        );
        if !follow {
            return Ok(());
        }
    }

    // Read whatever already exists, then optionally watch for appends.
    // Open with tokio so the loop integrates cleanly with `--follow`.
    let mut file = if path.exists() {
        Some(tokio::fs::File::open(&path).await.with_context(|| {
            format!("failed to open {}", path.display())
        })?)
    } else {
        None
    };

    if let Some(f) = file.as_mut() {
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            print_event_line(&line, filter.as_deref());
        }
    }

    if !follow {
        return Ok(());
    }

    // Tail loop: poll the file every 250 ms. The events file is
    // truncated at run-start (init()), so we also re-open if the size
    // shrinks below our cursor.
    let mut cursor: u64 = match file.as_mut() {
        Some(f) => f.seek(std::io::SeekFrom::Current(0)).await?,
        None => 0,
    };
    drop(file);

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if !path.exists() {
            cursor = 0;
            continue;
        }
        let metadata = tokio::fs::metadata(&path).await?;
        let size = metadata.len();
        if size < cursor {
            // File was truncated (new run started). Reset.
            cursor = 0;
        }
        if size == cursor {
            continue;
        }
        let mut f = tokio::fs::File::open(&path).await?;
        f.seek(std::io::SeekFrom::Start(cursor)).await?;
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                break;
            }
            print_event_line(&line, filter.as_deref());
            cursor += n as u64;
        }
    }
}

/// Pretty-print one JSONL line. Falls back to raw output on parse
/// failure — better to see something than nothing while debugging.
fn print_event_line(line: &str, filter: Option<&str>) {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return;
    }
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            println!("{trimmed}");
            return;
        }
    };
    let kind = value.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(f) = filter
        && !kind.contains(f)
    {
        return;
    }
    let ts = value
        .get("ts")
        .and_then(|v| v.as_i64())
        .map(format_ts)
        .unwrap_or_else(|| "??:??:??.???".into());

    let detail = format_event_detail(kind, &value);
    println!("[{ts}] {kind:<18} {detail}");
}

fn format_ts(ts_millis: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ts_millis)
        .single()
        .map(|dt| dt.format("%H:%M:%S%.3f").to_string())
        .unwrap_or_else(|| "??:??:??.???".into())
}

/// Render the event-specific fields. Stage/LLM events get a compact
/// per-shape summary; everything else falls back to the JSON tail.
fn format_event_detail(kind: &str, value: &serde_json::Value) -> String {
    match kind {
        "stage_enter" => str_field(value, "stage").to_string(),
        "stage_exit" => format!(
            "stage={} elapsed_ms={} ok={} extras={}",
            str_field(value, "stage"),
            value.get("elapsed_ms").map(|v| v.to_string()).unwrap_or_default(),
            value.get("ok").map(|v| v.to_string()).unwrap_or_default(),
            value.get("extras").map(|v| v.to_string()).unwrap_or_default(),
        ),
        "input_inventory" => format!(
            "{}/{} ref_count={}",
            str_field(value, "connector"),
            str_field(value, "source"),
            value.get("ref_count").map(|v| v.to_string()).unwrap_or_default(),
        ),
        "llm_call_start" => format!(
            "call_site={} prompt_chars={}",
            str_field(value, "call_site"),
            value.get("prompt_chars").map(|v| v.to_string()).unwrap_or_default(),
        ),
        "llm_call_end" => format!(
            "call_site={} latency_ms={} response_chars={} attempts={} ok={}",
            str_field(value, "call_site"),
            value.get("latency_ms").map(|v| v.to_string()).unwrap_or_default(),
            value.get("response_chars").map(|v| v.to_string()).unwrap_or_default(),
            value.get("attempts").map(|v| v.to_string()).unwrap_or_default(),
            value.get("ok").map(|v| v.to_string()).unwrap_or_default(),
        ),
        "llm_parse_failed" => format!(
            "call_site={} preview={:?}",
            str_field(value, "call_site"),
            str_field(value, "preview"),
        ),
        "input_filtered" => format!(
            "processor={} kept={} dropped={} reasons={}",
            str_field(value, "processor"),
            value.get("kept").map(|v| v.to_string()).unwrap_or_default(),
            value.get("dropped").map(|v| v.to_string()).unwrap_or_default(),
            value.get("reasons").map(|v| v.to_string()).unwrap_or_default(),
        ),
        "warning" | "error" => format!(
            "{}: {}",
            str_field(value, "source"),
            str_field(value, "message"),
        ),
        _ => value.to_string(),
    }
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("")
}
