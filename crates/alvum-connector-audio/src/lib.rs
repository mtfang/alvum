//! AudioConnector — user-facing plugin bundling audio capture + whisper processing.

pub mod processor;

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::processor::Processor;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use processor::AudioProcessor;

pub struct AudioConnector {
    mic_enabled: bool,
    system_enabled: bool,
    mic_device: Option<String>,
    chunk_duration_secs: u32,
    processor_mode: AudioProcessorMode,
    whisper_model: Option<PathBuf>,
    whisper_language: String,
    diarization_enabled: bool,
    diarization_model: String,
    pyannote_command: Option<String>,
    speaker_registry: Option<PathBuf>,
    provider: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudioProcessorMode {
    Local,
    Provider,
    Off,
}

impl AudioProcessorMode {
    fn from_str(value: &str) -> Self {
        match value {
            "provider" => Self::Provider,
            "off" => Self::Off,
            _ => Self::Local,
        }
    }
}

impl AudioConnector {
    pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let mic_enabled = settings
            .get("mic")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let system_enabled = settings
            .get("system")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let mic_device = settings
            .get("mic_device")
            .and_then(|v| v.as_str())
            .map(String::from);
        let chunk_duration_secs = settings
            .get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .map(|n| n as u32)
            .unwrap_or(60);
        let processor_mode = settings
            .get("mode")
            .and_then(|v| v.as_str())
            .map(AudioProcessorMode::from_str)
            .unwrap_or(AudioProcessorMode::Local);
        let whisper_model = settings
            .get("whisper_model")
            .and_then(|v| v.as_str())
            .map(expand_path)
            .or_else(default_whisper_model_path);
        let whisper_language = settings
            .get("whisper_language")
            .and_then(|v| v.as_str())
            .unwrap_or("en")
            .to_string();
        let diarization_enabled = settings
            .get("diarization_enabled")
            .and_then(|v| {
                v.as_bool()
                    .or_else(|| v.as_str().map(|value| value != "false" && value != "off"))
            })
            .unwrap_or(true);
        let diarization_model = settings
            .get("diarization_model")
            .and_then(|v| v.as_str())
            .unwrap_or("pyannote-local")
            .to_string();
        let pyannote_command = settings
            .get("pyannote_command")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(String::from);
        let speaker_registry = settings
            .get("speaker_registry")
            .and_then(|v| v.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(expand_path);
        let provider = settings
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("openai-api")
            .to_string();

        Ok(Self {
            mic_enabled,
            system_enabled,
            mic_device,
            chunk_duration_secs,
            processor_mode,
            whisper_model,
            whisper_language,
            diarization_enabled,
            diarization_model,
            pyannote_command,
            speaker_registry,
            provider,
        })
    }
}

impl Connector for AudioConnector {
    fn name(&self) -> &str {
        "audio"
    }

    /// Mic / system are independently config-gated; declare only the
    /// ones we'd actually expect refs from on this run. A user who
    /// disabled the mic shouldn't see a "audio-mic silent" warning.
    fn expected_sources(&self) -> Vec<&'static str> {
        let mut sources = Vec::new();
        if self.mic_enabled {
            sources.push("audio-mic");
        }
        if self.system_enabled {
            sources.push("audio-system");
        }
        sources
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        let mut sources: Vec<Box<dyn CaptureSource>> = Vec::new();

        if self.mic_enabled {
            let mut mic_settings = HashMap::new();
            if let Some(ref d) = self.mic_device {
                mic_settings.insert("device".into(), toml::Value::String(d.clone()));
            }
            mic_settings.insert(
                "chunk_duration_secs".into(),
                toml::Value::Integer(self.chunk_duration_secs as i64),
            );
            sources.push(Box::new(
                alvum_capture_audio::source::AudioMicSource::from_config(
                    &alvum_core::config::CaptureSourceConfig {
                        enabled: true,
                        settings: mic_settings,
                    },
                ),
            ));
        }

        if self.system_enabled {
            sources.push(Box::new(
                alvum_capture_audio::source::AudioSystemSource::from_config(
                    &alvum_core::config::CaptureSourceConfig {
                        enabled: true,
                        settings: HashMap::new(),
                    },
                ),
            ));
        }

        sources
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        match self.processor_mode {
            AudioProcessorMode::Off => vec![],
            AudioProcessorMode::Provider => {
                vec![Box::new(AudioProcessor::provider(self.provider.clone()))]
            }
            AudioProcessorMode::Local => match &self.whisper_model {
                Some(path) => {
                    let config = alvum_processor_audio::transcriber::TranscriberConfig {
                        language: self.whisper_language.clone(),
                        diarization_enabled: self.diarization_enabled,
                        diarization_model: self.diarization_model.clone(),
                        pyannote_command: self.pyannote_command.clone(),
                        speaker_registry_path: self.speaker_registry.clone(),
                        ..Default::default()
                    };
                    vec![Box::new(AudioProcessor::new(path.clone(), config))]
                }
                None => vec![],
            },
        }
    }

    fn gather_data_refs(
        &self,
        capture_dir: &std::path::Path,
    ) -> Result<Vec<alvum_core::data_ref::DataRef>> {
        let mut refs = Vec::new();
        if self.mic_enabled {
            refs.extend(scan_audio_dir(
                &capture_dir.join("audio").join("mic"),
                "audio-mic",
            )?);
        }
        if self.system_enabled {
            refs.extend(scan_audio_dir(
                &capture_dir.join("audio").join("system"),
                "audio-system",
            )?);
        }
        Ok(refs)
    }
}

fn expand_path(value: &str) -> PathBuf {
    if let Some(stripped) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(value)
}

fn default_whisper_model_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".alvum")
            .join("runtime")
            .join("models")
            .join("ggml-base.en.bin")
    })
}

/// Walk an audio capture sub-directory and emit one DataRef per WAV/Opus file.
/// `path` is recorded absolute; `ts` is the file's modification time so the
/// pipeline orders refs chronologically without parsing filenames.
///
/// Self-diagnose: a missing dir or a dir with no audio files emits a
/// `Warning` event so the operator knows whether (a) capture isn't
/// running or (b) it's running but producing no audio.
fn scan_audio_dir(
    dir: &std::path::Path,
    source: &str,
) -> Result<Vec<alvum_core::data_ref::DataRef>> {
    use alvum_core::pipeline_events::{Event, emit};
    use std::time::SystemTime;
    let mut refs = Vec::new();
    if !dir.exists() {
        emit(Event::Warning {
            source: format!("connector/audio[{source}]"),
            message: format!("scan dir does not exist: {}", dir.display()),
        });
        return Ok(refs);
    }
    let mut files_seen = 0usize;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        files_seen += 1;
        let path = entry.path();
        let mime = match path.extension().and_then(|e| e.to_str()) {
            Some("wav") => "audio/wav",
            Some("opus") => "audio/opus",
            _ => continue,
        };
        let mtime: SystemTime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        refs.push(alvum_core::data_ref::DataRef {
            ts: mtime.into(),
            source: source.into(),
            producer: format!("alvum.audio/{source}"),
            schema: format!("alvum.{}.v1", mime.replace('/', ".")),
            path: path.to_string_lossy().into_owned(),
            mime: mime.into(),
            metadata: None,
        });
    }
    if refs.is_empty() {
        emit(Event::Warning {
            source: format!("connector/audio[{source}]"),
            message: format!(
                "scanned {} ({} file(s)); no .wav or .opus matched",
                dir.display(),
                files_seen,
            ),
        });
    }
    Ok(refs)
}
