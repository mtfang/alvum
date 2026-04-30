use anyhow::Result;
use std::path::PathBuf;
use tracing::{info, warn};

pub(crate) async fn run(
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

pub(crate) fn devices() -> Result<()> {
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
