//! `ScreenSource` — wraps the screenshot trigger loop as a `CaptureSource`.

use alvum_core::capture::CaptureSource;
use anyhow::{bail, Context, Result};
use std::path::Path;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::screenshot;
use crate::trigger;
use crate::writer::ScreenWriter;

pub struct ScreenSource {
    idle_interval_secs: u64,
}

impl ScreenSource {
    pub fn from_config(settings: &std::collections::HashMap<String, toml::Value>) -> Self {
        let idle_interval_secs = settings
            .get("idle_interval_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(30) as u64;
        Self { idle_interval_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for ScreenSource {
    fn name(&self) -> &str {
        "screen"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        // Check Screen Recording permission before starting the capture loop.
        // Without it, CGWindowListCreateImage returns blank images silently.
        match screenshot::check_screen_recording_permission() {
            Ok(true) => info!("Screen Recording permission verified"),
            Ok(false) => {
                let _ = std::process::Command::new("open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                    .spawn();
                bail!(
                    "Screen Recording permission not granted.\n\
                     Opening System Settings > Privacy & Security > Screen Recording...\n\
                     Grant permission, then restart alvum capture."
                );
            }
            Err(e) => warn!(error = %e, "could not verify Screen Recording permission, proceeding anyway"),
        }

        let writer = ScreenWriter::new(capture_dir.to_path_buf())
            .context("failed to create screen writer")?;

        let mut triggers = trigger::start_triggers()
            .context("failed to start screen triggers")?;

        info!(capture_dir = %capture_dir.display(), idle_secs = self.idle_interval_secs, "screen capture started");

        let mut count: u64 = 0;

        loop {
            tokio::select! {
                Some(event) = triggers.recv() => {
                    match screenshot::capture_frontmost_window() {
                        Ok(Some(shot)) => {
                            match writer.save_screenshot(
                                &shot.png_bytes,
                                event.ts,
                                &shot.app_name,
                                &shot.window_title,
                                event.kind.as_str(),
                            ) {
                                Ok(_) => {
                                    count += 1;
                                    info!(count, app = %shot.app_name, trigger = event.kind.as_str(), "captured screenshot");
                                }
                                Err(e) => warn!(error = %e, "failed to save screenshot"),
                            }
                        }
                        Ok(None) => {}
                        Err(e) => warn!(error = %e, "screenshot capture failed"),
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }

        info!(total = count, "screen capture stopped");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_source_from_config_defaults() {
        let settings = std::collections::HashMap::new();
        let source = ScreenSource::from_config(&settings);
        assert_eq!(source.idle_interval_secs, 30);
        assert_eq!(source.name(), "screen");
    }

    #[test]
    fn screen_source_from_config_custom() {
        let mut settings = std::collections::HashMap::new();
        settings.insert("idle_interval_secs".into(), toml::Value::Integer(15));
        let source = ScreenSource::from_config(&settings);
        assert_eq!(source.idle_interval_secs, 15);
    }
}
