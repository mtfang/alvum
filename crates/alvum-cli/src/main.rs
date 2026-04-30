//! CLI entry point for alvum.
//!
//! Subcommands:
//! - `alvum capture` — start capture sources (audio + screen)
//! - `alvum devices` — list available audio devices
//! - `alvum extract` — extract decisions from data sources
//! - `alvum config-init` — initialize a default config file
//! - `alvum config-show` — show current configuration
//! - `alvum connectors` — list connectors and their status

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tracing::{info, warn};

fn connectors_from_config(
    config: &alvum_core::config::AlvumConfig,
    provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider>,
    since: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Vec<Box<dyn alvum_core::connector::Connector>> {
    let mut connectors: Vec<Box<dyn alvum_core::connector::Connector>> = Vec::new();

    for (name, cfg) in &config.connectors {
        if !cfg.enabled {
            continue;
        }

        match name.as_str() {
            "audio" => {
                let settings = audio_connector_settings(config, &cfg.settings);
                match alvum_connector_audio::AudioConnector::from_config(&settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "screen" => {
                let settings = screen_connector_settings(config, &cfg.settings);
                match alvum_connector_screen::ScreenConnector::from_config(
                    &settings,
                    Some(provider.clone()),
                ) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "claude-code" => match alvum_connector_claude::from_config(&cfg.settings) {
                Ok(c) => connectors.push(Box::new(c.with_since(since).with_before(before))),
                Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
            },
            "codex" => match alvum_connector_codex::from_config(&cfg.settings) {
                Ok(c) => connectors.push(Box::new(c.with_since(since).with_before(before))),
                Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
            },
            other => {
                if cfg.settings.get("kind").and_then(|v| v.as_str()) != Some("external-http") {
                    tracing::warn!(name = %other, "unknown connector type, skipping");
                }
            }
        }
    }

    match alvum_connector_external::connectors_from_config(config) {
        Ok(external) => connectors.extend(external),
        Err(e) => tracing::warn!(error = %e, "failed to build external connectors"),
    }

    connectors
}

fn merged_processor_settings(
    config: &alvum_core::config::AlvumConfig,
    connector_settings: &HashMap<String, toml::Value>,
    processor_name: &str,
) -> HashMap<String, toml::Value> {
    let mut settings = connector_settings.clone();
    if let Some(processor) = config.processor(processor_name) {
        for (key, value) in &processor.settings {
            settings.insert(key.clone(), value.clone());
        }
    }
    settings
}

fn audio_connector_settings(
    config: &alvum_core::config::AlvumConfig,
    connector_settings: &HashMap<String, toml::Value>,
) -> HashMap<String, toml::Value> {
    let mut settings = merged_processor_settings(config, connector_settings, "audio");
    if let Some(mic) = config.capture_source("audio-mic") {
        settings.insert("mic".into(), toml::Value::Boolean(mic.enabled));
        if let Some(device) = mic.settings.get("device") {
            settings.insert("mic_device".into(), device.clone());
        }
        if let Some(duration) = mic.settings.get("chunk_duration_secs") {
            settings.insert("chunk_duration_secs".into(), duration.clone());
        }
    }
    if let Some(system) = config.capture_source("audio-system") {
        settings.insert("system".into(), toml::Value::Boolean(system.enabled));
    }
    settings
}

fn screen_connector_settings(
    config: &alvum_core::config::AlvumConfig,
    connector_settings: &HashMap<String, toml::Value>,
) -> HashMap<String, toml::Value> {
    let mut settings = merged_processor_settings(config, connector_settings, "screen");
    if let Some(screen) = config.capture_source("screen") {
        for (key, value) in &screen.settings {
            settings.insert(key.clone(), value.clone());
        }
    }
    settings
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

    /// Output JSON model options for a provider. Uses live provider
    /// catalogs when available, with safe defaults as fallback options.
    Models {
        #[arg(long)]
        provider: String,
    },

    /// Download a provider model through the provider's native tooling.
    /// v1 supports Ollama via `ollama pull <model>`.
    InstallModel {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        model: String,
    },

    /// First-run bootstrap: live-ping detected providers and enable only
    /// the providers that pass. Safe to call repeatedly; it skips after
    /// the first successful bootstrap unless --force is passed.
    Bootstrap {
        #[arg(long)]
        force: bool,
    },

    /// Save provider config from a JSON object on stdin. Secrets are
    /// written to macOS Keychain, not config.toml.
    Configure { provider: String },

    /// Set the [pipeline] provider config key (same effect as
    /// `alvum config-set pipeline.provider <value>`, but accepts the
    /// shorter alias names like "claude" / "codex").
    SetActive { provider: String },

    /// Add a built-in provider back to Alvum's managed provider list.
    Enable { provider: String },

    /// Remove a built-in provider from Alvum's managed provider list.
    /// This does not uninstall CLIs or delete credentials.
    Disable { provider: String },
}

#[derive(Subcommand)]
enum ProfileAction {
    /// Show the effective synthesis profile.
    Show {
        /// Emit machine-readable JSON instead of TOML.
        #[arg(long)]
        json: bool,
    },

    /// Set a simple profile value.
    Set {
        /// Dotted key path, e.g. advanced_instructions, writing.detail_level, writing.tone, or writing.outline.
        key: String,
        /// Value to assign.
        value: String,
    },

    /// Replace the profile from a JSON payload.
    Save {
        /// JSON-encoded SynthesisProfile.
        #[arg(long)]
        json: String,
    },

    /// List detected knowledge suggestions that can be tracked.
    Suggestions {
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },

    /// Promote a detected suggestion into tracked profile interests.
    Promote { id: String },

    /// Ignore a detected suggestion.
    Ignore { id: String },
}

#[derive(Subcommand)]
enum ExtensionAction {
    /// List installed extension packages.
    List {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Install an extension package from a local path, git:<url>, or npm:<package>.
    Install { source: String },

    /// Create a starter external HTTP extension package.
    Scaffold {
        path: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
    },

    /// Reinstall an extension package from its original source.
    Update { id: String },

    /// Remove an installed extension package.
    Remove { id: String },

    /// Enable an installed package and write a connector config entry.
    Enable {
        id: String,
        #[arg(long)]
        connector: Option<String>,
    },

    /// Disable a package and its connector config entries.
    Disable { id: String },

    /// Validate installed package manifests.
    Doctor {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Run an analysis lens on demand.
    Run {
        package: String,
        analysis: String,
        #[arg(long)]
        date: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        capture_dir: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,
    },
}

#[derive(Subcommand)]
enum ConnectorAction {
    /// List user-facing connectors.
    List {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Enable a user-facing connector.
    Enable { id: String },

    /// Disable a user-facing connector.
    Disable { id: String },

    /// Validate connector health.
    Doctor {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
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

    /// Diagnose global configuration and runtime setup issues.
    Doctor {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Set a config value (e.g., alvum config-set capture.screen.enabled false)
    #[command(name = "config-set")]
    ConfigSet {
        /// Dotted key path (e.g., capture.audio-mic.device, processors.screen.vision)
        key: String,
        /// Value to set
        value: String,
    },

    /// Manage user-facing connectors.
    Connectors {
        #[command(subcommand)]
        action: Option<ConnectorAction>,
    },

    /// LLM provider status + test commands. Designed to be called from
    /// the menu-bar popover for the Provider settings section, but
    /// fine for direct CLI use too.
    Providers {
        #[command(subcommand)]
        action: ProviderAction,
    },

    /// Manage the user-customizable synthesis profile.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },

    /// Manage external extension packages.
    Extensions {
        #[command(subcommand)]
        action: ExtensionAction,
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
        #[arg(long)]
        provider: Option<String>,

        /// Model to use
        #[arg(long, default_value = "claude-sonnet-4-6")]
        model: String,

        /// Only include observations before this timestamp (ISO 8601)
        #[arg(long)]
        before: Option<String>,

        /// Only include session observations at or after this timestamp (ISO 8601).
        /// This scopes historical briefing regeneration without mutating connector config.
        #[arg(long)]
        since: Option<String>,

        /// Date to print in the generated briefing heading (YYYY-MM-DD).
        /// Defaults to today's date. Backfill/catch-up runners pass the
        /// capture day so historical briefings are titled correctly.
        #[arg(long)]
        briefing_date: Option<String>,

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
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Capture {
            capture_dir,
            only,
            disable,
        } => cmd_capture(capture_dir, only, disable).await,
        Commands::Devices => cmd_devices(),
        Commands::ConfigInit => cmd_config_init(),
        Commands::ConfigShow => cmd_config_show(),
        Commands::Doctor { json } => cmd_doctor(json),
        Commands::ConfigSet { key, value } => cmd_config_set(&key, &value),
        Commands::Connectors { action } => cmd_connectors(action).await,
        Commands::Providers { action } => cmd_providers(action).await,
        Commands::Profile { action } => cmd_profile(action),
        Commands::Extensions { action } => cmd_extensions(action).await,
        Commands::Tail { follow, filter } => cmd_tail(follow, filter).await,
        Commands::Extract {
            source,
            session,
            output,
            provider,
            model,
            before,
            since,
            briefing_date,
            capture_dir,
            whisper_model,
            relevance_threshold,
            vision,
            resume,
            no_skip_processed,
        } => {
            cmd_extract(
                source,
                session,
                output,
                provider,
                model,
                before,
                since,
                briefing_date,
                capture_dir,
                whisper_model,
                relevance_threshold,
                vision,
                resume,
                no_skip_processed,
            )
            .await
        }
    }
}

async fn cmd_providers(action: ProviderAction) -> Result<()> {
    match action {
        ProviderAction::List => cmd_providers_list(),
        ProviderAction::Test { provider, model } => {
            let model = model.unwrap_or_else(|| default_model_for_config(&provider));
            cmd_providers_test(&provider, &model).await
        }
        ProviderAction::Models { provider } => cmd_providers_models(&provider).await,
        ProviderAction::InstallModel { provider, model } => {
            cmd_providers_install_model(&provider, &model).await
        }
        ProviderAction::Bootstrap { force } => cmd_providers_bootstrap(force).await,
        ProviderAction::Configure { provider } => cmd_providers_configure(&provider),
        ProviderAction::SetActive { provider } => cmd_providers_set_active(&provider),
        ProviderAction::Enable { provider } => cmd_providers_set_enabled(&provider, true),
        ProviderAction::Disable { provider } => cmd_providers_set_enabled(&provider, false),
    }
}

fn cmd_profile(action: ProfileAction) -> Result<()> {
    use alvum_core::synthesis_profile::{SynthesisProfile, generated_knowledge_dir, profile_path};

    match action {
        ProfileAction::Show { json } => {
            let profile = SynthesisProfile::load_or_default()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&profile)?);
            } else {
                println!("{}", toml::to_string_pretty(&profile)?);
            }
            Ok(())
        }
        ProfileAction::Set { key, value } => {
            let mut profile = SynthesisProfile::load_or_default()?;
            set_profile_value(&mut profile, &key, &value)?;
            profile.save()?;
            println!("Saved synthesis profile: {}", profile_path().display());
            Ok(())
        }
        ProfileAction::Save { json } => {
            let profile: SynthesisProfile =
                serde_json::from_str(&json).context("failed to parse profile JSON")?;
            profile.save()?;
            println!("Saved synthesis profile: {}", profile_path().display());
            Ok(())
        }
        ProfileAction::Suggestions { json } => {
            let profile = SynthesisProfile::load_or_default()?;
            let suggestions = profile_suggestions(&profile)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "knowledge_dir": generated_knowledge_dir(),
                        "suggestions": suggestions,
                    }))?
                );
            } else if suggestions.is_empty() {
                println!("No untracked profile suggestions.");
            } else {
                for suggestion in suggestions {
                    println!(
                        "{} [{}] {} — {}",
                        suggestion.id,
                        suggestion.suggestion_type,
                        suggestion.name,
                        suggestion.description
                    );
                }
            }
            Ok(())
        }
        ProfileAction::Promote { id } => {
            let mut profile = SynthesisProfile::load_or_default()?;
            let suggestions = profile_suggestions(&profile)?;
            let suggestion = suggestions
                .into_iter()
                .find(|suggestion| suggestion.id == id)
                .with_context(|| format!("unknown profile suggestion: {id}"))?;
            profile.promote_suggestion(&suggestion);
            profile.save()?;
            println!("Tracked {}", suggestion.name);
            Ok(())
        }
        ProfileAction::Ignore { id } => {
            let mut profile = SynthesisProfile::load_or_default()?;
            profile.ignore_suggestion(&id);
            profile.save()?;
            println!("Ignored {id}");
            Ok(())
        }
    }
}

fn set_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    match key {
        "advanced_instructions" => {
            profile.advanced_instructions = value.to_string();
            Ok(())
        }
        "writing.detail_level" => {
            profile.writing.detail_level = value.to_string();
            Ok(())
        }
        "writing.tone" => {
            profile.writing.tone = value.to_string();
            Ok(())
        }
        "writing.outline" => {
            profile.writing.outline = value.to_string();
            Ok(())
        }
        _ if key.starts_with("intentions.") => set_intention_profile_value(profile, key, value),
        _ if key.starts_with("domains.") => set_domain_profile_value(profile, key, value),
        _ if key.starts_with("interests.") => set_interest_profile_value(profile, key, value),
        _ => bail!("unsupported profile key: {key}"),
    }
}

fn set_intention_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 3 {
        bail!("intention profile keys must be intentions.<id>.<field>");
    }
    let Some(intention) = profile
        .intentions
        .iter_mut()
        .find(|intention| intention.id == parts[1])
    else {
        bail!("unknown synthesis intention: {}", parts[1]);
    };
    match parts[2] {
        "enabled" => intention.enabled = parse_bool(value)?,
        "confirmed" => intention.confirmed = parse_bool(value)?,
        "kind" => intention.kind = value.to_string(),
        "domain" => intention.domain = value.to_string(),
        "description" => intention.description = value.to_string(),
        "notes" => intention.notes = value.to_string(),
        "success_criteria" => intention.success_criteria = value.to_string(),
        "cadence" => intention.cadence = value.to_string(),
        "target_date" => {
            intention.target_date = (!value.trim().is_empty()).then(|| value.to_string())
        }
        "aliases" => intention.aliases = parse_csv(value),
        "priority" => intention.priority = value.parse().context("priority must be an integer")?,
        "source" => intention.source = value.to_string(),
        "nudge" => intention.nudge = value.to_string(),
        _ => bail!("unsupported intention field: {}", parts[2]),
    }
    Ok(())
}

