//! CodexConnector — reads Codex CLI session files. No capture daemon.

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

pub struct CodexConnector {
    session_dir: PathBuf,
    before_ts: Option<chrono::DateTime<chrono::Utc>>,
    after_ts: Option<chrono::DateTime<chrono::Utc>>,
}

impl CodexConnector {
    pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> {
        // Default Codex session root is ~/.codex — sessions live under
        // sessions/YYYY/MM/DD/rollout-*.jsonl, and archived_sessions/*.jsonl.
        let session_dir = settings.get("session_dir")
            .and_then(|v| v.as_str())
            .map(|s| {
                if let Some(stripped) = s.strip_prefix("~/")
                    && let Some(home) = dirs::home_dir()
                {
                    return home.join(stripped);
                }
                PathBuf::from(s)
            })
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.join(".codex"))
                    .unwrap_or_else(|| PathBuf::from("."))
            });

        let after_ts = match settings.get("since").and_then(|v| v.as_str()) {
            Some(s) => match s.parse::<chrono::DateTime<chrono::Utc>>() {
                Ok(ts) => Some(ts),
                Err(e) => {
                    tracing::warn!(value = s, error = %e,
                        "codex 'since' is not a valid RFC3339 timestamp; ignoring");
                    None
                }
            },
            None => None,
        };

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

impl Connector for CodexConnector {
    fn name(&self) -> &str { "codex" }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        vec![] // No daemon — reads existing sessions.
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(CodexProcessor {
            before_ts: self.before_ts,
            after_ts: self.after_ts,
        })]
    }

    fn gather_data_refs(
        &self,
        _capture_dir: &Path,
    ) -> Result<Vec<DataRef>> {
        let mut refs = Vec::new();
        if !self.session_dir.exists() {
            return Ok(refs);
        }
        for entry in walkdir::WalkDir::new(&self.session_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Only consume rollout-*.jsonl — skip session_index.jsonl etc.
            if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                continue;
            }
            let mtime: std::time::SystemTime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            refs.push(DataRef {
                ts: mtime.into(),
                source: "codex".into(),
                path: path.to_string_lossy().into_owned(),
                mime: "application/x-jsonl".into(),
                metadata: None,
            });
        }
        Ok(refs)
    }
}

struct CodexProcessor {
    before_ts: Option<chrono::DateTime<chrono::Utc>>,
    after_ts: Option<chrono::DateTime<chrono::Utc>>,
}

#[async_trait]
impl Processor for CodexProcessor {
    fn name(&self) -> &str { "codex-parser" }

    fn handles(&self) -> Vec<String> {
        vec!["codex".into()]
    }

    async fn process(
        &self,
        data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        let mut observations = Vec::new();
        for dr in data_refs {
            if dr.source != "codex" { continue; }
            let session_obs = crate::parser::parse_session_filtered(
                Path::new(&dr.path),
                self.after_ts,
                self.before_ts,
            )?;
            observations.extend(session_obs);
        }
        info!(obs = observations.len(), "loaded codex observations");
        Ok(observations)
    }
}
