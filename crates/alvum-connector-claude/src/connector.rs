//! Claude Code connector — thin wrapper over `alvum-connector-session`.
//!
//! All schema-shared logic (file walking, timestamp windowing, the Connector
//! and Processor traits) lives in `alvum-connector-session`. This module just
//! supplies the per-tool `ClaudeSchema` and re-exports the resulting
//! `SessionConnector<ClaudeSchema>` as `ClaudeCodeConnector`.

use alvum_connector_session::SessionConnector;
use anyhow::Result;
use std::collections::HashMap;

use crate::parser::ClaudeSchema;

/// Public type alias preserving the historical name. Callers continue to
/// import `ClaudeCodeConnector` from this crate.
pub type ClaudeCodeConnector = SessionConnector<ClaudeSchema>;

/// Build a Claude Code connector from raw config settings.
///
/// Reads `session_dir` (default `~/.claude/projects`) and the optional
/// `since` RFC3339 lower bound. Mirrors the prior hand-rolled
/// `ClaudeCodeConnector::from_config` API.
pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<ClaudeCodeConnector> {
    SessionConnector::from_config(ClaudeSchema, settings)
}