fn set_domain_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 3 {
        bail!("domain profile keys must be domains.<id>.<field>");
    }
    let Some(domain) = profile
        .domains
        .iter_mut()
        .find(|domain| domain.id == parts[1])
    else {
        bail!("unknown synthesis domain: {}", parts[1]);
    };
    match parts[2] {
        "enabled" => domain.enabled = parse_bool(value)?,
        "name" => domain.name = value.to_string(),
        "description" => domain.description = value.to_string(),
        "aliases" => domain.aliases = parse_csv(value),
        "priority" => domain.priority = value.parse().context("priority must be an integer")?,
        _ => bail!("unsupported domain field: {}", parts[2]),
    }
    Ok(())
}

fn set_interest_profile_value(
    profile: &mut alvum_core::synthesis_profile::SynthesisProfile,
    key: &str,
    value: &str,
) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() != 3 {
        bail!("interest profile keys must be interests.<id>.<field>");
    }
    let Some(interest) = profile
        .interests
        .iter_mut()
        .find(|interest| interest.id == parts[1])
    else {
        bail!("unknown synthesis interest: {}", parts[1]);
    };
    match parts[2] {
        "enabled" => interest.enabled = parse_bool(value)?,
        "name" => interest.name = value.to_string(),
        "type" => interest.interest_type = value.to_string(),
        "notes" => interest.notes = value.to_string(),
        "aliases" => interest.aliases = parse_csv(value),
        "priority" => interest.priority = value.parse().context("priority must be an integer")?,
        _ => bail!("unsupported interest field: {}", parts[2]),
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("expected true or false"),
    }
}

fn parse_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

fn profile_suggestions(
    profile: &alvum_core::synthesis_profile::SynthesisProfile,
) -> Result<Vec<alvum_core::synthesis_profile::SynthesisProfileSuggestion>> {
    use alvum_core::synthesis_profile::{SynthesisProfileSuggestion, generated_knowledge_dir};

    let corpus = alvum_knowledge::store::load(&generated_knowledge_dir()).unwrap_or_default();
    let ignored: BTreeSet<&str> = profile
        .ignored_suggestions
        .iter()
        .map(String::as_str)
        .collect();
    let tracked_knowledge: BTreeSet<&str> = profile
        .interests
        .iter()
        .flat_map(|interest| interest.linked_knowledge_ids.iter().map(String::as_str))
        .collect();

    let mut suggestions = Vec::new();
    for entity in corpus.entities {
        if entity.id.is_empty() || tracked_knowledge.contains(entity.id.as_str()) {
            continue;
        }
        if !entity_is_recurring(&entity) {
            continue;
        }
        let id = profile_suggestion_id("entity", &entity.id);
        if ignored.contains(id.as_str()) {
            continue;
        }
        suggestions.push(SynthesisProfileSuggestion {
            id,
            suggestion_type: normalize_interest_type(&entity.entity_type),
            name: entity.name,
            description: entity.description,
            source: "knowledge.entity".into(),
            knowledge_id: entity.id,
        });
    }
    for pattern in corpus.patterns {
        if pattern.id.is_empty() || tracked_knowledge.contains(pattern.id.as_str()) {
            continue;
        }
        if !pattern_is_recurring(&pattern) {
            continue;
        }
        let id = profile_suggestion_id("pattern", &pattern.id);
        if ignored.contains(id.as_str()) {
            continue;
        }
        suggestions.push(SynthesisProfileSuggestion {
            id,
            suggestion_type: "topic".into(),
            name: pattern.id.replace('_', " "),
            description: pattern.description,
            source: "knowledge.pattern".into(),
            knowledge_id: pattern.id,
        });
    }
    suggestions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(suggestions)
}

fn entity_is_recurring(entity: &alvum_knowledge::types::Entity) -> bool {
    entity.first_seen != entity.last_seen
}

fn pattern_is_recurring(pattern: &alvum_knowledge::types::Pattern) -> bool {
    pattern.occurrences > 1 || pattern.first_seen != pattern.last_seen
}

fn profile_suggestion_id(kind: &str, knowledge_id: &str) -> String {
    let slug = knowledge_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    format!("{kind}_{slug}")
}

fn normalize_interest_type(entity_type: &str) -> String {
    match entity_type {
        "person" | "place" | "project" | "organization" | "tool" | "topic" => {
            entity_type.to_string()
        }
        _ => "topic".into(),
    }
}

async fn cmd_extensions(action: ExtensionAction) -> Result<()> {
    use alvum_connector_external::{ExtensionInstallSource, ExtensionRegistryStore};

    let store = ExtensionRegistryStore::default();
    match action {
        ExtensionAction::List { json } => cmd_extensions_list(&store, json),
        ExtensionAction::Install { source } => {
            let record = store.install(ExtensionInstallSource::parse(&source))?;
            println!("Installed extension package: {}", record.id);
            println!("Enable it with: alvum extensions enable {}", record.id);
            Ok(())
        }
        ExtensionAction::Scaffold { path, id, name } => cmd_extensions_scaffold(&path, &id, &name),
        ExtensionAction::Update { id } => {
            let registry = store.load()?;
            let record = registry
                .packages
                .get(&id)
                .with_context(|| format!("extension package not installed: {id}"))?;
            let was_enabled = record.enabled;
            let source = record
                .install_source
                .clone()
                .with_context(|| format!("extension package {id} has no install source"))?;
            let updated = store.install(ExtensionInstallSource::parse(&source))?;
            if was_enabled {
                store.set_enabled(&updated.id, true)?;
            }
            println!("Updated extension package: {}", updated.id);
            Ok(())
        }
        ExtensionAction::Remove { id } => {
            store.remove(&id)?;
            disable_extension_config(&id)?;
            println!("Removed extension package: {id}");
            Ok(())
        }
        ExtensionAction::Enable { id, connector } => {
            let record = store.set_enabled(&id, true)?;
            let manifest = ExtensionRegistryStore::load_manifest(&record)?;
            let connector = connector.or_else(|| manifest.connectors.first().map(|c| c.id.clone()));
            if let Some(connector) = connector {
                if !manifest.connectors.iter().any(|c| c.id == connector) {
                    anyhow::bail!("extension package {id} has no connector {connector}");
                }
                write_external_connector_config(&id, &connector, true)?;
                println!("Enabled extension connector: {id}/{connector}");
            } else {
                println!("Enabled extension package: {id}");
            }
            Ok(())
        }
        ExtensionAction::Disable { id } => {
            store.set_enabled(&id, false)?;
            disable_extension_config(&id)?;
            println!("Disabled extension package: {id}");
            Ok(())
        }
        ExtensionAction::Doctor { json } => {
            let store = store.clone();
            tokio::task::spawn_blocking(move || cmd_extensions_doctor(&store, json)).await?
        }
        ExtensionAction::Run {
            package,
            analysis,
            date,
            output,
            capture_dir,
            provider,
            model,
        } => {
            let registry = store.load()?;
            let record = registry
                .packages
                .get(&package)
                .with_context(|| format!("extension package not installed: {package}"))?
                .clone();
            if !record.enabled {
                anyhow::bail!("extension package is disabled: {package}");
            }
            let manifest = ExtensionRegistryStore::load_manifest(&record)?;
            let date = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            let output = output.unwrap_or_else(|| {
                home.join(".alvum")
                    .join("generated")
                    .join("briefings")
                    .join(&date)
            });
            let capture_dir =
                capture_dir.unwrap_or_else(|| home.join(".alvum").join("capture").join(&date));
            let provider = provider.unwrap_or_else(|| {
                alvum_core::config::AlvumConfig::load()
                    .map(|config| config.pipeline.provider)
                    .unwrap_or_else(|_| "auto".into())
            });
            let provider_box =
                alvum_pipeline::llm::create_provider_async(&provider, &model).await?;
            let provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider> = provider_box.into();
            let response = alvum_connector_external::run_analysis(
                record,
                manifest,
                &analysis,
                &date,
                &capture_dir,
                &output,
                provider,
            )
            .await?;
            println!(
                "Analysis {package}/{analysis} wrote {} artifact(s), {} graph overlay(s)",
                response.artifacts.len(),
                response.graph_overlays.len()
            );
            Ok(())
        }
    }
}

#[derive(serde::Serialize)]
struct ExtensionListOutput {
    extensions: Vec<ExtensionSummary>,
    core: Vec<ExtensionSummary>,
}

#[derive(serde::Serialize)]
struct ExtensionSummary {
    id: String,
    kind: String,
    enabled: bool,
    read_only: bool,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    captures: Vec<ComponentSummary>,
    processors: Vec<ComponentSummary>,
    analyses: Vec<ComponentSummary>,
    connectors: Vec<ConnectorSummary>,
}

#[derive(serde::Serialize)]
struct ComponentSummary {
    id: String,
    component_id: String,
    display_name: String,
}

#[derive(serde::Serialize)]
struct ConnectorSummary {
    id: String,
    component_id: String,
    display_name: String,
    route_count: usize,
    analysis_count: usize,
}

fn extension_summary(record: &alvum_core::extension::ExtensionPackageRecord) -> ExtensionSummary {
    let base = || ExtensionSummary {
        id: record.id.clone(),
        kind: "external".into(),
        enabled: record.enabled,
        read_only: false,
        manifest_path: record.manifest_path.display().to_string(),
        package_dir: record.package_dir.display().to_string(),
        install_source: record.install_source.clone(),
        error: None,
        name: None,
        version: None,
        captures: Vec::new(),
        processors: Vec::new(),
        analyses: Vec::new(),
        connectors: Vec::new(),
    };
    let manifest = match alvum_connector_external::ExtensionRegistryStore::load_manifest(record) {
        Ok(manifest) => manifest,
        Err(e) => {
            return ExtensionSummary {
                error: Some(format!("{e:#}")),
                ..base()
            };
        }
    };
    extension_summary_from_manifest(
        &manifest,
        "external",
        record.enabled,
        false,
        record.manifest_path.display().to_string(),
        record.package_dir.display().to_string(),
        record.install_source.clone(),
    )
}

fn extension_summary_from_manifest(
    manifest: &alvum_core::extension::ExtensionManifest,
    kind: &str,
    enabled: bool,
    read_only: bool,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
) -> ExtensionSummary {
    let component = |id: &str, display_name: &str| ComponentSummary {
        id: id.to_string(),
        component_id: manifest.component_id(id),
        display_name: display_name.to_string(),
    };
    ExtensionSummary {
        id: manifest.id.clone(),
        kind: kind.into(),
        enabled,
        read_only,
        manifest_path,
        package_dir,
        install_source,
        error: None,
        name: Some(manifest.name.clone()),
        version: Some(manifest.version.clone()),
        captures: manifest
            .captures
            .iter()
            .map(|c| component(&c.id, &c.display_name))
            .collect(),
        processors: manifest
            .processors
            .iter()
            .map(|p| component(&p.id, &p.display_name))
            .collect(),
        analyses: manifest
            .analyses
            .iter()
            .map(|a| component(&a.id, &a.display_name))
            .collect(),
        connectors: manifest
            .connectors
            .iter()
            .map(|c| ConnectorSummary {
                id: c.id.clone(),
                component_id: manifest.component_id(&c.id),
                display_name: c.display_name.clone(),
                route_count: c.routes.len(),
                analysis_count: c.analyses.len(),
            })
            .collect(),
    }
}

fn core_extension_summaries(config: &alvum_core::config::AlvumConfig) -> Vec<ExtensionSummary> {
    alvum_core::builtin_components::manifests()
        .into_iter()
        .map(|manifest| {
            let enabled = match manifest.id.as_str() {
                "alvum.audio" => {
                    config
                        .connector("audio")
                        .map(|connector| connector.enabled)
                        .unwrap_or(false)
                        || config
                            .capture_source("audio-mic")
                            .map(|source| source.enabled)
                            .unwrap_or(false)
                        || config
                            .capture_source("audio-system")
                            .map(|source| source.enabled)
                            .unwrap_or(false)
                }
                "alvum.screen" => {
                    config
                        .connector("screen")
                        .map(|connector| connector.enabled)
                        .unwrap_or(false)
                        || config
                            .capture_source("screen")
                            .map(|source| source.enabled)
                            .unwrap_or(false)
                }
                "alvum.session" => {
                    config
                        .connector("claude-code")
                        .map(|connector| connector.enabled)
                        .unwrap_or(false)
                        || config
                            .connector("codex")
                            .map(|connector| connector.enabled)
                            .unwrap_or(false)
                }
                _ => false,
            };
            extension_summary_from_manifest(
                &manifest,
                "core",
                enabled,
                true,
                format!("builtin://{}", manifest.id),
                format!("builtin://{}", manifest.id),
                None,
            )
        })
        .collect()
}

fn cmd_extensions_list(
    store: &alvum_connector_external::ExtensionRegistryStore,
    json: bool,
) -> Result<()> {
    let registry = store.load()?;
    let summaries: Vec<ExtensionSummary> =
        registry.packages.values().map(extension_summary).collect();
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let core = core_extension_summaries(&config);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ExtensionListOutput {
                extensions: summaries,
                core,
            })?
        );
        return Ok(());
    }
    if summaries.is_empty() {
        println!("No external extensions installed.");
    } else {
        for summary in summaries {
            let status = if summary.enabled {
                "enabled"
            } else {
                "disabled"
            };
            println!("{} ({})", summary.id, status);
            if let Some(name) = &summary.name {
                println!("  name: {name}");
            }
            println!("  manifest: {}", summary.manifest_path);
            if let Some(source) = &summary.install_source {
                println!("  source: {source}");
            }
            if let Some(error) = &summary.error {
                println!("  error: {error}");
            }
        }
    }
    println!("Core components:");
    for summary in core {
        let status = if summary.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("  {} (core, {status})", summary.id);
        if let Some(name) = &summary.name {
            println!("  name: {name}");
        }
    }
    Ok(())
}

#[derive(Clone)]
struct ConnectorPackageSource {
    manifest: alvum_core::extension::ExtensionManifest,
    kind: String,
    record_enabled: bool,
    package_read_only: bool,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
}

#[derive(Clone)]
struct IndexedComponent {
    display_name: String,
    description: String,
    kind: &'static str,
    analysis: Option<alvum_core::extension::AnalysisComponent>,
}

#[derive(Clone)]
struct IndexedCapture {
    capture: alvum_core::extension::CaptureComponent,
    package_kind: String,
}

#[derive(serde::Serialize)]
struct ConnectorListOutput {
    connectors: Vec<ConnectorRecord>,
}

