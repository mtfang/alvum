# MVP Decision Extractor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract a causal decision graph from Claude Code conversation logs, producing `decisions.jsonl` and a morning-style briefing — validating the core alvum concept against our own design sessions.

**Architecture:** Rust Cargo workspace with 4 crates: `alvum-core` (types + storage), `alvum-connector-claude` (parse Claude Code JSONL → Observations), `alvum-pipeline` (LLM-driven extraction + causal linking + briefing), and `alvum-cli` (command-line entry point). The pipeline sends the conversation to Claude API for decision extraction, causal analysis, and briefing generation.

**Tech Stack:** Rust, `reqwest` (HTTP client for Claude API), `serde` + `serde_json` (serialization), `tokio` (async runtime), `clap` (CLI args).

---

## File Structure

```
alvum/
├── Cargo.toml                              workspace root
├── crates/
│   ├── alvum-core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      re-exports
│   │       ├── observation.rs              Observation (universal intermediate format)
│   │       ├── decision.rs                 Decision, CausalLink types
│   │       ├── config.rs                   AlvumConfig, data paths
│   │       └── storage.rs                  JSONL read/write, ensure_dir
│   ├── alvum-connector-claude/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      re-exports + Connector trait
│   │       └── parser.rs                   Claude Code JSONL → Vec<Observation>
│   ├── alvum-pipeline/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs                      re-exports
│   │       ├── llm.rs                      Claude API client (reqwest)
│   │       ├── distill.rs                  extract decisions from observations
│   │       ├── causal.rs                   link decisions, detect patterns
│   │       └── briefing.rs                 generate morning briefing
│   └── alvum-cli/
│       ├── Cargo.toml
│       └── src/
│           └── main.rs                     CLI entry point
```

---

### Task 1: Workspace + Core Types

**Files:**
- Create: `Cargo.toml`
- Create: `crates/alvum-core/Cargo.toml`
- Create: `crates/alvum-core/src/lib.rs`
- Create: `crates/alvum-core/src/observation.rs`
- Create: `crates/alvum-core/src/decision.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
# Cargo.toml
[workspace]
resolver = "2"
members = [
    "crates/alvum-core",
    "crates/alvum-connector-claude",
    "crates/alvum-pipeline",
    "crates/alvum-cli",
]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
reqwest = { version = "0.12", features = ["json"] }
clap = { version = "4", features = ["derive"] }
```

- [ ] **Step 2: Create alvum-core Cargo.toml**

```toml
# crates/alvum-core/Cargo.toml
[package]
name = "alvum-core"
version = "0.1.0"
edition = "2024"

[dependencies]
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Write failing tests for Observation**

```rust
// crates/alvum-core/src/observation.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    Dialogue { speaker: String },
    Action { detail: String },
    Visual { description: String },
    Note { context: String },
}

