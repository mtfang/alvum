use crate::fingerprint::AudioFingerprint;
use crate::speaker_registry::{SpeakerRegistry, SpeakerSample};
use crate::voice::{AudioIntelligenceArtifact, FingerprintRef, SpeakerTurn};
use alvum_core::data_ref::DataRef;
use alvum_core::observation::{MediaRef, Observation};
use alvum_core::pipeline_events::{self as events, Event};
use anyhow::{Context, Result, bail};
use reqwest::multipart;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::Instant;

const OPENAI_TRANSCRIPTIONS_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const DEFAULT_OPENAI_AUDIO_MODEL: &str = "gpt-4o-transcribe-diarize";
const MAX_AUDIO_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiDiarizedTranscript {
    pub text: String,
    pub turns: Vec<OpenAiSpeakerTurn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenAiSpeakerTurn {
    pub start_secs: f32,
    pub end_secs: f32,
    pub text: String,
    pub provider_speaker: Option<String>,
    pub confidence: Option<f32>,
}

impl OpenAiDiarizedTranscript {
    pub fn from_value(value: serde_json::Value) -> Result<Self> {
        let text = value
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        let turns = value
            .get("segments")
            .or_else(|| value.get("turns"))
            .and_then(|value| value.as_array())
            .context("OpenAI diarized transcription response did not include segments")?
            .iter()
            .map(parse_turn)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { text, turns })
    }

    pub fn into_speaker_turns(
        self,
        speaker_id_for_provider: impl Fn(&str) -> String,
        label_for_speaker_id: impl Fn(&str) -> Option<String>,
    ) -> Vec<SpeakerTurn> {
        self.turns
            .into_iter()
            .map(|turn| {
                let provider_speaker = turn.provider_speaker.unwrap_or_else(|| "speaker".into());
                let speaker_id = speaker_id_for_provider(&provider_speaker);
                let speaker_label = label_for_speaker_id(&speaker_id);
                SpeakerTurn {
                    start_secs: turn.start_secs,
                    end_secs: turn.end_secs,
                    text: turn.text,
                    speaker_id,
                    speaker_label,
                    provider_speaker: Some(provider_speaker),
                    confidence: turn.confidence,
                    fingerprint_ref: None,
                }
            })
            .collect()
    }

    pub fn into_speaker_turns_with_registry(
        self,
        samples: &[f32],
        sample_rate_hz: u32,
        mut registry: Option<&mut SpeakerRegistry>,
        source: &str,
        ts: &str,
        media_path: Option<&str>,
        mime: Option<&str>,
    ) -> Vec<SpeakerTurn> {
        self.turns
            .into_iter()
            .map(|turn| {
                let fingerprint = turn_fingerprint(samples, sample_rate_hz, &turn);
                let provider_speaker = turn.provider_speaker.unwrap_or_else(|| "speaker".into());
                let (speaker_id, speaker_label, fingerprint_ref) =
                    if let Some(fingerprint) = fingerprint {
                        let fingerprint_ref = FingerprintRef {
                            model: fingerprint.model.clone(),
                            digest: fingerprint.digest.clone(),
                        };
                        if let Some(registry) = registry.as_deref_mut() {
                            let speaker_id = registry.resolve_or_create(&fingerprint);
                            let speaker_label = registry.label_for(&speaker_id);
                            let _ = registry.record_sample_with_fingerprint(
                                &speaker_id,
                                Some(fingerprint),
                                SpeakerSample {
                                    text: turn.text.trim().to_string(),
                                    source: source.to_string(),
                                    ts: ts.to_string(),
                                    start_secs: turn.start_secs,
                                    end_secs: turn.end_secs,
                                    media_path: media_path.map(str::to_string),
                                    mime: mime.map(str::to_string),
                                },
                                "openai_diarized",
                            );
                            (speaker_id, speaker_label, Some(fingerprint_ref))
                        } else {
                            (
                                local_speaker_id_for_fingerprint(&fingerprint),
                                None,
                                Some(fingerprint_ref),
                            )
                        }
                    } else {
                        (provider_speaker_id_for_key(&provider_speaker), None, None)
                    };
                SpeakerTurn {
                    start_secs: turn.start_secs,
                    end_secs: turn.end_secs,
                    text: turn.text,
                    speaker_id,
                    speaker_label,
                    provider_speaker: Some(provider_speaker),
                    confidence: turn.confidence,
                    fingerprint_ref,
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiAudioConfig {
    pub api_key: String,
    pub model: String,
    pub language: Option<String>,
    pub endpoint: String,
}

impl OpenAiAudioConfig {
    pub fn from_alvum_config() -> Result<Self> {
        let config = alvum_core::config::AlvumConfig::load()
            .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
        let model = config
            .provider("openai-api")
            .and_then(|provider| provider.settings.get("audio_model"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(DEFAULT_OPENAI_AUDIO_MODEL)
            .to_string();
        Ok(Self {
            api_key: openai_api_key()?,
            model,
            language: config.processor_setting("audio", "whisper_language"),
            endpoint: OPENAI_TRANSCRIPTIONS_URL.into(),
        })
    }
}

pub fn default_openai_audio_model() -> &'static str {
    DEFAULT_OPENAI_AUDIO_MODEL
}

pub fn openai_api_key() -> Result<String> {
    if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    alvum_core::keychain::read_provider_secret("openai-api", "api_key")?
        .filter(|key| !key.trim().is_empty())
        .context("OpenAI API key required. Add it in Alvum Providers setup or set OPENAI_API_KEY")
}

pub async fn process_audio_data_refs(
    config: OpenAiAudioConfig,
    data_refs: &[DataRef],
) -> Result<Vec<Observation>> {
    if data_refs.is_empty() {
        return Ok(vec![]);
    }
    let client = reqwest::Client::new();
    let registry_path = SpeakerRegistry::default_path();
    let mut registry = SpeakerRegistry::load_or_default(&registry_path)?;
    let mut observations = Vec::new();
    for data_ref in data_refs {
        match transcribe_data_ref(&client, &config, data_ref, &mut registry).await {
            Ok(artifact) => {
                if let Some(text) = artifact.text()
                    && !text.trim().is_empty()
                {
                    observations.push(Observation {
                        ts: artifact.data_ref.ts,
                        source: artifact.data_ref.source.clone(),
                        kind: "speech_segment".into(),
                        content: text.to_string(),
                        metadata: artifact
                            .layer("structured.audio.v2")
                            .cloned()
                            .or_else(|| artifact.layer("structured").cloned()),
                        media_ref: Some(MediaRef {
                            path: artifact.data_ref.path.clone(),
                            mime: artifact.data_ref.mime.clone(),
                        }),
                    });
                }
            }
            Err(error) => {
                events::emit(Event::Error {
                    source: "processor/audio/openai".into(),
                    message: format!("{}: {error:#}", data_ref.path),
                });
                return Err(error);
            }
        }
        alvum_core::progress::tick_stage(alvum_core::progress::STAGE_PROCESS);
    }
    observations.sort_by_key(|observation| observation.ts);
    registry.save()?;
    Ok(observations)
}

async fn transcribe_data_ref(
    client: &reqwest::Client,
    config: &OpenAiAudioConfig,
    data_ref: &DataRef,
    registry: &mut SpeakerRegistry,
) -> Result<alvum_core::artifact::Artifact> {
    let path = Path::new(&data_ref.path);
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to stat audio file {}", data_ref.path))?;
    if metadata.len() > MAX_AUDIO_BYTES {
        bail!(
            "OpenAI transcription file limit is 25 MiB; {} is {:.1} MiB",
            data_ref.path,
            metadata.len() as f64 / (1024.0 * 1024.0)
        );
    }
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read audio file {}", data_ref.path))?;
    let samples = crate::decoder::decode_wav_file(path).unwrap_or_default();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.wav")
        .to_string();
    let file_part = multipart::Part::bytes(bytes.clone()).file_name(file_name.clone());
    let file_part = match file_part.mime_str(&data_ref.mime) {
        Ok(part) => part,
        Err(_) => multipart::Part::bytes(bytes).file_name(file_name),
    };
    let mut form = multipart::Form::new()
        .text("model", config.model.clone())
        .text("response_format", "diarized_json")
        .text("chunking_strategy", "auto")
        .part("file", file_part);
    if let Some(language) = config.language.as_deref().filter(|value| *value != "auto") {
        form = form.text("language", language.to_string());
    }

    events::emit(Event::LlmCallStart {
        call_site: "audio/openai/transcribe".into(),
        provider: "openai-api".into(),
        prompt_chars: 0,
        prompt_tokens_estimate: 0,
    });
    let started = Instant::now();
    let response = client
        .post(&config.endpoint)
        .bearer_auth(&config.api_key)
        .multipart(form)
        .send()
        .await
        .context("OpenAI transcription request failed")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read OpenAI transcription response")?;
    if !status.is_success() {
        events::emit(Event::LlmCallEnd {
            call_site: "audio/openai/transcribe".into(),
            provider: "openai-api".into(),
            prompt_chars: 0,
            latency_ms: started.elapsed().as_millis() as u64,
            response_chars: body.len(),
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            tokens_per_sec: None,
            token_source: None,
            prompt_tokens_estimate: 0,
            response_tokens_estimate: 0,
            total_tokens_estimate: 0,
            tokens_per_sec_estimate: None,
            stop_reason: Some(format!("http_{status}")),
            content_block_kinds: None,
            attempts: 1,
            ok: false,
        });
        bail!("OpenAI transcription failed with {status}: {body}");
    }
    let value: serde_json::Value =
        serde_json::from_str(&body).context("OpenAI returned malformed transcription JSON")?;
    let transcript = OpenAiDiarizedTranscript::from_value(value)?;
    let response_chars = transcript.text.len();
    events::emit(Event::LlmCallEnd {
        call_site: "audio/openai/transcribe".into(),
        provider: "openai-api".into(),
        prompt_chars: 0,
        latency_ms: started.elapsed().as_millis() as u64,
        response_chars,
        input_tokens: None,
        output_tokens: None,
        total_tokens: None,
        tokens_per_sec: None,
        token_source: None,
        prompt_tokens_estimate: 0,
        response_tokens_estimate: 0,
        total_tokens_estimate: 0,
        tokens_per_sec_estimate: None,
        stop_reason: None,
        content_block_kinds: Some(vec!["audio_transcription".into(), "diarization".into()]),
        attempts: 1,
        ok: true,
    });

    let text = transcript.text.clone();
    let turns = transcript.into_speaker_turns_with_registry(
        &samples,
        16_000,
        Some(registry),
        &data_ref.source,
        &data_ref.ts.to_rfc3339(),
        Some(&data_ref.path),
        Some(&data_ref.mime),
    );
    events::emit(Event::InputFiltered {
        processor: "openai-transcribe-diarize".into(),
        file: Some(data_ref.path.clone()),
        kept: turns.len(),
        dropped: 0,
        reasons: serde_json::json!({}),
    });
    Ok(AudioIntelligenceArtifact::new(
        data_ref.clone(),
        text,
        turns,
        "openai_diarized_transcription",
        if samples.is_empty() {
            "provider_speaker_labels"
        } else {
            "alvum.acoustic-v1"
        },
    )
    .into_artifact())
}

fn parse_turn(value: &serde_json::Value) -> Result<OpenAiSpeakerTurn> {
    Ok(OpenAiSpeakerTurn {
        start_secs: value
            .get("start")
            .or_else(|| value.get("start_secs"))
            .and_then(|value| value.as_f64())
            .unwrap_or_default() as f32,
        end_secs: value
            .get("end")
            .or_else(|| value.get("end_secs"))
            .and_then(|value| value.as_f64())
            .unwrap_or_default() as f32,
        text: value
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string(),
        provider_speaker: value
            .get("speaker")
            .or_else(|| value.get("speaker_id"))
            .and_then(|value| value.as_str())
            .map(str::to_string),
        confidence: value
            .get("confidence")
            .and_then(|value| value.as_f64())
            .map(|value| value as f32),
    })
}

fn provider_speaker_id_for_key(provider_speaker: &str) -> String {
    let mut hasher = StableHasher::default();
    provider_speaker.hash(&mut hasher);
    format!(
        "spk_provider_{:012x}",
        hasher.finish() & 0x0000_ffff_ffff_ffff
    )
}

fn local_speaker_id_for_fingerprint(fingerprint: &AudioFingerprint) -> String {
    format!(
        "spk_local_{}",
        &fingerprint.digest[..12.min(fingerprint.digest.len())]
    )
}

fn turn_fingerprint(
    samples: &[f32],
    sample_rate_hz: u32,
    turn: &OpenAiSpeakerTurn,
) -> Option<AudioFingerprint> {
    if samples.is_empty() || sample_rate_hz == 0 {
        return None;
    }
    let start = (turn.start_secs.max(0.0) * sample_rate_hz as f32).floor() as usize;
    let mut end = (turn.end_secs.max(turn.start_secs) * sample_rate_hz as f32).ceil() as usize;
    end = end.min(samples.len());
    if start >= end || start >= samples.len() {
        return None;
    }
    Some(AudioFingerprint::from_samples(
        &samples[start..end],
        sample_rate_hz,
    ))
}

#[derive(Default)]
struct StableHasher(u64);

impl Hasher for StableHasher {
    fn write(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf29ce484222325;
        }
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