#[derive(Clone, serde::Serialize)]
struct ConnectorRecord {
    id: String,
    component_id: String,
    package_id: String,
    connector_id: String,
    kind: String,
    enabled: bool,
    read_only: bool,
    package_read_only: bool,
    display_name: String,
    description: String,
    package_name: String,
    version: String,
    manifest_path: String,
    package_dir: String,
    install_source: Option<String>,
    aggregate_state: String,
    source_count: usize,
    enabled_source_count: usize,
    source_controls: Vec<SourceControlSummary>,
    processor_controls: Vec<ProcessorControlSummary>,
    route_count: usize,
    analysis_count: usize,
    captures: Vec<ComponentRefSummary>,
    processors: Vec<ComponentRefSummary>,
    analyses: Vec<AnalysisRefSummary>,
    routes: Vec<RouteSummary>,
    issues: Vec<String>,
    #[serde(skip_serializing)]
    config_key: Option<String>,
}

#[derive(Clone, serde::Serialize)]
struct SourceControlSummary {
    id: String,
    label: String,
    component: String,
    kind: String,
    enabled: bool,
    toggleable: bool,
    detail: String,
}

#[derive(Clone, serde::Serialize)]
struct ProcessorControlSummary {
    id: String,
    component: String,
    label: String,
    kind: String,
    detail: String,
    settings: Vec<ProcessorSettingSummary>,
}

#[derive(Clone, serde::Serialize)]
struct ProcessorSettingSummary {
    key: String,
    label: String,
    value: Option<String>,
    value_label: String,
    detail: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<SettingOptionSummary>,
}

#[derive(Clone, serde::Serialize)]
struct SettingOptionSummary {
    value: String,
    label: String,
}

#[derive(Clone, serde::Serialize)]
struct ComponentRefSummary {
    component: String,
    display_name: Option<String>,
    kind: Option<String>,
    exists: bool,
}

#[derive(Clone, serde::Serialize)]
struct RouteEndpointSummary {
    component: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<String>,
    display_name: Option<String>,
    exists: bool,
}

#[derive(Clone, serde::Serialize)]
struct RouteSummary {
    from: RouteEndpointSummary,
    to: Vec<RouteEndpointSummary>,
    issues: Vec<String>,
}

#[derive(Clone, serde::Serialize)]
struct AnalysisRefSummary {
    id: String,
    component_id: String,
    display_name: Option<String>,
    output: Option<&'static str>,
    scopes: Vec<&'static str>,
    exists: bool,
}

#[derive(serde::Serialize)]
struct ConnectorDoctorOutput {
    connectors: Vec<ConnectorDoctorSummary>,
}

#[derive(serde::Serialize)]
struct ConnectorDoctorSummary {
    id: String,
    component_id: String,
    ok: bool,
    enabled: bool,
    message: String,
}

fn connector_package_sources(
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<ConnectorPackageSource>> {
    let mut sources: Vec<ConnectorPackageSource> = alvum_core::builtin_components::manifests()
        .into_iter()
        .map(|manifest| ConnectorPackageSource {
            manifest: manifest.clone(),
            kind: "core".into(),
            record_enabled: true,
            package_read_only: true,
            manifest_path: format!("builtin://{}", manifest.id),
            package_dir: format!("builtin://{}", manifest.id),
            install_source: None,
        })
        .collect();

    let registry = store.load()?;
    for record in registry.packages.values() {
        let manifest = match alvum_connector_external::ExtensionRegistryStore::load_manifest(record)
        {
            Ok(manifest) => manifest,
            Err(_) => continue,
        };
        sources.push(ConnectorPackageSource {
            manifest,
            kind: "external".into(),
            record_enabled: record.enabled,
            package_read_only: false,
            manifest_path: record.manifest_path.display().to_string(),
            package_dir: record.package_dir.display().to_string(),
            install_source: record.install_source.clone(),
        });
    }
    Ok(sources)
}

fn component_index(sources: &[ConnectorPackageSource]) -> BTreeMap<String, IndexedComponent> {
    let mut index = BTreeMap::new();
    for source in sources {
        for capture in &source.manifest.captures {
            index.insert(
                source.manifest.component_id(&capture.id),
                IndexedComponent {
                    display_name: capture.display_name.clone(),
                    description: capture.description.clone(),
                    kind: "capture",
                    analysis: None,
                },
            );
        }
        for processor in &source.manifest.processors {
            index.insert(
                source.manifest.component_id(&processor.id),
                IndexedComponent {
                    display_name: processor.display_name.clone(),
                    description: processor.description.clone(),
                    kind: "processor",
                    analysis: None,
                },
            );
        }
        for analysis in &source.manifest.analyses {
            index.insert(
                source.manifest.component_id(&analysis.id),
                IndexedComponent {
                    display_name: analysis.display_name.clone(),
                    description: analysis.description.clone(),
                    kind: "analysis",
                    analysis: Some(analysis.clone()),
                },
            );
        }
    }
    index
}

fn capture_index(sources: &[ConnectorPackageSource]) -> BTreeMap<String, IndexedCapture> {
    let mut index = BTreeMap::new();
    for source in sources {
        for capture in &source.manifest.captures {
            index.insert(
                source.manifest.component_id(&capture.id),
                IndexedCapture {
                    capture: capture.clone(),
                    package_kind: source.kind.clone(),
                },
            );
        }
    }
    index
}

fn core_connector_config_key(package_id: &str, connector_id: &str) -> Option<&'static str> {
    match (package_id, connector_id) {
        ("alvum.audio", "audio") => Some("audio"),
        ("alvum.screen", "screen") => Some("screen"),
        ("alvum.session", "claude-code") => Some("claude-code"),
        ("alvum.session", "codex") => Some("codex"),
        _ => None,
    }
}

fn external_connector_config_enabled(
    config: &alvum_core::config::AlvumConfig,
    package_id: &str,
    connector_id: &str,
) -> bool {
    config
        .connectors
        .iter()
        .any(|(config_name, connector_cfg)| {
            if connector_cfg.settings.get("kind").and_then(|v| v.as_str()) != Some("external-http")
            {
                return false;
            }
            let configured_package = connector_cfg
                .settings
                .get("package")
                .and_then(|v| v.as_str())
                .unwrap_or(config_name);
            let configured_connector = connector_cfg
                .settings
                .get("connector")
                .and_then(|v| v.as_str())
                .unwrap_or("main");
            configured_package == package_id
                && configured_connector == connector_id
                && connector_cfg.enabled
        })
}

fn component_ref(
    component: &str,
    index: &BTreeMap<String, IndexedComponent>,
) -> ComponentRefSummary {
    let indexed = index.get(component);
    ComponentRefSummary {
        component: component.into(),
        display_name: indexed.map(|component| component.display_name.clone()),
        kind: indexed.map(|component| component.kind.to_string()),
        exists: indexed.is_some(),
    }
}

fn route_endpoint(
    selector: &alvum_core::extension::RouteSelector,
    index: &BTreeMap<String, IndexedComponent>,
) -> RouteEndpointSummary {
    let indexed = index.get(&selector.component);
    RouteEndpointSummary {
        component: selector.component.clone(),
        source: selector.source.clone(),
        mime: selector.mime.clone(),
        schema: selector.schema.clone(),
        display_name: indexed.map(|component| component.display_name.clone()),
        exists: indexed.is_some(),
    }
}

fn source_control_enabled(
    config: &alvum_core::config::AlvumConfig,
    package_kind: &str,
    source_id: &str,
    connector_enabled: bool,
) -> bool {
    if package_kind != "core" {
        return connector_enabled;
    }
    if let Some(capture) = config.capture_source(source_id) {
        return connector_enabled && capture.enabled;
    }
    if let Some(connector) = config.connector(source_id) {
        return connector.enabled;
    }
    connector_enabled
}

fn source_control_toggleable(
    config: &alvum_core::config::AlvumConfig,
    package_kind: &str,
    source_id: &str,
) -> bool {
    package_kind == "core"
        && (config.capture_source(source_id).is_some() || config.connector(source_id).is_some())
}

fn source_controls_for_captures(
    config: &alvum_core::config::AlvumConfig,
    capture_ids: &BTreeSet<String>,
    captures: &BTreeMap<String, IndexedCapture>,
    connector_enabled: bool,
) -> Vec<SourceControlSummary> {
    let mut controls = Vec::new();
    for component_id in capture_ids {
        let Some(indexed) = captures.get(component_id) else {
            continue;
        };
        for source in &indexed.capture.sources {
            controls.push(SourceControlSummary {
                id: source.id.clone(),
                label: source.display_name.clone(),
                component: component_id.clone(),
                kind: "capture".into(),
                enabled: source_control_enabled(
                    config,
                    &indexed.package_kind,
                    &source.id,
                    connector_enabled,
                ),
                toggleable: source_control_toggleable(config, &indexed.package_kind, &source.id),
                detail: indexed.capture.description.clone(),
            });
        }
    }
    controls
}

fn toml_value_summary(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Float(value) => value.to_string(),
        toml::Value::Boolean(value) => value.to_string(),
        _ => value.to_string(),
    }
}

fn configured_processor_value(
    config: &alvum_core::config::AlvumConfig,
    processor_key: &str,
    connector_key: &str,
    setting_key: &str,
) -> Option<String> {
    config
        .processor(processor_key)
        .and_then(|processor| processor.settings.get(setting_key))
        .map(toml_value_summary)
        .or_else(|| {
            config
                .connector(connector_key)
                .and_then(|connector| connector.settings.get(setting_key))
                .map(toml_value_summary)
        })
}

fn processor_setting_summary(
    key: &str,
    label: &str,
    value: Option<String>,
    default_label: &str,
    detail: &str,
    options: Vec<SettingOptionSummary>,
) -> ProcessorSettingSummary {
    let value_label = value
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(default_label)
        .to_string();
    ProcessorSettingSummary {
        key: key.into(),
        label: label.into(),
        value,
        value_label,
        detail: detail.into(),
        options,
    }
}

fn setting_option(value: impl Into<String>, label: impl Into<String>) -> SettingOptionSummary {
    SettingOptionSummary {
        value: value.into(),
        label: label.into(),
    }
}

fn screen_vision_label(value: &str) -> String {
    match value {
        "ocr" => "OCR".into(),
        "local" => "Local vision".into(),
        "api" => "API vision".into(),
        "off" => "Off".into(),
        other => other.into(),
    }
}

fn screen_vision_options() -> Vec<SettingOptionSummary> {
    ["ocr", "local", "api", "off"]
        .into_iter()
        .map(|value| setting_option(value, screen_vision_label(value)))
        .collect()
}

fn whisper_language_options() -> Vec<SettingOptionSummary> {
    vec![
        setting_option("en", "English"),
        setting_option("auto", "Auto detect"),
    ]
}

fn path_label(value: &str) -> String {
    Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(value)
        .to_string()
}

fn whisper_model_options(current: Option<&str>) -> Vec<SettingOptionSummary> {
    let mut options = BTreeMap::new();
    if let Some(current) = current.filter(|value| !value.is_empty()) {
        options.insert(current.to_string(), path_label(current));
    }
    if let Some(home) = dirs::home_dir() {
        let model_dir = home.join(".alvum/runtime/models");
        if let Ok(entries) = std::fs::read_dir(model_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let supported = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| matches!(ext, "bin" | "gguf"))
                    .unwrap_or(false);
                if !supported {
                    continue;
                }
                let value = path.to_string_lossy().into_owned();
                options
                    .entry(value.clone())
                    .or_insert_with(|| path_label(&value));
            }
        }
    }
    options
        .into_iter()
        .map(|(value, label)| setting_option(value, label))
        .collect()
}

fn builtin_processor_settings(
    config: &alvum_core::config::AlvumConfig,
    package_id: &str,
    connector_id: &str,
    component_id: &str,
) -> Vec<ProcessorSettingSummary> {
    match (package_id, connector_id, component_id) {
        ("alvum.audio", "audio", "alvum.audio/whisper") => {
            let model = configured_processor_value(config, "audio", "audio", "whisper_model");
            let language = configured_processor_value(config, "audio", "audio", "whisper_language")
                .or_else(|| Some("en".into()));
            vec![
                processor_setting_summary(
                    "whisper_model",
                    "Whisper model",
                    model.clone(),
                    "Not configured",
                    "Model file used for audio transcription.",
                    whisper_model_options(model.as_deref()),
                ),
                processor_setting_summary(
                    "whisper_language",
                    "Language",
                    language.clone(),
                    "en",
                    "Language hint passed to Whisper.",
                    whisper_language_options(),
                ),
            ]
        }
        ("alvum.screen", "screen", "alvum.screen/vision") => {
            let vision = configured_processor_value(config, "screen", "screen", "vision")
                .or_else(|| Some("ocr".into()));
            let value_label = vision
                .as_deref()
                .map(screen_vision_label)
                .unwrap_or_else(|| "OCR".into());
            vec![ProcessorSettingSummary {
                key: "vision".into(),
                label: "Recognition method".into(),
                value: vision,
                value_label,
                detail: "Text and content recognition method for screenshots.".into(),
                options: screen_vision_options(),
            }]
        }
        _ => Vec::new(),
    }
}

fn processor_controls_for_connector(
    config: &alvum_core::config::AlvumConfig,
    package_id: &str,
    connector_id: &str,
    processor_ids: &BTreeSet<String>,
    index: &BTreeMap<String, IndexedComponent>,
) -> Vec<ProcessorControlSummary> {
    processor_ids
        .iter()
        .map(|component_id| {
            let indexed = index.get(component_id);
            ProcessorControlSummary {
                id: component_id.clone(),
                component: component_id.clone(),
                label: indexed
                    .map(|component| component.display_name.clone())
                    .unwrap_or_else(|| component_id.clone()),
                kind: "processor".into(),
                detail: indexed
                    .map(|component| component.description.clone())
                    .unwrap_or_default(),
                settings: builtin_processor_settings(
                    config,
                    package_id,
                    connector_id,
                    component_id,
                ),
            }
        })
        .collect()
}

fn aggregate_state(enabled: bool, source_controls: &[SourceControlSummary]) -> String {
    if source_controls.is_empty() {
        return if enabled { "all_on" } else { "all_off" }.into();
    }
    let enabled_count = source_controls
        .iter()
        .filter(|control| control.enabled)
        .count();
    if enabled_count == 0 {
        "all_off".into()
    } else if enabled_count == source_controls.len() {
        "all_on".into()
    } else {
        "partial".into()
    }
}

fn output_label(output: &alvum_core::extension::AnalysisOutput) -> &'static str {
    match output {
        alvum_core::extension::AnalysisOutput::Artifact => "artifact",
        alvum_core::extension::AnalysisOutput::GraphOverlay => "graph_overlay",
    }
}

