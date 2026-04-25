//! Whisper-based audio transcription. Takes f32 PCM samples, returns timestamped segments.

use alvum_core::artifact::Artifact;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::{MediaRef, Observation};
use anyhow::{Context, Result};
use std::path::Path;
use tracing::info;

/// A transcribed segment with timing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Segment {
    pub start_secs: f32,
    pub end_secs: f32,
    pub text: String,
}

/// Runtime config for the Whisper transcriber.
#[derive(Debug, Clone)]
pub struct TranscriberConfig {
    /// Whisper language code ("en", "es", "auto", etc.).
    pub language: String,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self { language: "en".into() }
    }
}

/// Transcribe audio files referenced by DataRefs, producing Artifacts with text + structured layers.
pub struct AudioTranscriber {
    ctx: whisper_rs::WhisperContext,
    config: TranscriberConfig,
}

impl AudioTranscriber {
    /// Create a new transcriber with a Whisper model file.
    /// Model files: download from https://huggingface.co/ggerganov/whisper.cpp/
    /// e.g., ggml-base.bin, ggml-small.bin, ggml-large-v3.bin
    pub fn new(model_path: &Path, config: TranscriberConfig) -> Result<Self> {
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path.to_str().context("model path must be valid UTF-8")?,
            whisper_rs::WhisperContextParameters::default(),
        ).context("failed to load Whisper model")?;

        info!(model = %model_path.display(), language = %config.language, "loaded Whisper model");
        Ok(Self { ctx, config })
    }

    /// Transcribe a single audio DataRef. Returns an Artifact with text + structured layers.
    pub fn transcribe_data_ref(&self, data_ref: &DataRef) -> Result<Artifact> {
        let path = Path::new(&data_ref.path);

        // Decode audio to PCM
        let samples = crate::decoder::decode_wav_file(path)
            .with_context(|| format!("failed to decode audio: {}", data_ref.path))?;

        let duration_secs = samples.len() as f32 / 16000.0;
        info!(
            path = %data_ref.path,
            duration_secs = format!("{:.1}", duration_secs),
            "transcribing audio"
        );

        // Transcribe
        let segments = self.transcribe_samples(&samples)?;

        // Build full transcript text
        let full_text = segments.iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");

        // Build artifact with text + structured layers
        let mut artifact = Artifact::with_text(data_ref.clone(), &full_text);
        artifact.add_layer("structured", serde_json::json!({
            "segments": segments,
            "duration_secs": duration_secs,
            "sample_count": samples.len(),
        }));

        info!(
            segments = segments.len(),
            text_len = full_text.len(),
            "transcription complete"
        );

        Ok(artifact)
    }

    /// Low-level: transcribe f32 PCM samples (16kHz mono) to segments.
    fn transcribe_samples(&self, samples: &[f32]) -> Result<Vec<Segment>> {
        let mut state = self.ctx.create_state()
            .context("failed to create Whisper state")?;

        let mut params = whisper_rs::FullParams::new(
            whisper_rs::SamplingStrategy::Greedy { best_of: 1 }
        );
        params.set_language(Some(&self.config.language));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state.full(params, samples)
            .context("Whisper transcription failed")?;

        let n = state.full_n_segments();
        let mut segments = Vec::new();

        for i in 0..n {
            let seg = state.get_segment(i)
                .with_context(|| format!("segment {i} out of bounds"))?;

            // Timestamps are in centiseconds (10ms units); convert to seconds
            let start = seg.start_timestamp() as f32 / 100.0;
            let end = seg.end_timestamp() as f32 / 100.0;
            let text = seg.to_str()
                .with_context(|| format!("failed to get text for segment {i}"))?
                .to_string();

            if !text.trim().is_empty() {
                segments.push(Segment {
                    start_secs: start,
                    end_secs: end,
                    text,
                });
            }
        }

        Ok(segments)
    }
}

/// Process all audio DataRefs, producing Observations for the pipeline.
/// This is the main entry point for the audio processor.
pub fn process_audio_data_refs(
    model_path: &Path,
    config: TranscriberConfig,
    data_refs: &[DataRef],
) -> Result<Vec<Observation>> {
    if data_refs.is_empty() {
        return Ok(vec![]);
    }

    let transcriber = AudioTranscriber::new(model_path, config)?;
    let mut observations = Vec::new();

    for data_ref in data_refs {
        match transcriber.transcribe_data_ref(data_ref) {
            Ok(artifact) => {
                if let Some(text) = artifact.text()
                    && !text.is_empty()
                {
                    observations.push(Observation {
                        ts: artifact.data_ref.ts,
                        source: artifact.data_ref.source.clone(),
                        kind: "speech_segment".into(),
                        content: text.to_string(),
                        metadata: artifact.layer("structured").cloned(),
                        media_ref: Some(MediaRef {
                            path: artifact.data_ref.path.clone(),
                            mime: artifact.data_ref.mime.clone(),
                        }),
                    });
                }
            }
            Err(e) => {
                tracing::warn!(path = %data_ref.path, error = %e, "failed to transcribe, skipping");
            }
        }
    }

    // Sort by timestamp
    observations.sort_by_key(|o| o.ts);

    info!(observations = observations.len(), "audio processing complete");
    Ok(observations)
}
