//! `ScreenSource` — wraps the SCK screen stream as a `CaptureSource`.
//!
//! The SCK stream (see `sck.rs`) delivers frames at ~2 fps into a shared
//! slot. This source's trigger loop reads the slot on focus-change / idle
//! events and writes one PNG per trigger — the raw frame rate is decoupled
//! from disk writes.

use alvum_core::capture::CaptureSource;
use anyhow::{bail, Context, Result};
use std::path::Path;
use tokio::sync::watch;
use tracing::{info, warn};

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
        // Ensure the shared SCK stream is up. Idempotent — audio-system may
        // have already brought it up.
        if let Err(e) = alvum_capture_sck::ensure_started() {
            // Surface in a shape lib.sh::detect_permission_issue matches
            // ("capture source failed ... permission not granted") so the
            // menu-bar "blocked" state still works.
            let _ = std::process::Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                .spawn();
            bail!(
                "Screen Recording permission not granted ({}).\n\
                 Opening System Settings > Privacy & Security > Screen Recording...\n\
                 Grant permission, then restart alvum capture.",
                e
            );
        }

        let writer = ScreenWriter::new(capture_dir.to_path_buf())
            .context("failed to create screen writer")?;

        let mut triggers = trigger::start_triggers()
            .context("failed to start screen triggers")?;

        info!(
            capture_dir = %capture_dir.display(),
            idle_secs = self.idle_interval_secs,
            "screen capture started (SCK)"
        );

        let mut count: u64 = 0;

        loop {
            tokio::select! {
                Some(event) = triggers.recv() => {
                    // Align the SCK filter with whatever display the user's
                    // frontmost window is on. Single-display = no-op; multi-
                    // monitor = filter swaps so we capture the active screen.
                    // A swap drops the stale frame and returns early (we'll
                    // snap on the next trigger once a fresh frame arrives).
                    match alvum_capture_sck::sync_active_display() {
                        Ok(true) => {
                            info!("active display changed; skipping this trigger");
                            continue;
                        }
                        Ok(false) => {}
                        Err(e) => warn!(error = %e, "sync_active_display failed"),
                    }
                    if let Some(frame) = alvum_capture_sck::pop_latest_frame() {
                        match writer.save_screenshot(
                            &frame.png_bytes,
                            event.ts,
                            &frame.app_name,
                            &frame.window_title,
                            event.kind.as_str(),
                        ) {
                            Ok(_) => {
                                count += 1;
                                info!(
                                    count,
                                    app = %frame.app_name,
                                    trigger = event.kind.as_str(),
                                    "captured screenshot"
                                );
                            }
                            Err(e) => warn!(error = %e, "failed to save screenshot"),
                        }
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