fn scope_label(scope: &alvum_core::extension::DataScope) -> &'static str {
    match scope {
        alvum_core::extension::DataScope::Capture => "capture",
        alvum_core::extension::DataScope::Observations => "observations",
        alvum_core::extension::DataScope::Threads => "threads",
        alvum_core::extension::DataScope::Decisions => "decisions",
        alvum_core::extension::DataScope::Edges => "edges",
        alvum_core::extension::DataScope::Briefing => "briefing",
        alvum_core::extension::DataScope::Knowledge => "knowledge",
        alvum_core::extension::DataScope::RawFiles => "raw_files",
        alvum_core::extension::DataScope::All => "all",
    }
}

fn connector_records(
    config: &alvum_core::config::AlvumConfig,
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<ConnectorRecord>> {
    let sources = connector_package_sources(store)?;
    let index = component_index(&sources);
    let captures = capture_index(&sources);
    let mut records = Vec::new();

    for source in &sources {
        for connector in &source.manifest.connectors {
            let component_id = source.manifest.component_id(&connector.id);
            let config_key =
                core_connector_config_key(&source.manifest.id, &connector.id).map(str::to_string);
            let enabled = if source.kind == "core" {
                config_key
                    .as_deref()
                    .and_then(|key| config.connector(key))
                    .map(|connector| connector.enabled)
                    .unwrap_or(false)
            } else {
                source.record_enabled
                    && external_connector_config_enabled(config, &source.manifest.id, &connector.id)
            };

            let mut issues = Vec::new();
            let mut capture_ids = BTreeSet::new();
            let mut processor_ids = BTreeSet::new();
            let routes = connector
                .routes
                .iter()
                .map(|route| {
                    capture_ids.insert(route.from.component.clone());
                    let from = route_endpoint(&route.from, &index);
                    let mut route_issues = Vec::new();
                    if !from.exists {
                        route_issues.push(format!(
                            "Capture component {} is not installed",
                            from.component
                        ));
                    }
                    let to = route
                        .to
                        .iter()
                        .map(|target| {
                            processor_ids.insert(target.clone());
                            let endpoint = route_endpoint(
                                &alvum_core::extension::RouteSelector {
                                    component: target.clone(),
                                    source: None,
                                    mime: None,
                                    schema: None,
                                },
                                &index,
                            );
                            if !endpoint.exists {
                                route_issues.push(format!(
                                    "Processor component {} is not installed",
                                    endpoint.component
                                ));
                            }
                            endpoint
                        })
                        .collect();
                    issues.extend(route_issues.iter().cloned());
                    RouteSummary {
                        from,
                        to,
                        issues: route_issues,
                    }
                })
                .collect::<Vec<_>>();

            let analyses = connector
                .analyses
                .iter()
                .map(|analysis_id| {
                    let indexed = index.get(analysis_id);
                    if indexed.is_none() {
                        issues.push(format!("Analysis component {analysis_id} is not installed"));
                    }
                    let analysis = indexed.and_then(|component| component.analysis.as_ref());
                    AnalysisRefSummary {
                        id: analysis_id
                            .split_once('/')
                            .map(|(_, local)| local.to_string())
                            .unwrap_or_else(|| analysis_id.clone()),
                        component_id: analysis_id.clone(),
                        display_name: indexed.map(|component| component.display_name.clone()),
                        output: analysis.map(|analysis| output_label(&analysis.output)),
                        scopes: analysis
                            .map(|analysis| analysis.scopes.iter().map(scope_label).collect())
                            .unwrap_or_default(),
                        exists: indexed.is_some(),
                    }
                })
                .collect::<Vec<_>>();
            let source_controls =
                source_controls_for_captures(config, &capture_ids, &captures, enabled);
            let processor_controls = processor_controls_for_connector(
                config,
                &source.manifest.id,
                &connector.id,
                &processor_ids,
                &index,
            );
            let enabled_source_count = source_controls
                .iter()
                .filter(|control| control.enabled)
                .count();

            records.push(ConnectorRecord {
                id: component_id.clone(),
                component_id,
                package_id: source.manifest.id.clone(),
                connector_id: connector.id.clone(),
                kind: source.kind.clone(),
                enabled,
                read_only: false,
                package_read_only: source.package_read_only,
                display_name: connector.display_name.clone(),
                description: connector.description.clone(),
                package_name: source.manifest.name.clone(),
                version: source.manifest.version.clone(),
                manifest_path: source.manifest_path.clone(),
                package_dir: source.package_dir.clone(),
                install_source: source.install_source.clone(),
                aggregate_state: aggregate_state(enabled, &source_controls),
                source_count: source_controls.len(),
                enabled_source_count,
                source_controls,
                processor_controls,
                route_count: routes.len(),
                analysis_count: analyses.len(),
                captures: capture_ids
                    .iter()
                    .map(|component| component_ref(component, &index))
                    .collect(),
                processors: processor_ids
                    .iter()
                    .map(|component| component_ref(component, &index))
                    .collect(),
                analyses,
                routes,
                issues,
                config_key,
            });
        }
    }
    Ok(records)
}

fn connector_record_by_id<'a>(
    records: &'a [ConnectorRecord],
    id: &str,
) -> Result<&'a ConnectorRecord> {
    let exact = records
        .iter()
        .find(|record| record.id == id || record.component_id == id);
    if let Some(record) = exact {
        return Ok(record);
    }

    let matches = records
        .iter()
        .filter(|record| {
            record.connector_id == id
                || (record.package_id == id
                    && records
                        .iter()
                        .filter(|candidate| candidate.package_id == id)
                        .count()
                        == 1)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [record] => Ok(record),
        [] => bail!("connector not found: {id}"),
        _ => bail!("connector id is ambiguous: {id}; use a component id like package/connector"),
    }
}

fn default_config_table() -> Result<toml::Table> {
    Ok(toml::to_string(&alvum_core::config::AlvumConfig::default())?.parse()?)
}

fn config_doc() -> Result<toml::Table> {
    let config_path = alvum_core::config::config_path();
    if config_path.exists() {
        Ok(std::fs::read_to_string(&config_path)?.parse()?)
    } else {
        default_config_table()
    }
}

fn write_config_doc(doc: &toml::Table) -> Result<()> {
    let config_path = alvum_core::config::config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, toml::to_string_pretty(doc)?)?;
    Ok(())
}

fn set_table_enabled(
    parent: &mut toml::Table,
    defaults: &toml::Table,
    section_name: &str,
    key: &str,
    enabled: bool,
) -> Result<()> {
    let section = parent
        .entry(section_name.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .with_context(|| format!("{section_name} is not a table"))?;
    if !section.contains_key(key) {
        if let Some(default_value) = defaults
            .get(section_name)
            .and_then(|value| value.as_table())
            .and_then(|table| table.get(key))
        {
            section.insert(key.to_string(), default_value.clone());
        }
    }
    let table = section
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .with_context(|| format!("{section_name}.{key} is not a table"))?;
    table.insert("enabled".into(), toml::Value::Boolean(enabled));
    Ok(())
}

fn core_capture_config_keys(record: &ConnectorRecord) -> Vec<String> {
    if record.kind != "core" {
        return Vec::new();
    }
    let default_capture_keys = alvum_core::config::AlvumConfig::default()
        .capture
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut keys = BTreeSet::new();
    for capture in &record.captures {
        if let Some(component) =
            alvum_core::builtin_components::capture_component(&capture.component)
        {
            for source in component.sources {
                if default_capture_keys.contains(&source.id) {
                    keys.insert(source.id);
                }
            }
            continue;
        }
        if let Some((_package, local_id)) = capture.component.split_once('/') {
            if default_capture_keys.contains(local_id) {
                keys.insert(local_id.to_string());
            }
        }
    }
    keys.into_iter().collect()
}

fn write_core_connector_enabled(record: &ConnectorRecord, enabled: bool) -> Result<()> {
    let config_key = record
        .config_key
        .as_deref()
        .with_context(|| format!("core connector {} has no config key", record.id))?;
    let mut doc = config_doc()?;
    let defaults = default_config_table()?;
    set_table_enabled(&mut doc, &defaults, "connectors", config_key, enabled)?;
    for capture_key in core_capture_config_keys(record) {
        set_table_enabled(&mut doc, &defaults, "capture", &capture_key, enabled)?;
    }
    write_config_doc(&doc)
}

fn extension_doctor_summaries(
    store: &alvum_connector_external::ExtensionRegistryStore,
) -> Result<Vec<DoctorSummary>> {
    let registry = store.load()?;
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("logs")
        .join("extensions");
    Ok(registry
        .packages
        .values()
        .map(|record| extension_doctor_summary(record, &log_dir))
        .collect())
}

#[derive(serde::Serialize)]
struct GlobalDoctorOutput {
    ok: bool,
    error_count: usize,
    warning_count: usize,
    checks: Vec<GlobalDoctorCheck>,
}

#[derive(serde::Serialize)]
struct GlobalDoctorCheck {
    id: &'static str,
    label: &'static str,
    level: &'static str,
    message: String,
}

fn doctor_check(
    id: &'static str,
    label: &'static str,
    level: &'static str,
    message: impl Into<String>,
) -> GlobalDoctorCheck {
    GlobalDoctorCheck {
        id,
        label,
        level,
        message: message.into(),
    }
}

fn load_config_for_doctor(checks: &mut Vec<GlobalDoctorCheck>) -> alvum_core::config::AlvumConfig {
    let path = alvum_core::config::config_path();
    if !path.exists() {
        checks.push(doctor_check(
            "config",
            "Config",
            "ok",
            format!("No config file at {}; using defaults.", path.display()),
        ));
        return alvum_core::config::AlvumConfig::default();
    }

    match alvum_core::config::AlvumConfig::load() {
        Ok(config) => {
            checks.push(doctor_check(
                "config",
                "Config",
                "ok",
                format!("Loaded {}.", path.display()),
            ));
            config
        }
        Err(e) => {
            checks.push(doctor_check("config", "Config", "error", format!("{e:#}")));
            alvum_core::config::AlvumConfig::default()
        }
    }
}

fn diagnose_connectors(
    config: &alvum_core::config::AlvumConfig,
    store: &alvum_connector_external::ExtensionRegistryStore,
    checks: &mut Vec<GlobalDoctorCheck>,
) {
    match connector_records(config, store) {
        Ok(records) => {
            if records.is_empty() {
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "warning",
                    "No connectors are available.",
                ));
                return;
            }

            let route_issues = records
                .iter()
                .flat_map(|record| {
                    record
                        .issues
                        .iter()
                        .map(move |issue| format!("{}: {issue}", record.component_id))
                })
                .collect::<Vec<_>>();
            let disabled_sources = records
                .iter()
                .filter(|record| {
                    record.enabled && record.source_count > 0 && record.enabled_source_count == 0
                })
                .map(|record| record.component_id.clone())
                .collect::<Vec<_>>();

            if !route_issues.is_empty() {
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "error",
                    format!(
                        "{} route issue{}: {}",
                        route_issues.len(),
                        if route_issues.len() == 1 { "" } else { "s" },
                        route_issues
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            } else if !disabled_sources.is_empty() {
                let connector_word = if disabled_sources.len() == 1 {
                    "connector has"
                } else {
                    "connectors have"
                };
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "warning",
                    format!(
                        "{} enabled {connector_word} all sources off: {}.",
                        disabled_sources.len(),
                        disabled_sources
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            } else {
                let enabled = records.iter().filter(|record| record.enabled).count();
                checks.push(doctor_check(
                    "connectors",
                    "Connectors",
                    "ok",
                    format!(
                        "{enabled}/{} connectors enabled; route matrix is valid.",
                        records.len()
                    ),
                ));
            }
        }
        Err(e) => checks.push(doctor_check(
            "connectors",
            "Connectors",
            "error",
            format!("{e:#}"),
        )),
    }
}

fn diagnose_extensions(
    store: &alvum_connector_external::ExtensionRegistryStore,
    checks: &mut Vec<GlobalDoctorCheck>,
) {
    match extension_doctor_summaries(store) {
        Ok(summaries) => {
            if summaries.is_empty() {
                checks.push(doctor_check(
                    "extensions",
                    "Extensions",
                    "ok",
                    "No external extensions installed.",
                ));
                return;
            }
            let failed = summaries
                .iter()
                .filter(|summary| !summary.ok)
                .collect::<Vec<_>>();
            if failed.is_empty() {
                checks.push(doctor_check(
                    "extensions",
                    "Extensions",
                    "ok",
                    format!(
                        "{} external extension packages passed health checks.",
                        summaries.len()
                    ),
                ));
            } else {
                checks.push(doctor_check(
                    "extensions",
                    "Extensions",
                    "error",
                    format!(
                        "{} extension package{} failed: {}.",
                        failed.len(),
                        if failed.len() == 1 { "" } else { "s" },
                        failed
                            .iter()
                            .take(3)
                            .map(|summary| format!("{} ({})", summary.id, summary.message))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                ));
            }
        }
        Err(e) => checks.push(doctor_check(
            "extensions",
            "Extensions",
            "error",
            format!("{e:#}"),
        )),
    }
}

fn normalize_provider_name(provider: &str) -> String {
    match provider {
        "claude" => "claude-cli".to_string(),
        "cli" => "claude-cli".to_string(),
        "codex" => "codex-cli".to_string(),
        "api" => "anthropic-api".to_string(),
        other => other.to_string(),
    }
}

fn diagnose_providers(
    config: &alvum_core::config::AlvumConfig,
    checks: &mut Vec<GlobalDoctorCheck>,
) {
    let configured = normalize_provider_name(&config.pipeline.provider);
    let entries = provider_entries(config);
    let available = entries
        .iter()
        .filter(|entry| entry.available && config.provider_enabled(entry.name))
        .map(|entry| entry.name)
        .collect::<Vec<_>>();

    if configured == "auto" {
        if let Some(provider) = available.first() {
            checks.push(doctor_check(
                "providers",
                "Providers",
                "ok",
                format!("Auto provider can use {provider}."),
            ));
        } else {
            checks.push(doctor_check(
                "providers",
                "Providers",
                "warning",
                "No LLM providers were detected on PATH or in the environment.",
            ));
        }
        return;
    }

    match entries.iter().find(|entry| entry.name == configured) {
        Some(entry) if !config.provider_enabled(entry.name) => checks.push(doctor_check(
            "providers",
            "Providers",
            "warning",
            format!("Configured provider {configured} is removed from Alvum's provider list."),
        )),
        Some(entry) if entry.available => checks.push(doctor_check(
            "providers",
            "Providers",
            "ok",
            format!("Configured provider {configured} is available."),
        )),
        Some(entry) => checks.push(doctor_check(
            "providers",
            "Providers",
            "warning",
            format!(
                "Configured provider {configured} is not detected; {}.",
                entry.auth_hint
            ),
        )),
        None => checks.push(doctor_check(
            "providers",
            "Providers",
            "warning",
            format!("Configured provider {configured} is not recognized."),
        )),
    }
}

fn global_doctor_output() -> GlobalDoctorOutput {
    let mut checks = Vec::new();
    let store = alvum_connector_external::ExtensionRegistryStore::default();
    let config = load_config_for_doctor(&mut checks);

    diagnose_connectors(&config, &store, &mut checks);
    diagnose_extensions(&store, &mut checks);
    diagnose_providers(&config, &mut checks);

    let error_count = checks.iter().filter(|check| check.level == "error").count();
    let warning_count = checks
        .iter()
        .filter(|check| check.level == "warning")
        .count();
    GlobalDoctorOutput {
        ok: error_count == 0,
        error_count,
        warning_count,
        checks,
    }
}

fn cmd_doctor(json: bool) -> Result<()> {
    let output = global_doctor_output();
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    for check in &output.checks {
        println!("[{}] {}: {}", check.level, check.label, check.message);
    }
    if output.ok {
        println!(
            "Diagnostics completed with {} warning{}.",
            output.warning_count,
            if output.warning_count == 1 { "" } else { "s" }
        );
    } else {
        println!(
            "Diagnostics found {} error{} and {} warning{}.",
            output.error_count,
            if output.error_count == 1 { "" } else { "s" },
            output.warning_count,
            if output.warning_count == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

async fn cmd_connectors(action: Option<ConnectorAction>) -> Result<()> {
    let store = alvum_connector_external::ExtensionRegistryStore::default();
    match action.unwrap_or(ConnectorAction::List { json: false }) {
        ConnectorAction::List { json } => {
            let config = alvum_core::config::AlvumConfig::load()?;
            let records = connector_records(&config, &store)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ConnectorListOutput {
                        connectors: records
                    })?
                );
                return Ok(());
            }
            if records.is_empty() {
                println!("No connectors available.");
                return Ok(());
            }
            for record in records {
                let status = if record.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                println!("{} ({}, {status})", record.component_id, record.kind);
                println!(
                    "  routes: {}, analyses: {}",
                    record.route_count, record.analysis_count
                );
                if !record.issues.is_empty() {
                    println!("  issues: {}", record.issues.join("; "));
                }
            }
            Ok(())
        }
        ConnectorAction::Enable { id } => cmd_connector_set_enabled(&store, &id, true),
        ConnectorAction::Disable { id } => cmd_connector_set_enabled(&store, &id, false),
        ConnectorAction::Doctor { json } => {
            let config = alvum_core::config::AlvumConfig::load()?;
            let records = connector_records(&config, &store)?;
            let extension_doctor_by_id = extension_doctor_summaries(&store)?
                .into_iter()
                .map(|summary| (summary.id.clone(), summary))
                .collect::<BTreeMap<_, _>>();
            let summaries = records
                .into_iter()
                .map(|record| {
                    if record.kind == "core" {
                        ConnectorDoctorSummary {
                            id: record.id,
                            component_id: record.component_id,
                            ok: true,
                            enabled: record.enabled,
                            message: "core connector".into(),
                        }
                    } else {
                        let doctor = extension_doctor_by_id.get(&record.package_id);
                        ConnectorDoctorSummary {
                            id: record.id,
                            component_id: record.component_id,
                            ok: doctor.map(|summary| summary.ok).unwrap_or(false),
                            enabled: record.enabled,
                            message: doctor
                                .map(|summary| summary.message.clone())
                                .unwrap_or_else(|| "extension package not installed".into()),
                        }
                    }
                })
                .collect::<Vec<_>>();
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&ConnectorDoctorOutput {
                        connectors: summaries
                    })?
                );
                return Ok(());
            }
            for summary in summaries {
                if summary.ok {
                    println!("{}: ok", summary.component_id);
                } else {
                    println!("{}: error: {}", summary.component_id, summary.message);
                }
            }
            Ok(())
        }
    }
}

