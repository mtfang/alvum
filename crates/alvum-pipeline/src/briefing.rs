use alvum_core::decision::Decision;
use anyhow::{Context, Result};
use tracing::info;

use crate::llm::LlmClient;

const BRIEFING_SYSTEM_PROMPT: &str = r#"You are a thoughtful advisor analyzing a person's decision history.

Given a set of decisions with causal links, produce a morning-style briefing. The briefing should:

1. SUMMARY — How many decisions were made, across which domains, over what time period.

2. KEY DECISIONS — The 3-5 most significant decisions. For each:
   - What was decided and why
   - What alternatives were rejected
   - What caused this decision (trace the causal chain)

3. CAUSAL CHAINS — Identify decision chains where one decision led to another.
   Show the cascade: "A led to B, which constrained C, which forced D."

4. OPEN THREADS — Decisions that are still open or have pending outcomes.

5. PATTERNS — Recurring themes in the decision-making:
   - Are there repeated deferrals?
   - Are there domains getting disproportionate attention?
   - Are there cross-domain effects?

6. QUESTIONS — End with 2-3 questions the person should consider.

Write in second person ("you decided...", "you might want to consider...").
Use markdown formatting. Be concise but specific — cite decision IDs.
"#;

pub async fn generate_briefing(
    client: &LlmClient,
    decisions: &[Decision],
) -> Result<String> {
    let decisions_json = serde_json::to_string_pretty(decisions)
        .context("failed to serialize decisions for briefing")?;

    info!(decisions = decisions.len(), "generating briefing");

    let briefing = client
        .complete(BRIEFING_SYSTEM_PROMPT, &decisions_json)
        .await
        .context("LLM briefing generation failed")?;

    info!(briefing_len = briefing.len(), "generated briefing");
    Ok(briefing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn briefing_prompt_references_causal_chains() {
        assert!(BRIEFING_SYSTEM_PROMPT.contains("causal chain"));
        assert!(BRIEFING_SYSTEM_PROMPT.contains("cross-domain"));
        assert!(BRIEFING_SYSTEM_PROMPT.contains("decision IDs"));
    }
}
