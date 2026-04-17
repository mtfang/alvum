//! Extract entities, patterns, and facts from observations using an LLM.

use alvum_core::observation::Observation;
use alvum_core::llm::LlmProvider;
use alvum_core::util::strip_markdown_fences;
use anyhow::{Context, Result};
use tracing::info;

use crate::types::KnowledgeCorpus;

const KNOWLEDGE_EXTRACTION_PROMPT: &str = r#"You are extracting knowledge from a person's daily observations.
Given a set of observations and the person's existing knowledge corpus,
identify NEW or UPDATED:

1. ENTITIES — people, projects, places, organizations, tools mentioned.
   For each: id (snake_case), name, entity_type, description, relationships to other entities.

2. PATTERNS — recurring behavioral patterns you notice.
   For each: id, description, domains affected.

3. FACTS — persistent facts about the person's life (routines, preferences, constraints).
   For each: id, content, category (routine/preference/constraint/context).

RULES:
- Only extract entities/facts with evidence in the observations.
- Update existing corpus entries if you see new information.
- Don't repeat unchanged entries — only include new or updated ones.
- Use the existing corpus to avoid duplicates.
- Relationships should reference entity IDs, not names.
- For dates, use ISO format (YYYY-MM-DD). Use today's date for first_seen/last_seen/learned/last_confirmed.

Output ONLY a JSON object with three arrays:
{
  "entities": [...],
  "patterns": [...],
  "facts": [...]
}

No markdown, no explanation."#;

/// Extract new knowledge from observations, given the existing corpus for context.
pub async fn extract_knowledge(
    provider: &dyn LlmProvider,
    observations: &[Observation],
    existing_corpus: &KnowledgeCorpus,
) -> Result<KnowledgeCorpus> {
    if observations.is_empty() {
        return Ok(KnowledgeCorpus::default());
    }

    let mut user_message = String::new();

    // Include existing corpus for dedup context
    let corpus_summary = existing_corpus.format_for_llm();
    if !corpus_summary.is_empty() {
        user_message.push_str("EXISTING KNOWLEDGE CORPUS:\n");
        user_message.push_str(&corpus_summary);
        user_message.push_str("\n\n");
    }

    // Include observations
    user_message.push_str("TODAY'S OBSERVATIONS:\n");
    for obs in observations {
        let ts = obs.ts.format("%H:%M:%S");
        user_message.push_str(&format!("[{ts}] [{}/{}] {}\n", obs.source, obs.kind, obs.content));
    }

    info!(observations = observations.len(), "extracting knowledge");

    let response = provider
        .complete(KNOWLEDGE_EXTRACTION_PROMPT, &user_message)
        .await
        .context("LLM knowledge extraction failed")?;

    let json_str = strip_markdown_fences(&response);
    let new_knowledge: KnowledgeCorpus = serde_json::from_str(json_str).with_context(|| {
        format!("failed to parse knowledge extraction. First 500 chars:\n{}",
            &response[..response.len().min(500)])
    })?;

    info!(
        entities = new_knowledge.entities.len(),
        patterns = new_knowledge.patterns.len(),
        facts = new_knowledge.facts.len(),
        "extracted new knowledge"
    );

    Ok(new_knowledge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_prompt_contains_key_instructions() {
        assert!(KNOWLEDGE_EXTRACTION_PROMPT.contains("ENTITIES"));
        assert!(KNOWLEDGE_EXTRACTION_PROMPT.contains("PATTERNS"));
        assert!(KNOWLEDGE_EXTRACTION_PROMPT.contains("FACTS"));
        assert!(KNOWLEDGE_EXTRACTION_PROMPT.contains("existing corpus"));
    }
}
