use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(value)?;
    writeln!(file, "{line}")?;
    Ok(())
}

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

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decision::{Actor, ActorKind, Decision};
    use tempfile::TempDir;

    fn self_actor() -> Actor {
        Actor { name: "user".into(), kind: ActorKind::Self_ }
    }

    #[test]
    fn append_and_read_jsonl_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.jsonl");

        let dec1 = Decision {
            id: "dec_001".into(),
            timestamp: "2026-04-02T04:35:00Z".into(),
            summary: "Overnight batch processing".into(),
            reasoning: None,
            alternatives: vec![],
            domain: "Architecture".into(),
            source: "claude-code".into(),
            actor: self_actor(),
            causes: vec![],
            tags: vec![],
            expected_outcome: None,
        };
        let dec2 = Decision {
            id: "dec_002".into(),
            timestamp: "2026-04-03T17:54:00Z".into(),
            summary: "Camera for physical alignment".into(),
            reasoning: None,
            alternatives: vec![],
            domain: "Product".into(),
            source: "claude-code".into(),
            actor: self_actor(),
            causes: vec![],
            tags: vec![],
            expected_outcome: None,
        };

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
