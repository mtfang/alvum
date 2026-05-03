//! Whisper-based audio transcription. Takes f32 PCM samples, returns timestamped segments.

use alvum_core::artifact::Artifact;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::{MediaRef, Observation};
use alvum_core::pipeline_events::{self as events, Event};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::info;

use crate::fingerprint::AudioFingerprint;
use crate::pyannote::{PyannoteDiarization, align_segments_to_diarization};
use crate::speaker_registry::{SpeakerRegistry, SpeakerSample};
use crate::voice::{AudioIntelligenceArtifact, FingerprintRef, SpeakerTurn};

const PYANNOTE_HF_ACCESS_MESSAGE: &str = "Pyannote Community-1 requires Hugging Face access. Accept the model terms at https://huggingface.co/pyannote/speaker-diarization-community-1, then sign in with Hugging Face or set HF_TOKEN and run install again.";

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
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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
    /// Local speaker registry used to stabilize anonymous speaker IDs.
    pub speaker_registry_path: Option<PathBuf>,
    /// Persist anonymous speaker IDs across runs.
    pub diarization_enabled: bool,
    /// Local diarization backend/model. `pyannote-local` enables speaker
    /// turns only when a pyannote JSON command is configured.
    pub diarization_model: String,
    /// Optional command that emits pyannote-compatible diarization JSON for
    /// one audio file. Alvum does not execute arbitrary renderer input; this
    /// is config-owned processor state.
    pub pyannote_command: Option<String>,
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self {
            language: "en".into(),
            filter: SegmentFilter::default(),
            speaker_registry_path: None,
            diarization_enabled: true,
            diarization_model: "pyannote-local".into(),
            pyannote_command: None,
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
            model_path
                .to_str()
                .context("model path must be valid UTF-8")?,
            whisper_rs::WhisperContextParameters::default(),
        )
        .context("failed to load Whisper model")?;

        info!(model = %model_path.display(), language = %config.language, "loaded Whisper model");
        Ok(Self { ctx, config })
    }

    /// Transcribe a single audio DataRef. Returns an Artifact with text + structured layers.
    pub fn transcribe_data_ref(&self, data_ref: &DataRef) -> Result<Artifact> {
        self.transcribe_data_ref_with_registry(data_ref, None)
    }

    pub fn transcribe_data_ref_with_registry(
        &self,
        data_ref: &DataRef,
        registry: Option<&mut SpeakerRegistry>,
    ) -> Result<Artifact> {
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
        let full_text = segments
            .iter()
            .map(|s| s.text.trim())
            .collect::<Vec<_>>()
            .join(" ");

        let full_fingerprint = AudioFingerprint::from_samples(&samples, 16_000);
        let aligned_diarization = self.pyannote_alignment(path, &segments);
        let mut registry = registry;
        let turns = segments
            .iter()
            .enumerate()
            .map(|(index, segment)| {
                let aligned = aligned_diarization
                    .as_ref()
                    .and_then(|aligned| aligned.get(index));
                let fingerprint =
                    aligned
                        .map(|aligned| aligned.fingerprint.clone())
                        .or_else(|| {
                            if self.config.diarization_model == "alvum.acoustic-v1" {
                                segment_fingerprint(&samples, 16_000, segment)
                            } else {
                                None
                            }
                        });
                let fingerprint_ref = FingerprintRef {
                    model: fingerprint
                        .as_ref()
                        .map(|fingerprint| fingerprint.model.clone())
                        .unwrap_or_else(|| "unassigned".into()),
                    digest: fingerprint
                        .as_ref()
                        .map(|fingerprint| fingerprint.digest.clone())
                        .unwrap_or_default(),
                };
                let provider_speaker = aligned.and_then(|aligned| aligned.provider_speaker.clone());
                let confidence = aligned.and_then(|aligned| aligned.confidence);
                let (speaker_id, speaker_label, fingerprint_ref) =
                    if let Some(fingerprint) = fingerprint {
                        if let Some(registry) = registry.as_deref_mut() {
                            let speaker_id = registry.resolve_or_create(&fingerprint);
                            let label = registry.label_for(&speaker_id);
                            let _ = registry.record_sample_with_fingerprint(
                                &speaker_id,
                                Some(fingerprint.clone()),
                                SpeakerSample {
                                    text: segment.text.trim().to_string(),
                                    source: data_ref.source.clone(),
                                    ts: data_ref.ts.to_rfc3339(),
                                    start_secs: segment.start_secs,
                                    end_secs: segment.end_secs,
                                    media_path: Some(data_ref.path.clone()),
                                    mime: Some(data_ref.mime.clone()),
                                },
                                if aligned.is_some() {
                                    "pyannote"
                                } else {
                                    "legacy_acoustic"
                                },
                            );
                            (speaker_id, label, Some(fingerprint_ref))
                        } else {
                            (
                                speaker_id_for_fingerprint(&fingerprint),
                                None,
                                Some(fingerprint_ref),
                            )
                        }
                    } else {
                        ("voice_unassigned".into(), None, None)
                    };
                SpeakerTurn {
                    start_secs: segment.start_secs,
                    end_secs: segment.end_secs,
                    text: segment.text.clone(),
                    speaker_id,
                    speaker_label,
                    provider_speaker,
                    confidence,
                    fingerprint_ref,
                }
            })
            .collect::<Vec<_>>();

        let artifact = AudioIntelligenceArtifact::new(
            data_ref.clone(),
            full_text.clone(),
            turns,
            "local_whisper",
            full_fingerprint.model.clone(),
        );
        let mut artifact = artifact.into_artifact();
        artifact.add_layer(
            "structured.whisper",
            serde_json::json!({
                "segments": segments,
                "duration_secs": duration_secs,
                "sample_count": samples.len(),
                "filtered_count": dropped.values().sum::<usize>(),
            }),
        );
        artifact.add_layer(
            "structured.diarization.readiness",
            serde_json::json!({
                "enabled": self.config.diarization_enabled,
                "model": self.config.diarization_model,
                "available": aligned_diarization.is_some(),
                "source": if aligned_diarization.is_some() { "pyannote" } else { "unavailable" },
            }),
        );

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

    fn pyannote_alignment(
        &self,
        audio_path: &Path,
        segments: &[Segment],
    ) -> Option<Vec<crate::pyannote::AlignedDiarizedSegment>> {
        if !self.config.diarization_enabled
            || self.config.diarization_model != "pyannote-local"
            || segments.is_empty()
        {
            return None;
        }
        let command = self.config.pyannote_command.as_deref()?.trim();
        if command.is_empty() {
            return None;
        }
        let output = Command::new(command)
            .arg(audio_path)
            .output()
            .inspect_err(|error| {
                tracing::warn!(
                    command,
                    path = %audio_path.display(),
                    error = %error,
                    "pyannote diarization command failed to start"
                );
            })
            .ok()?;
        if !output.status.success() {
            let stderr = pyannote_stderr_summary(&output.stderr);
            tracing::warn!(
                command,
                path = %audio_path.display(),
                status = ?output.status.code(),
                stderr = %stderr,
                "pyannote diarization command failed"
            );
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)
            .inspect_err(|error| {
                tracing::warn!(
                    command,
                    path = %audio_path.display(),
                    error = %error,
                    "pyannote diarization command returned malformed JSON"
                );
            })
            .ok()?;
        let diarization = PyannoteDiarization::from_value(value)
            .inspect_err(|error| {
                tracing::warn!(
                    command,
                    path = %audio_path.display(),
                    error = %error,
                    "pyannote diarization JSON was not usable"
                );
            })
            .ok()?;
        Some(align_segments_to_diarization(segments, &diarization))
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
        let mut state = self
            .ctx
            .create_state()
            .context("failed to create Whisper state")?;

        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.config.language));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // Decoder-side gate: whisper.cpp culls segments above this no-speech
        // probability before they reach our post-filter. Belt + braces with
        // the per-segment check below.
        params.set_no_speech_thold(self.config.filter.no_speech_prob_max);

        state
            .full(params, samples)
            .context("Whisper transcription failed")?;

        let n = state.full_n_segments();
        let mut segments = Vec::new();
        let mut dropped: BTreeMap<String, usize> = BTreeMap::new();

        for i in 0..n {
            let seg = state
                .get_segment(i)
                .with_context(|| format!("segment {i} out of bounds"))?;

            // Timestamps are in centiseconds (10ms units); convert to seconds
            let start = seg.start_timestamp() as f32 / 100.0;
            let end = seg.end_timestamp() as f32 / 100.0;
            let text = seg
                .to_str()
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

    let mut registry = if config.diarization_enabled {
        let registry_path = config
            .speaker_registry_path
            .clone()
            .unwrap_or_else(SpeakerRegistry::default_path);
        Some(SpeakerRegistry::load_or_default(&registry_path)?)
    } else {
        None
    };
    let transcriber = AudioTranscriber::new(model_path, config)?;
    let mut observations = Vec::new();

    for data_ref in data_refs {
        match transcriber.transcribe_data_ref_with_registry(data_ref, registry.as_mut()) {
            Ok(artifact) => {
                if let Some(text) = artifact.text()
                    && !text.is_empty()
                {
                    let metadata = artifact
                        .layer("structured.audio.v2")
                        .cloned()
                        .or_else(|| artifact.layer("structured").cloned());
                    observations.push(Observation {
                        ts: artifact.data_ref.ts,
                        source: artifact.data_ref.source.clone(),
                        kind: "speech_segment".into(),
                        content: text.to_string(),
                        metadata,
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
    if let Some(registry) = registry.as_ref() {
        registry.save()?;
    }

    info!(
        observations = observations.len(),
        "audio processing complete"
    );
    Ok(observations)
}

fn segment_fingerprint(
    samples: &[f32],
    sample_rate_hz: u32,
    segment: &Segment,
) -> Option<AudioFingerprint> {
    if samples.is_empty() || sample_rate_hz == 0 {
        return None;
    }
    let start = (segment.start_secs.max(0.0) * sample_rate_hz as f32).floor() as usize;
    let mut end =
        (segment.end_secs.max(segment.start_secs) * sample_rate_hz as f32).ceil() as usize;
    end = end.min(samples.len());
    if start >= end || start >= samples.len() {
        return None;
    }
    Some(AudioFingerprint::from_samples(
        &samples[start..end],
        sample_rate_hz,
    ))
}

fn speaker_id_for_fingerprint(fingerprint: &AudioFingerprint) -> String {
    format!(
        "spk_local_{}",
        &fingerprint.digest[..12.min(fingerprint.digest.len())]
    )
}

fn pyannote_stderr_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lower = text.to_lowercase();
    if lower.contains("pyannote community-1 requires hugging face access")
        || (lower.contains("huggingface_hub") && lower.contains("get_token_to_send"))
        || (lower.contains("hugging face") && lower.contains("token"))
        || lower.contains("gated repo")
    {
        return PYANNOTE_HF_ACCESS_MESSAGE.into();
    }
    let text = text.trim();
    if text.len() <= 1600 {
        return text.to_string();
    }
    let mut tail = text.chars().rev().take(1600).collect::<Vec<_>>();
    tail.reverse();
    tail.into_iter().collect()
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

    #[test]
    fn pyannote_stderr_summary_maps_huggingface_token_traceback() {
        let stderr = br#"Traceback (most recent call last):
  File "/Users/michael/.alvum/runtime/pyannote/venv/lib/python3.14/site-packages/huggingface_hub/utils/_headers.py", line 108, in build_hf_headers
    token_to_send = get_token_to_send(token)
ValueError: Invalid token
"#;

        assert_eq!(pyannote_stderr_summary(stderr), PYANNOTE_HF_ACCESS_MESSAGE);
    }
}
