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
    backend: AudioProcessorBackend,
}

enum AudioProcessorBackend {
    Local {
        model_path: PathBuf,
        config: TranscriberConfig,
    },
    Provider {
        provider: String,
    },
}

impl AudioProcessor {
    pub fn new(model_path: PathBuf, config: TranscriberConfig) -> Self {
        Self {
            backend: AudioProcessorBackend::Local { model_path, config },
        }
    }

    pub fn provider(provider: impl Into<String>) -> Self {
        Self {
            backend: AudioProcessorBackend::Provider {
                provider: provider.into(),
            },
        }
    }
}

#[async_trait]
impl Processor for AudioProcessor {
    fn name(&self) -> &str {
        "audio"
    }

    fn handles(&self) -> Vec<String> {
        vec![
            "audio-mic".into(),
            "audio-system".into(),
            "audio-wearable".into(),
        ]
    }

    async fn process(
        &self,
        data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        match &self.backend {
            AudioProcessorBackend::Local { model_path, config } => {
                if !model_path.exists() {
                    anyhow::bail!("Whisper model not found: {}", model_path.display());
                }

                info!(
                    model = %model_path.display(),
                    language = %config.language,
                    refs = data_refs.len(),
                    "whisper processing"
                );

                let model_path = model_path.clone();
                let config = config.clone();
                let refs = data_refs.to_vec();
                tokio::task::spawn_blocking(move || {
                    alvum_processor_audio::transcriber::process_audio_data_refs(
                        &model_path,
                        config,
                        &refs,
                    )
                })
                .await
                .context("whisper task panicked")?
            }
            AudioProcessorBackend::Provider { provider } if provider == "openai-api" => {
                let config = alvum_processor_audio::openai::OpenAiAudioConfig::from_alvum_config()?;
                info!(
                    model = %config.model,
                    refs = data_refs.len(),
                    "openai diarized audio processing"
                );
                alvum_processor_audio::openai::process_audio_data_refs(config, data_refs).await
            }
            AudioProcessorBackend::Provider { provider } => {
                anyhow::bail!("provider audio mode is not implemented for {provider}")
            }
        }
    }
}