fn cmd_connector_set_enabled(
    store: &alvum_connector_external::ExtensionRegistryStore,
    id: &str,
    enabled: bool,
) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;
    let records = connector_records(&config, store)?;
    let record = connector_record_by_id(&records, id)?;
    if record.kind == "core" {
        write_core_connector_enabled(record, enabled)?;
    } else {
        if enabled {
            store.set_enabled(&record.package_id, true)?;
        }
        write_external_connector_config(&record.package_id, &record.connector_id, enabled)?;
    }
    println!(
        "{} connector: {}",
        if enabled { "Enabled" } else { "Disabled" },
        record.component_id
    );
    Ok(())
}

#[derive(serde::Serialize)]
struct DoctorOutput {
    extensions: Vec<DoctorSummary>,
}

#[derive(serde::Serialize)]
struct DoctorSummary {
    id: String,
    ok: bool,
    enabled: bool,
    connector_count: usize,
    message: String,
}

fn extension_doctor_summary(
    record: &alvum_core::extension::ExtensionPackageRecord,
    log_dir: &std::path::Path,
) -> DoctorSummary {
    let manifest = match alvum_connector_external::ExtensionRegistryStore::load_manifest(record)
        .and_then(|m| m.validate().map(|_| m))
    {
        Ok(manifest) => manifest,
        Err(e) => {
            return DoctorSummary {
                id: record.id.clone(),
                ok: false,
                enabled: record.enabled,
                connector_count: 0,
                message: format!("{e:#}"),
            };
        }
    };
    let health = alvum_connector_external::ManagedExtension::start(
        &manifest,
        &record.package_dir,
        log_dir,
        None,
    )
    .and_then(|managed| {
        let remote = managed.client().manifest()?;
        if remote.id != manifest.id {
            bail!(
                "/v1/manifest reported {}, expected {}",
                remote.id,
                manifest.id
            );
        }
        Ok(())
    });
    match health {
        Ok(()) => DoctorSummary {
            id: record.id.clone(),
            ok: true,
            enabled: record.enabled,
            connector_count: manifest.connectors.len(),
            message: "ok".into(),
        },
        Err(e) => DoctorSummary {
            id: record.id.clone(),
            ok: false,
            enabled: record.enabled,
            connector_count: manifest.connectors.len(),
            message: format!("{e:#}"),
        },
    }
}

fn cmd_extensions_doctor(
    store: &alvum_connector_external::ExtensionRegistryStore,
    json: bool,
) -> Result<()> {
    let registry = store.load()?;
    if registry.packages.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&DoctorOutput { extensions: vec![] })?
            );
        } else {
            println!("No extensions installed.");
        }
        return Ok(());
    }
    let log_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("logs")
        .join("extensions");
    let summaries: Vec<DoctorSummary> = registry
        .packages
        .values()
        .map(|record| extension_doctor_summary(record, &log_dir))
        .collect();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&DoctorOutput {
                extensions: summaries
            })?
        );
        return Ok(());
    }
    for summary in summaries {
        if summary.ok {
            println!(
                "{}: ok ({} connector(s))",
                summary.id, summary.connector_count
            );
        } else {
            println!("{}: error: {}", summary.id, summary.message);
        }
    }
    Ok(())
}

fn cmd_extensions_scaffold(path: &std::path::Path, id: &str, name: &str) -> Result<()> {
    if path.exists() && path.read_dir()?.next().is_some() {
        bail!("target directory is not empty: {}", path.display());
    }
    let manifest = serde_json::json!({
        "schema_version": 1,
        "id": id,
        "name": name,
        "version": "0.1.0",
        "description": "Starter Alvum external extension.",
        "server": {
            "start": ["node", "src/server.mjs"],
            "health_path": "/v1/health",
            "startup_timeout_ms": 5000
        },
        "captures": [{
            "id": "capture",
            "display_name": "Starter capture",
            "sources": [{"id": id, "display_name": name, "expected": false}],
            "schemas": [format!("{id}.event.v1")]
        }],
        "processors": [{
            "id": "processor",
            "display_name": "Starter processor",
            "accepts": [{"component": format!("{id}/capture"), "schema": format!("{id}.event.v1")}]
        }],
        "analyses": [{
            "id": "analysis",
            "display_name": "Starter analysis",
            "scopes": ["observations", "briefing"],
            "output": "artifact"
        }],
        "connectors": [{
            "id": "main",
            "display_name": name,
            "routes": [{
                "from": {"component": format!("{id}/capture"), "schema": format!("{id}.event.v1")},
                "to": [format!("{id}/processor")]
            }],
            "analyses": [format!("{id}/analysis")]
        }],
        "permissions": [{
            "kind": "network",
            "description": "Declare any external APIs this package calls."
        }]
    });
    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    alvum_core::extension::ExtensionManifest::from_json_str(&manifest_json)?;

    std::fs::create_dir_all(path.join("src"))?;
    std::fs::write(path.join("alvum.extension.json"), manifest_json)?;
    std::fs::write(
        path.join("package.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": format!("alvum-extension-{id}"),
            "version": "0.1.0",
            "type": "module",
            "private": true,
            "scripts": {
                "start": "node src/server.mjs"
            }
        }))?,
    )?;
    std::fs::write(path.join("README.md"), scaffold_readme(id, name))?;
    std::fs::write(path.join("src/server.mjs"), scaffold_server(id)?)?;
    println!("Scaffolded extension package: {}", path.display());
    println!("Try it with: alvum extensions install {}", path.display());
    Ok(())
}

fn scaffold_readme(id: &str, name: &str) -> String {
    format!(
        "# {name}\n\nStarter Alvum external extension package.\n\n## Run locally\n\n```bash\nnpm start\n```\n\n## Install into Alvum\n\n```bash\nalvum extensions install .\nalvum extensions enable {id}\nalvum extensions doctor\n```\n"
    )
}

fn scaffold_server(id: &str) -> Result<String> {
    let component = format!("{id}/capture");
    let schema = format!("{id}.event.v1");
    Ok(format!(
        r#"import http from 'node:http';
import fs from 'node:fs/promises';

const port = Number(process.env.ALVUM_EXTENSION_PORT || 0);
const token = process.env.ALVUM_EXTENSION_TOKEN || '';
const manifest = JSON.parse(await fs.readFile(new URL('../alvum.extension.json', import.meta.url), 'utf8'));

function send(res, status, body) {{
  const text = typeof body === 'string' ? body : JSON.stringify(body);
  res.writeHead(status, {{ 'content-type': typeof body === 'string' ? 'text/plain' : 'application/json' }});
  res.end(text);
}}

async function readJson(req) {{
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const text = Buffer.concat(chunks).toString('utf8') || '{{}}';
  return JSON.parse(text);
}}

function authorized(req) {{
  return req.headers.authorization === `Bearer ${{token}}`;
}}

const server = http.createServer(async (req, res) => {{
  if (req.url !== '/v1/health' && !authorized(req)) return send(res, 401, {{ error: 'unauthorized' }});
  if (req.method === 'GET' && req.url === '/v1/health') return send(res, 200, 'ok');
  if (req.method === 'GET' && req.url === '/v1/manifest') return send(res, 200, manifest);

  if (req.method === 'POST' && req.url === '/v1/gather') {{
    const body = await readJson(req);
    const ts = new Date().toISOString();
    return send(res, 200, {{
      data_refs: [{{
        ts,
        source: '{id}',
        producer: '{component}',
        schema: '{schema}',
        path: 'starter-events.jsonl',
        mime: 'application/x-jsonl',
        metadata: {{ connector: body.connector }}
      }}],
      observations: [],
      warnings: []
    }});
  }}

  if (req.method === 'POST' && req.url === '/v1/process') {{
    const body = await readJson(req);
    return send(res, 200, {{
      observations: (body.data_refs || []).map((ref) => ({{
        ts: ref.ts,
        source: ref.source,
        kind: 'custom',
        content: `Starter observation from ${{ref.path}}`,
        confidence: 0.5,
        refs: [ref]
      }})),
      warnings: []
    }});
  }}

  if (req.method === 'POST' && req.url === '/v1/capture/start') return send(res, 200, {{ run_id: 'starter' }});
  if (req.method === 'POST' && req.url === '/v1/capture/stop') return send(res, 200, {{ ok: true }});

  if (req.method === 'POST' && req.url === '/v1/analyze') {{
    const body = await readJson(req);
    return send(res, 200, {{
      artifacts: [{{
        relative_path: 'starter-analysis.md',
        mime: 'text/markdown',
        content: `# Starter analysis\n\nRan ${{body.analysis}} for ${{body.date}}.`
      }}],
      graph_overlays: [],
      warnings: []
    }});
  }}

  send(res, 404, {{ error: 'not found' }});
}});

server.listen(port, '127.0.0.1', () => {{
  console.log(`{id} listening on 127.0.0.1:${{port}}`);
}});
"#
    ))
}

fn write_external_connector_config(package: &str, connector: &str, enabled: bool) -> Result<()> {
    let config_path = alvum_core::config::config_path();
    let mut doc: toml::Table = if config_path.exists() {
        std::fs::read_to_string(&config_path)?.parse()?
    } else {
        toml::to_string(&alvum_core::config::AlvumConfig::default())?.parse()?
    };
    let connectors = doc
        .entry("connectors".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        .as_table_mut()
        .context("connectors is not a table")?;
    let key = if connector == "main" {
        package.to_string()
    } else {
        format!("{package}-{connector}")
    };
    let mut table = toml::Table::new();
    table.insert("enabled".into(), toml::Value::Boolean(enabled));
    table.insert("kind".into(), toml::Value::String("external-http".into()));
    table.insert("package".into(), toml::Value::String(package.into()));
    table.insert("connector".into(), toml::Value::String(connector.into()));
    connectors.insert(key, toml::Value::Table(table));
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, toml::to_string_pretty(&doc)?)?;
    Ok(())
}

fn disable_extension_config(package: &str) -> Result<()> {
    let config_path = alvum_core::config::config_path();
    if !config_path.exists() {
        return Ok(());
    }
    let mut doc: toml::Table = std::fs::read_to_string(&config_path)?.parse()?;
    if let Some(connectors) = doc.get_mut("connectors").and_then(|v| v.as_table_mut()) {
        for (_name, value) in connectors.iter_mut() {
            let Some(table) = value.as_table_mut() else {
                continue;
            };
            if table.get("package").and_then(|v| v.as_str()) == Some(package) {
                table.insert("enabled".into(), toml::Value::Boolean(false));
            }
        }
    }
    std::fs::write(config_path, toml::to_string_pretty(&doc)?)?;
    Ok(())
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
        "codex" | "codex-cli" => "", // let codex pick from its config
        "ollama" => "llama3.2",
        "bedrock" => "anthropic.claude-sonnet-4-20250514-v1:0",
        // claude-cli / anthropic-api / cli / api / auto / unknown
        _ => "claude-sonnet-4-6",
    }
}

