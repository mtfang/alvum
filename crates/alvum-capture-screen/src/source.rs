//! `ScreenSource` — wraps the SCK screen stream as a `CaptureSource`.
//!
//! The SCK stream (see `sck.rs`) delivers frames at ~2 fps into a shared
//! slot. This source's trigger loop reads the slot on focus-change / idle
//! events and writes one PNG per trigger — the raw frame rate is decoupled
//! from disk writes.

use alvum_core::capture::CaptureSource;
use anyhow::{Context, Result, bail};
use std::path::Path;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::trigger;
use crate::writer::ScreenWriter;

pub struct ScreenSource {
    idle_interval_secs: u64,
    /// Minimum wall-clock gap between two saved screenshots. Applies after
    /// the trigger layer — so trigger-happy apps (animated titles, rapid
    /// window cycling) don't produce one PNG per second. Set to 0 to disable.
    min_interval_secs: u64,
    /// Toggle AppFocus-driven triggers.
    app_focus: bool,
    /// Toggle WindowFocus-driven triggers (titles change on spinners, tab
    /// switches, etc. — loudest source of noise).
    window_focus: bool,
}

impl ScreenSource {
    pub fn from_config(settings: &std::collections::HashMap<String, toml::Value>) -> Self {
        let idle_interval_secs = settings
            .get("idle_interval_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(30) as u64;
        let min_interval_secs = settings
            .get("min_interval_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(10)
            .max(0) as u64;
        let app_focus = settings
            .get("app_focus")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let window_focus = settings
            .get("window_focus")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        Self {
            idle_interval_secs,
            min_interval_secs,
            app_focus,
            window_focus,
        }
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
            if let Err(e) = std::process::Command::new("open")
                .arg(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
                )
                .spawn()
            {
                tracing::warn!(error = %e, "failed to open Settings.app for Screen Recording grant");
            }
            bail!(
                "Screen Recording permission not granted ({}).\n\
                 Opening System Settings > Privacy & Security > Screen Recording...\n\
                 Grant permission, then restart alvum capture.",
                e
            );
        }

        let writer = ScreenWriter::new(capture_dir.to_path_buf())
            .context("failed to create screen writer")?;

        let trigger_config = trigger::TriggerConfig {
            idle_interval: std::time::Duration::from_secs(self.idle_interval_secs),
            app_focus: self.app_focus,
            window_focus: self.window_focus,
        };
        let mut triggers =
            trigger::start_triggers(trigger_config).context("failed to start screen triggers")?;

        info!(
            capture_dir = %capture_dir.display(),
            idle_secs = self.idle_interval_secs,
            min_interval_secs = self.min_interval_secs,
            app_focus = self.app_focus,
            window_focus = self.window_focus,
            "screen capture started (SCK)"
        );

        let min_interval = std::time::Duration::from_secs(self.min_interval_secs);
        let mut last_saved_at: Option<std::time::Instant> = None;
        let mut count: u64 = 0;
        let mut skipped: u64 = 0;

        loop {
            tokio::select! {
                Some(event) = triggers.recv() => {
                    // Source-level debounce: don't save two PNGs closer than
                    // min_interval. Catches trigger-happy apps (animated titles,
                    // rapid tab cycling) that the trigger layer can't tell
                    // apart from legitimate focus changes.
                    if self.min_interval_secs > 0 {
                        if let Some(prev) = last_saved_at {
                            if prev.elapsed() < min_interval {
                                skipped += 1;
                                if skipped % 50 == 0 {
                                    info!(skipped, "screen: debounced triggers (min_interval)");
                                }
                                continue;
                            }
                        }
                    }
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
                                last_saved_at = Some(std::time::Instant::now());
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
        assert_eq!(source.min_interval_secs, 10);
        assert!(source.app_focus);
        assert!(source.window_focus);
        assert_eq!(source.name(), "screen");
    }

    #[test]
    fn screen_source_from_config_custom() {
        let mut settings = std::collections::HashMap::new();
        settings.insert("idle_interval_secs".into(), toml::Value::Integer(15));
        settings.insert("min_interval_secs".into(), toml::Value::Integer(60));
        settings.insert("window_focus".into(), toml::Value::Boolean(false));
        let source = ScreenSource::from_config(&settings);
        assert_eq!(source.idle_interval_secs, 15);
        assert_eq!(source.min_interval_secs, 60);
        assert!(source.app_focus);
        assert!(!source.window_focus);
    }
}
