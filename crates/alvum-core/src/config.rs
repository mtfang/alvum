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

impl AlvumConfig {
    /// Load config from the default path (~/.config/alvum/config.toml).
    /// Returns default config if file doesn't exist.
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read config: {}", path.display()))?;
            toml::from_str(&content)
                .with_context(|| format!("failed to parse config: {}", path.display()))
        } else {
            Ok(Self::default())
        }
    }

    /// Load config from a specific path.
    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", path.display()))
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

        // Audio connector - enabled by default, uses system defaults
        let mut audio_settings = HashMap::new();
        audio_settings.insert("capture_dir".into(), toml::Value::String("capture".into()));
        connectors.insert("audio".into(), ConnectorConfig {
            enabled: true,
            settings: audio_settings,
        });

        Self {
            pipeline: PipelineConfig::default(),
            connectors,
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
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("alvum")
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
    fn default_config_has_audio_connector() {
        let config = AlvumConfig::default();
        assert!(config.connectors.contains_key("audio"));
    }

    #[test]
    fn roundtrip_toml() {
        let config = AlvumConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AlvumConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.pipeline.provider, "cli");
        assert!(parsed.connectors.contains_key("claude-code"));
    }

    #[test]
    fn enabled_connectors_filters() {
        let mut config = AlvumConfig::default();
        config.connectors.get_mut("audio").unwrap().enabled = false;
        let enabled = config.enabled_connectors();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].0, "claude-code");
    }

    #[test]
    fn connector_setting_returns_value() {
        let config = AlvumConfig::default();
        let capture_dir = config.connector_setting("audio", "capture_dir");
        assert_eq!(capture_dir, Some("capture".into()));
    }

    #[test]
    fn missing_connector_returns_none() {
        let config = AlvumConfig::default();
        assert!(config.connector("nonexistent").is_none());
    }
}
