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
    whisper_model: Option<PathBuf>,
    whisper_language: String,
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
        let whisper_model = settings
            .get("whisper_model")
            .and_then(|v| v.as_str())
            .map(|s| {
                // Expand ~
                if let Some(stripped) = s.strip_prefix("~/") {
                    if let Some(home) = dirs::home_dir() {
                        return home.join(stripped);
                    }
                }
                PathBuf::from(s)
            });
        let whisper_language = settings
            .get("whisper_language")
            .and_then(|v| v.as_str())
            .unwrap_or("en")
            .to_string();

        Ok(Self {
            mic_enabled,
            system_enabled,
            mic_device,
            chunk_duration_secs,
            whisper_model,
            whisper_language,
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
        match &self.whisper_model {
            Some(path) => {
                let config = alvum_processor_audio::transcriber::TranscriberConfig {
                    language: self.whisper_language.clone(),
                    ..Default::default()
                };
                vec![Box::new(AudioProcessor::new(path.clone(), config))]
            }
            None => vec![],
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