fn default_model_for_config(provider: &str) -> String {
    let normalized = normalize_provider_name(provider);
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    provider_setting_string(&config, &normalized, "model")
        .unwrap_or_else(|| default_model_for(&normalized).into())
}

/// Each entry the popover renders. `available` reflects the cheap
/// detection check; an entry that's `available` may still fail at call
/// time if the user hasn't actually completed `claude login` etc. —
/// the Test action proves end-to-end auth.
#[derive(serde::Serialize)]
struct ProviderInfo {
    name: &'static str,
    display_name: &'static str,
    description: &'static str,
    enabled: bool,
    available: bool,
    auth_hint: &'static str,
    setup_kind: &'static str,
    setup_label: &'static str,
    setup_hint: &'static str,
    setup_command: Option<&'static str>,
    setup_url: Option<&'static str>,
    config_fields: Vec<ProviderConfigField>,
    active: bool,
}

#[derive(Clone, serde::Serialize)]
struct ProviderConfigField {
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    secret: bool,
    configured: bool,
    value: Option<String>,
    placeholder: &'static str,
    detail: &'static str,
    options: Vec<ProviderModelOption>,
}

#[derive(Clone, serde::Serialize)]
struct ProviderModelOption {
    value: String,
    label: String,
}

#[derive(Clone, serde::Serialize)]
struct ProviderInstallableModel {
    value: String,
    label: String,
    detail: String,
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
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let configured_raw = config.pipeline.provider.clone();
    // Legacy aliases — old install.sh wrote "cli", we now use canonical
    // provider names everywhere. Treat the legacy short form as equivalent
    // for "is this provider currently active" comparisons.
    let configured = normalize_provider_name(&configured_raw);

    let entries = provider_entries(&config);
    let auto_resolved = entries
        .iter()
        .find(|p| p.available && config.provider_enabled(p.name))
        .map(|p| p.name);

    let providers: Vec<ProviderInfo> = entries
        .into_iter()
        .map(|p| ProviderInfo {
            name: p.name,
            display_name: p.display_name,
            description: p.description,
            enabled: config.provider_enabled(p.name),
            available: p.available,
            auth_hint: p.auth_hint,
            setup_kind: p.setup_kind,
            setup_label: p.setup_label,
            setup_hint: p.setup_hint,
            setup_command: p.setup_command,
            setup_url: p.setup_url,
            config_fields: provider_config_fields(&config, p.name),
            active: configured == p.name || (configured == "auto" && Some(p.name) == auto_resolved),
        })
        .collect();

    let report = ProviderListReport {
        configured,
        auto_resolved,
        providers,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn known_provider_name(provider: &str) -> bool {
    provider == "auto" || known_provider_ids().iter().any(|entry| *entry == provider)
}

struct ProviderEntry {
    name: &'static str,
    display_name: &'static str,
    description: &'static str,
    available: bool,
    auth_hint: &'static str,
    setup_kind: &'static str,
    setup_label: &'static str,
    setup_hint: &'static str,
    setup_command: Option<&'static str>,
    setup_url: Option<&'static str>,
}

fn known_provider_ids() -> [&'static str; 5] {
    [
        "claude-cli",
        "codex-cli",
        "anthropic-api",
        "bedrock",
        "ollama",
    ]
}

fn provider_entries(config: &alvum_core::config::AlvumConfig) -> Vec<ProviderEntry> {
    vec![
        ProviderEntry {
            name: "claude-cli",
            display_name: "Claude CLI",
            description: "Uses the Claude Code subscription already logged in on this Mac.",
            available: cli_binary_on_path("claude"),
            auth_hint: "subscription via `claude login`",
            setup_kind: "terminal",
            setup_label: "Login",
            setup_hint: "Opens Terminal and runs `claude login`.",
            setup_command: Some("claude login"),
            setup_url: None,
        },
        ProviderEntry {
            name: "codex-cli",
            display_name: "Codex CLI",
            description: "Uses the Codex CLI subscription already logged in on this Mac.",
            available: cli_binary_on_path("codex"),
            auth_hint: "subscription via `codex login`",
            setup_kind: "terminal",
            setup_label: "Login",
            setup_hint: "Opens Terminal and runs `codex login`.",
            setup_command: Some("codex login"),
            setup_url: None,
        },
        ProviderEntry {
            name: "anthropic-api",
            display_name: "Anthropic API",
            description: "Uses an Anthropic API key stored in Keychain or the Alvum process environment.",
            available: anthropic_api_key_present(),
            auth_hint: "add an Anthropic API key",
            setup_kind: "inline",
            setup_label: "Setup",
            setup_hint: "Enter an Anthropic API key. Alvum stores it in macOS Keychain.",
            setup_command: None,
            setup_url: Some("https://console.anthropic.com/settings/keys"),
        },
        ProviderEntry {
            name: "bedrock",
            display_name: "AWS Bedrock",
            description: "Uses AWS credentials and Anthropic-on-Bedrock models.",
            available: aws_credentials_present(config),
            auth_hint: "configure an AWS profile or credentials",
            setup_kind: "inline",
            setup_label: "Setup",
            setup_hint: "Choose an AWS profile and region. Credentials still come from the standard AWS chain.",
            setup_command: Some("aws configure"),
            setup_url: None,
        },
        ProviderEntry {
            name: "ollama",
            display_name: "Ollama",
            description: "Uses a local Ollama server and local model.",
            available: cli_binary_on_path("ollama"),
            auth_hint: "install from ollama.com and `ollama run <model>`",
            setup_kind: "inline",
            setup_label: "Setup",
            setup_hint: "Set the local Ollama URL and model. `ollama serve` starts the server; if it says the address is already in use, Ollama is already running.",
            setup_command: Some("ollama serve"),
            setup_url: Some("https://ollama.com/download"),
        },
    ]
}

fn provider_setting_string(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &str,
) -> Option<String> {
    config
        .provider(provider)
        .and_then(|provider| provider.settings.get(key))
        .and_then(toml_value_to_string)
        .filter(|value| !value.trim().is_empty())
}

fn toml_value_to_string(value: &toml::Value) -> Option<String> {
    match value {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(n) => Some(n.to_string()),
        toml::Value::Float(n) => Some(n.to_string()),
        toml::Value::Boolean(v) => Some(v.to_string()),
        _ => None,
    }
}

fn config_field(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    detail: &'static str,
    placeholder: &'static str,
) -> ProviderConfigField {
    let value = provider_setting_string(config, provider, key);
    let options = if key == "model" {
        static_model_options(provider)
    } else {
        vec![]
    };
    ProviderConfigField {
        key,
        label,
        kind,
        secret: false,
        configured: value.is_some(),
        value,
        placeholder,
        detail,
        options,
    }
}

fn secret_field(
    provider: &str,
    key: &'static str,
    label: &'static str,
    detail: &'static str,
) -> ProviderConfigField {
    ProviderConfigField {
        key,
        label,
        kind: "secret",
        secret: true,
        configured: provider_secret_present(provider, key),
        value: None,
        placeholder: "Stored in Keychain",
        detail,
        options: vec![],
    }
}

fn provider_config_fields(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Vec<ProviderConfigField> {
    match provider {
        "anthropic-api" => vec![
            secret_field(
                provider,
                "api_key",
                "API key",
                "Stored in macOS Keychain. Environment variable ANTHROPIC_API_KEY still works.",
            ),
            config_field(
                config,
                provider,
                "model",
                "Model",
                "text",
                "Default model for Anthropic API calls.",
                default_model_for(provider),
            ),
        ],
        "bedrock" => vec![
            config_field(
                config,
                provider,
                "aws_profile",
                "AWS profile",
                "text",
                "Optional AWS profile name from ~/.aws/config or ~/.aws/credentials.",
                "default",
            ),
            config_field(
                config,
                provider,
                "aws_region",
                "AWS region",
                "text",
                "Optional AWS region for Bedrock.",
                "us-east-1",
            ),
            config_field(
                config,
                provider,
                "model",
                "Model",
                "text",
                "Bedrock model ID or inference profile ID.",
                default_model_for(provider),
            ),
        ],
        "ollama" => vec![
            config_field(
                config,
                provider,
                "base_url",
                "Server URL",
                "url",
                "Local Ollama API endpoint.",
                "http://localhost:11434",
            ),
            config_field(
                config,
                provider,
                "model",
                "Model",
                "text",
                "Local model to use for synthesis.",
                default_model_for(provider),
            ),
        ],
        "claude-cli" | "codex-cli" => vec![config_field(
            config,
            provider,
            "model",
            "Model",
            "text",
            "Optional model override. Leave blank to use the CLI default.",
            default_model_for(provider),
        )],
        _ => vec![],
    }
}

fn provider_secret_present(provider: &str, key: &str) -> bool {
    match (provider, key) {
        ("anthropic-api", "api_key") if std::env::var("ANTHROPIC_API_KEY").is_ok() => true,
        _ => alvum_core::keychain::provider_secret_available(provider, key),
    }
}

fn anthropic_api_key_present() -> bool {
    provider_secret_present("anthropic-api", "api_key")
}

fn model_option(value: impl Into<String>, label: impl Into<String>) -> ProviderModelOption {
    ProviderModelOption {
        value: value.into(),
        label: label.into(),
    }
}

fn installable_model(
    value: impl Into<String>,
    label: impl Into<String>,
    detail: impl Into<String>,
) -> ProviderInstallableModel {
    ProviderInstallableModel {
        value: value.into(),
        label: label.into(),
        detail: detail.into(),
    }
}

fn dedupe_model_options(options: Vec<ProviderModelOption>) -> Vec<ProviderModelOption> {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();
    for option in options {
        if option.value.trim().is_empty() && seen.contains("") {
            continue;
        }
        if !seen.insert(option.value.clone()) {
            continue;
        }
        deduped.push(option);
    }
    deduped
}

fn ollama_installable_models() -> Vec<ProviderInstallableModel> {
    vec![
        installable_model(
            "gemma4:e2b",
            "Gemma 4 E2B",
            "Small edge model; good first Ollama download for laptops.",
        ),
        installable_model(
            "gemma4:e4b",
            "Gemma 4 E4B",
            "Stronger edge model when you have more memory available.",
        ),
        installable_model("gemma4", "Gemma 4", "Default Gemma 4 local model."),
        installable_model(
            "llama3.2",
            "Llama 3.2",
            "Compact general-purpose local model.",
        ),
        installable_model(
            "qwen3:4b",
            "Qwen 3 4B",
            "Small reasoning-oriented local model.",
        ),
        installable_model(
            "mistral",
            "Mistral",
            "Reliable lightweight general-purpose model.",
        ),
    ]
}

fn static_model_options(provider: &str) -> Vec<ProviderModelOption> {
    match provider {
        "claude-cli" => vec![
            model_option("sonnet", "Sonnet"),
            model_option("opus", "Opus"),
            model_option(default_model_for(provider), default_model_for(provider)),
        ],
        "codex-cli" => vec![model_option("", "CLI default")],
        "anthropic-api" => vec![model_option(
            default_model_for(provider),
            default_model_for(provider),
        )],
        "bedrock" => vec![model_option(
            default_model_for(provider),
            default_model_for(provider),
        )],
        "ollama" => vec![model_option(
            default_model_for(provider),
            default_model_for(provider),
        )],
        _ => vec![],
    }
}

fn cli_binary_on_path(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn aws_credentials_present(config: &alvum_core::config::AlvumConfig) -> bool {
    std::env::var("AWS_PROFILE").is_ok()
        || std::env::var("AWS_ACCESS_KEY_ID").is_ok()
        || std::env::var("AWS_SESSION_TOKEN").is_ok()
        || provider_setting_string(config, "bedrock", "aws_profile").is_some()
        || dirs::home_dir()
            .map(|h| h.join(".aws/credentials").exists() || h.join(".aws/config").exists())
            .unwrap_or(false)
}

const PROVIDER_TEST_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, serde::Serialize)]
struct ProviderTestReport {
    provider: String,
    status: String,
    ok: bool,
    elapsed_ms: u128,
    response_preview: Option<String>,
    error: Option<String>,
}

async fn provider_test_report(provider_name: &str, model: &str) -> ProviderTestReport {
    // Tiny prompt. The expected response is "OK" — anything containing
    // it counts as success. Some providers may include leading
    // whitespace or quote marks, hence the contains() check.
    const TEST_SYSTEM: &str =
        "You are a connectivity probe. Reply with the exact word OK and nothing else.";
    const TEST_USER: &str = "ping";
    let started = std::time::Instant::now();
    let normalized = normalize_provider_name(provider_name);

    if !known_provider_name(&normalized) || normalized == "auto" {
        return ProviderTestReport {
            provider: normalized,
            status: "unknown_provider".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("unknown provider: {provider_name}")),
        };
    }

    if normalized == "ollama" {
        return ollama_provider_test_report(model, started).await;
    }

    let probe = async {
        let provider = alvum_pipeline::llm::create_provider_async(&normalized, model)
            .await
            .with_context(|| format!("provider construction failed for {normalized}"))?;
        provider.complete(TEST_SYSTEM, TEST_USER).await
    };

    match tokio::time::timeout(PROVIDER_TEST_TIMEOUT, probe).await {
        Err(_) => ProviderTestReport {
            provider: normalized,
            status: "timeout".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!(
                "provider probe timed out after {}s",
                PROVIDER_TEST_TIMEOUT.as_secs()
            )),
        },
        Ok(Ok(text)) => {
            let preview: String = text.chars().take(80).collect();
            let ok = text.to_uppercase().contains("OK");
            ProviderTestReport {
                provider: normalized,
                status: if ok {
                    "available".into()
                } else {
                    "unexpected_response".into()
                },
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
        Ok(Err(e)) => ProviderTestReport {
            provider: normalized,
            status: alvum_pipeline::llm::classify_provider_error_status(&e).into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("{e:#}")),
        },
    }
}

async fn ollama_provider_test_report(
    model: &str,
    started: std::time::Instant,
) -> ProviderTestReport {
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    match tokio::time::timeout(PROVIDER_TEST_TIMEOUT, ollama_model_options(&config)).await {
        Err(_) => ProviderTestReport {
            provider: "ollama".into(),
            status: "timeout".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!(
                "Ollama model list timed out after {}s",
                PROVIDER_TEST_TIMEOUT.as_secs()
            )),
        },
        Ok(Err(e)) => ProviderTestReport {
            provider: "ollama".into(),
            status: "unavailable".into(),
            ok: false,
            elapsed_ms: started.elapsed().as_millis(),
            response_preview: None,
            error: Some(format!("{e:#}")),
        },
        Ok(Ok((source, options))) => {
            let requested = model.trim();
            let installed = options.iter().any(|option| option.value == requested);
            let has_models = !options.is_empty();
            let ok = has_models && (requested.is_empty() || installed);
            ProviderTestReport {
                provider: "ollama".into(),
                status: if ok {
                    "available".into()
                } else if has_models {
                    "model_not_installed".into()
                } else {
                    "no_models".into()
                },
                ok,
                elapsed_ms: started.elapsed().as_millis(),
                response_preview: Some(format!(
                    "{} installed model(s) from {source}",
                    options.len()
                )),
                error: if ok {
                    None
                } else if has_models {
                    Some(format!(
                        "Ollama is running, but model {requested:?} is not installed. Choose an installed model or download it."
                    ))
                } else {
                    Some("Ollama is running, but no local models are installed.".into())
                },
            }
        }
    }
}

