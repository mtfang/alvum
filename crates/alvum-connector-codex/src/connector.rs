//! Codex CLI connector — thin wrapper over `alvum-connector-session`.
//!
//! All schema-shared logic lives in `alvum-connector-session`. This module
//! supplies the per-tool `CodexSchema` and re-exports
//! `SessionConnector<CodexSchema>` as `CodexConnector`.

use alvum_connector_session::SessionConnector;
use anyhow::Result;
use std::collections::HashMap;

use crate::parser::CodexSchema;

/// Public type alias preserving the historical name. Callers continue to
/// import `CodexConnector` from this crate.
pub type CodexConnector = SessionConnector<CodexSchema>;

/// Build a Codex connector from raw config settings. Reads `session_dir`
/// (default `~/.codex`) and the optional `since` RFC3339 lower bound.
pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<CodexConnector> {
    SessionConnector::from_config(CodexSchema, settings)
}
