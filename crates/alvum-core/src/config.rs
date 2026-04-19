//! Application configuration loaded from ~/.config/alvum/config.toml
//! CLI flags override config values.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Top-level config structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlvumConfig {
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub connectors: HashMap<String, ConnectorConfig>,
    #[serde(default)]
    pub capture: HashMap<String, CaptureSourceConfig>,
    #[serde(default)]
    pub processors: HashMap<String, ProcessorConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_output_dir")]
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Connector-specific settings as key-value pairs.
    #[serde(flatten)]
    pub settings: HashMap<String, toml::Value>,
}

/// Configuration for a capture source (always-on daemon).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Source-specific settings as key-value pairs.
    #[serde(flatten)]
    pub settings: HashMap<String, toml::Value>,
}

/// Configuration for a processor (processing settings used during extract).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorConfig {
    /// Processor-specific settings as key-value pairs.
    #[serde(flatten)]
    pub settings: HashMap<String, toml::Value>,
}

impl AlvumConfig {
    /// Load config from the default path (~/.config/alvum/config.toml).
    /// Returns default config if file doesn't exist.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read config: {}", path.display()))?;
            let mut config: Self = toml::from_str(&content)
                .with_context(|| format!("failed to parse config: {}", path.display()))?;
            config.migrate();
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Load config from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let mut config: Self = toml::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        config.migrate();
        Ok(config)
    }

    /// Save config to the default path.
    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)
            .context("failed to serialize config")?;
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write config: {}", path.display()))?;
        Ok(())
    }

    /// Get a connector config by name. Returns None if not configured.
    pub fn connector(&self, name: &str) -> Option<&ConnectorConfig> {
        self.connectors.get(name)
    }

    /// Get a connector setting as a string.
    pub fn connector_setting(&self, connector: &str, key: &str) -> Option<String> {
        self.connectors.get(connector)?
            .settings.get(key)?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Get all enabled connectors.
    pub fn enabled_connectors(&self) -> Vec<(&str, &ConnectorConfig)> {
        self.connectors.iter()
            .filter(|(_, c)| c.enabled)
            .map(|(name, config)| (name.as_str(), config))
            .collect()
    }

    /// Get a capture source config by name. Returns None if not configured.
    pub fn capture_source(&self, name: &str) -> Option<&CaptureSourceConfig> {
        self.capture.get(name)
    }

    /// Get a capture source setting as a string.
    pub fn capture_setting(&self, source: &str, key: &str) -> Option<String> {
        self.capture.get(source)?
            .settings.get(key)?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Get all enabled capture sources.
    pub fn enabled_capture_sources(&self) -> Vec<(&str, &CaptureSourceConfig)> {
        self.capture.iter()
            .filter(|(_, c)| c.enabled)
            .map(|(name, config)| (name.as_str(), config))
            .collect()
    }

    /// Get a processor config by name.
    pub fn processor(&self, name: &str) -> Option<&ProcessorConfig> {
        self.processors.get(name)
    }

    /// Get a processor setting as a string.
    pub fn processor_setting(&self, processor: &str, key: &str) -> Option<String> {
        self.processors.get(processor)?
            .settings.get(key)?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Migrate deprecated config formats to current.
    /// - `[connectors.audio]` → `[capture.audio-mic]` + `[capture.audio-system]`
    /// - Fills in connector + capture-source defaults added in newer versions,
    ///   so existing users pick up new data sources (e.g., `codex`) automatically.
    fn migrate(&mut self) {
        let defaults = Self::default();

        // Fill any capture source the user doesn't have yet.
        for (name, default_config) in &defaults.capture {
            if !self.capture.contains_key(name) {
                self.capture.insert(name.clone(), default_config.clone());
            }
        }

        // Fill any connector the user doesn't have yet. New connectors added
        // to AlvumConfig::default() propagate to existing users on next load.
        for (name, default_config) in &defaults.connectors {
            if !self.connectors.contains_key(name) {
                self.connectors.insert(name.clone(), default_config.clone());
            }
        }

        // Legacy migration: propagate enabled state from [connectors.audio]
        if let Some(audio_connector) = self.connectors.get("audio") {
            let enabled = audio_connector.enabled;
            if let Some(mic) = self.capture.get_mut("audio-mic") {
                mic.enabled = enabled;
            }
            if let Some(sys) = self.capture.get_mut("audio-system") {
                sys.enabled = enabled;
            }
        }
    }
}

impl Default for AlvumConfig {
    fn default() -> Self {
        let mut connectors = HashMap::new();

        // Claude Code connector - enabled by default
        let mut claude_settings = HashMap::new();
        claude_settings.insert("session_dir".into(), toml::Value::String(
            dirs::home_dir()
                .map(|h| h.join(".claude/projects").to_string_lossy().into_owned())
                .unwrap_or_else(|| "~/.claude/projects".into())
        ));
        claude_settings.insert("auto_detect_latest".into(), toml::Value::Boolean(true));
        connectors.insert("claude-code".into(), ConnectorConfig {
            enabled: true,
            settings: claude_settings,
        });

        // Codex CLI connector — enabled by default. Reads ~/.codex/sessions/.
        let mut codex_settings = HashMap::new();
        codex_settings.insert("session_dir".into(), toml::Value::String(
            dirs::home_dir()
                .map(|h| h.join(".codex").to_string_lossy().into_owned())
                .unwrap_or_else(|| "~/.codex".into())
        ));
        connectors.insert("codex".into(), ConnectorConfig {
            enabled: true,
            settings: codex_settings,
        });

        // Screen connector — enabled by default. Reads capture/<date>/screen/
        // captures.jsonl produced by the screen daemon; produces text via OCR by
        // default (no network, no LLM cost). Swap `vision` to "local" (Ollama)
        // or "api" (Claude) for richer description.
        let mut screen_settings = HashMap::new();
        screen_settings.insert("vision".into(), toml::Value::String("ocr".into()));
        connectors.insert("screen".into(), ConnectorConfig {
            enabled: true,
            settings: screen_settings,
        });

        // Audio connector — enabled by default. Reads capture/<date>/audio/{mic,system}/
        // produced by the audio daemon; transcribes via Whisper.
        let audio_settings = HashMap::new();
        connectors.insert("audio".into(), ConnectorConfig {
            enabled: true,
            settings: audio_settings,
        });

        // Capture sources
        let mut capture = HashMap::new();

        let mut mic_settings = HashMap::new();
        mic_settings.insert("device".into(), toml::Value::String("default".into()));
        mic_settings.insert("chunk_duration_secs".into(), toml::Value::Integer(60));
        capture.insert("audio-mic".into(), CaptureSourceConfig {
            enabled: true,
            settings: mic_settings,
        });

        let mut sys_settings = HashMap::new();
        sys_settings.insert("device".into(), toml::Value::String("default".into()));
        capture.insert("audio-system".into(), CaptureSourceConfig {
            enabled: true,
            settings: sys_settings,
        });

        let mut screen_settings = HashMap::new();
        screen_settings.insert("idle_interval_secs".into(), toml::Value::Integer(30));
        capture.insert("screen".into(), CaptureSourceConfig {
            enabled: true,
            settings: screen_settings,
        });

        // Processors (empty by default — no model paths to assume)
        let processors = HashMap::new();

        Self {
            pipeline: PipelineConfig::default(),
            connectors,
            capture,
            processors,
        }
    }
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            model: default_model(),
            output_dir: default_output_dir(),
        }
    }
}

