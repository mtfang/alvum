//! AudioConnector — user-facing plugin bundling audio capture + whisper processing.

pub mod processor;

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::processor::Processor;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use processor::WhisperProcessor;

pub struct AudioConnector {
    mic_enabled: bool,
    system_enabled: bool,
    mic_device: Option<String>,
    chunk_duration_secs: u32,
    whisper_model: Option<PathBuf>,
}

impl AudioConnector {
    pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let mic_enabled = settings.get("mic")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let system_enabled = settings.get("system")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let mic_device = settings.get("mic_device")
            .and_then(|v| v.as_str())
            .map(String::from);
        let chunk_duration_secs = settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .map(|n| n as u32)
            .unwrap_or(60);
        let whisper_model = settings.get("whisper_model")
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

        Ok(Self {
            mic_enabled,
            system_enabled,
            mic_device,
            chunk_duration_secs,
            whisper_model,
        })
    }
}

impl Connector for AudioConnector {
    fn name(&self) -> &str {
        "audio"
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        let mut sources: Vec<Box<dyn CaptureSource>> = Vec::new();

        if self.mic_enabled {
            let mut mic_settings = HashMap::new();
            if let Some(ref d) = self.mic_device {
                mic_settings.insert("device".into(), toml::Value::String(d.clone()));
            }
            mic_settings.insert("chunk_duration_secs".into(),
                toml::Value::Integer(self.chunk_duration_secs as i64));
            sources.push(Box::new(
                alvum_capture_audio::source::AudioMicSource::from_config(
                    &alvum_core::config::CaptureSourceConfig {
                        enabled: true,
                        settings: mic_settings,
                    }
                )
            ));
        }

        if self.system_enabled {
            sources.push(Box::new(
                alvum_capture_audio::source::AudioSystemSource::from_config(
                    &alvum_core::config::CaptureSourceConfig {
                        enabled: true,
                        settings: HashMap::new(),
                    }
                )
            ));
        }

        sources
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        match &self.whisper_model {
            Some(path) => vec![Box::new(WhisperProcessor::new(path.clone()))],
            None => vec![],
        }
    }
}
