//! Whisper-based audio transcription. Takes f32 PCM samples, returns timestamped segments.

use alvum_core::artifact::Artifact;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::{MediaRef, Observation};
use alvum_core::pipeline_events::{self as events, Event};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::Path;
use tracing::info;

// === Whisper hallucination filter =====================================
//
// Whisper hallucinates filler phrases on near-silent input. The model
// itself flags this via two purpose-built confidence signals exposed by
// whisper-rs, so the filter relies on those rather than maintaining a
// brittle phrase denylist:
//
//   1. `Segment::no_speech_probability()` — whisper's own estimate that
//      the segment is non-speech. The decoder is calibrated for this.
//   2. Mean per-token probability — averaged over the segment's tokens.
//      Hallucinated text correlates with low average token confidence.
//
// Defense-in-depth: we also pass `no_speech_thold` to the decoder so
// whisper.cpp culls obvious non-speech segments before we ever see them.

/// Confidence thresholds for the post-decode segment filter. Tunable
/// via `TranscriberConfig` so we can move them without code changes
/// once we have empirical distributions from the pipeline event stream.
#[derive(Debug, Clone, Copy)]
pub struct SegmentFilter {
    /// Drop segments where Whisper's no-speech probability is at or above
    /// this value. 0.6 matches OpenAI whisper's reference default.
    pub no_speech_prob_max: f32,
    /// Drop segments whose mean per-token probability falls below this.
    /// Hallucinations on silence tend to score < 0.5; real speech scores
    /// considerably higher even on a small Whisper model.
    pub mean_token_prob_min: f32,
}

impl Default for SegmentFilter {
    fn default() -> Self {
        Self {
            no_speech_prob_max: 0.6,
            mean_token_prob_min: 0.5,
        }
    }
}

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
    /// Per-segment confidence filter applied after decode.
    pub filter: SegmentFilter,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self {
            language: "en".into(),
            filter: SegmentFilter::default(),
        }
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

        // Transcribe — returns kept segments + per-reason drop counts.
        let (segments, dropped) = self.transcribe_samples(&samples)?;

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

        let dropped_total: usize = dropped.values().sum();
        info!(
            segments = segments.len(),
            text_len = full_text.len(),
            filtered_count = dropped_total,
            kept_count = segments.len(),
            filter_reasons = ?dropped,
            "transcription complete"
        );

        // Surface the per-file filter outcome on the live event channel so
        // the popover and `alvum tail` can show running drop counts. We
        // emit even when nothing was filtered — a "kept N, dropped 0"
        // line confirms the file flowed through the filter cleanly.
        events::emit(Event::InputFiltered {
            processor: "whisper".into(),
            file: Some(data_ref.path.clone()),
            kept: segments.len(),
            dropped: dropped_total,
            reasons: serde_json::json!(dropped),
        });

        Ok(artifact)
    }

    /// Low-level: transcribe f32 PCM samples (16kHz mono) to segments.
    /// Returns kept segments and a per-reason count of dropped (filtered)
    /// segments. The filter is driven entirely by Whisper's own
    /// confidence signals — see the `Whisper hallucination filter`
    /// section above.
    fn transcribe_samples(
        &self,
        samples: &[f32],
    ) -> Result<(Vec<Segment>, BTreeMap<String, usize>)> {
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
        // Decoder-side gate: whisper.cpp culls segments above this no-speech
        // probability before they reach our post-filter. Belt + braces with
        // the per-segment check below.
        params.set_no_speech_thold(self.config.filter.no_speech_prob_max);

        state.full(params, samples)
            .context("Whisper transcription failed")?;

        let n = state.full_n_segments();
        let mut segments = Vec::new();
        let mut dropped: BTreeMap<String, usize> = BTreeMap::new();

        for i in 0..n {
            let seg = state.get_segment(i)
                .with_context(|| format!("segment {i} out of bounds"))?;

            // Timestamps are in centiseconds (10ms units); convert to seconds
            let start = seg.start_timestamp() as f32 / 100.0;
            let end = seg.end_timestamp() as f32 / 100.0;
            let text = seg.to_str()
                .with_context(|| format!("failed to get text for segment {i}"))?
                .to_string();

            // An empty string after trimming is unconditionally dropped —
            // it's never useful and predates the confidence filter.
            if text.trim().is_empty() {
                *dropped.entry("empty".into()).or_insert(0) += 1;
                continue;
            }

            // Whisper's own no-speech probability. Above the threshold the
            // decoder is signalling "this segment is non-speech."
            let no_speech_prob = seg.no_speech_probability();
            if no_speech_prob >= self.config.filter.no_speech_prob_max {
                *dropped.entry("no_speech_prob".into()).or_insert(0) += 1;
                continue;
            }

            // Mean per-token probability. Hallucinations correlate with low
            // average token confidence; real speech scores noticeably higher
            // even on the base model.
            if let Some(mean_prob) = mean_token_probability(&seg)
                && mean_prob < self.config.filter.mean_token_prob_min
            {
                *dropped.entry("low_token_prob".into()).or_insert(0) += 1;
                continue;
            }

            segments.push(Segment {
                start_secs: start,
                end_secs: end,
                text,
            });
        }

        Ok((segments, dropped))
    }
}

/// Average the per-token probability across all tokens in a segment.
/// Returns `None` when the segment has zero tokens (caller treats it as
/// "no signal" and lets the segment through; the no-speech check has
/// already had a say).
fn mean_token_probability(seg: &whisper_rs::WhisperSegment<'_>) -> Option<f32> {
    let n = seg.n_tokens();
    if n <= 0 {
        return None;
    }
    let mut sum = 0.0_f32;
    let mut count = 0usize;
    for i in 0..n {
        if let Some(tok) = seg.get_token(i) {
            sum += tok.token_probability();
            count += 1;
        }
    }
    if count == 0 {
        None
    } else {
        Some(sum / count as f32)
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
        // Tick whether the file succeeded or failed — the bar must
        // advance for every input file the user can see in capture/.
        alvum_core::progress::tick_stage(alvum_core::progress::STAGE_PROCESS);
    }

    // Sort by timestamp
    observations.sort_by_key(|o| o.ts);

    info!(observations = observations.len(), "audio processing complete");
    Ok(observations)
}

#[cfg(test)]
mod segment_filter_tests {
    use super::*;

    #[test]
    fn segment_filter_default_matches_openai_reference() {
        // The Whisper reference defaults are no_speech_threshold=0.6 and
        // logprob_threshold=-1.0 (≈ token-prob ≥ 0.37). We use 0.5 on
        // mean token probability as a slightly stricter floor for the
        // small "base" model we ship. Lock these down so a casual edit
        // doesn't silently change filter behaviour.
        let f = SegmentFilter::default();
        assert!((f.no_speech_prob_max - 0.6).abs() < f32::EPSILON);
        assert!((f.mean_token_prob_min - 0.5).abs() < f32::EPSILON);
    }
}
