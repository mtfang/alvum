use alvum_core::decision::Decision;
use alvum_core::observation::Observation;
use anyhow::{Context, Result};
use tracing::info;

use crate::llm::LlmProvider;

/// Slices `s` to at most `max_chars` Unicode scalar values, avoiding mid-char byte splits.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are analyzing a conversation to extract decisions.

A decision is a choice that was made, deferred, or agreed upon — by ANY actor. Not every decision is made by the user. Identify WHO made each decision:
- The user ("self") — decisions the user made or explicitly accepted
- A named person — decisions made by someone else mentioned in the conversation
- An agent/AI — decisions suggested or made by an AI assistant, algorithm, or automated system
- An organization — decisions made by a company, team, or institution
- Environment — external circumstances that forced a particular outcome

This distinction matters because decisions made FOR you by others are often the most consequential and least examined.

For each decision, extract:
- id: sequential identifier (dec_001, dec_002, ...)
- timestamp: when the decision was made (ISO 8601 from the conversation)
- summary: one-sentence description of what was decided
- reasoning: why this choice was made (if stated)
- alternatives: what other options were considered
- domain: the life/work domain this falls under (e.g., Architecture, Product, Technology, Business)
- actor: {"name": "who made it", "kind": "self|person|agent|organization|environment"}
- tags: 3-6 relevant keywords
- causes: ALWAYS set to [] — causal analysis is done in a separate pass
- expected_outcome: what the decision is expected to produce (if applicable)

Output ONLY a JSON array of decisions. No markdown, no explanation, just the raw JSON array.

Example output format:
[
  {
    "id": "dec_001",
    "timestamp": "2026-04-02T04:35:00Z",
    "summary": "Process data overnight rather than real-time",
    "reasoning": "Overnight batch gives full-day context, reduces cost",
    "alternatives": ["Real-time streaming", "Hybrid approach"],
    "domain": "Architecture",
    "source": "claude-code",
    "actor": {"name": "user", "kind": "self"},
    "causes": [],
    "tags": ["pipeline", "batch-processing", "cost"],
    "expected_outcome": "Cheaper processing, better context"
  },
  {
    "id": "dec_002",
    "timestamp": "2026-04-02T05:10:00Z",
    "summary": "Proposed stripping all differentiating features for simplicity",
    "reasoning": "Applied 5-step simplification process aggressively",
    "alternatives": ["Keep differentiators with simpler implementation"],
    "domain": "Architecture",
    "source": "claude-code",
    "actor": {"name": "claude", "kind": "agent"},
    "causes": [],
    "tags": ["simplification", "scope-reduction"],
    "expected_outcome": null
  }
]"#;

fn format_conversation(observations: &[Observation]) -> String {
    let mut parts = Vec::new();
    for obs in observations {
        let speaker = match &obs.kind {
            alvum_core::observation::ObservationKind::Dialogue { speaker } => speaker.clone(),
            _ => "system".into(),
        };
        let ts = obs.ts.format("%Y-%m-%d %H:%M");
        let content = if obs.content.len() > 2000 {
            format!("{}...[truncated]", truncate_chars(&obs.content, 2000))
        } else {
            obs.content.clone()
        };
        parts.push(format!("[{ts}] {speaker}: {content}"));
    }
    parts.join("\n\n")
}

pub async fn extract_decisions(
    client: &dyn LlmProvider,
    observations: &[Observation],
) -> Result<Vec<Decision>> {
    let conversation = format_conversation(observations);
    info!(
        observations = observations.len(),
        conversation_chars = conversation.len(),
        "extracting decisions"
    );

    // Wrap in XML tags so the LLM treats this as data to analyze, not
    // as a continuation of any live conversation it might be running in.
    let user_message = format!(
        "<conversation>\n{conversation}\n</conversation>\n\nExtract all decisions from the conversation above. Output ONLY the JSON array."
    );

    let response = client
        .complete(EXTRACTION_SYSTEM_PROMPT, &user_message)
        .await
        .context("LLM extraction call failed")?;

    let json_str = response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let decisions: Vec<Decision> = serde_json::from_str(json_str).with_context(|| {
        format!(
            "failed to parse LLM response as Decision array. Response:\n{}",
            truncate_chars(&response, 500)
        )
    })?;

    info!(decisions = decisions.len(), "extracted decisions");
    Ok(decisions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::observation::ObservationKind;

    #[test]
    fn format_conversation_produces_readable_transcript() {
        let obs = vec![
            Observation {
                ts: "2026-04-02T04:31:55Z".parse().unwrap(),
                source: "claude-code".into(),
                kind: ObservationKind::Dialogue {
                    speaker: "user".into(),
                },
                content: "Should we use real-time or batch?".into(),
            },
            Observation {
                ts: "2026-04-02T04:33:57Z".parse().unwrap(),
                source: "claude-code".into(),
                kind: ObservationKind::Dialogue {
                    speaker: "assistant".into(),
                },
                content: "Batch processing is better because...".into(),
            },
        ];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[2026-04-02 04:31] user:"));
        assert!(formatted.contains("[2026-04-02 04:33] assistant:"));
        assert!(formatted.contains("Should we use"));
    }

    #[test]
    fn format_conversation_truncates_long_messages() {
        let long_content = "x".repeat(5000);
        let obs = vec![Observation {
            ts: "2026-04-02T04:33:57Z".parse().unwrap(),
            source: "claude-code".into(),
            kind: ObservationKind::Dialogue {
                speaker: "assistant".into(),
            },
            content: long_content,
        }];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[truncated]"));
        assert!(formatted.len() < 5000);
    }
}