/// Universal intermediate format. Every connector produces these.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Observation {
    pub ts: DateTime<Utc>,
    pub source: String,
    pub kind: ObservationKind,
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_dialogue_observation() {
        let obs = Observation {
            ts: "2026-04-02T04:31:55Z".parse().unwrap(),
            source: "claude-code".into(),
            kind: ObservationKind::Dialogue {
                speaker: "user".into(),
            },
            content: "imagine we have endless context about a person's life".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["source"], "claude-code");
        assert_eq!(parsed["kind"]["dialogue"]["speaker"], "user");
    }

    #[test]
    fn roundtrip_observation() {
        let obs = Observation {
            ts: "2026-04-02T04:31:55Z".parse().unwrap(),
            source: "claude-code".into(),
            kind: ObservationKind::Dialogue {
                speaker: "assistant".into(),
            },
            content: "This is a fascinating problem.".into(),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let deserialized: Observation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, obs);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alvum-core observation`
Expected: 2 tests PASS (we wrote types and tests together since the types are simple)

- [ ] **Step 5: Write failing tests for Decision**

```rust
// crates/alvum-core/src/decision.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub id: String,
    pub timestamp: String,
    pub summary: String,
    pub reasoning: Option<String>,
    pub alternatives: Vec<String>,
    pub domain: String,
    pub source: String,
    pub causes: Vec<CausalLink>,
    pub tags: Vec<String>,
    pub open: bool,
    pub expected_outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalLink {
    pub from_id: String,
    pub mechanism: String,
    pub strength: CausalStrength,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CausalStrength {
    Primary,
    Contributing,
    Background,
}

/// The complete output of one pipeline run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionResult {
    pub session_id: String,
    pub extracted_at: String,
    pub decisions: Vec<Decision>,
    pub briefing: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_decision_to_jsonl() {
        let dec = Decision {
            id: "dec_001".into(),
            timestamp: "2026-04-02T04:35:00Z".into(),
            summary: "Process data overnight, not real-time".into(),
            reasoning: Some("Overnight batch gives full-day context, reduces cost".into()),
            alternatives: vec!["Real-time streaming".into(), "Hybrid approach".into()],
            domain: "Architecture".into(),
            source: "claude-code".into(),
            causes: vec![],
            tags: vec!["pipeline".into(), "cost".into()],
            open: false,
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], "dec_001");
        assert_eq!(parsed["alternatives"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn serialize_causal_link() {
        let link = CausalLink {
            from_id: "dec_003".into(),
            mechanism: "User pushback on oversimplification".into(),
            strength: CausalStrength::Primary,
        };
        let json = serde_json::to_string(&link).unwrap();
        assert!(json.contains("primary"));
    }

    #[test]
    fn roundtrip_decision() {
        let dec = Decision {
            id: "dec_002".into(),
            timestamp: "2026-04-03T17:54:00Z".into(),
            summary: "Restore camera for physical-world alignment".into(),
            reasoning: Some("Camera captures physical actions vs intentions".into()),
            alternatives: vec![],
            domain: "Product".into(),
            source: "claude-code".into(),
            causes: vec![CausalLink {
                from_id: "dec_001".into(),
                mechanism: "direct".into(),
                strength: CausalStrength::Primary,
            }],
            tags: vec!["capture".into(), "camera".into()],
            open: false,
            expected_outcome: None,
        };
        let json = serde_json::to_string(&dec).unwrap();
        let deserialized: Decision = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, dec);
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p alvum-core decision`
Expected: 3 tests PASS

- [ ] **Step 7: Write lib.rs, commit**

```rust
// crates/alvum-core/src/lib.rs
pub mod observation;
pub mod decision;
pub mod config;
pub mod storage;
```

Create placeholder files so it compiles:

```rust
// crates/alvum-core/src/config.rs
// Implemented in Task 2
```

```rust
// crates/alvum-core/src/storage.rs
// Implemented in Task 2
```

```bash
git add Cargo.toml crates/alvum-core/
git commit -m "feat(core): add workspace, Observation, and Decision types"
```

---

### Task 2: Config + Storage

**Files:**
- Modify: `crates/alvum-core/src/config.rs`
- Modify: `crates/alvum-core/src/storage.rs`

- [ ] **Step 1: Write failing tests for config and storage**

```rust
// crates/alvum-core/src/config.rs

use std::path::PathBuf;

pub struct AlvumConfig {
    pub data_dir: PathBuf,
    pub anthropic_api_key: String,
    pub model: String,
}

impl AlvumConfig {
    pub fn new(data_dir: PathBuf) -> Self {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .unwrap_or_default();
        Self {
            data_dir,
            anthropic_api_key: api_key,
            model: "claude-sonnet-4-6".into(),
        }
    }

    pub fn decisions_path(&self) -> PathBuf {
        self.data_dir.join("decisions.jsonl")
    }

    pub fn briefing_path(&self) -> PathBuf {
        self.data_dir.join("briefing.md")
    }

    pub fn extraction_path(&self) -> PathBuf {
        self.data_dir.join("extraction.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn paths_relative_to_data_dir() {
        let tmp = TempDir::new().unwrap();
        let config = AlvumConfig::new(tmp.path().to_path_buf());
        assert!(config.decisions_path().starts_with(tmp.path()));
        assert!(config.decisions_path().ends_with("decisions.jsonl"));
        assert!(config.briefing_path().ends_with("briefing.md"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alvum-core config`
Expected: PASS

- [ ] **Step 3: Write failing tests for storage**

```rust
// crates/alvum-core/src/storage.rs

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
    use crate::decision::Decision;
    use tempfile::TempDir;

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
            causes: vec![],
            tags: vec![],
            open: false,
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
            causes: vec![],
            tags: vec![],
            open: false,
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
```

- [ ] **Step 4: Run tests, commit**

Run: `cargo test -p alvum-core`
Expected: all tests PASS

```bash
git add crates/alvum-core/
git commit -m "feat(core): add config and JSONL storage utilities"
```

---

### Task 3: Claude Code JSONL Parser

**Files:**
- Create: `crates/alvum-connector-claude/Cargo.toml`
- Create: `crates/alvum-connector-claude/src/lib.rs`
- Create: `crates/alvum-connector-claude/src/parser.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/alvum-connector-claude/Cargo.toml
[package]
name = "alvum-connector-claude"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true
tracing.workspace = true
```

- [ ] **Step 2: Write failing test with a fixture**

```rust
// crates/alvum-connector-claude/src/parser.rs

use alvum_core::observation::{Observation, ObservationKind};
use anyhow::{Context, Result};
use std::path::Path;

/// Parse a Claude Code JSONL session file into Observations.
pub fn parse_session(path: &Path) -> Result<Vec<Observation>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read session file: {}", path.display()))?;

    let mut observations = Vec::new();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let obj: serde_json::Value = serde_json::from_str(line)
            .with_context(|| "failed to parse JSONL line")?;

        let msg_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let is_meta = obj.get("isMeta").and_then(|m| m.as_bool()).unwrap_or(false);
        let timestamp = obj
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("");

        match msg_type {
            "user" if !is_meta => {
                if let Some(content) = extract_user_content(&obj) {
                    // Skip system-injected messages (start with <)
                    let trimmed = content.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('<') {
                        if let Ok(ts) = timestamp.parse() {
                            observations.push(Observation {
                                ts,
                                source: "claude-code".into(),
                                kind: ObservationKind::Dialogue {
                                    speaker: "user".into(),
                                },
                                content: trimmed.to_string(),
                            });
                        }
                    }
                }
            }
            "assistant" => {
                if let Some(content) = extract_assistant_content(&obj) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if let Ok(ts) = timestamp.parse() {
                            observations.push(Observation {
                                ts,
                                source: "claude-code".into(),
                                kind: ObservationKind::Dialogue {
                                    speaker: "assistant".into(),
                                },
                                content: trimmed.to_string(),
                            });
                        }
                    }
                }
            }
            _ => {} // skip system, file-history-snapshot, permission-mode, etc.
        }
    }

    tracing::info!(
        path = %path.display(),
        observations = observations.len(),
        "parsed Claude Code session"
    );

    Ok(observations)
}

fn extract_user_content(obj: &serde_json::Value) -> Option<String> {
    obj.get("message")?
        .get("content")?
        .as_str()
        .map(|s| s.to_string())
}

fn extract_assistant_content(obj: &serde_json::Value) -> Option<String> {
    let content = obj.get("message")?.get("content")?;

    // Assistant content is an array of blocks
    if let Some(arr) = content.as_array() {
        let mut text_parts = Vec::new();
        for block in arr {
            if let Some(block_type) = block.get("type").and_then(|t| t.as_str()) {
                // Only extract "text" blocks, skip "thinking" blocks
                if block_type == "text" {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(text.to_string());
                    }
                }
            }
        }
        if text_parts.is_empty() {
            return None;
        }
        return Some(text_parts.join("\n\n"));
    }

    // Fallback: content is a string
    content.as_str().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_fixture(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f
    }

    #[test]
    fn parse_user_message() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55.446Z","message":{"role":"user","content":"imagine we have endless context"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "imagine we have endless context");
        assert!(matches!(&obs[0].kind, ObservationKind::Dialogue { speaker } if speaker == "user"));
    }

    #[test]
    fn parse_assistant_text_block() {
        let fixture = make_fixture(&[
            r#"{"type":"assistant","timestamp":"2026-04-02T04:33:57.406Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"This is a fascinating problem."}]}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "This is a fascinating problem.");
        assert!(matches!(&obs[0].kind, ObservationKind::Dialogue { speaker } if speaker == "assistant"));
    }

    #[test]
    fn skip_meta_messages() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":true,"timestamp":"2026-04-02T04:29:19.735Z","message":{"role":"user","content":"meta stuff"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55.446Z","message":{"role":"user","content":"real message"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "real message");
    }

    #[test]
    fn skip_system_injected_content() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:30:00Z","message":{"role":"user","content":"<system-reminder>ignore this</system-reminder>"}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55Z","message":{"role":"user","content":"real question here"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
        assert_eq!(obs[0].content, "real question here");
    }

    #[test]
    fn skip_non_message_types() {
        let fixture = make_fixture(&[
            r#"{"type":"permission-mode","permissionMode":"bypassPermissions"}"#,
            r#"{"type":"file-history-snapshot","messageId":"abc"}"#,
            r#"{"type":"system","content":"bridge status"}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55Z","message":{"role":"user","content":"the real one"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 1);
    }

    #[test]
    fn preserves_chronological_order() {
        let fixture = make_fixture(&[
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:31:55Z","message":{"role":"user","content":"first"}}"#,
            r#"{"type":"assistant","timestamp":"2026-04-02T04:33:57Z","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#,
            r#"{"type":"user","isMeta":false,"timestamp":"2026-04-02T04:35:00Z","message":{"role":"user","content":"third"}}"#,
        ]);
        let obs = parse_session(fixture.path()).unwrap();
        assert_eq!(obs.len(), 3);
        assert_eq!(obs[0].content, "first");
        assert_eq!(obs[1].content, "second");
        assert_eq!(obs[2].content, "third");
    }
}
```

- [ ] **Step 3: Write lib.rs**

```rust
// crates/alvum-connector-claude/src/lib.rs
pub mod parser;
```

- [ ] **Step 4: Run tests, commit**

Run: `cargo test -p alvum-connector-claude`
Expected: 6 tests PASS

```bash
git add crates/alvum-connector-claude/
git commit -m "feat(connector): add Claude Code JSONL parser"
```

---

### Task 4: LLM Client (Claude API)

**Files:**
- Create: `crates/alvum-pipeline/Cargo.toml`
- Create: `crates/alvum-pipeline/src/lib.rs`
- Create: `crates/alvum-pipeline/src/llm.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/alvum-pipeline/Cargo.toml
[package]
name = "alvum-pipeline"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true
reqwest.workspace = true
```

- [ ] **Step 2: Implement the LLM client**

```rust
// crates/alvum-pipeline/src/llm.rs

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

pub struct LlmClient {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessage>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

impl LlmClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            http: reqwest::Client::new(),
        }
    }

    /// Send a system prompt + user message, return the text response.
    pub async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        let request = ApiRequest {
            model: self.model.clone(),
            max_tokens: 16000,
            system: system.to_string(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: user_message.to_string(),
            }],
        };

        debug!(model = %self.model, system_len = system.len(), user_len = user_message.len(), "sending LLM request");

        let response = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send request to Claude API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Claude API error {status}: {body}");
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .context("failed to parse Claude API response")?;

        let text = api_response
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        debug!(response_len = text.len(), "received LLM response");
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_request_serializes_correctly() {
        let req = ApiRequest {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 8000,
            system: "You are helpful.".into(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: "Hello".into(),
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["messages"][0]["role"], "user");
    }
}
```

- [ ] **Step 3: Write lib.rs, run tests, commit**

```rust
// crates/alvum-pipeline/src/lib.rs
pub mod llm;
pub mod distill;
pub mod causal;
pub mod briefing;
```

Create placeholder files:

```rust
// crates/alvum-pipeline/src/distill.rs
// Implemented in Task 5

// crates/alvum-pipeline/src/causal.rs
// Implemented in Task 6

// crates/alvum-pipeline/src/briefing.rs
// Implemented in Task 7
```

Run: `cargo test -p alvum-pipeline llm`
Expected: 1 test PASS

```bash
git add crates/alvum-pipeline/
git commit -m "feat(pipeline): add Claude API LLM client"
```

---

### Task 5: Decision Extractor

**Files:**
- Modify: `crates/alvum-pipeline/src/distill.rs`

- [ ] **Step 1: Write the decision extraction module**

```rust
// crates/alvum-pipeline/src/distill.rs

use alvum_core::decision::Decision;
use alvum_core::observation::Observation;
use anyhow::{Context, Result};
use tracing::info;

use crate::llm::LlmClient;

const EXTRACTION_SYSTEM_PROMPT: &str = r#"You are analyzing a conversation to extract decisions.

A decision is a choice that was made, deferred, or agreed upon. For each decision, extract:
- id: sequential identifier (dec_001, dec_002, ...)
- timestamp: when the decision was made (ISO 8601 from the conversation)
- summary: one-sentence description of what was decided
- reasoning: why this choice was made (if stated)
- alternatives: what other options were considered
- domain: the life/work domain this falls under (e.g., Architecture, Product, Technology, Business)
- tags: relevant keywords
- open: true if the outcome is still pending, false if resolved
- expected_outcome: what the decision is expected to produce (if applicable)

Output ONLY a JSON array of decisions. No markdown, no explanation, just the JSON array.

Example output format:
[
  {
    "id": "dec_001",
    "timestamp": "2026-04-02T04:35:00Z",
    "summary": "Process data overnight rather than real-time",
    "reasoning": "Overnight batch gives full-day context, reduces cost, improves extraction quality",
    "alternatives": ["Real-time streaming", "Hybrid approach"],
    "domain": "Architecture",
    "tags": ["pipeline", "batch-processing"],
    "open": false,
    "expected_outcome": null
  }
]"#;

/// Format observations into a conversation transcript for the LLM.
fn format_conversation(observations: &[Observation]) -> String {
    let mut parts = Vec::new();
    for obs in observations {
        let speaker = match &obs.kind {
            alvum_core::observation::ObservationKind::Dialogue { speaker } => speaker.clone(),
            _ => "system".into(),
        };
        let ts = obs.ts.format("%Y-%m-%d %H:%M");
        // Truncate very long assistant messages to focus on key content
        let content = if obs.content.len() > 2000 {
            format!("{}...[truncated]", &obs.content[..2000])
        } else {
            obs.content.clone()
        };
        parts.push(format!("[{ts}] {speaker}: {content}"));
    }
    parts.join("\n\n")
}

/// Extract decisions from a set of observations using the LLM.
pub async fn extract_decisions(
    client: &LlmClient,
    observations: &[Observation],
) -> Result<Vec<Decision>> {
    let conversation = format_conversation(observations);
    info!(
        observations = observations.len(),
        conversation_chars = conversation.len(),
        "extracting decisions"
    );

    let response = client
        .complete(EXTRACTION_SYSTEM_PROMPT, &conversation)
        .await
        .context("LLM extraction call failed")?;

    // Parse the JSON array from the response
    // The response might have markdown code fences, strip them
    let json_str = response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let decisions: Vec<Decision> = serde_json::from_str(json_str).with_context(|| {
        format!(
            "failed to parse LLM response as Decision array. Response:\n{}",
            &response[..response.len().min(500)]
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
```

- [ ] **Step 2: Run tests, commit**

Run: `cargo test -p alvum-pipeline distill`
Expected: 2 tests PASS

```bash
git add crates/alvum-pipeline/src/distill.rs
git commit -m "feat(pipeline): add decision extraction with LLM"
```

---

### Task 6: Causal Linker

**Files:**
- Modify: `crates/alvum-pipeline/src/causal.rs`

- [ ] **Step 1: Implement the causal linking module**

```rust
// crates/alvum-pipeline/src/causal.rs

use alvum_core::decision::{CausalLink, CausalStrength, Decision};
use anyhow::{Context, Result};
use tracing::info;

use crate::llm::LlmClient;

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
   - "primary": THE cause — without this, the decision wouldn't have happened
   - "contributing": one of several factors
   - "background": distant/indirect influence

3. CROSS-DOMAIN — does this decision create effects in other domains?

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

/// Analyze decisions for causal relationships and update them in place.
pub async fn link_decisions(
    client: &LlmClient,
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
            &response[..response.len().min(500)]
        )
    })?;

    // Apply causal links to decisions
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
        let raw = CausalLinkRaw {
            from_id: "dec_001".into(),
            mechanism: "direct cause".into(),
            strength: "primary".into(),
        };
        let strength = match raw.strength.to_lowercase().as_str() {
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
```

- [ ] **Step 2: Run tests, commit**

Run: `cargo test -p alvum-pipeline causal`
Expected: 2 tests PASS

```bash
git add crates/alvum-pipeline/src/causal.rs
git commit -m "feat(pipeline): add causal linker with mechanism detection"
```

---

### Task 7: Briefing Generator

**Files:**
- Modify: `crates/alvum-pipeline/src/briefing.rs`

- [ ] **Step 1: Implement the briefing generator**

```rust
// crates/alvum-pipeline/src/briefing.rs

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
   These are the most important patterns to surface.

4. OPEN THREADS — Decisions that are still open or have pending outcomes.
   What should the person be thinking about?

5. PATTERNS — Recurring themes in the decision-making:
   - Are there repeated deferrals?
   - Are there domains getting disproportionate attention?
   - Are there cross-domain effects (a decision in one area affecting another)?

6. QUESTIONS — End with 2-3 questions the person should consider.
   These should be specific, grounded in the decisions, and provocative.

Write in second person ("you decided...", "you might want to consider...").
Use markdown formatting. Be concise but specific — cite decision IDs.
"#;

/// Generate a morning-style briefing from a set of linked decisions.
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
```

- [ ] **Step 2: Run tests, commit**

Run: `cargo test -p alvum-pipeline briefing`
Expected: 1 test PASS

```bash
git add crates/alvum-pipeline/src/briefing.rs
git commit -m "feat(pipeline): add briefing generator"
```

---

### Task 8: CLI Assembly

**Files:**
- Create: `crates/alvum-cli/Cargo.toml`
- Create: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/alvum-cli/Cargo.toml
[package]
name = "alvum-cli"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "alvum"
path = "src/main.rs"

[dependencies]
alvum-core = { path = "../alvum-core" }
alvum-connector-claude = { path = "../alvum-connector-claude" }
alvum-pipeline = { path = "../alvum-pipeline" }
serde_json.workspace = true
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
clap.workspace = true
chrono.workspace = true
```

- [ ] **Step 2: Implement the CLI**

```rust
// crates/alvum-cli/src/main.rs

use anyhow::{bail, Context, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "alvum", about = "Extract decisions from conversation logs")]
struct Cli {
    /// Path to a Claude Code JSONL session file
    #[arg(long)]
    session: PathBuf,

    /// Output directory for decisions.jsonl and briefing.md
    #[arg(long, default_value = ".")]
    output: PathBuf,

    /// Claude API model to use
    #[arg(long, default_value = "claude-sonnet-4-6")]
    model: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("ANTHROPIC_API_KEY environment variable not set")?;

    if !cli.session.exists() {
        bail!("session file not found: {}", cli.session.display());
    }

    std::fs::create_dir_all(&cli.output)?;
    let decisions_path = cli.output.join("decisions.jsonl");
    let briefing_path = cli.output.join("briefing.md");
    let extraction_path = cli.output.join("extraction.json");

    // Step 1: Parse Claude Code logs → Observations
    info!("parsing session: {}", cli.session.display());
    let observations = alvum_connector_claude::parser::parse_session(&cli.session)?;
    info!(observations = observations.len(), "parsed observations");

    if observations.is_empty() {
        bail!("no observations found in session file");
    }

    // Step 2: Extract decisions from observations
    let client = alvum_pipeline::llm::LlmClient::new(api_key, cli.model);

    info!("extracting decisions...");
    let mut decisions = alvum_pipeline::distill::extract_decisions(&client, &observations).await?;
    info!(decisions = decisions.len(), "extracted");

    // Step 3: Analyze causal links
    info!("analyzing causal links...");
    alvum_pipeline::causal::link_decisions(&client, &mut decisions).await?;

    let link_count: usize = decisions.iter().map(|d| d.causes.len()).sum();
    info!(links = link_count, "linked");

    // Step 4: Generate briefing
    info!("generating briefing...");
    let briefing = alvum_pipeline::briefing::generate_briefing(&client, &decisions).await?;

    // Step 5: Write outputs
    // Write decisions as JSONL
    for dec in &decisions {
        alvum_core::storage::append_jsonl(&decisions_path, dec)?;
    }
    info!(path = %decisions_path.display(), "wrote decisions");

    // Write briefing as markdown
    std::fs::write(&briefing_path, &briefing)?;
    info!(path = %briefing_path.display(), "wrote briefing");

    // Write full extraction as JSON (for debugging)
    let result = alvum_core::decision::ExtractionResult {
        session_id: cli
            .session
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        decisions: decisions.clone(),
        briefing: briefing.clone(),
    };
    std::fs::write(&extraction_path, serde_json::to_string_pretty(&result)?)?;

    println!("\n✓ Extracted {} decisions with {} causal links", decisions.len(), link_count);
    println!("  decisions: {}", decisions_path.display());
    println!("  briefing:  {}", briefing_path.display());

    // Print briefing to stdout
    println!("\n{}", "=".repeat(60));
    println!("{briefing}");

    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p alvum-cli`
Expected: compiles successfully

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-cli/
git commit -m "feat(cli): add alvum CLI to extract decisions from Claude Code logs"
```

- [ ] **Step 5: Run against our own conversation**

```bash
ANTHROPIC_API_KEY=<your-key> cargo run -p alvum-cli -- \
    --session ~/.claude/projects/-Users-michael-git-alvum/d38be5b9-82b7-4f06-a6f2-12c7bb727c38.jsonl \
    --output ./output
```

Expected output:
- `./output/decisions.jsonl` — every decision from our design sessions, with causal links
- `./output/briefing.md` — a morning-style briefing summarizing the decision landscape
- `./output/extraction.json` — full extraction result for debugging

- [ ] **Step 6: Review output, iterate on prompts if needed, commit results**

```bash
git add output/
git commit -m "feat: first decision extraction from our own design conversation"
```

---

## Implementation Notes

### API Key

Set `ANTHROPIC_API_KEY` environment variable before running. The CLI reads it from the env.

### Token Limits

The main conversation (439 lines, 1.7MB) contains long assistant messages with code examples. The `format_conversation` function in `distill.rs` truncates messages to 2000 chars each. If the full conversation still exceeds context limits, increase the truncation or split into chunks. With Sonnet at 200K context, it should fit.

### Prompt Iteration

The extraction and causal linking prompts are the intellectual core. After the first run, review the output and tune:
- Are decisions being extracted at the right granularity? (too fine = noise, too coarse = missing signal)
- Are causal links plausible? (the LLM might hallucinate connections)
- Is the briefing actionable? (specific citations vs. vague summaries)

Expect 2-3 rounds of prompt tuning to get good output.

### Cost

Three Claude API calls per run:
- Extraction: ~100K input tokens (conversation) + ~4K output (decisions JSON)
- Causal linking: ~8K input (decisions) + ~4K output (links JSON)
- Briefing: ~8K input (linked decisions) + ~2K output (markdown)

Estimated cost: ~$1-2 per run with Sonnet. Use `--model claude-haiku-4-5-20251001` for cheaper iteration while tuning prompts.

### Future Connectors

The `Observation` type is source-agnostic. To add a new connector (Slack, email, screen events), implement a parser that produces `Vec<Observation>` and feed it to the same pipeline. The extraction, causal linking, and briefing stages don't change.