async fn cmd_providers_test(provider_name: &str, model: &str) -> Result<()> {
    let report = provider_test_report(provider_name, model).await;

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

const PROVIDER_MODELS_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(serde::Serialize)]
struct ProviderModelsReport {
    ok: bool,
    provider: String,
    source: String,
    options: Vec<ProviderModelOption>,
    installable_options: Vec<ProviderInstallableModel>,
    error: Option<String>,
}

fn model_options_with_config(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    options: Vec<ProviderModelOption>,
) -> Vec<ProviderModelOption> {
    let mut merged = Vec::new();
    if let Some(current) = provider_setting_string(config, provider, "model") {
        merged.push(model_option(current.clone(), current));
    }
    if provider == "codex-cli" {
        merged.push(model_option("", "CLI default"));
    }
    merged.extend(options);
    dedupe_model_options(merged)
}

async fn run_json_command(
    command: &str,
    args: &[String],
    timeout: Duration,
) -> Result<serde_json::Value> {
    let output = tokio::time::timeout(timeout, async {
        tokio::process::Command::new(command)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
    })
    .await
    .with_context(|| format!("{command} timed out after {}s", timeout.as_secs()))?
    .with_context(|| format!("failed to run {command}"))?;

    if !output.status.success() {
        bail!(
            "{command} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("{command} returned malformed JSON"))
}

async fn codex_model_options() -> Result<Vec<ProviderModelOption>> {
    let json = run_json_command(
        "codex",
        &["debug".into(), "models".into()],
        PROVIDER_MODELS_TIMEOUT,
    )
    .await?;
    let options = json
        .get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter(|model| {
            model
                .get("visibility")
                .and_then(|value| value.as_str())
                .map(|visibility| visibility == "list")
                .unwrap_or(true)
        })
        .filter_map(|model| {
            let slug = model.get("slug").and_then(|value| value.as_str())?;
            let label = model
                .get("display_name")
                .and_then(|value| value.as_str())
                .unwrap_or(slug);
            Some(model_option(slug, label))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

async fn ollama_api_model_options(
    config: &alvum_core::config::AlvumConfig,
) -> Result<Vec<ProviderModelOption>> {
    let base_url = provider_setting_string(config, "ollama", "base_url")
        .unwrap_or_else(|| "http://localhost:11434".into())
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(PROVIDER_MODELS_TIMEOUT)
        .build()?;
    let json: serde_json::Value = client
        .get(format!("{base_url}/api/tags"))
        .send()
        .await
        .context("failed to query Ollama models")?
        .error_for_status()
        .context("Ollama model list request failed")?
        .json()
        .await
        .context("Ollama returned malformed model list JSON")?;
    let options = json
        .get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let name = model
                .get("model")
                .or_else(|| model.get("name"))
                .and_then(|value| value.as_str())?;
            Some(model_option(name, name))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

async fn ollama_cli_model_options() -> Result<Vec<ProviderModelOption>> {
    let output = tokio::time::timeout(PROVIDER_MODELS_TIMEOUT, async {
        tokio::process::Command::new("ollama")
            .arg("ls")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
    })
    .await
    .with_context(|| {
        format!(
            "ollama ls timed out after {}s",
            PROVIDER_MODELS_TIMEOUT.as_secs()
        )
    })?
    .context("failed to run ollama ls")?;

    if !output.status.success() {
        bail!(
            "ollama ls exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let options = stdout
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| !name.trim().is_empty())
        .map(|name| model_option(name, name))
        .collect::<Vec<_>>();
    Ok(options)
}

async fn ollama_model_options(
    config: &alvum_core::config::AlvumConfig,
) -> Result<(String, Vec<ProviderModelOption>)> {
    match ollama_api_model_options(config).await {
        Ok(options) => Ok(("ollama".into(), options)),
        Err(api_error) => match ollama_cli_model_options().await {
            Ok(options) => Ok(("ollama-cli".into(), options)),
            Err(cli_error) => {
                Err(api_error).context(format!("ollama ls fallback failed: {cli_error:#}"))
            }
        },
    }
}

async fn anthropic_model_options() -> Result<Vec<ProviderModelOption>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|key| !key.trim().is_empty())
        .or_else(|| {
            alvum_core::keychain::read_provider_secret("anthropic-api", "api_key")
                .ok()
                .flatten()
        })
        .context("Anthropic API key is not configured")?;
    let client = reqwest::Client::builder()
        .timeout(PROVIDER_MODELS_TIMEOUT)
        .build()?;
    let json: serde_json::Value = client
        .get("https://api.anthropic.com/v1/models")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .context("failed to query Anthropic models")?
        .error_for_status()
        .context("Anthropic model list request failed")?
        .json()
        .await
        .context("Anthropic returned malformed model list JSON")?;
    let options = json
        .get("data")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("id").and_then(|value| value.as_str())?;
            let label = model
                .get("display_name")
                .and_then(|value| value.as_str())
                .unwrap_or(id);
            Some(model_option(id, label))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

async fn bedrock_model_options(
    config: &alvum_core::config::AlvumConfig,
) -> Result<Vec<ProviderModelOption>> {
    let mut args = vec![
        "bedrock".to_string(),
        "list-foundation-models".to_string(),
        "--by-provider".to_string(),
        "Anthropic".to_string(),
        "--output".to_string(),
        "json".to_string(),
    ];
    if let Some(region) = provider_setting_string(config, "bedrock", "aws_region") {
        args.push("--region".into());
        args.push(region);
    }
    if let Some(profile) = provider_setting_string(config, "bedrock", "aws_profile") {
        args.push("--profile".into());
        args.push(profile);
    }
    let json = run_json_command("aws", &args, PROVIDER_MODELS_TIMEOUT).await?;
    let options = json
        .get("modelSummaries")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            let id = model.get("modelId").and_then(|value| value.as_str())?;
            Some(model_option(id, id))
        })
        .collect::<Vec<_>>();
    Ok(options)
}

async fn live_model_options(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
) -> Result<(String, Vec<ProviderModelOption>)> {
    match provider {
        "claude-cli" => Ok(("static".into(), static_model_options(provider))),
        "codex-cli" => Ok(("codex-cli".into(), codex_model_options().await?)),
        "anthropic-api" => Ok(("anthropic-api".into(), anthropic_model_options().await?)),
        "bedrock" => Ok(("aws-bedrock".into(), bedrock_model_options(config).await?)),
        "ollama" => ollama_model_options(config).await,
        _ => bail!("unknown provider: {provider}"),
    }
}

async fn cmd_providers_models(provider_name: &str) -> Result<()> {
    let normalized = normalize_provider_name(provider_name);
    if normalized == "auto" || !known_provider_name(&normalized) {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderModelsReport {
                ok: false,
                provider: normalized,
                source: "none".into(),
                options: vec![],
                installable_options: vec![],
                error: Some(format!("unknown provider: {provider_name}")),
            })?
        );
        return Ok(());
    }

    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    let fallback =
        model_options_with_config(&config, &normalized, static_model_options(&normalized));
    let installable_options = if normalized == "ollama" {
        ollama_installable_models()
    } else {
        vec![]
    };
    let report = match live_model_options(&config, &normalized).await {
        Ok((source, options)) if !options.is_empty() => ProviderModelsReport {
            ok: true,
            provider: normalized.clone(),
            source,
            options: model_options_with_config(&config, &normalized, options),
            installable_options,
            error: None,
        },
        Ok((source, _)) => ProviderModelsReport {
            ok: false,
            provider: normalized.clone(),
            source,
            options: fallback,
            installable_options,
            error: Some("provider returned no model options".into()),
        },
        Err(e) => ProviderModelsReport {
            ok: false,
            provider: normalized.clone(),
            source: "fallback".into(),
            options: fallback,
            installable_options,
            error: Some(format!("{e:#}")),
        },
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

const PROVIDER_MODEL_INSTALL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(serde::Serialize)]
struct ProviderInstallModelReport {
    ok: bool,
    provider: String,
    model: String,
    status: String,
    elapsed_ms: u128,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
    error: Option<String>,
}

fn tail_string(value: &str, max_chars: usize) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let char_count = trimmed.chars().count();
    if char_count <= max_chars {
        return Some(trimmed.to_string());
    }
    Some(trimmed.chars().skip(char_count - max_chars).collect())
}

fn valid_ollama_model_ref(model: &str) -> bool {
    let model = model.trim();
    !model.is_empty()
        && model.len() <= 160
        && !model.starts_with('-')
        && model
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
}

async fn cmd_providers_install_model(provider_name: &str, model: &str) -> Result<()> {
    let normalized = normalize_provider_name(provider_name);
    let started = std::time::Instant::now();

    if normalized != "ollama" {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderInstallModelReport {
                ok: false,
                provider: normalized,
                model: model.into(),
                status: "unsupported_provider".into(),
                elapsed_ms: started.elapsed().as_millis(),
                stdout_tail: None,
                stderr_tail: None,
                error: Some("model downloads are currently supported for Ollama only".into()),
            })?
        );
        return Ok(());
    }

    if !valid_ollama_model_ref(model) {
        println!(
            "{}",
            serde_json::to_string_pretty(&ProviderInstallModelReport {
                ok: false,
                provider: normalized,
                model: model.into(),
                status: "invalid_model".into(),
                elapsed_ms: started.elapsed().as_millis(),
                stdout_tail: None,
                stderr_tail: None,
                error: Some(
                    "Ollama model names may only contain letters, numbers, ., _, -, :, and /"
                        .into()
                ),
            })?
        );
        return Ok(());
    }

    let mut command = tokio::process::Command::new("ollama");
    command
        .args(["pull", model])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(PROVIDER_MODEL_INSTALL_TIMEOUT, command.output()).await;
    let report = match output {
        Err(_) => ProviderInstallModelReport {
            ok: false,
            provider: normalized,
            model: model.into(),
            status: "timeout".into(),
            elapsed_ms: started.elapsed().as_millis(),
            stdout_tail: None,
            stderr_tail: None,
            error: Some(format!(
                "ollama pull timed out after {}s",
                PROVIDER_MODEL_INSTALL_TIMEOUT.as_secs()
            )),
        },
        Ok(Err(e)) => ProviderInstallModelReport {
            ok: false,
            provider: normalized,
            model: model.into(),
            status: "spawn_error".into(),
            elapsed_ms: started.elapsed().as_millis(),
            stdout_tail: None,
            stderr_tail: None,
            error: Some(format!("failed to run ollama pull: {e}")),
        },
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            ProviderInstallModelReport {
                ok: output.status.success(),
                provider: normalized,
                model: model.into(),
                status: if output.status.success() {
                    "installed".into()
                } else {
                    "failed".into()
                },
                elapsed_ms: started.elapsed().as_millis(),
                stdout_tail: tail_string(&stdout, 1200),
                stderr_tail: tail_string(&stderr, 1200),
                error: if output.status.success() {
                    None
                } else {
                    Some(format!("ollama pull exited {}", output.status))
                },
            }
        }
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(serde::Serialize)]
struct ProviderBootstrapReport {
    ok: bool,
    skipped: bool,
    reason: Option<String>,
    enabled: Vec<String>,
    providers: Vec<ProviderTestReport>,
}

fn provider_bootstrap_marker_path() -> PathBuf {
    alvum_core::config::config_path()
        .parent()
        .map(|p| p.join("provider-bootstrap.json"))
        .unwrap_or_else(|| PathBuf::from("provider-bootstrap.json"))
}

fn provider_bootstrap_done() -> bool {
    provider_bootstrap_marker_path().exists()
}

fn provider_config_looks_uninitialized(config_path: &Path, doc: &toml::Table) -> bool {
    if !config_path.exists() {
        return true;
    }
    let configured = doc
        .get("pipeline")
        .and_then(|v| v.as_table())
        .and_then(|pipeline| pipeline.get("provider"))
        .and_then(|v| v.as_str())
        .map(normalize_provider_name)
        .unwrap_or_else(|| "auto".into());
    if configured != "auto" {
        return false;
    }

    let Some(providers) = doc.get("providers").and_then(|v| v.as_table()) else {
        return true;
    };

    known_provider_ids().iter().all(|provider| {
        let Some(value) = providers.get(*provider) else {
            return true;
        };
        let Some(table) = value.as_table() else {
            return false;
        };
        let enabled = table
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        enabled && table.keys().all(|key| key == "enabled")
    })
}

fn write_provider_bootstrap_marker(report: &ProviderBootstrapReport) -> Result<()> {
    let path = provider_bootstrap_marker_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(&serde_json::json!({
            "bootstrapped_at": Utc::now().to_rfc3339(),
            "enabled": &report.enabled,
        }))?,
    )?;
    Ok(())
}

