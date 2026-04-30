use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::{capture, config_cmd, extensions, extract, models, profile, providers, tail};

#[derive(Parser)]
#[command(name = "alvum", about = "Life decision tracking and alignment engine")]
pub(crate) struct Cli {
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

    /// Diagnose global configuration and runtime setup issues.
    Doctor {
        /// Emit machine-readable JSON for app/front-end integrations.
        #[arg(long)]
        json: bool,
    },

    /// Set a config value (e.g., alvum config-set capture.screen.enabled false)
    #[command(name = "config-set")]
    ConfigSet {
        /// Dotted key path (e.g., capture.audio-mic.device, processors.screen.mode)
        key: String,
        /// Value to set
        value: String,
    },

    /// Manage user-facing connectors.
    Connectors {
        #[command(subcommand)]
        action: Option<extensions::ConnectorAction>,
    },

    /// LLM provider status + test commands. Designed to be called from
    /// the menu-bar popover for the Provider settings section, but
    /// fine for direct CLI use too.
    Providers {
        #[command(subcommand)]
        action: providers::Action,
    },

    /// Manage locally owned model assets such as Whisper.
    Models {
        #[command(subcommand)]
        action: models::Action,
    },

    /// Manage the user-customizable synthesis profile.
    Profile {
        #[command(subcommand)]
        action: profile::Action,
    },

    /// Manage external extension packages.
    Extensions {
        #[command(subcommand)]
        action: extensions::ExtensionAction,
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
        ///   auto         - pick the first authenticated backend (default)
        ///   claude-cli   - installed Claude CLI configured backend
        ///   codex-cli    - Codex / ChatGPT subscription (`codex login`)
        ///   anthropic-api - direct Anthropic API (needs ANTHROPIC_API_KEY)
        ///   bedrock      - Anthropic-on-Bedrock (needs AWS credentials)
        ///   ollama       - local Ollama
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

pub(crate) async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Capture {
            capture_dir,
            only,
            disable,
        } => capture::run(capture_dir, only, disable).await,
        Commands::Devices => capture::devices(),
        Commands::ConfigInit => config_cmd::init(),
        Commands::ConfigShow => config_cmd::show(),
        Commands::Doctor { json } => extensions::run_doctor(json),
        Commands::ConfigSet { key, value } => config_cmd::set(&key, &value),
        Commands::Connectors { action } => extensions::run_connectors(action).await,
        Commands::Providers { action } => providers::run(action).await,
        Commands::Models { action } => models::run(action).await,
        Commands::Profile { action } => profile::run(action),
        Commands::Extensions { action } => extensions::run_extensions(action).await,
        Commands::Tail { follow, filter } => tail::run(follow, filter).await,
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
            extract::run(extract::Options {
                source,
                session,
                output,
                provider_name: provider,
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
            })
            .await
        }
    }
}
