//! ClaudeCodeConnector — reads Claude Code session files (no capture daemon).

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ClaudeCodeConnector {
    session_dir: PathBuf,
    /// Exclude observations at or after this timestamp.
    before_ts: Option<chrono::DateTime<chrono::Utc>>,
    /// Exclude observations earlier than this timestamp. Read from the `since`
    /// TOML key — set per-run by the briefing script to scope to the last 24h.
    after_ts: Option<chrono::DateTime<chrono::Utc>>,
}

impl ClaudeCodeConnector {
    pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let session_dir = settings.get("session_dir")
            .and_then(|v| v.as_str())
            .map(|s| {
                if let Some(stripped) = s.strip_prefix("~/") {
                    if let Some(home) = dirs::home_dir() {
                        return home.join(stripped);
                    }
                }
                PathBuf::from(s)
            })
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.join(".claude/projects"))
                    .unwrap_or_else(|| PathBuf::from("."))
            });

        let after_ts = settings.get("since")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());

        Ok(Self {
            session_dir,
            before_ts: None,
            after_ts,
        })
    }

    pub fn with_before(mut self, before: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        self.before_ts = before;
        self
    }

    pub fn with_since(mut self, since: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        self.after_ts = since;
        self
    }
}

impl Connector for ClaudeCodeConnector {
    fn name(&self) -> &str { "claude-code" }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        vec![] // no capture daemon — reads existing sessions
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(ClaudeCodeProcessor {
            session_dir: self.session_dir.clone(),
            before_ts: self.before_ts,
            after_ts: self.after_ts,
        })]
    }
}

struct ClaudeCodeProcessor {
    session_dir: PathBuf,
    before_ts: Option<chrono::DateTime<chrono::Utc>>,
    after_ts: Option<chrono::DateTime<chrono::Utc>>,
}

#[async_trait]
impl Processor for ClaudeCodeProcessor {
    fn name(&self) -> &str { "claude-code-parser" }

    fn handles(&self) -> Vec<String> {
        vec!["claude-code".into()]
    }

    async fn process(
        &self,
        _data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        // Claude-code is a one-shot connector: ignore data_refs, read session files directly
        if !self.session_dir.exists() {
            return Ok(vec![]);
        }

        info!(dir = %self.session_dir.display(), "scanning claude sessions");

        let mut observations = Vec::new();
        for entry in walkdir::WalkDir::new(&self.session_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    let session_obs = crate::parser::parse_session_filtered(
                        path,
                        self.after_ts,
                        self.before_ts,
                    )?;
                    observations.extend(session_obs);
                }
            }
        }

        info!(obs = observations.len(), "loaded claude observations");
        Ok(observations)
    }
}
