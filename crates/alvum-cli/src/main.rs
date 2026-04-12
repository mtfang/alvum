//! CLI entry point for alvum.
//!
//! Subcommands:
//! - `alvum record` — start audio recording (mic + system)
//! - `alvum devices` — list available audio devices
//! - `alvum extract` — extract decisions from data sources
//! - `alvum config-init` — initialize a default config file
//! - `alvum config-show` — show current configuration
//! - `alvum connectors` — list connectors and their status

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "alvum", about = "Life decision tracking and alignment engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start audio recording (mic + system audio)
    Record {
        /// Capture directory (default: ./capture/<today>)
        #[arg(long)]
        capture_dir: Option<PathBuf>,

        /// Microphone device name (default: system default)
        #[arg(long)]
        mic: Option<String>,

        /// System audio device name (default: system default, "off" to disable)
        #[arg(long)]
        system: Option<String>,
    },

    /// List available audio devices
    Devices,

    /// Initialize a default config file
    #[command(name = "config-init")]
    ConfigInit,

    /// Show current configuration
    #[command(name = "config-show")]
    ConfigShow,

    /// List connectors and their status
    Connectors,

    /// Start screen capture (active window screenshots)
    #[command(name = "capture-screen")]
    CaptureScreen {
        /// Capture directory (default: ./capture/<today>)
        #[arg(long)]
        capture_dir: Option<PathBuf>,
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

        /// Path to Whisper model file (for --source audio)
        #[arg(long)]
        whisper_model: Option<PathBuf>,

        /// Minimum relevance score for threads sent to decision extraction (0.0-1.0)
        #[arg(long, default_value = "0.5")]
        relevance_threshold: f32,
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
        Commands::Record { capture_dir, mic, system } => {
            cmd_record(capture_dir, mic, system).await
        }
        Commands::Devices => {
            cmd_devices()
        }
        Commands::ConfigInit => cmd_config_init(),
        Commands::ConfigShow => cmd_config_show(),
        Commands::Connectors => cmd_connectors(),
        Commands::CaptureScreen { capture_dir } => {
            cmd_capture_screen(capture_dir).await
        }
        Commands::Extract { source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold } => {
            cmd_extract(source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold).await
        }
    }
}