/// Default config file path.
/// Per the storage-layout spec (2026-04-18), config lives under the single
/// ~/.alvum/ root in runtime/, not in the OS config dir.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".alvum")
        .join("runtime")
        .join("config.toml")
}

fn default_provider() -> String { "cli".into() }
fn default_model() -> String { "claude-sonnet-4-6".into() }
fn default_output_dir() -> PathBuf { PathBuf::from("output") }
fn default_true() -> bool { true }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_claude_connector() {
        let config = AlvumConfig::default();
        assert!(config.connectors.contains_key("claude-code"));
        assert!(config.connector("claude-code").unwrap().enabled);
    }

    #[test]
    fn default_config_has_capture_sources() {
        let config = AlvumConfig::default();
        assert!(config.capture.contains_key("audio-mic"));
        assert!(config.capture.contains_key("audio-system"));
        assert!(config.capture.contains_key("screen"));
        assert!(config.capture_source("audio-mic").unwrap().enabled);
    }

    #[test]
    fn default_config_has_all_connectors() {
        // Default enables every first-class connector so fresh installs work
        // end-to-end. The legacy combined [connectors.audio] from pre-capture
        // days is NOT the same thing — capture.audio-* handles daemon-level
        // config now, while connectors.audio enables extract-time processing.
        let config = AlvumConfig::default();
        assert!(config.connectors.contains_key("claude-code"));
        assert!(config.connectors.contains_key("codex"));
        assert!(config.connectors.contains_key("screen"));
        assert!(config.connectors.contains_key("audio"));
    }

    #[test]
    fn roundtrip_toml() {
        let config = AlvumConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AlvumConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.pipeline.provider, "cli");
        assert!(parsed.connectors.contains_key("claude-code"));
        assert!(parsed.capture.contains_key("audio-mic"));
        assert!(parsed.capture.contains_key("screen"));
    }

    #[test]
    fn enabled_connectors_filters() {
        let mut config = AlvumConfig::default();
        // Disable every connector and confirm the filter returns empty.
        for c in config.connectors.values_mut() {
            c.enabled = false;
        }
        let enabled = config.enabled_connectors();
        assert_eq!(enabled.len(), 0);
    }

    #[test]
    fn enabled_capture_sources_filters() {
        let mut config = AlvumConfig::default();
        config.capture.get_mut("audio-system").unwrap().enabled = false;
        let enabled = config.enabled_capture_sources();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.iter().any(|(name, _)| *name == "audio-mic"));
        assert!(enabled.iter().any(|(name, _)| *name == "screen"));
    }

    #[test]
    fn capture_setting_returns_value() {
        let config = AlvumConfig::default();
        let device = config.capture_setting("audio-mic", "device");
        assert_eq!(device, Some("default".into()));
    }

    #[test]
    fn missing_capture_source_returns_none() {
        let config = AlvumConfig::default();
        assert!(config.capture_source("nonexistent").is_none());
    }

    #[test]
    fn processor_setting_returns_value() {
        let mut config = AlvumConfig::default();
        let mut audio_proc = HashMap::new();
        audio_proc.insert("whisper_model".into(), toml::Value::String("/path/to/model.bin".into()));
        config.processors.insert("audio".into(), ProcessorConfig {
            settings: audio_proc,
        });
        assert_eq!(
            config.processor_setting("audio", "whisper_model"),
            Some("/path/to/model.bin".into())
        );
    }

    #[test]
    fn migration_from_old_audio_connector() {
        // Simulate old config with [connectors.audio]
        let toml_str = r#"
[pipeline]
provider = "cli"
model = "claude-sonnet-4-6"
output_dir = "output"

[connectors.audio]
enabled = true
capture_dir = "capture"

[connectors.claude-code]
enabled = true
session_dir = "~/.claude/projects"
"#;
        let config: AlvumConfig = toml::from_str(toml_str).unwrap();
        // Before migration, capture is empty
        assert!(config.capture.is_empty());

        // After migration via load path
        let mut config = config;
        config.migrate();
        assert!(config.capture.contains_key("audio-mic"));
        assert!(config.capture.contains_key("audio-system"));
        assert!(config.capture.contains_key("screen"));
        assert!(config.capture_source("audio-mic").unwrap().enabled);
    }

    #[test]
    fn migration_skipped_when_capture_already_configured() {
        let toml_str = r#"
[pipeline]
provider = "cli"

[connectors.audio]
enabled = true
capture_dir = "capture"

[capture.audio-mic]
enabled = true
device = "Rode NT-USB"
chunk_duration_secs = 120
"#;
        let mut config: AlvumConfig = toml::from_str(toml_str).unwrap();
        config.migrate();
        // Existing capture config preserved (not overwritten by defaults)
        assert_eq!(
            config.capture_setting("audio-mic", "device"),
            Some("Rode NT-USB".into())
        );
        // Missing sources filled from defaults
        assert!(config.capture.contains_key("audio-system"));
        assert!(config.capture.contains_key("screen"));
    }
}
