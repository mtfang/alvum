//! AudioProcessor — implements the Processor trait using alvum-processor-audio.

use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use alvum_processor_audio::transcriber::TranscriberConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct AudioProcessor {
    model_path: PathBuf,
    config: TranscriberConfig,
}

impl AudioProcessor {
    pub fn new(model_path: PathBuf, config: TranscriberConfig) -> Self {
        Self { model_path, config }
    }
}

#[async_trait]
impl Processor for AudioProcessor {
    fn name(&self) -> &str {
        "audio"
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
            language = %self.config.language,
            refs = data_refs.len(),
            "whisper processing"
        );

        let model_path = self.model_path.clone();
        let config = self.config.clone();
        let refs = data_refs.to_vec();
        tokio::task::spawn_blocking(move || {
            alvum_processor_audio::transcriber::process_audio_data_refs(&model_path, config, &refs)
        })
        .await
        .context("whisper task panicked")?
    }
}
