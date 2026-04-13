//! Knowledge corpus types: entities, patterns, and facts.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

fn default_date() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

/// A known entity in the person's life.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Free-form: "person", "project", "place", "organization", "tool", etc.
    #[serde(default)]
    pub entity_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    #[serde(default = "default_date")]
    pub first_seen: NaiveDate,
    #[serde(default = "default_date")]
    pub last_seen: NaiveDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
}

/// A relationship between two entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relationship {
    #[serde(default)]
    pub target_id: String,
    /// Free-form: "manages", "reports_to", "blocks", "part_of", etc.
    #[serde(default)]
    pub relation: String,
    #[serde(default = "default_date")]
    pub last_confirmed: NaiveDate,
}

/// A recurring behavioral pattern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pattern {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub occurrences: u32,
    #[serde(default = "default_date")]
    pub first_seen: NaiveDate,
    #[serde(default = "default_date")]
    pub last_seen: NaiveDate,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// A persistent fact about the person's life.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub content: String,
    /// Free-form: "routine", "preference", "constraint", "context".
    #[serde(default)]
    pub category: String,
    #[serde(default = "default_date")]
    pub learned: NaiveDate,
    #[serde(default = "default_date")]
    pub last_confirmed: NaiveDate,
    #[serde(default)]
    pub source: String,
}

/// The full knowledge corpus.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnowledgeCorpus {
    pub entities: Vec<Entity>,
    pub patterns: Vec<Pattern>,
    pub facts: Vec<Fact>,
}

impl KnowledgeCorpus {
    /// Get entity names for injection into LLM prompts.
    pub fn entity_names(&self) -> Vec<&str> {
        self.entities.iter().map(|e| e.name.as_str()).collect()
    }

    /// Format a summary for LLM context injection.
    pub fn format_for_llm(&self) -> String {
        let mut parts = Vec::new();

        if !self.entities.is_empty() {
            parts.push("KNOWN ENTITIES:".to_string());
            for e in &self.entities {
                let rels: Vec<String> = e.relationships.iter()
                    .map(|r| format!("{} {}", r.relation, r.target_id))
                    .collect();
                let rel_str = if rels.is_empty() { String::new() } else { format!(" ({})", rels.join(", ")) };
                parts.push(format!("  {} [{}]: {}{}", e.name, e.entity_type, e.description, rel_str));
            }
        }

        if !self.patterns.is_empty() {
            parts.push("\nKNOWN PATTERNS:".to_string());
            for p in &self.patterns {
                parts.push(format!("  {} (seen {}x): {}", p.id, p.occurrences, p.description));
            }
        }

        if !self.facts.is_empty() {
            parts.push("\nKNOWN FACTS:".to_string());
            for f in &self.facts {
                parts.push(format!("  [{}] {}", f.category, f.content));
            }
        }

        parts.join("\n")
    }

    /// Merge new knowledge into the corpus, updating existing entries.
    pub fn merge(&mut self, new: KnowledgeCorpus) {
        for new_entity in new.entities {
            if let Some(existing) = self.entities.iter_mut().find(|e| e.id == new_entity.id) {
                existing.last_seen = new_entity.last_seen;
                existing.description = new_entity.description;
                for rel in new_entity.relationships {
                    if !existing.relationships.iter().any(|r| r.target_id == rel.target_id && r.relation == rel.relation) {
                        existing.relationships.push(rel);
                    }
                }
            } else {
                self.entities.push(new_entity);
            }
        }

        for new_pattern in new.patterns {
            if let Some(existing) = self.patterns.iter_mut().find(|p| p.id == new_pattern.id) {
                existing.occurrences = new_pattern.occurrences;
                existing.last_seen = new_pattern.last_seen;
            } else {
                self.patterns.push(new_pattern);
            }
        }

        for new_fact in new.facts {
            if let Some(existing) = self.facts.iter_mut().find(|f| f.id == new_fact.id) {
                existing.last_confirmed = new_fact.last_confirmed;
                existing.content = new_fact.content;
            } else {
                self.facts.push(new_fact);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_for_llm_includes_entities() {
        let corpus = KnowledgeCorpus {
            entities: vec![Entity {
                id: "sarah".into(),
                name: "Sarah".into(),
                entity_type: "person".into(),
                description: "Engineering manager".into(),
                relationships: vec![Relationship {
                    target_id: "user".into(),
                    relation: "manages".into(),
                    last_confirmed: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                }],
                first_seen: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                last_seen: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                attributes: None,
            }],
            patterns: vec![],
            facts: vec![],
        };
        let formatted = corpus.format_for_llm();
        assert!(formatted.contains("Sarah"));
        assert!(formatted.contains("manages"));
    }

    #[test]
    fn merge_updates_existing_entity() {
        let mut corpus = KnowledgeCorpus {
            entities: vec![Entity {
                id: "sarah".into(),
                name: "Sarah".into(),
                entity_type: "person".into(),
                description: "Engineering manager".into(),
                relationships: vec![],
                first_seen: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                last_seen: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                attributes: None,
            }],
            patterns: vec![],
            facts: vec![],
        };

        let new = KnowledgeCorpus {
            entities: vec![Entity {
                id: "sarah".into(),
                name: "Sarah".into(),
                entity_type: "person".into(),
                description: "Engineering manager, leading Q3 planning".into(),
                relationships: vec![],
                first_seen: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                last_seen: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                attributes: None,
            }],
            patterns: vec![],
            facts: vec![],
        };

        corpus.merge(new);
        assert_eq!(corpus.entities.len(), 1);
        assert!(corpus.entities[0].description.contains("Q3 planning"));
        assert_eq!(corpus.entities[0].last_seen, NaiveDate::from_ymd_opt(2026, 4, 11).unwrap());
    }

    #[test]
    fn merge_adds_new_entity() {
        let mut corpus = KnowledgeCorpus::default();
        let new = KnowledgeCorpus {
            entities: vec![Entity {
                id: "james".into(),
                name: "James".into(),
                entity_type: "person".into(),
                description: "Backend lead".into(),
                relationships: vec![],
                first_seen: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                last_seen: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                attributes: None,
            }],
            patterns: vec![],
            facts: vec![],
        };
        corpus.merge(new);
        assert_eq!(corpus.entities.len(), 1);
    }

    #[test]
    fn roundtrip_corpus() {
        let corpus = KnowledgeCorpus {
            entities: vec![Entity {
                id: "project_alvum".into(),
                name: "Alvum".into(),
                entity_type: "project".into(),
                description: "Alignment engine".into(),
                relationships: vec![],
                first_seen: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                last_seen: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                attributes: None,
            }],
            patterns: vec![Pattern {
                id: "defer_under_pressure".into(),
                description: "Defers infrastructure decisions under time pressure".into(),
                occurrences: 4,
                first_seen: NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
                last_seen: NaiveDate::from_ymd_opt(2026, 4, 3).unwrap(),
                domains: vec!["Architecture".into()],
                evidence: vec!["dec_002".into()],
            }],
            facts: vec![Fact {
                id: "standup_time".into(),
                content: "Daily standup at 9:30am".into(),
                category: "routine".into(),
                learned: NaiveDate::from_ymd_opt(2026, 4, 2).unwrap(),
                last_confirmed: NaiveDate::from_ymd_opt(2026, 4, 11).unwrap(),
                source: "audio-mic".into(),
            }],
        };
        let json = serde_json::to_string_pretty(&corpus).unwrap();
        let deserialized: KnowledgeCorpus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.entities.len(), 1);
        assert_eq!(deserialized.patterns.len(), 1);
        assert_eq!(deserialized.facts.len(), 1);
    }
}
