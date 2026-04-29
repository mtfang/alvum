//! Generic JSONL session connector.
//!
//! Backs `alvum-connector-claude` and `alvum-connector-codex` (and any future
//! per-line JSONL session source). Each schema impl supplies its own line
//! parser, filename filter, default session directory, and source-name string.
//!
//! ## Why a shared crate
//!
//! Both Claude Code and the Codex CLI write conversation history as JSONL
//! files in a per-tool root directory (`~/.claude/projects`, `~/.codex`). The
//! ingest path is structurally identical — walk the dir, filter filenames,
//! parse each line, apply a `[after, before)` timestamp window — only the
//! per-line schema differs. Collapsing the duplication keeps the differences
//! literal: one struct per schema (`ClaudeSchema`, `CodexSchema`) implements
//! `SessionSchema`, and the generic [`SessionConnector`] does the rest.

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

/// Schema-specific behaviour for a JSONL session source. Implementations are
/// expected to be cheap (zero-state structs); they get cloned into each
/// processor instance.
pub trait SessionSchema: Send + Sync + Clone + 'static {
    /// Source/connector name (e.g., "claude-code", "codex"). Stable string —
    /// this is the value that appears in `Observation::source` and
    /// `DataRef::source` and is referenced by config keys.
    fn source_name(&self) -> &'static str;

    /// Default session directory if none was configured.
    fn default_session_dir(&self) -> PathBuf;

    /// Whether a filename should be considered a session file. Called for
    /// every file under `session_dir` during `gather_data_refs`.
    fn matches_session_file(&self, name: &str) -> bool;

    /// Parse one JSONL line into an Observation, applying a `[after, before)`
    /// timestamp window. Returns `None` for lines that should be skipped
    /// (non-message records, system-injected content, out-of-window, etc).
    fn parse_line(
        &self,
        line: &str,
        after: Option<DateTime<Utc>>,
        before: Option<DateTime<Utc>>,
    ) -> Option<Observation>;
}

/// Generic JSONL session connector. Parameterized over a [`SessionSchema`]
/// that supplies the schema-specific behaviour.
pub struct SessionConnector<S: SessionSchema> {
    schema: S,
    session_dir: PathBuf,
    after_ts: Option<DateTime<Utc>>,
    before_ts: Option<DateTime<Utc>>,
}

impl<S: SessionSchema> SessionConnector<S> {
    /// Build from raw config settings. Reads `session_dir` (with `~` expansion)
    /// and `since` (RFC3339 lower bound). Invalid `since` values produce a
    /// warning and are dropped — the connector still runs without a lower
    /// bound rather than failing.
    pub fn from_config(schema: S, settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let session_dir = settings
            .get("session_dir")
            .and_then(|v| v.as_str())
            .map(|s| {
                if let Some(stripped) = s.strip_prefix("~/")
                    && let Some(home) = dirs::home_dir()
                {
                    return home.join(stripped);
                }
                PathBuf::from(s)
            })
            .unwrap_or_else(|| schema.default_session_dir());

        let after_ts = match settings.get("since").and_then(|v| v.as_str()) {
            Some(s) => match s.parse::<DateTime<Utc>>() {
                Ok(ts) => Some(ts),
                Err(e) => {
                    tracing::warn!(
                        connector = schema.source_name(),
                        value = s,
                        error = %e,
                        "ignoring invalid 'since' timestamp"
                    );
                    None
                }
            },
            None => None,
        };

        Ok(Self {
            schema,
            session_dir,
            after_ts,
            before_ts: None,
        })
    }

    /// Set the `before` upper bound (exclusive). Called by the briefing script
    /// to scope a run to a specific window.
    pub fn with_before(mut self, before: Option<DateTime<Utc>>) -> Self {
        self.before_ts = before;
        self
    }

    /// Set the `since` lower bound (inclusive). Same use case as `with_before`.
    pub fn with_since(mut self, since: Option<DateTime<Utc>>) -> Self {
        self.after_ts = since;
        self
    }
}

impl<S: SessionSchema> Connector for SessionConnector<S> {
    fn name(&self) -> &str {
        self.schema.source_name()
    }

    fn expected_sources(&self) -> Vec<&'static str> {
        // Each schema-typed session connector emits exactly one source
        // (e.g. `claude-code`, `codex`). Listing it here lets the
        // pipeline emit a silent-modality warning if the user's
        // session directory is missing or empty.
        vec![self.schema.source_name()]
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        // No daemon — sessions are read off disk on each run.
        vec![]
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(SessionProcessor {
            schema: self.schema.clone(),
            after_ts: self.after_ts,
            before_ts: self.before_ts,
        })]
    }

    fn gather_data_refs(&self, _capture_dir: &Path) -> Result<Vec<DataRef>> {
        let mut refs = Vec::new();
        if !self.session_dir.exists() {
            // Self-diagnose: emit a warning the popover/`alvum tail` can
            // surface so a missing session dir doesn't manifest as a
            // silent zero-modality run.
            alvum_core::pipeline_events::emit(alvum_core::pipeline_events::Event::Warning {
                source: format!("connector/{}", self.schema.source_name()),
                message: format!("session dir does not exist: {}", self.session_dir.display()),
            });
            return Ok(refs);
        }
        let mut files_seen = 0usize;
        for entry in walkdir::WalkDir::new(&self.session_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            files_seen += 1;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !self.schema.matches_session_file(name) {
                continue;
            }
            let mtime: std::time::SystemTime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            refs.push(DataRef {
                ts: mtime.into(),
                source: self.schema.source_name().into(),
                producer: format!("alvum.session/{}", self.schema.source_name()),
                schema: "alvum.session.jsonl.v1".into(),
                path: path.to_string_lossy().into_owned(),
                mime: "application/x-jsonl".into(),
                metadata: None,
            });
        }
        if refs.is_empty() {
            // Distinguish "dir exists but no files match the schema" from
            // "dir is just plain empty" — those have very different
            // remediations (schema mismatch vs. start using the tool).
            alvum_core::pipeline_events::emit(alvum_core::pipeline_events::Event::Warning {
                source: format!("connector/{}", self.schema.source_name()),
                message: format!(
                    "scanned {} ({} file(s)); none matched the session-file pattern",
                    self.session_dir.display(),
                    files_seen,
                ),
            });
        }
        Ok(refs)
    }
}

/// Generic processor matching a [`SessionConnector`]. Iterates the supplied
/// DataRefs (filtered by the runner against `handles()`), reads each session
/// file, and forwards every non-empty line to `schema.parse_line`.
struct SessionProcessor<S: SessionSchema> {
    schema: S,
    after_ts: Option<DateTime<Utc>>,
    before_ts: Option<DateTime<Utc>>,
}

#[async_trait]
impl<S: SessionSchema> Processor for SessionProcessor<S> {
    fn name(&self) -> &str {
        self.schema.source_name()
    }

    fn handles(&self) -> Vec<String> {
        vec![self.schema.source_name().into()]
    }

    async fn process(
        &self,
        data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        let want = self.schema.source_name();
        let mut observations = Vec::new();
        for dr in data_refs {
            if dr.source != want {
                continue;
            }
            let content = std::fs::read_to_string(&dr.path)
                .map_err(|e| anyhow::anyhow!("read {}: {}", dr.path, e))?;
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(obs) = self.schema.parse_line(line, self.after_ts, self.before_ts) {
                    observations.push(obs);
                }
            }
        }
        info!(
            obs = observations.len(),
            connector = %want,
            "loaded session observations"
        );
        Ok(observations)
    }
}
