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

        let device = devices::get_input_device(self.device_name.as_deref())
            .context("failed to get mic device")?;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(mic_dir, SAMPLE_RATE)?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "mic".into());
        let stream = capture::start_capture(&device, "mic", callback)?;

        info!("audio-mic source started");

        // Hold stream alive until shutdown
        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        // Flush remaining audio data
        drop(stream);
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
