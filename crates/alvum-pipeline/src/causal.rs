use alvum_core::decision::{CausalLink, CausalStrength, Decision};
use alvum_core::llm::complete_observed;
use alvum_core::pipeline_events::{self as events, Event};
use anyhow::{Context, Result};
use tracing::info;

use crate::llm::LlmProvider;
use crate::util::{strip_markdown_fences, truncate_chars};

const CAUSAL_SYSTEM_PROMPT: &str = r#"You are analyzing a set of decisions to identify causal relationships and cross-domain effects.

For each decision, determine:
1. CAUSES — which prior decisions influenced this one? Name the mechanism:
   - "direct": explicit causal statement ("because of X, we decided Y")
   - "resource_competition": X consumed time/energy that Y needed
   - "emotional_influence": X created a feeling that shaped Y
   - "precedent": X set a pattern that Y followed
   - "constraint": X eliminated options, forcing Y
   - "accumulation": X contributed to a state that triggered Y

2. STRENGTH — how directly:
   - "primary": THE cause
   - "contributing": one of several factors
   - "background": distant/indirect influence

Output a JSON array where each item has:
- decision_id: the id of the decision being linked
- causes: array of {from_id, mechanism, strength}

Only include decisions that HAVE causes. Decisions with no identifiable cause can be omitted.

Example:
[
  {
    "decision_id": "dec_005",
    "causes": [
      {"from_id": "dec_003", "mechanism": "User pushed back, forcing reconsideration", "strength": "primary"},
      {"from_id": "dec_001", "mechanism": "Original architecture constrained options", "strength": "background"}
    ]
  }
]"#;

#[derive(serde::Deserialize)]
struct CausalOutput {
    decision_id: String,
    causes: Vec<CausalLinkRaw>,
}

#[derive(serde::Deserialize)]
struct CausalLinkRaw {
    from_id: String,
    mechanism: String,
    strength: String,
}

/// Analyze causal relationships between decisions and update them in place.
/// Each decision's `causes` field is populated with links to prior decisions.
pub async fn link_decisions(
    client: &dyn LlmProvider,
    decisions: &mut [Decision],
) -> Result<()> {
    let decisions_json = serde_json::to_string_pretty(decisions)
        .context("failed to serialize decisions")?;

    info!(decisions = decisions.len(), "analyzing causal links");

    let user_message = format!(
        "<decisions>\n{decisions_json}\n</decisions>\n\nAnalyze the decisions above and output ONLY the JSON array of causal links."
    );

    let response = complete_observed(client, CAUSAL_SYSTEM_PROMPT, &user_message, "causal")
        .await
        .context("LLM causal linking call failed")?;

    let json_str = strip_markdown_fences(&response);

    let links: Vec<CausalOutput> = serde_json::from_str(json_str).with_context(|| {
        format!(
            "failed to parse causal links. Response:\n{}",
            truncate_chars(&response, 500)
        )
    })?;

    // Index decisions by id → timestamp for the forward-reference
    // guard. Decision.timestamp is RFC 3339 / ISO 8601, which sorts
    // chronologically as a string, so no parsing required.
    let ts_by_id: std::collections::HashMap<String, String> = decisions
        .iter()
        .map(|d| (d.id.clone(), d.timestamp.clone()))
        .collect();

    let mut link_count = 0;
    let mut forward_ref_dropped = 0usize;
    for causal in &links {
        let Some(dec_ts) = ts_by_id.get(&causal.decision_id) else {
            // The LLM cited a decision_id we didn't pass in. Skip
            // silently — this is a hallucination at the wrong layer
            // for the forward-reference guard to police.
            continue;
        };
        if let Some(dec) = decisions.iter_mut().find(|d| d.id == causal.decision_id) {
            for link in &causal.causes {
                // Forward-reference guard: a cause that's chronologically
                // LATER than the decision it supposedly caused is
                // physically impossible. Drop the edge with a tagged
                // event so the operator can see how often the LLM
                // invents these.
                if let Some(cause_ts) = ts_by_id.get(&link.from_id)
                    && cause_ts.as_str() > dec_ts.as_str()
                {
                    forward_ref_dropped += 1;
                    continue;
                }

                let strength = match link.strength.to_lowercase().as_str() {
                    "primary" => CausalStrength::Primary,
                    "contributing" => CausalStrength::Contributing,
                    _ => CausalStrength::Background,
                };
                dec.causes.push(CausalLink {
                    from_id: link.from_id.clone(),
                    mechanism: link.mechanism.clone(),
                    strength,
                });
                link_count += 1;
            }
        }
    }

    if forward_ref_dropped > 0 {
        events::emit(Event::InputFiltered {
            processor: "causal".into(),
            file: None,
            kept: link_count,
            dropped: forward_ref_dropped,
            reasons: serde_json::json!({"forward_reference": forward_ref_dropped}),
        });
    }

    info!(
        links = link_count,
        forward_ref_dropped,
        "applied causal links"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_causal_strength() {
        let strength = match "primary".to_lowercase().as_str() {
            "primary" => CausalStrength::Primary,
            "contributing" => CausalStrength::Contributing,
            _ => CausalStrength::Background,
        };
        assert_eq!(strength, CausalStrength::Primary);
    }

    #[test]
    fn unknown_strength_defaults_to_background() {
        let strength = match "something_else".to_lowercase().as_str() {
            "primary" => CausalStrength::Primary,
            "contributing" => CausalStrength::Contributing,
            _ => CausalStrength::Background,
        };
        assert_eq!(strength, CausalStrength::Background);
    }

    /// Sanity-check the forward-reference comparison: RFC 3339
    /// timestamps must sort chronologically as plain strings. The
    /// guard relies on this rather than parsing — a regression here
    /// would silently let backwards causal links through.
    #[test]
    fn rfc3339_timestamps_sort_chronologically_as_strings() {
        let earlier = "2026-04-14T09:30:00Z";
        let later = "2026-04-22T19:15:00Z";
        assert!(later > earlier);
        // Nanosecond precision still works.
        assert!("2026-04-22T19:15:00.500Z" > "2026-04-22T19:15:00.499Z");
        // Mixed timezones don't sort correctly as strings — note this
        // limitation. RFC 3339 timestamps from our pipeline are all
        // emitted in UTC (`Z` suffix) so this is a known constraint
        // not a bug.
        // Cross-timezone string comparison would NOT be chronological.
    }
}
