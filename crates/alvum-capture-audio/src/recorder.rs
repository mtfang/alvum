use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tracing::info;

use crate::capture::{self, SampleCallback, SAMPLE_RATE};
use crate::devices;
use crate::encoder::AudioEncoder;
use crate::vad::{VadEvent, VoiceDetector};

/// Configuration for a recording session.
#[derive(Debug, Clone)]
pub struct RecordConfig {
    pub capture_dir: PathBuf,
    pub mic_device: Option<String>,
    /// Output device for system audio loopback. "off" to disable.
    pub system_device: Option<String>,
}

/// A running recording session.
pub struct Recorder {
    shutdown_tx: watch::Sender<bool>,
}

impl Recorder {
    /// Start recording. Returns immediately; recording runs in background threads.
    pub fn start(config: RecordConfig) -> Result<Self> {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Mic stream
        let mic_dir = config.capture_dir.join("audio").join("mic");
        let mic_device = devices::get_input_device(config.mic_device.as_deref())
            .context("failed to get mic device")?;
        let mic_encoder = Arc::new(Mutex::new(AudioEncoder::new(mic_dir, SAMPLE_RATE)?));
        let mic_vad = Arc::new(Mutex::new(VoiceDetector::new(SAMPLE_RATE as usize)?));

        let mic_cb = make_vad_callback(mic_encoder.clone(), mic_vad);
        let mic_stream = capture::start_capture(&mic_device, "mic", mic_cb)?;

        // System audio stream (if not disabled)
        let sys_stream = if config.system_device.as_deref() != Some("off") {
            let sys_dir = config.capture_dir.join("audio").join("system");
            match devices::get_output_device(config.system_device.as_deref()) {
                Ok(sys_device) => {
                    match (|| -> Result<_> {
                        let sys_encoder = Arc::new(Mutex::new(AudioEncoder::new(sys_dir, SAMPLE_RATE)?));
                        let sys_vad = Arc::new(Mutex::new(VoiceDetector::new(SAMPLE_RATE as usize)?));
                        let sys_cb = make_vad_callback(sys_encoder, sys_vad);
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

        // Hold streams alive in a background task until shutdown
        let mut rx = shutdown_rx;
        tokio::spawn(async move {
            let _mic = mic_stream;
            let _sys = sys_stream;
            let _enc = mic_encoder; // keep encoder alive for flush on shutdown
            while !*rx.borrow_and_update() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            // Flush remaining audio on shutdown
            if let Ok(mut enc) = _enc.lock() {
                let _ = enc.flush_segment();
            }
            info!("recording stopped");
        });

        Ok(Self { shutdown_tx })
    }

    /// Signal the recorder to stop.
    pub fn stop(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Create a sample callback that runs VAD and writes speech segments.
fn make_vad_callback(
    encoder: Arc<Mutex<AudioEncoder>>,
    vad: Arc<Mutex<VoiceDetector>>,
) -> SampleCallback {
    Arc::new(Mutex::new(move |samples: &[f32]| {
        let chunk_size = 512;
        for chunk in samples.chunks(chunk_size) {
            if chunk.len() < chunk_size {
                break;
            }

            let event = {
                let mut v = vad.lock().unwrap();
                v.process_chunk(chunk)
            };

            let mut enc = encoder.lock().unwrap();
            match event {
                VadEvent::SpeechStart | VadEvent::Speech => {
                    enc.push_samples(chunk);
                }
                VadEvent::SpeechEnd => {
                    enc.push_samples(chunk);
                    let _ = enc.flush_segment();
                }
                VadEvent::Silence => {}
            }
        }
    }))
}
