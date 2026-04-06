use alvum_core::observation::{Observation, ObservationKind};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::Path;

/// Parse a Claude Code JSONL session file into chronologically-ordered observations.
/// Extracts user messages and assistant text blocks, filtering out system messages,
/// metadata, thinking blocks, and system-injected content.
///
/// If `before` is provided, only includes observations before that timestamp.
pub fn parse_session(path: &Path) -> Result<Vec<Observation>> {
    parse_session_filtered(path, None)
}

/// Parse with an optional timestamp cutoff.
pub fn parse_session_filtered(path: &Path, before: Option<DateTime<Utc>>) -> Result<Vec<Observation>> {
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

        // Apply timestamp cutoff if specified
        if let Some(cutoff) = before {
            if let Ok(ts) = timestamp.parse::<DateTime<Utc>>() {
                if ts >= cutoff {
                    continue;
                }
            }
        }

        match msg_type {
            "user" if !is_meta => {
                if let Some(content) = extract_user_content(&obj) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('<')
                        && let Ok(ts) = timestamp.parse() {
                        observations.push(Observation {
                            ts,
                            source: "claude-code".into(),
                            kind: ObservationKind::Dialogue {
                                speaker: "user".into(),
                            },
                            content: trimmed.to_string(),
                        });
                    }
                }
            }
            "assistant" => {
                if let Some(content) = extract_assistant_content(&obj) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty()
                        && let Ok(ts) = timestamp.parse() {
                        observations.push(Observation {
                            ts,
                            source: "claude-code".into(),
                            kind: ObservationKind::Dialogue {
                                speaker: "assistant".into(),
                            },
                            content: trimmed.to_string(),
                        });
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
        assert!(matches!(&obs[0].kind, ObservationKind::Dialogue { speaker } if speaker == "user"));
    }

    #[test]
    fn parse_assistant_text_block() {
        let fixture = make_fixture(&[
            r#"{"type":"assistant","timestamp":"2026-04-02T04:33:57.406Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"This is a fascinating problem."}]}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "This is a fascinating problem.");
        assert!(matches!(&obs[0].kind, ObservationKind::Dialogue { speaker } if speaker == "assistant"));
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
}
