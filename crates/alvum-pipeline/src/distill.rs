use alvum_core::decision::Decision;
use alvum_core::observation::Observation;
use anyhow::{Context, Result};
use tracing::info;

use crate::llm::LlmProvider;
use crate::util::{strip_markdown_fences, truncate_chars};

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are analyzing a conversation to extract decisions.

A decision is any choice proposed, made, deferred, or rejected — by ANY actor. Track both who PROPOSED it and what HAPPENED to it:

PROPOSED_BY — who originated this decision:
- "self" kind for the user
- "agent" kind for an AI assistant (name it)
- "person" kind for a named individual
- "organization" kind for a company/team
- "environment" kind for external circumstances

STATUS — what happened to the proposal:
- "acted_on": someone actually did the thing
- "accepted": agreed to but not yet done
- "rejected": explicitly turned down
- "pending": still under consideration
- "ignored": proposed but got no response

RESOLVED_BY — who acted on / accepted / rejected it (null if pending or ignored)

CONFIDENCE — 0.0 to 1.0 on each attribution. Use lower confidence when:
- The proposal emerged organically from discussion (hard to attribute)
- Silent acceptance vs. explicit agreement (hard to tell if truly accepted)
- The proposer is unclear (assistant refining vs. originating an idea)

ATTRIBUTION RULES — apply these strictly:

1. DIRECTIVE QUESTIONS: If the user asks "should we do X?", "what about Y?",
   "can we use Z?", or similar — the USER proposed it, even if Claude wrote
   the detailed elaboration. The question IS the proposal. Claude agreeing or
   designing the implementation is resolution, not proposal.
   Example: User asks "domains should be editable?" → proposed_by: user.
   Claude responds "Not complex at all, here's how..." → resolved_by: user (Claude confirmed feasibility but user proposed the requirement).

2. COLLABORATIVE DECISIONS: When both actors contributed meaningfully — user
   defined the requirement, Claude designed the solution — use confidence
   0.5-0.7 on proposed_by, never above 0.8. If you cannot clearly identify
   a single originator, default to 0.5-0.6 confidence.

3. PROPOSAL vs IMPLEMENTATION: Proposing a decision means originating the IDEA
   that something should be done or changed. Designing HOW to implement it is
   not proposing. If the user says "we need to clean up the data" and Claude
   designs a 5-stage funnel — the user proposed "clean the data", Claude
   proposed the specific "5-stage funnel" implementation. These may be separate
   decisions or one decision with collaborative attribution depending on granularity.

