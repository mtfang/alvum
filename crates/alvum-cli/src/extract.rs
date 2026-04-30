use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::warn;

pub(crate) struct Options {
    pub(crate) source: Option<String>,
    pub(crate) session: Option<PathBuf>,
    pub(crate) output: PathBuf,
    pub(crate) provider_name: Option<String>,
    pub(crate) model: String,
    pub(crate) before: Option<String>,
    pub(crate) since: Option<String>,
    pub(crate) briefing_date: Option<String>,
    pub(crate) capture_dir: Option<PathBuf>,
    pub(crate) whisper_model: Option<PathBuf>,
    pub(crate) relevance_threshold: f32,
    pub(crate) vision: Option<String>,
    pub(crate) resume: bool,
    pub(crate) no_skip_processed: bool,
}

fn connectors_from_config(
    config: &alvum_core::config::AlvumConfig,
    image_provider: std::sync::Arc<dyn alvum_core::llm::LlmProvider>,
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
                    Some(image_provider.clone()),
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

fn screen_processor_mode(config: &alvum_core::config::AlvumConfig) -> String {
    config
        .processor_setting("screen", "mode")
        .or_else(|| config.processor_setting("screen", "vision"))
        .unwrap_or_else(|| "ocr".into())
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

pub(crate) async fn run(options: Options) -> Result<()> {
    let Options {
        source: _source,
        session: _session,
        output,
        provider_name,
        model,
        before,
        since,
        briefing_date,
        capture_dir,
        whisper_model: _whisper_model,
        relevance_threshold,
        vision: _vision,
        resume,
        no_skip_processed,
    } = options;
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

    let image_provider = if screen_processor_mode(&config) == "provider" {
        let provider_box = alvum_pipeline::llm::create_provider_for_modality_async(
            &provider_name,
            &model,
            "image",
        )
        .await?;
        provider_box.into()
    } else {
        provider.clone()
    };

    let connectors = connectors_from_config(&config, image_provider, since_ts, before_ts);

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
