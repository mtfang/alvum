use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tracing::info;

use crate::capture::{self, SampleCallback, SAMPLE_RATE};
use crate::devices;
use crate::encoder::AudioEncoder;

/// Configuration for a recording session.
#[derive(Debug, Clone)]
pub struct RecordConfig {
    pub capture_dir: PathBuf,
    pub mic_device: Option<String>,
    /// Output device for system audio. "off" to disable.
    pub system_device: Option<String>,
    /// Duration of each audio chunk file in seconds.
    pub chunk_duration_secs: u32,
}

impl RecordConfig {
    pub fn with_defaults(capture_dir: PathBuf) -> Self {
        Self {
            capture_dir,
            mic_device: None,
            system_device: None,
            chunk_duration_secs: 30,
        }
    }
}

/// A running recording session.
pub struct Recorder {
    shutdown_tx: watch::Sender<bool>,
}

impl Recorder {
    /// Start recording. Returns immediately; recording runs in background.
    pub fn start(config: RecordConfig) -> Result<Self> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let samples_per_chunk = SAMPLE_RATE as usize * config.chunk_duration_secs as usize;

        // Mic stream
        let mic_dir = config.capture_dir.join("audio").join("mic");
        let mic_device = devices::get_input_device(config.mic_device.as_deref())
            .context("failed to get mic device")?;
        let mic_encoder = Arc::new(Mutex::new(AudioEncoder::new(mic_dir, SAMPLE_RATE)?));

        let mic_cb = make_chunked_callback(mic_encoder.clone(), samples_per_chunk, "mic".into());
        let mic_stream = capture::start_capture(&mic_device, "mic", mic_cb)?;

        // System audio stream (if not disabled)
        let sys_stream = if config.system_device.as_deref() != Some("off") {
            let sys_dir = config.capture_dir.join("audio").join("system");
            match devices::get_output_device(config.system_device.as_deref()) {
                Ok(sys_device) => {
                    match (|| -> Result<_> {
                        let sys_encoder = Arc::new(Mutex::new(AudioEncoder::new(sys_dir, SAMPLE_RATE)?));
                        let sys_cb = make_chunked_callback(sys_encoder, samples_per_chunk, "system".into());
                        capture::start_capture(&sys_device, "system", sys_cb)
                    })() {
                        Ok(stream) => Some(stream),
                        Err(e) => {
                            tracing::warn!(error = %e, "system audio capture not available, continuing with mic only");
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "system audio device not found, continuing with mic only");
                    None
                }
            }
        } else {
            info!("system audio capture disabled");
            None
        };

        // Hold streams alive until shutdown
        let mut rx = shutdown_rx;
        tokio::spawn(async move {
            let _mic = mic_stream;
            let _sys = sys_stream;
            let _enc = mic_encoder;
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            if let Ok(mut enc) = _enc.lock() {
                let _ = enc.flush_segment();
            }
            info!("recording stopped");
        });

        Ok(Self { shutdown_tx })
    }

    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Create a callback that writes audio in fixed-length chunks.
/// No VAD — every sample is recorded. Chunks are flushed every `samples_per_chunk` samples.
fn make_chunked_callback(
    encoder: Arc<Mutex<AudioEncoder>>,
    samples_per_chunk: usize,
    label: String,
) -> SampleCallback {
    let sample_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let chunk_count = Arc::new(std::sync::atomic::AtomicU64::new(0));

    Arc::new(Mutex::new(move |samples: &[f32]| {
        let mut enc = encoder.lock().unwrap();
        enc.push_samples(samples);

        let count = sample_count.fetch_add(samples.len(), std::sync::atomic::Ordering::Relaxed) + samples.len();

        // Log audio level every ~5 seconds
        let chunks = chunk_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if chunks % 155 == 0 && !samples.is_empty() {
            let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
            tracing::debug!(label = %label, rms = format!("{:.4}", rms), "audio level");
        }

        // Flush chunk when we've accumulated enough samples
        if count >= samples_per_chunk {
            if let Err(e) = enc.flush_segment() {
                tracing::error!(label = %label, error = %e, "failed to flush audio chunk");
            }
            sample_count.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }))
}
