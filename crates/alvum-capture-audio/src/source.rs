//! CaptureSource implementations for audio: mic and system audio.
//! Each source independently manages one audio stream + encoder.

use alvum_core::capture::CaptureSource;
use alvum_core::config::CaptureSourceConfig;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tracing::info;

use crate::capture::{self, SAMPLE_RATE};
use crate::devices;
use crate::encoder::AudioEncoder;
use crate::recorder::make_chunked_callback;

/// Captures microphone audio. Reads `device` and `chunk_duration_secs` from config.
pub struct AudioMicSource {
    device_name: Option<String>,
    chunk_duration_secs: u32,
}

impl AudioMicSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let device_name = config.settings.get("device")
            .and_then(|v| v.as_str())
            .filter(|s| *s != "default")
            .map(|s| s.to_string());

        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;

        Self { device_name, chunk_duration_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for AudioMicSource {
    fn name(&self) -> &str {
        "audio-mic"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let mic_dir = capture_dir.join("audio").join("mic");
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(mic_dir, SAMPLE_RATE)?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "mic".into());

        let mut current_bound: Option<String> = None;
        let mut current_stream: Option<capture::AudioStream> = None;

        // Repoll cadence for device-change detection. When a call starts
        // and macOS swaps default-input to AirPods-HFP, we pick it up at
        // most this long afterward and rebind the cpal stream.
        const REPOLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

        loop {
            let hal_devices = crate::coreaudio_hal::list_input_devices()
                .context("enumerate CoreAudio input devices")?;
            let default_id = crate::coreaudio_hal::default_input_device_id()
                .context("query default input device")?;

            let want = crate::mic_selection::decide_swap(
                &hal_devices,
                default_id,
                self.device_name.as_deref(),
                current_bound.as_deref(),
            );

            if let Some(new_name) = want {
                // Drop the old stream first so cpal releases the device
                // handle before we open the new one.
                drop(current_stream.take());
                let new_name = new_name.to_string();
                let device = devices::get_input_device(Some(&new_name))
                    .with_context(|| format!("open cpal device {new_name:?}"))?;
                let stream = capture::start_capture(&device, "mic", callback.clone())?;
                info!(device = %new_name, "audio-mic bound input device");
                current_bound = Some(new_name);
                current_stream = Some(stream);
            }

            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(REPOLL_INTERVAL) => {}
            }
        }

        drop(current_stream);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }
        info!("audio-mic source stopped");
        Ok(())
    }
}

/// Captures system audio via ScreenCaptureKit. The audio is tapped at the
/// macOS process graph, independent of which output device is active — so
/// it stays alive across AirPods/AirPlay/HDMI switches. No `device` config
/// key is consulted: SCK owns device selection.
pub struct AudioSystemSource {
    chunk_duration_secs: u32,
}

impl AudioSystemSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;
        Self { chunk_duration_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for AudioSystemSource {
    fn name(&self) -> &str {
        "audio-system"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let sys_dir = capture_dir.join("audio").join("system");
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(sys_dir, SAMPLE_RATE)?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "system".into());

        // System audio flows through the shared SCK stream (owned by
        // alvum_capture_sck). Starting is idempotent — screen may already
        // have brought the stream up. Failure is typically a Screen
        // Recording permission denial; degrade to no-op instead of
        // aborting other sources.
        if let Err(e) = alvum_capture_sck::ensure_started() {
            tracing::warn!(error = %e, "SCK shared stream unavailable, audio-system will not run");
            while !*shutdown.borrow_and_update() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
            return Ok(());
        }

        alvum_capture_sck::set_audio_callback(Some(callback));
        info!("audio-system source started (SCK)");

        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        alvum_capture_sck::set_audio_callback(None);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }

        info!("audio-system source stopped");
        Ok(())
    }
}
