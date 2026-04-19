use alvum_core::observation::Observation;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::Path;

/// Parse a Claude Code JSONL session file into chronologically-ordered observations.
/// Extracts user messages and assistant text blocks, filtering out system messages,
/// metadata, thinking blocks, and system-injected content.
pub fn parse_session(path: &Path) -> Result<Vec<Observation>> {
    parse_session_filtered(path, None, None)
}

/// Parse with an optional `[after, before)` timestamp window.
/// - `after`: exclude records with timestamp < after (lower bound, inclusive of later).
/// - `before`: exclude records with timestamp >= before (upper bound, exclusive).
pub fn parse_session_filtered(
    path: &Path,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Result<Vec<Observation>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read session file: {}", path.display()))?;

    let mut observations = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let obj: serde_json::Value = serde_json::from_str(line)
            .with_context(|| "failed to parse JSONL line")?;

        let msg_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let is_meta = obj.get("isMeta").and_then(|m| m.as_bool()).unwrap_or(false);
        let timestamp = obj
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        // Apply timestamp window if specified. A record missing a parseable
        // timestamp is skipped when any bound is active (we can't tell where
        // it falls in the window).
        if after.is_some() || before.is_some() {
            let ts = match timestamp.parse::<DateTime<Utc>>() {
                Ok(ts) => ts,
                Err(_) => continue,
            };
            if let Some(lower) = after
                && ts < lower
            {
                continue;
            }
            if let Some(upper) = before
                && ts >= upper
            {
                continue;
            }
        }

        match msg_type {
            "user" if !is_meta => {
                if let Some(content) = extract_user_content(&obj) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('<')
                        && let Ok(ts) = timestamp.parse() {
                        observations.push(Observation::dialogue(ts, "claude-code", "user", trimmed));
                    }
                }
            }
            "assistant" => {
                if let Some(content) = extract_assistant_content(&obj) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty()
                        && let Ok(ts) = timestamp.parse() {
                        observations.push(Observation::dialogue(ts, "claude-code", "assistant", trimmed));
                    }
                }
            }
            _ => {}
        }
    }

    tracing::info!(
        path = %path.display(),
        observations = observations.len(),
        "parsed Claude Code session"
    );

    Ok(observations)
}

fn extract_user_content(obj: &serde_json::Value) -> Option<String> {
    obj.get("message")?
        .get("content")?
        .as_str()
        .map(|s| s.to_string())
}

fn extract_assistant_content(obj: &serde_json::Value) -> Option<String> {
    let content = obj.get("message")?.get("content")?;

    if let Some(arr) = content.as_array() {
        let mut text_parts = Vec::new();
        for block in arr {
            if let Some(block_type) = block.get("type").and_then(|t| t.as_str())
                && block_type == "text"
                && let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                text_parts.push(text.to_string());
            }
        }
        if text_parts.is_empty() {
            return None;
        }
        return Some(text_parts.join("\n\n"));
    }

    content.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_fixture(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f
    }

    #[test]
    fn parse_user_message() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55.446Z","message":{"role":"user","content":"imagine we have endless context"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "imagine we have endless context");
        assert_eq!(obs[0].kind, "dialogue");
        assert_eq!(obs[0].speaker(), Some("user"));
    }

    #[test]
    fn parse_assistant_text_block() {
        let fixture = make_fixture(&[
            r#"{"type":"assistant","timestamp":"2026-04-02T04:33:57.406Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"This is a fascinating problem."}]}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "This is a fascinating problem.");
        assert_eq!(obs[0].kind, "dialogue");
        assert_eq!(obs[0].speaker(), Some("assistant"));
    }

    #[test]
    fn skip_meta_messages() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":true,"timestamp":"2026-04-02T04:29:19.735Z","message":{"role":"user","content":"meta stuff"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55.446Z","message":{"role":"user","content":"real message"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "real message");
    }

    #[test]
    fn skip_system_injected_content() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:30:00Z","message":{"role":"user","content":"<system-reminder>ignore this</system-reminder>"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55Z","message":{"role":"user","content":"real question here"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "real question here");
    }

    #[test]
    fn skip_non_message_types() {
        let fixture = make_fixture(&[
            r#"{"type":"permission-mode","permissionMode":"bypassPermissions"}"#,
            r#"{"type":"file-history-snapshot","messageId":"abc"}"#,
            r#"{"type":"system","content":"bridge status"}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55Z","message":{"role":"user","content":"the real one"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
    }

    #[test]
    fn preserves_chronological_order() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55Z","message":{"role":"user","content":"first"}}"#,
            r#"{"type":"assistant","timestamp":"2026-04-02T04:33:57Z","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:35:00Z","message":{"role":"user","content":"third"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 3);
        assert_eq!(obs[0].content, "first");
        assert_eq!(obs[1].content, "second");
        assert_eq!(obs[2].content, "third");
    }

    #[test]
    fn after_filter_excludes_earlier_records() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T10:00:00Z","message":{"role":"user","content":"early"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T11:00:00Z","message":{"role":"user","content":"boundary"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T12:00:00Z","message":{"role":"user","content":"late"}}"#,
        ]);
        let after: DateTime<Utc> = "2026-04-02T11:30:00Z".parse().unwrap();
        let obs = parse_session_filtered(fixture.path(), Some(after), None).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "late");
    }

    #[test]
    fn after_and_before_define_window() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T09:00:00Z","message":{"role":"user","content":"before window"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T11:00:00Z","message":{"role":"user","content":"in window"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T13:00:00Z","message":{"role":"user","content":"after window"}}"#,
        ]);
        let after: DateTime<Utc>  = "2026-04-02T10:00:00Z".parse().unwrap();
        let before: DateTime<Utc> = "2026-04-02T12:00:00Z".parse().unwrap();
        let obs = parse_session_filtered(fixture.path(), Some(after), Some(before)).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "in window");
    }
}
