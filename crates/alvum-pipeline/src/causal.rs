use alvum_core::decision::{CausalLink, CausalStrength, Decision};
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

pub async fn link_decisions(
    client: &dyn LlmProvider,
    decisions: &mut Vec<Decision>,
) -> Result<()> {
    let decisions_json = serde_json::to_string_pretty(decisions)
        .context("failed to serialize decisions")?;

    info!(decisions = decisions.len(), "analyzing causal links");

    let response = client
        .complete(CAUSAL_SYSTEM_PROMPT, &decisions_json)
        .await
        .context("LLM causal linking call failed")?;

    let json_str = response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let links: Vec<CausalOutput> = serde_json::from_str(json_str).with_context(|| {
        format!(
            "failed to parse causal links. Response:\n{}",
            truncate_chars(&response, 500)
        )
    })?;

    let mut link_count = 0;
    for causal in &links {
        if let Some(dec) = decisions.iter_mut().find(|d| d.id == causal.decision_id) {
            for link in &causal.causes {
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

    info!(links = link_count, "applied causal links");
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
}
