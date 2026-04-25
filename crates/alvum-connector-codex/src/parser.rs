//! Per-line parser for Codex CLI session rollout JSONL.
//!
//! Schema (reverse-engineered from live sessions, 2026-04):
//! Each line is a JSON object with top-level `timestamp` and `type`. The types
//! we consume are `response_item` entries whose `payload.type == "message"` and
//! whose `payload.role` is `user` or `assistant`. `developer`-role messages
//! (system / tool-use prompts) are skipped.
//!
//! Content is an array of blocks; text lives at `.type in {input_text, output_text, text}`
//! with `.text` as the string. Reasoning blocks and tool-call blocks are skipped.

use alvum_connector_session::SessionSchema;
use alvum_core::observation::Observation;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// Schema marker for Codex CLI sessions. Used to instantiate the generic
/// [`alvum_connector_session::SessionConnector`].
#[derive(Clone, Default)]
pub struct CodexSchema;

impl SessionSchema for CodexSchema {
    fn source_name(&self) -> &'static str {
        "codex"
    }

    fn default_session_dir(&self) -> PathBuf {
        dirs::home_dir().map(|h| h.join(".codex")).unwrap_or_else(|| PathBuf::from("."))
    }

    fn matches_session_file(&self, name: &str) -> bool {
        // Only consume rollout-*.jsonl — skip session_index.jsonl, history.jsonl,
        // etc. which have different schemas.
        name.starts_with("rollout-") && name.ends_with(".jsonl")
    }

    fn parse_line(
        &self,
        line: &str,
        after: Option<DateTime<Utc>>,
        before: Option<DateTime<Utc>>,
    ) -> Option<Observation> {
        parse_codex_line(line, after, before)
    }
}

/// Parse a single Codex rollout JSONL line. Returns `None` for non-message
/// records, developer/system role messages, system-injected content, or
/// records outside the `[after, before)` window.
pub fn parse_codex_line(
    line: &str,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Option<Observation> {
    let obj: serde_json::Value = serde_json::from_str(line).ok()?;

    if obj.get("type").and_then(|t| t.as_str()) != Some("response_item") {
        return None;
    }

    let ts_str = obj.get("timestamp")?.as_str()?;
    let ts: DateTime<Utc> = ts_str.parse().ok()?;
    if let Some(lower) = after
        && ts < lower
    {
        return None;
    }
    if let Some(upper) = before
        && ts >= upper
    {
        return None;
    }

    let payload = obj.get("payload")?;
    if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
        return None;
    }

    let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
    if role != "user" && role != "assistant" {
        return None;
    }

    let text = extract_text(payload)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if role == "user" && trimmed.starts_with('<') {
        return None;
    }

    Some(Observation::dialogue(ts, "codex", role, trimmed))
}

/// Whole-file convenience wrapper. Kept for tests that want to parse a fixture
/// in one call without going through the connector.
pub fn parse_session_filtered(
    path: &Path,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Result<Vec<Observation>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read codex session: {}", path.display()))?;
    let mut observations = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(obs) = parse_codex_line(line, after, before) {
            observations.push(obs);
        }
    }
    tracing::info!(
        path = %path.display(),
        observations = observations.len(),
        "parsed Codex session"
    );
    Ok(observations)
}

/// Whole-file parser without timestamp filtering. Convenience wrapper.
pub fn parse_session(path: &Path) -> Result<Vec<Observation>> {
    parse_session_filtered(path, None, None)
}

/// Concatenate all text blocks in a message payload's `content` array.
/// Skips blocks that aren't text (e.g., reasoning, tool_use).
fn extract_text(payload: &serde_json::Value) -> Option<String> {
    let content = payload.get("content")?;

    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for block in arr {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "input_text" | "output_text" | "text" => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        parts.push(text.to_string());
                    }
                }
                _ => {}
            }
        }
        if parts.is_empty() {
            return None;
        }
        return Some(parts.join("\n\n"));
    }

    content.as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn fixture(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f
    }

    #[test]
    fn parse_user_message() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:05:52.003Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hey codex, refactor this"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "hey codex, refactor this");
        assert_eq!(obs[0].source, "codex");
        assert_eq!(obs[0].speaker(), Some("user"));
    }

    #[test]
    fn parse_assistant_multi_block() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:06:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"reasoning","text":"thinking..."},{"type":"output_text","text":"Here's the plan."},{"type":"output_text","text":"Step 1: blah"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "Here's the plan.\n\nStep 1: blah");
        assert_eq!(obs[0].speaker(), Some("assistant"));
    }

    #[test]
    fn skip_developer_role() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<permissions instructions>..."}]}}"#,
            r#"{"timestamp":"2026-04-18T18:06:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real question"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "real question");
    }

    #[test]
    fn skip_system_injected_user_content() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<app-context>...</app-context>"}]}}"#,
            r#"{"timestamp":"2026-04-18T18:06:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"actual question here"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "actual question here");
    }

    #[test]
    fn skip_non_response_item_types() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"session_meta","id":"abc"}"#,
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"turn_context","model":"gpt-5"}"#,
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"event_msg","event":"whatever"}"#,
            r#"{"timestamp":"2026-04-18T18:06:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 1);
    }

    #[test]
    fn skip_non_message_payloads() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"response_item","payload":{"type":"function_call","name":"read_file"}}"#,
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"response_item","payload":{"type":"reasoning","content":[{"type":"text","text":"think think"}]}}"#,
            r#"{"timestamp":"2026-04-18T18:06:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "answer");
    }

    #[test]
    fn preserves_chronological_order() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T18:05:52Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first"}]}}"#,
            r#"{"timestamp":"2026-04-18T18:06:00Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"second"}]}}"#,
            r#"{"timestamp":"2026-04-18T18:07:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"third"}]}}"#,
        ]);
        let obs = parse_session(f.path()).unwrap();
        assert_eq!(obs.len(), 3);
        assert_eq!(obs[0].content, "first");
        assert_eq!(obs[1].content, "second");
        assert_eq!(obs[2].content, "third");
    }

    #[test]
    fn after_filter_excludes_earlier_records() {
        let f = fixture(&[
            r#"{"timestamp":"2026-04-18T10:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"early"}]}}"#,
            r#"{"timestamp":"2026-04-18T12:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"late"}]}}"#,
        ]);
        let after: DateTime<Utc> = "2026-04-18T11:00:00Z".parse().unwrap();
        let obs = parse_session_filtered(f.path(), Some(after), None).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "late");
    }
}
