use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// Append a single JSON-serialized value as one line to a JSONL file.
/// Creates parent directories and the file if they don't exist.
pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(value)?;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Read all lines from a JSONL file, deserializing each into `T`.
/// Returns an empty vec if the file doesn't exist.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut items = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        items.push(serde_json::from_str(&line)?);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{
        Actor, ActorAttribution, ActorKind, Decision, DecisionSource, DecisionStatus,
    };
    use tempfile::TempDir;

    fn self_attr() -> ActorAttribution {
        ActorAttribution {
            actor: Actor {
                name: "user".into(),
                kind: ActorKind::Self_,
            },
            confidence: 0.9,
        }
    }

    fn fixture(id: &str, domain: &str) -> Decision {
        Decision {
            id: id.into(),
            date: "2026-04-22".into(),
            time: "10:30".into(),
            summary: "Roundtrip fixture".into(),
            domain: domain.into(),
            source: DecisionSource::Spoken,
            magnitude: 0.5,
            reasoning: None,
            alternatives: vec![],
            participants: vec![],
            proposed_by: self_attr(),
            status: DecisionStatus::ActedOn,
            resolved_by: Some(self_attr()),
            open: false,
            check_by: None,
            cross_domain: vec![],
            evidence: vec![],
            multi_source_evidence: false,
            confidence_overall: 0.5,
            anchor_observations: vec![],
            knowledge_refs: vec![],
            interest_refs: vec![],
            intention_refs: vec![],
            causes: vec![],
            effects: vec![],
        }
    }

    #[test]
    fn append_and_read_jsonl_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.jsonl");

        let dec1 = fixture("dec_001", "Career");
        let dec2 = fixture("dec_002", "Career");

        append_jsonl(&path, &dec1).unwrap();
        append_jsonl(&path, &dec2).unwrap();

        let loaded: Vec<Decision> = read_jsonl(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "dec_001");
        assert_eq!(loaded[1].id, "dec_002");
    }

    #[test]
    fn read_jsonl_returns_empty_for_missing_file() {
        let result: Vec<Decision> = read_jsonl(Path::new("/nonexistent/path.jsonl")).unwrap();
        assert!(result.is_empty());
    }
}
