//! Screen capture daemon: orchestrates triggers, screenshots, and file writing.
//!
//! Listens for trigger events (app focus changes, idle timer) and captures a
//! screenshot of the frontmost window on each event. Runs until the trigger
//! channel closes (i.e., all trigger producers are dropped).

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use tracing::{info, warn};

use crate::screenshot;
use crate::trigger;
use crate::writer::ScreenWriter;

#[derive(Debug, Clone)]
pub struct ScreenCaptureConfig {
    pub capture_dir: PathBuf,
}

/// Run the screen capture daemon. Blocks until the trigger channel closes.
///
/// Checks for Screen Recording permission at startup and fails with a clear
/// error message if not granted. Without permission, macOS silently returns
/// blank screenshots — this check prevents wasting storage on empty images.
pub async fn run(config: ScreenCaptureConfig) -> Result<()> {
    // Check permission before starting the capture loop
    match screenshot::check_screen_recording_permission() {
        Ok(true) => info!("Screen Recording permission verified"),
        Ok(false) => {
            // Open System Settings directly to the Screen Recording permission page
            let _ = std::process::Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                .spawn();

            bail!(
                "Screen Recording permission not granted.\n\n\
                 alvum needs Screen Recording access to capture window screenshots.\n\
                 Opening System Settings > Privacy & Security > Screen Recording...\n\
                 Grant permission, then restart alvum capture-screen."
            );
        }
        Err(e) => {
            warn!(error = %e, "could not verify Screen Recording permission, proceeding anyway");
        }
    }

    let writer =
        ScreenWriter::new(config.capture_dir.clone()).context("failed to create screen writer")?;

    let mut triggers = trigger::start_triggers().context("failed to start triggers")?;

    info!(
        capture_dir = %config.capture_dir.display(),
        "screen capture daemon started"
    );

    let mut capture_count: u64 = 0;

    while let Some(event) = triggers.recv().await {
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
                        capture_count += 1;
                        info!(
                            count = capture_count,
                            app = %shot.app_name,
                            trigger = event.kind.as_str(),
                            "captured screenshot"
                        );
                    }
                    Err(e) => warn!(error = %e, "failed to save screenshot"),
                }
            }
            Ok(None) => {}
            Err(e) => warn!(error = %e, "screenshot capture failed"),
        }
    }

    info!(total = capture_count, "screen capture daemon stopped");
    Ok(())
}
