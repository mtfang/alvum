//! Load and save the knowledge corpus as JSONL files.

use anyhow::Result;
use std::path::Path;
use tracing::info;

use crate::types::{Entity, Fact, KnowledgeCorpus, Pattern};

/// Load the knowledge corpus from a directory.
/// Returns an empty corpus if the directory doesn't exist.
pub fn load(knowledge_dir: &Path) -> Result<KnowledgeCorpus> {
    let entities: Vec<Entity> = load_jsonl(&knowledge_dir.join("entities.jsonl"))?;
    let patterns: Vec<Pattern> = load_jsonl(&knowledge_dir.join("patterns.jsonl"))?;
    let facts: Vec<Fact> = load_jsonl(&knowledge_dir.join("facts.jsonl"))?;

    info!(entities = entities.len(), patterns = patterns.len(), facts = facts.len(), "loaded knowledge corpus");
    Ok(KnowledgeCorpus { entities, patterns, facts })
}

/// Save the knowledge corpus to a directory.
pub fn save(knowledge_dir: &Path, corpus: &KnowledgeCorpus) -> Result<()> {
    std::fs::create_dir_all(knowledge_dir)?;

    save_jsonl(&knowledge_dir.join("entities.jsonl"), &corpus.entities)?;
    save_jsonl(&knowledge_dir.join("patterns.jsonl"), &corpus.patterns)?;
    save_jsonl(&knowledge_dir.join("facts.jsonl"), &corpus.facts)?;

    info!(entities = corpus.entities.len(), patterns = corpus.patterns.len(), facts = corpus.facts.len(), "saved knowledge corpus");
    Ok(())
}

fn load_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    alvum_core::storage::read_jsonl(path)
}

fn save_jsonl<T: serde::Serialize>(path: &Path, items: &[T]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = String::new();
    for item in items {
        content.push_str(&serde_json::to_string(item)?);
        content.push('\n');
    }
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use tempfile::TempDir;

    #[test]
    fn save_and_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let corpus = KnowledgeCorpus {
            entities: vec![Entity {
                id: "sarah".into(),
                name: "Sarah".into(),
                entity_type: "person".into(),
                description: "Manager".into(),
                relationships: vec![],
                first_seen: chrono::NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                last_seen: chrono::NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                attributes: None,
            }],
            patterns: vec![],
            facts: vec![Fact {
                id: "gym".into(),
                content: "Goes to gym 3x/week".into(),
                category: "routine".into(),
                learned: chrono::NaiveDate::from_ymd_opt(2026, 4, 5).unwrap(),
                last_confirmed: chrono::NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                source: "audio-mic".into(),
            }],
        };

        save(tmp.path(), &corpus).unwrap();
        let loaded = load(tmp.path()).unwrap();
        assert_eq!(loaded.entities.len(), 1);
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.entities[0].name, "Sarah");
    }

    #[test]
    fn load_empty_directory_returns_empty_corpus() {
        let tmp = TempDir::new().unwrap();
        let corpus = load(tmp.path()).unwrap();
        assert!(corpus.entities.is_empty());
        assert!(corpus.patterns.is_empty());
        assert!(corpus.facts.is_empty());
    }
}