async fn cmd_providers_bootstrap(force: bool) -> Result<()> {
    let (config_path, mut doc) = load_config_doc()?;
    if !force && provider_bootstrap_done() {
        let report = ProviderBootstrapReport {
            ok: true,
            skipped: true,
            reason: Some("provider bootstrap already completed".into()),
            enabled: vec![],
            providers: vec![],
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if !force && !provider_config_looks_uninitialized(&config_path, &doc) {
        let report = ProviderBootstrapReport {
            ok: true,
            skipped: true,
            reason: Some("provider config already customized".into()),
            enabled: vec![],
            providers: vec![],
        };
        write_provider_bootstrap_marker(&report)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let config_for_entries: alvum_core::config::AlvumConfig =
        toml::from_str(&toml::to_string(&doc)?)?;
    let entries = provider_entries(&config_for_entries);
    let mut reports = Vec::new();
    for entry in &entries {
        let report = if entry.available {
            provider_test_report(entry.name, default_model_for(entry.name)).await
        } else {
            ProviderTestReport {
                provider: entry.name.into(),
                status: "not_installed".into(),
                ok: false,
                elapsed_ms: 0,
                response_preview: None,
                error: Some(entry.auth_hint.into()),
            }
        };
        reports.push(report);
    }

    let enabled = reports
        .iter()
        .filter(|report| report.ok)
        .map(|report| report.provider.clone())
        .collect::<Vec<_>>();
    for entry in &entries {
        set_config_doc_value(
            &mut doc,
            &format!("providers.{}.enabled", entry.name),
            toml::Value::Boolean(enabled.iter().any(|name| name == entry.name)),
        )?;
    }
    set_config_doc_value(
        &mut doc,
        "pipeline.provider",
        toml::Value::String("auto".into()),
    )?;
    save_config_doc(&config_path, &doc)?;

    let report = ProviderBootstrapReport {
        ok: true,
        skipped: false,
        reason: None,
        enabled,
        providers: reports,
    };
    write_provider_bootstrap_marker(&report)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(serde::Serialize)]
struct ProviderMutationReport {
    ok: bool,
    provider: String,
    configured: String,
    enabled: Option<bool>,
}

#[derive(serde::Deserialize)]
struct ProviderConfigureRequest {
    #[serde(default)]
    settings: HashMap<String, serde_json::Value>,
    #[serde(default)]
    secrets: HashMap<String, String>,
    enabled: Option<bool>,
}

#[derive(serde::Serialize)]
struct ProviderConfigureReport {
    ok: bool,
    provider: String,
    configured: String,
    enabled: bool,
    saved_settings: Vec<String>,
    saved_secrets: Vec<String>,
}

fn json_value_to_toml(value: serde_json::Value) -> Result<toml::Value> {
    Ok(match value {
        serde_json::Value::Null => toml::Value::String(String::new()),
        serde_json::Value::Bool(v) => toml::Value::Boolean(v),
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                toml::Value::Integer(v)
            } else if let Some(v) = n.as_f64() {
                toml::Value::Float(v)
            } else {
                bail!("unsupported numeric provider setting")
            }
        }
        serde_json::Value::String(v) => toml::Value::String(v),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            bail!("provider settings must be scalar values")
        }
    })
}

fn cmd_providers_configure(provider: &str) -> Result<()> {
    let normalized = normalize_provider_name(provider);
    if normalized == "auto" || !known_provider_name(&normalized) {
        bail!("unknown provider: {normalized}");
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("failed to read provider config JSON from stdin")?;
    let request: ProviderConfigureRequest = if input.trim().is_empty() {
        ProviderConfigureRequest {
            settings: HashMap::new(),
            secrets: HashMap::new(),
            enabled: None,
        }
    } else {
        serde_json::from_str(&input).context("failed to parse provider config JSON")?
    };

    let (config_path, mut doc) = load_config_doc()?;
    let config: alvum_core::config::AlvumConfig = toml::from_str(&toml::to_string(&doc)?)?;
    let fields = provider_config_fields(&config, &normalized);
    let mut saved_settings = Vec::new();
    let mut saved_secrets = Vec::new();

    for (key, value) in request.settings {
        let Some(field) = fields
            .iter()
            .find(|field| field.key == key && !field.secret)
        else {
            bail!("unknown provider setting for {normalized}: {key}");
        };
        set_config_doc_value(
            &mut doc,
            &format!("providers.{normalized}.{}", field.key),
            json_value_to_toml(value)?,
        )?;
        saved_settings.push(field.key.to_string());
    }

    for (key, secret) in request.secrets {
        let Some(field) = fields.iter().find(|field| field.key == key && field.secret) else {
            bail!("unknown provider secret for {normalized}: {key}");
        };
        if !secret.is_empty() {
            alvum_core::keychain::write_provider_secret(&normalized, field.key, &secret)?;
            saved_secrets.push(field.key.to_string());
        }
    }

    if let Some(enabled) = request.enabled {
        set_config_doc_value(
            &mut doc,
            &format!("providers.{normalized}.enabled"),
            toml::Value::Boolean(enabled),
        )?;
    }
    save_config_doc(&config_path, &doc)?;

    let configured = doc
        .get("pipeline")
        .and_then(|v| v.as_table())
        .and_then(|pipeline| pipeline.get("provider"))
        .and_then(|v| v.as_str())
        .map(normalize_provider_name)
        .unwrap_or_else(|| "auto".into());
    let enabled = doc
        .get("providers")
        .and_then(|v| v.as_table())
        .and_then(|providers| providers.get(&normalized))
        .and_then(|v| v.as_table())
        .and_then(|provider| provider.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    println!(
        "{}",
        serde_json::to_string_pretty(&ProviderConfigureReport {
            ok: true,
            provider: normalized,
            configured,
            enabled,
            saved_settings,
            saved_secrets,
        })?
    );
    Ok(())
}

fn cmd_providers_set_active(provider: &str) -> Result<()> {
    let normalized = normalize_provider_name(provider);
    if !known_provider_name(&normalized) {
        bail!("unknown provider: {normalized}");
    }
    let (config_path, mut doc) = load_config_doc()?;
    set_config_doc_value(
        &mut doc,
        "pipeline.provider",
        toml::Value::String(normalized.clone()),
    )?;
    save_config_doc(&config_path, &doc)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&ProviderMutationReport {
            ok: true,
            provider: normalized.clone(),
            configured: normalized,
            enabled: None,
        })?
    );
    Ok(())
}

fn cmd_providers_set_enabled(provider: &str, enabled: bool) -> Result<()> {
    let normalized = normalize_provider_name(provider);
    if normalized == "auto" || !known_provider_name(&normalized) {
        bail!("unknown provider: {normalized}");
    }

    let (config_path, mut doc) = load_config_doc()?;
    set_config_doc_value(
        &mut doc,
        &format!("providers.{normalized}.enabled"),
        toml::Value::Boolean(enabled),
    )?;

    let configured = doc
        .get("pipeline")
        .and_then(|v| v.as_table())
        .and_then(|pipeline| pipeline.get("provider"))
        .and_then(|v| v.as_str())
        .map(normalize_provider_name)
        .unwrap_or_else(|| "auto".into());
    let next_configured = if !enabled && configured == normalized {
        set_config_doc_value(
            &mut doc,
            "pipeline.provider",
            toml::Value::String("auto".into()),
        )?;
        "auto".to_string()
    } else {
        configured
    };

    save_config_doc(&config_path, &doc)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&ProviderMutationReport {
            ok: true,
            provider: normalized,
            configured: next_configured,
            enabled: Some(enabled),
        })?
    );
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
    match alvum_connector_external::capture_sources_from_config(&config) {
        Ok(mut external_sources) => {
            if let Some(ref only_str) = only {
                let only_set: Vec<&str> = only_str.split(',').map(|s| s.trim()).collect();
                external_sources.retain(|source| only_set.contains(&source.name()));
            }
            if let Some(ref disable_str) = disable {
                let disable_set: Vec<&str> = disable_str.split(',').map(|s| s.trim()).collect();
                external_sources.retain(|source| !disable_set.contains(&source.name()));
            }
            for source in external_sources {
                info!(source = source.name(), "created external capture source");
                source_impls.push(source);
            }
        }
        Err(e) => warn!(error = %e, "failed to create external capture sources"),
    }

    if source_impls.is_empty() {
        println!("No capture sources could be initialized.");
        return Ok(());
    }

    // Shared shutdown channel
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    // Print status
    let source_names: Vec<&str> = source_impls.iter().map(|s| s.name()).collect();
    println!(
        "Capturing: {} — Press Ctrl-C to stop.",
        source_names.join(", ")
    );

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
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
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
            alvum_capture_audio::source::AudioMicSource::from_config(config),
        )),
        "audio-system" => Ok(Box::new(
            alvum_capture_audio::source::AudioSystemSource::from_config(config),
        )),
        "screen" => Ok(Box::new(
            alvum_capture_screen::source::ScreenSource::from_config(&config.settings),
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

fn load_config_doc() -> Result<(std::path::PathBuf, toml::Table)> {
    let config_path = alvum_core::config::config_path();
    let doc: toml::Table = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        content.parse().context("failed to parse config")?
    } else {
        let config = alvum_core::config::AlvumConfig::default();
        let toml_str = toml::to_string(&config)?;
        toml_str.parse().context("failed to serialize defaults")?
    };
    Ok((config_path, doc))
}

fn set_config_doc_value(doc: &mut toml::Table, key: &str, value: toml::Value) -> Result<()> {
    // Parse the dotted key path (e.g., "capture.audio-mic.device")
    let parts: Vec<&str> = key.split('.').collect();
    if parts.len() < 2 {
        bail!("key must be dotted path (e.g., capture.screen.enabled)");
    }

    // Navigate to the parent table, creating intermediate tables as needed
    let mut current = doc;
    for part in &parts[..parts.len() - 1] {
        current = current
            .entry(part.to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .with_context(|| format!("{part} is not a table"))?;
    }

    let leaf = parts.last().unwrap();
    current.insert(leaf.to_string(), value);
    Ok(())
}

fn save_config_doc(config_path: &std::path::Path, doc: &toml::Table) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, toml::to_string_pretty(doc)?)?;
    Ok(())
}

fn parse_config_value(value: &str) -> toml::Value {
    // Infer type from value string
    if value == "true" {
        toml::Value::Boolean(true)
    } else if value == "false" {
        toml::Value::Boolean(false)
    } else if let Ok(n) = value.parse::<i64>() {
        toml::Value::Integer(n)
    } else if let Ok(f) = value.parse::<f64>() {
        toml::Value::Float(f)
    } else {
        toml::Value::String(value.to_string())
    }
}

fn cmd_config_set(key: &str, value: &str) -> Result<()> {
    let (config_path, mut doc) = load_config_doc()?;
    let toml_value = parse_config_value(value);
    set_config_doc_value(&mut doc, key, toml_value.clone())?;
    save_config_doc(&config_path, &doc)?;

    println!("{key} = {toml_value}");
    println!("Saved to {}", config_path.display());
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
    _source: Option<String>,   // legacy, ignored
    _session: Option<PathBuf>, // legacy, ignored
    output: PathBuf,
    provider_name: Option<String>,
    model: String,
    before: Option<String>,
    since: Option<String>,
    briefing_date: Option<String>,
    capture_dir: Option<PathBuf>,
    _whisper_model: Option<PathBuf>, // now read from processor config
    relevance_threshold: f32,
    _vision: Option<String>, // now read from processor config
    resume: bool,
    no_skip_processed: bool,
) -> Result<()> {
    let capture_dir = capture_dir.context("--capture-dir required")?;
    let config = alvum_core::config::AlvumConfig::load()?;
    let provider_name = provider_name.unwrap_or_else(|| config.pipeline.provider.clone());

    // Provider built from flags — convert Box to Arc for sharing across connectors.
    // Use the async builder so `auto` and `bedrock` work; non-async providers
    // fall through unchanged.
    let provider_box = alvum_pipeline::llm::create_provider_async(&provider_name, &model).await?;
    let provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider> = provider_box.into();

    let before_ts = match before {
        Some(value) => Some(
            value
                .parse::<DateTime<Utc>>()
                .with_context(|| format!("invalid --before timestamp: {value}"))?,
        ),
        None => None,
    };
    let since_ts = match since {
        Some(value) => Some(
            value
                .parse::<DateTime<Utc>>()
                .with_context(|| format!("invalid --since timestamp: {value}"))?,
        ),
        None => None,
    };

    let connectors = connectors_from_config(&config, provider.clone(), since_ts, before_ts);

    if connectors.is_empty() {
        println!("No connectors enabled. Check config.");
        return Ok(());
    }

    let names: Vec<&str> = connectors.iter().map(|c| c.name()).collect();
    println!("Running connectors: {}", names.join(", "));

    let extract_config = alvum_pipeline::extract::ExtractConfig {
        capture_dir: capture_dir.clone(),
        output_dir: output.clone(),
        relevance_threshold,
        resume,
        no_skip_processed,
        briefing_date: briefing_date.clone(),
    };

    let result =
        alvum_pipeline::extract::extract_and_pipeline(connectors, provider.clone(), extract_config)
            .await?;

    println!(
        "\nExtracted {} decisions from {} observations across {} threads.",
        result.result.decisions.len(),
        result.observations.len(),
        result.threading.threads.len(),
    );
    println!("\nOutput: {}", output.display());
    println!("\n{}", "=".repeat(60));
    println!("{}", result.result.briefing);

    let analysis_date = briefing_date
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    match alvum_connector_external::run_enabled_analyses(
        &config,
        &analysis_date,
        &capture_dir,
        &output,
        provider.clone(),
    )
    .await
    {
        Ok(results) if !results.is_empty() => {
            println!("\nRan {} extension analysis lens(es).", results.len());
        }
        Ok(_) => {}
        Err(e) => warn!(error = %e, "extension analyses failed"),
    }

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
        Some(
            tokio::fs::File::open(&path)
                .await
                .with_context(|| format!("failed to open {}", path.display()))?,
        )
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
            value
                .get("elapsed_ms")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value.get("ok").map(|v| v.to_string()).unwrap_or_default(),
            value
                .get("extras")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "input_inventory" => format!(
            "{}/{} ref_count={}",
            str_field(value, "connector"),
            str_field(value, "source"),
            value
                .get("ref_count")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "llm_call_start" => format!(
            "provider={} call_site={} prompt_chars={} prompt_tokens≈{}",
            str_field(value, "provider"),
            str_field(value, "call_site"),
            value
                .get("prompt_chars")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("prompt_tokens_estimate")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "llm_call_end" => format!(
            "provider={} call_site={} latency_ms={} output_tokens={} output_tokens≈{} tok_sec={} tok_sec≈{} attempts={} ok={}",
            str_field(value, "provider"),
            str_field(value, "call_site"),
            value
                .get("latency_ms")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("output_tokens")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("response_tokens_estimate")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("tokens_per_sec")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("tokens_per_sec_estimate")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("attempts")
                .map(|v| v.to_string())
                .unwrap_or_default(),
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
            value
                .get("dropped")
                .map(|v| v.to_string())
                .unwrap_or_default(),
            value
                .get("reasons")
                .map(|v| v.to_string())
                .unwrap_or_default(),
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