4. SILENT ACCEPTANCE: When Claude proposes something and the user responds with
   just "ok", "yeah", or moves to the next topic without objecting — that is
   acceptance but at LOW confidence (0.4-0.6). Explicit agreement ("yes let's
   do that", "good idea") is higher confidence (0.7-0.9).

For each decision, extract:
- id: sequential identifier (dec_001, dec_002, ...)
- timestamp: ISO 8601
- summary: one-sentence description
- reasoning: why (if stated)
- alternatives: options considered
- domain: Architecture, Product, Technology, Business, etc.
- proposed_by: {"actor": {"name": "...", "kind": "..."}, "confidence": 0.0-1.0}
- status: acted_on | accepted | rejected | pending | ignored
- resolved_by: {"actor": {"name": "...", "kind": "..."}, "confidence": 0.0-1.0} or null
- source: copy from the observation's source field (e.g., "audio-mic", "claude-code", "git")
- tags: 3-6 keywords
- causes: ALWAYS [] — causal analysis is separate
- expected_outcome: if applicable, else null

Output ONLY a JSON array. No markdown, no explanation.

Example:
[
  {
    "id": "dec_001",
    "timestamp": "2026-04-02T04:35:00Z",
    "summary": "Process data overnight rather than real-time",
    "reasoning": "Overnight batch gives full-day context, reduces cost",
    "alternatives": ["Real-time streaming", "Hybrid approach"],
    "domain": "Architecture",
    "source": "audio-mic",
    "proposed_by": {"actor": {"name": "user", "kind": "self"}, "confidence": 0.95},
    "status": "acted_on",
    "resolved_by": {"actor": {"name": "user", "kind": "self"}, "confidence": 0.95},
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
    "source": "audio-mic",
    "proposed_by": {"actor": {"name": "claude", "kind": "agent"}, "confidence": 0.95},
    "status": "rejected",
    "resolved_by": {"actor": {"name": "user", "kind": "self"}, "confidence": 0.95},
    "causes": [],
    "tags": ["simplification", "scope-reduction"],
    "expected_outcome": null
  },
  {
    "id": "dec_003",
    "timestamp": "2026-04-02T05:15:00Z",
    "summary": "Dedicated hardware box as product north star",
    "reasoning": "Extrapolated from local LLM requirement",
    "alternatives": ["Software-only product"],
    "domain": "Product",
    "source": "audio-mic",
    "proposed_by": {"actor": {"name": "claude", "kind": "agent"}, "confidence": 0.8},
    "status": "pending",
    "resolved_by": null,
    "causes": [],
    "tags": ["hardware", "product-direction"],
    "expected_outcome": null
  }
]"#;

fn format_conversation(observations: &[Observation]) -> String {
    let mut parts = Vec::new();
    for obs in observations {
        let speaker = obs.speaker().unwrap_or("system").to_string();
        let ts = obs.ts.format("%Y-%m-%d %H:%M");
        let content = if obs.content.len() > 2000 {
            format!("{}...[truncated]", truncate_chars(&obs.content, 2000))
        } else {
            obs.content.clone()
        };

        let mut line = format!("[{ts}] [{source}/{kind}] {speaker}: {content}",
            source = obs.source, kind = obs.kind);

        // Include actor hints so the extraction LLM can attribute decisions
        if let Some(hints) = obs.metadata.as_ref()
            .and_then(|m| m.get("actor_hints"))
            .and_then(|h| h.as_array())
        {
            if !hints.is_empty() {
                let hint_strs: Vec<String> = hints.iter()
                    .filter_map(|h| {
                        let actor = h.get("actor")?.as_str()?;
                        let kind = h.get("kind")?.as_str()?;
                        let conf = h.get("confidence")?.as_f64()?;
                        Some(format!("{actor}({kind},{conf:.1})"))
                    })
                    .collect();
                if !hint_strs.is_empty() {
                    line.push_str(&format!("  [actors: {}]", hint_strs.join(", ")));
                }
            }
        }

        parts.push(line);
    }
    parts.join("\n\n")
}

/// Extract decisions from observations by sending the conversation to an LLM.
/// Returns a vec of decisions with empty `causes` (causal linking is a separate step).
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

    let json_str = strip_markdown_fences(&response);

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

    #[test]
    fn format_conversation_produces_readable_transcript() {
        let obs = vec![
            Observation::dialogue(
                "2026-04-02T04:31:55Z".parse().unwrap(),
                "claude-code",
                "user",
                "Should we use real-time or batch?",
            ),
            Observation::dialogue(
                "2026-04-02T04:33:57Z".parse().unwrap(),
                "claude-code",
                "assistant",
                "Batch processing is better because...",
            ),
        ];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[2026-04-02 04:31] [claude-code/dialogue] user:"));
        assert!(formatted.contains("[2026-04-02 04:33] [claude-code/dialogue] assistant:"));
        assert!(formatted.contains("Should we use"));
    }

    #[test]
    fn format_conversation_truncates_long_messages() {
        let long_content = "x".repeat(5000);
        let obs = vec![Observation::dialogue(
            "2026-04-02T04:33:57Z".parse().unwrap(),
            "claude-code",
            "assistant",
            &long_content,
        )];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[truncated]"));
        assert!(formatted.len() < 5000);
    }

    #[test]
    fn format_conversation_includes_actor_hints() {
        let obs = vec![Observation {
            ts: "2026-04-12T09:00:15Z".parse().unwrap(),
            source: "screen".into(),
            kind: "screen_capture".into(),
            content: "VS Code showing main.rs".into(),
            metadata: Some(serde_json::json!({
                "app": "VS Code",
                "actor_hints": [
                    {"actor": "self", "kind": "self", "confidence": 0.4, "signal": "screen"},
                    {"actor": "claude", "kind": "agent", "confidence": 0.7, "signal": "terminal"}
                ]
            })),
            media_ref: None,
        }];
        let formatted = format_conversation(&obs);
        assert!(formatted.contains("[actors: self(self,0.4), claude(agent,0.7)]"));
    }

    #[test]
    fn format_conversation_skips_empty_actor_hints() {
        let obs = vec![Observation {
            ts: "2026-04-12T09:00:15Z".parse().unwrap(),
            source: "audio-mic".into(),
            kind: "speech".into(),
            content: "Hello world".into(),
            metadata: Some(serde_json::json!({"actor_hints": []})),
            media_ref: None,
        }];
        let formatted = format_conversation(&obs);
        assert!(!formatted.contains("[actors:"));
    }
}
