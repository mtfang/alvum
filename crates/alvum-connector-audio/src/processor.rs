//! WhisperProcessor — implements the Processor trait using alvum-processor-audio.

use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct WhisperProcessor {
    model_path: PathBuf,
}

impl WhisperProcessor {
    pub fn new(model_path: PathBuf) -> Self {
        Self { model_path }
    }
}

#[async_trait]
impl Processor for WhisperProcessor {
    fn name(&self) -> &str {
        "whisper"
    }

    fn handles(&self) -> Vec<String> {
        vec!["audio-mic".into(), "audio-system".into(), "audio-wearable".into()]
    }

    async fn process(
        &self,
        data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        if !self.model_path.exists() {
            anyhow::bail!("Whisper model not found: {}", self.model_path.display());
        }

        info!(
            model = %self.model_path.display(),
            refs = data_refs.len(),
            "whisper processing"
        );

        let model_path = self.model_path.clone();
        let refs = data_refs.to_vec();
        tokio::task::spawn_blocking(move || {
            alvum_processor_audio::transcriber::process_audio_data_refs(&model_path, &refs)
        })
        .await
        .context("whisper task panicked")?
    }
}