async fn cmd_record(
    capture_dir: Option<PathBuf>,
    mic: Option<String>,
    system: Option<String>,
) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;
    let audio_config = config.connector("audio");

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let capture_dir = capture_dir
        .or_else(|| audio_config
            .and_then(|c| c.settings.get("capture_dir"))
            .and_then(|v| v.as_str())
            .map(|s| PathBuf::from(s).join(&today)))
        .unwrap_or_else(|| PathBuf::from("capture").join(&today));

    let mic = mic.or_else(|| audio_config
        .and_then(|c| c.settings.get("mic"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()));

    let system = system.or_else(|| audio_config
        .and_then(|c| c.settings.get("system"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()));

    info!(dir = %capture_dir.display(), "starting recording");

    let rec_config = alvum_capture_audio::recorder::RecordConfig {
        capture_dir,
        mic_device: mic,
        system_device: system,
        chunk_duration_secs: 60,
    };

    let recorder = alvum_capture_audio::recorder::Recorder::start(rec_config)?;

    println!("Recording... Press Ctrl-C to stop.");

    tokio::signal::ctrl_c().await?;

    println!("\nStopping...");
    recorder.stop();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    println!("Done.");
    Ok(())
}

async fn cmd_capture_screen(capture_dir: Option<PathBuf>) -> Result<()> {
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let capture_dir = capture_dir
        .unwrap_or_else(|| PathBuf::from("capture").join(&today));

    info!(dir = %capture_dir.display(), "starting screen capture");

    let config = alvum_capture_screen::daemon::ScreenCaptureConfig {
        capture_dir,
    };

    println!("Screen capture running... Press Ctrl-C to stop.");

    tokio::select! {
        result = alvum_capture_screen::daemon::run(config) => {
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nStopping...");
        }
    }

    println!("Done.");
    Ok(())
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

    println!("\nUse --mic <name> or --system <name> with `alvum record` to select a device.");
    Ok(())
}

async fn cmd_extract(
    source: Option<String>,
    session: Option<PathBuf>,
    output: PathBuf,
    provider_name: String,
    model: String,
    before: Option<String>,
    capture_dir: Option<PathBuf>,
    whisper_model: Option<PathBuf>,
    relevance_threshold: f32,
) -> Result<()> {
    std::fs::create_dir_all(&output)?;
    let decisions_path = output.join("decisions.jsonl");
    let briefing_path = output.join("briefing.md");
    let extraction_path = output.join("extraction.json");

    let provider = alvum_pipeline::llm::create_provider(&provider_name, &model)?;

    let before_ts = before.as_deref()
        .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
        .transpose()
        .context("invalid --before timestamp")?;

    // Cross-source mode: gather all observations, run episodic alignment, extract from relevant threads
    if source.is_none() {
        let capture_dir = capture_dir.context("--capture-dir required for cross-source mode")?;
        let mut all_observations: Vec<alvum_core::observation::Observation> = Vec::new();

        // Scan for audio files
        if let Some(ref model_path) = whisper_model {
            let mut audio_refs = Vec::new();
            for subdir in &["audio/mic", "audio/system", "audio/wearable"] {
                let dir = capture_dir.join(subdir);
                if dir.is_dir() {
                    for entry in std::fs::read_dir(&dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext == "wav" || ext == "opus" {
                            let source = format!("audio-{}", subdir.split('/').last().unwrap_or("unknown"));
                            let mime = if ext == "wav" { "audio/wav" } else { "audio/opus" };
                            audio_refs.push(alvum_core::data_ref::DataRef {
                                ts: chrono::Utc::now(),
                                source,
                                path: path.to_string_lossy().into_owned(),
                                mime: mime.into(),
                                metadata: None,
                            });
                        }
                    }
                }
            }
            if !audio_refs.is_empty() {
                info!(files = audio_refs.len(), "found audio files, transcribing");
                let audio_obs = alvum_processor_audio::transcriber::process_audio_data_refs(model_path, &audio_refs)?;
                all_observations.extend(audio_obs);
            }
        }

        // Scan for screen events
        let events_path = capture_dir.join("events.jsonl");
        if events_path.exists() {
            info!("loading screen events");
            let screen_obs: Vec<alvum_core::observation::Observation> = alvum_core::storage::read_jsonl(&events_path)?;
            all_observations.extend(screen_obs);
        }

        // Save all as episodic evidence
        let transcript_path = output.join("transcript.jsonl");
        for obs in &all_observations {
            alvum_core::storage::append_jsonl(&transcript_path, obs)?;
        }
        info!(path = %transcript_path.display(), observations = all_observations.len(), "saved transcript");

        if all_observations.is_empty() {
            println!("No observations found in capture directory.");
            return Ok(());
        }

        // Load existing knowledge corpus for context-aware threading
        let knowledge_dir = output.join("knowledge");
        let corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

        // Episodic alignment: Pass 1 + Pass 2
        info!("running episodic alignment...");
        let result = alvum_episode::threading::align_episodes(
            provider.as_ref(),
            &all_observations,
            chrono::Duration::minutes(5),
            Some(&corpus),
        ).await?;

        // Save threading result
        let threads_path = output.join("threads.json");
        std::fs::write(&threads_path, serde_json::to_string_pretty(&result)?)?;
        info!(
            threads = result.threads.len(),
            blocks = result.time_blocks.len(),
            "episodic alignment complete"
        );

        // Filter to high-relevance threads
        let relevant: Vec<&alvum_episode::types::ContextThread> = result.threads.iter()
            .filter(|t| t.is_relevant(relevance_threshold))
            .collect();

        info!(
            total_threads = result.threads.len(),
            relevant = relevant.len(),
            threshold = relevance_threshold,
            "filtered by relevance"
        );

        if relevant.is_empty() {
            println!("Threads: {} identified, none above relevance threshold {:.1}",
                result.threads.len(), relevance_threshold);
            println!("  threads: {}", threads_path.display());
            println!("  transcript: {}", transcript_path.display());
            for t in &result.threads {
                println!("    {} ({:.2}) - {}", t.id, t.relevance, t.label);
            }
            return Ok(());
        }

        // Collect observations from relevant threads for decision extraction
        let relevant_observations: Vec<alvum_core::observation::Observation> = relevant.iter()
            .flat_map(|t| t.observations.clone())
            .collect();

        info!(observations = relevant_observations.len(), "observations from relevant threads");

        // Extract decisions from relevant observations only
        info!("extracting decisions from relevant threads...");
        let mut decisions =
            alvum_pipeline::distill::extract_decisions(provider.as_ref(), &relevant_observations).await?;
        info!(decisions = decisions.len(), "extracted");

        if !decisions.is_empty() {
            info!("analyzing causal links...");
            alvum_pipeline::causal::link_decisions(provider.as_ref(), &mut decisions).await?;
            let link_count: usize = decisions.iter().map(|d| d.causes.len()).sum();
            info!(links = link_count, "linked");

            info!("generating briefing...");
            let briefing =
                alvum_pipeline::briefing::generate_briefing(provider.as_ref(), &decisions).await?;

            for dec in &decisions {
                alvum_core::storage::append_jsonl(&decisions_path, dec)?;
            }
            std::fs::write(&briefing_path, &briefing)?;

            let extraction = alvum_core::decision::ExtractionResult {
                session_id: "cross-source".into(),
                extracted_at: chrono::Utc::now().to_rfc3339(),
                decisions: decisions.clone(),
                briefing: briefing.clone(),
            };
            std::fs::write(&extraction_path, serde_json::to_string_pretty(&extraction)?)?;

            println!("\n{} threads -> {} relevant -> {} decisions",
                result.threads.len(), relevant.len(), decisions.len());
            println!("  threads:    {}", threads_path.display());
            println!("  decisions:  {}", decisions_path.display());
            println!("  briefing:   {}", briefing_path.display());
            println!("\n{}", "=".repeat(60));
            println!("{briefing}");
        } else {
            println!("{} relevant threads, no decisions found.", relevant.len());
            println!("  threads: {}", threads_path.display());
        }

        // Extract and accumulate knowledge from relevant observations
        info!("extracting knowledge...");
        let new_knowledge = alvum_knowledge::extract::extract_knowledge(
            provider.as_ref(),
            &relevant_observations,
            &corpus,
        ).await?;
        let mut updated_corpus = corpus;
        updated_corpus.merge(new_knowledge);
        alvum_knowledge::store::save(&knowledge_dir, &updated_corpus)?;
        info!(
            entities = updated_corpus.entities.len(),
            patterns = updated_corpus.patterns.len(),
            facts = updated_corpus.facts.len(),
            "knowledge corpus updated"
        );

        return Ok(());
    }

    // Single-source mode
    let source = source.unwrap();
    let observations = match source.as_str() {
        "claude" => {
            let session = session.context("--session required for --source claude")?;
            if !session.exists() {
                bail!("session file not found: {}", session.display());
            }
            if let Some(ts) = &before_ts {
                info!("parsing Claude Code session: {} (before {})", session.display(), ts);
            } else {
                info!("parsing Claude Code session: {}", session.display());
            }
            alvum_connector_claude::parser::parse_session_filtered(&session, before_ts)?
        }
        "audio" => {
            let capture_dir = capture_dir.context("--capture-dir required for --source audio")?;
            let model_path = whisper_model.context("--whisper-model required for --source audio")?;

            if !model_path.exists() {
                bail!("Whisper model not found: {}", model_path.display());
            }

            info!("scanning audio files in: {}", capture_dir.display());

            // Find all .opus files in the capture dir
            let mut data_refs = Vec::new();
            for subdir in &["audio/mic", "audio/system", "audio/wearable"] {
                let dir = capture_dir.join(subdir);
                if dir.is_dir() {
                    for entry in std::fs::read_dir(&dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext == "wav" || ext == "opus" {
                            let source = format!("audio-{}", subdir.split('/').last().unwrap_or("unknown"));
                            let mime = if ext == "wav" { "audio/wav" } else { "audio/opus" };

                            let ts = chrono::Utc::now();

                            data_refs.push(alvum_core::data_ref::DataRef {
                                ts,
                                source,
                                path: path.to_string_lossy().into_owned(),
                                mime: mime.into(),
                                metadata: None,
                            });
                        }
                    }
                }
            }

            info!(files = data_refs.len(), "found audio files");
            alvum_processor_audio::transcriber::process_audio_data_refs(&model_path, &data_refs)?
        }
        other => bail!("unknown source: {other}. Options: claude, audio"),
    };

    info!(observations = observations.len(), source = %source, "parsed observations");

    // Always save raw observations as episodic evidence — even if no decisions are found
    let transcript_path = output.join("transcript.jsonl");
    for obs in &observations {
        alvum_core::storage::append_jsonl(&transcript_path, obs)?;
    }
    info!(path = %transcript_path.display(), "saved transcript");

    if observations.is_empty() {
        println!("No observations found. Nothing to extract.");
        return Ok(());
    }

    info!("extracting decisions...");
    let mut decisions =
        alvum_pipeline::distill::extract_decisions(provider.as_ref(), &observations).await?;
    info!(decisions = decisions.len(), "extracted");

    if decisions.is_empty() {
        println!("✓ Transcript saved ({} observations), no decisions found.", observations.len());
        println!("  transcript: {}", transcript_path.display());
        return Ok(());
    }

    info!("analyzing causal links...");
    alvum_pipeline::causal::link_decisions(provider.as_ref(), &mut decisions).await?;
    let link_count: usize = decisions.iter().map(|d| d.causes.len()).sum();
    info!(links = link_count, "linked");

    info!("generating briefing...");
    let briefing =
        alvum_pipeline::briefing::generate_briefing(provider.as_ref(), &decisions).await?;

    for dec in &decisions {
        alvum_core::storage::append_jsonl(&decisions_path, dec)?;
    }
    info!(path = %decisions_path.display(), "wrote decisions");

    std::fs::write(&briefing_path, &briefing)?;
    info!(path = %briefing_path.display(), "wrote briefing");

    let result = alvum_core::decision::ExtractionResult {
        session_id: source.clone(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        decisions: decisions.clone(),
        briefing: briefing.clone(),
    };
    std::fs::write(&extraction_path, serde_json::to_string_pretty(&result)?)?;

    println!("\n✓ Extracted {} decisions with {} causal links", decisions.len(), link_count);
    println!("  transcript: {}", transcript_path.display());
    println!("  decisions:  {}", decisions_path.display());
    println!("  briefing:  {}", briefing_path.display());
    println!("\n{}", "=".repeat(60));
    println!("{briefing}");

    Ok(())
}
