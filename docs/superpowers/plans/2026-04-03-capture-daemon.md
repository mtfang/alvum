# Capture Daemon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the always-on macOS capture daemon that records screen activity (event-driven screenshots + accessibility tree semantic events) and audio (mic + system audio with VAD segmentation).

**Architecture:** A Rust Cargo workspace with two crates: `alvum-core` (shared types, config, storage) and `alvum-capture` (macOS-specific capture daemon). The capture daemon is event-driven — it listens for OS events (app switch, visual change, idle timer) and atomically captures a screenshot + a11y tree diff on each trigger. Audio runs as continuous streams with Silero VAD segmentation and Opus encoding. All output is files on disk in a date-organized directory structure.

**Tech Stack:** Rust, `screencapturekit` (ScreenCaptureKit bindings), `accessibility-sys` (AXUIElement FFI), `cpal` (mic audio), `silero-vad-rust` + `ort` (voice activity detection), `opus` (audio encoding), `webp` (image encoding), `objc2-core-location` (GPS), `tokio` (async runtime), `serde` + `serde_json` (serialization).

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
│   │       ├── event.rs                    SemanticEvent types + serialization
│   │       ├── config.rs                   AlvumConfig, data paths
│   │       └── storage.rs                  JSONL append, screenshot save, dir mgmt
│   └── alvum-capture/
│       ├── Cargo.toml
│       ├── src/
│       │   ├── lib.rs                      re-exports
│       │   ├── accessibility/
│       │   │   ├── mod.rs
│       │   │   ├── node.rs                 A11yNode, A11yTree types
│       │   │   ├── walker.rs               macOS AXUIElement tree walking
│       │   │   └── differ.rs               generic semantic differ (5 patterns)
│       │   ├── screen.rs                   ScreenCaptureKit screenshot + WebP
│       │   ├── audio/
│       │   │   ├── mod.rs
│       │   │   ├── processor.rs            VAD + Opus encoding + file segmentation
│       │   │   ├── mic.rs                  cpal microphone source
│       │   │   └── system.rs               SCK system audio source
│       │   ├── triggers.rs                 event-driven capture orchestration
│       │   ├── location.rs                 CoreLocation wrapper
│       │   └── daemon.rs                   main daemon: lifecycle, daily rotation
│       └── tests/
│           └── capture_integration.rs      full daemon integration test (requires permissions)
```

---

### Task 1: Cargo Workspace + Core Types

**Files:**
- Create: `Cargo.toml`
- Create: `crates/alvum-core/Cargo.toml`
- Create: `crates/alvum-core/src/lib.rs`
- Create: `crates/alvum-core/src/event.rs`

- [ ] **Step 1: Create workspace Cargo.toml**

```toml
# Cargo.toml
[workspace]
resolver = "2"
members = ["crates/alvum-core", "crates/alvum-capture"]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
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
```

- [ ] **Step 3: Write failing test for SemanticEvent serialization**

```rust
// crates/alvum-core/src/event.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_app_focus_to_jsonl_format() {
        let event = SemanticEvent::AppFocus {
            ts: "2026-04-03T09:00:00Z".parse().unwrap(),
            app: "VS Code".into(),
            window: "api_spec.py".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["type"], "app_focus");
        assert_eq!(parsed["app"], "VS Code");
        assert_eq!(parsed["window"], "api_spec.py");
        assert!(parsed["ts"].is_string());
    }

    #[test]
    fn serialize_field_changed_to_jsonl_format() {
        let event = SemanticEvent::FieldChanged {
            ts: "2026-04-03T09:30:00Z".parse().unwrap(),
            app: "Linear".into(),
            field: "Status".into(),
            from: "In Progress".into(),
            to: "Backlog".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["type"], "field_changed");
        assert_eq!(parsed["field"], "Status");
        assert_eq!(parsed["from"], "In Progress");
        assert_eq!(parsed["to"], "Backlog");
    }

    #[test]
    fn deserialize_app_focus_from_jsonl() {
        let json = r#"{"type":"app_focus","ts":"2026-04-03T09:00:00Z","app":"VS Code","window":"main.rs"}"#;
        let event: SemanticEvent = serde_json::from_str(json).unwrap();
        match event {
            SemanticEvent::AppFocus { app, window, .. } => {
                assert_eq!(app, "VS Code");
                assert_eq!(window, "main.rs");
            }
            _ => panic!("expected AppFocus"),
        }
    }
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test -p alvum-core`
Expected: FAIL — `SemanticEvent` type not defined

- [ ] **Step 5: Implement SemanticEvent types**

```rust
// crates/alvum-core/src/event.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A semantic change event captured from the desktop.
/// Each variant serializes flat with a "type" tag for JSONL compatibility.
/// Every line in events.jsonl is one of these — self-contained, human-readable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SemanticEvent {
    AppFocus {
        ts: DateTime<Utc>,
        app: String,
        window: String,
    },
    WindowChange {
        ts: DateTime<Utc>,
        app: String,
        from: String,
        to: String,
    },
    FieldChanged {
        ts: DateTime<Utc>,
        app: String,
        field: String,
        from: String,
        to: String,
    },
    TextChanged {
        ts: DateTime<Utc>,
        app: String,
        detail: String,
    },
    Navigated {
        ts: DateTime<Utc>,
        app: String,
        from: String,
        to: String,
    },
    NodeAppeared {
        ts: DateTime<Utc>,
        app: String,
        detail: String,
    },
    NodeDisappeared {
        ts: DateTime<Utc>,
        app: String,
        detail: String,
    },
}
```

- [ ] **Step 6: Write lib.rs**

```rust
// crates/alvum-core/src/lib.rs

pub mod event;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test -p alvum-core`
Expected: 3 tests PASS

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/alvum-core/
git commit -m "feat(core): add workspace and SemanticEvent types"
```

---

### Task 2: Config + Storage Utilities

**Files:**
- Create: `crates/alvum-core/src/config.rs`
- Create: `crates/alvum-core/src/storage.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write failing test for data directory resolution**

```rust
// crates/alvum-core/src/config.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_uses_application_support() {
        let config = AlvumConfig::default();
        let path = config.data_dir();
        assert!(path.ends_with("com.alvum.app"));
    }

    #[test]
    fn capture_dir_for_date() {
        let config = AlvumConfig::default();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 4, 3).unwrap();
        let path = config.capture_dir(date);
        assert!(path.ends_with("capture/2026-04-03"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alvum-core config`
Expected: FAIL — `AlvumConfig` not defined

- [ ] **Step 3: Implement AlvumConfig**

```rust
// crates/alvum-core/src/config.rs

use chrono::NaiveDate;
use std::path::PathBuf;

pub struct AlvumConfig {
    base_dir: PathBuf,
}

impl AlvumConfig {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.base_dir
    }

    pub fn capture_dir(&self, date: NaiveDate) -> PathBuf {
        self.base_dir
            .join("capture")
            .join(date.format("%Y-%m-%d").to_string())
    }

    pub fn events_path(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("events.jsonl")
    }

    pub fn snapshots_dir(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("snapshots")
    }

    pub fn audio_mic_dir(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("audio").join("mic")
    }

    pub fn audio_system_dir(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("audio").join("system")
    }

    pub fn audio_wearable_dir(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("audio").join("wearable")
    }

    pub fn frames_dir(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("frames")
    }

    pub fn location_path(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("location.jsonl")
    }
}

impl Default for AlvumConfig {
    fn default() -> Self {
        let base_dir = dirs::data_dir()
            .expect("no Application Support directory found")
            .join("com.alvum.app");
        Self::new(base_dir)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alvum-core config`
Expected: PASS

- [ ] **Step 5: Write failing test for JSONL append**

```rust
// crates/alvum-core/src/storage.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::SemanticEvent;
    use tempfile::TempDir;

    #[test]
    fn append_jsonl_creates_file_and_appends_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("events.jsonl");

        let event1 = SemanticEvent::AppFocus {
            ts: "2026-04-03T09:00:00Z".parse().unwrap(),
            app: "VS Code".into(),
            window: "main.rs".into(),
        };
        let event2 = SemanticEvent::TextChanged {
            ts: "2026-04-03T09:05:00Z".parse().unwrap(),
            app: "VS Code".into(),
            detail: "~3 lines added".into(),
        };

        append_jsonl(&path, &event1).unwrap();
        append_jsonl(&path, &event2).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);

        let parsed: SemanticEvent = serde_json::from_str(lines[0]).unwrap();
        assert!(matches!(parsed, SemanticEvent::AppFocus { .. }));
    }

    #[test]
    fn ensure_dir_creates_nested_directories() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a").join("b").join("c");
        ensure_dir(&path).unwrap();
        assert!(path.is_dir());
    }
}
```

- [ ] **Step 6: Run test to verify it fails**

Run: `cargo test -p alvum-core storage`
Expected: FAIL — `append_jsonl` not defined

- [ ] **Step 7: Implement storage utilities**

Add `tempfile` and `dirs` to alvum-core Cargo.toml:
```toml
# crates/alvum-core/Cargo.toml — add to [dependencies]
dirs = "6"

[dev-dependencies]
tempfile = "3"
```

```rust
// crates/alvum-core/src/storage.rs

use anyhow::Result;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Append a single serializable value as one JSONL line.
/// Creates the file and parent directories if they don't exist.
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

/// Create a directory and all parent directories if they don't exist.
pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}
```

- [ ] **Step 8: Update lib.rs, run tests, commit**

```rust
// crates/alvum-core/src/lib.rs
pub mod config;
pub mod event;
pub mod storage;
```

Run: `cargo test -p alvum-core`
Expected: all tests PASS

```bash
git add crates/alvum-core/
git commit -m "feat(core): add config paths and JSONL storage utilities"
```

---

### Task 3: A11y Node Types + Tree Representation

**Files:**
- Create: `crates/alvum-capture/Cargo.toml`
- Create: `crates/alvum-capture/src/lib.rs`
- Create: `crates/alvum-capture/src/accessibility/mod.rs`
- Create: `crates/alvum-capture/src/accessibility/node.rs`

- [ ] **Step 1: Create alvum-capture Cargo.toml**

```toml
# crates/alvum-capture/Cargo.toml
[package]
name = "alvum-capture"
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

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Write failing tests for A11yNode**

```rust
// crates/alvum-capture/src/accessibility/node.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_text_extracts_labeled_values() {
        let tree = A11yTree {
            app: "Linear".into(),
            window_title: "INGEST-342".into(),
            url: None,
            root: A11yNode {
                role: "AXGroup".into(),
                label: Some("Details".into()),
                value: None,
                children: vec![
                    A11yNode {
                        role: "AXStaticText".into(),
                        label: Some("Status".into()),
                        value: Some("In Progress".into()),
                        children: vec![],
                    },
                    A11yNode {
                        role: "AXStaticText".into(),
                        label: Some("Priority".into()),
                        value: Some("High".into()),
                        children: vec![],
                    },
                ],
            },
        };

        let text = tree.content_text();
        assert!(text.contains("Status: In Progress"));
        assert!(text.contains("Priority: High"));
    }

    #[test]
    fn labeled_values_returns_label_value_pairs() {
        let tree = A11yTree {
            app: "Test".into(),
            window_title: "Test".into(),
            url: None,
            root: A11yNode {
                role: "AXWindow".into(),
                label: None,
                value: None,
                children: vec![
                    A11yNode {
                        role: "AXTextField".into(),
                        label: Some("Name".into()),
                        value: Some("Alice".into()),
                        children: vec![],
                    },
                    A11yNode {
                        role: "AXButton".into(),
                        label: Some("Submit".into()),
                        value: None,
                        children: vec![],
                    },
                ],
            },
        };

        let pairs = tree.labeled_values();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("Name".to_string(), "Alice".to_string()));
    }

    #[test]
    fn is_significant_filters_chrome() {
        let menu = A11yNode {
            role: "AXMenuBar".into(),
            label: None,
            value: None,
            children: vec![],
        };
        assert!(!menu.is_significant());

        let text = A11yNode {
            role: "AXTextArea".into(),
            label: Some("Editor".into()),
            value: Some("hello world".into()),
            children: vec![],
        };
        assert!(text.is_significant());
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p alvum-capture accessibility::node`
Expected: FAIL — types not defined

- [ ] **Step 4: Implement A11yNode and A11yTree**

```rust
// crates/alvum-capture/src/accessibility/node.rs

use serde::{Deserialize, Serialize};

const CHROME_ROLES: &[&str] = &[
    "AXMenuBar",
    "AXMenu",
    "AXMenuItem",
    "AXToolbar",
    "AXStatusBar",
    "AXScrollBar",
    "AXSplitter",
    "AXGrowArea",
];

const CONTENT_ROLES: &[&str] = &[
    "AXTextArea",
    "AXTextField",
    "AXStaticText",
    "AXWebArea",
    "AXTable",
    "AXList",
    "AXCell",
    "AXImage",
    "AXLink",
    "AXButton",
    "AXCheckBox",
    "AXPopUpButton",
    "AXComboBox",
    "AXRadioButton",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct A11yNode {
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub children: Vec<A11yNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct A11yTree {
    pub app: String,
    pub window_title: String,
    pub url: Option<String>,
    pub root: A11yNode,
}

impl A11yNode {
    /// Whether this node carries meaningful content (not UI chrome).
    pub fn is_significant(&self) -> bool {
        if CHROME_ROLES.iter().any(|r| *r == self.role) {
            return false;
        }
        CONTENT_ROLES.iter().any(|r| *r == self.role)
            || self.label.is_some()
            || self.value.is_some()
    }

    /// Collect all (label, value) pairs from this node and its descendants.
    fn collect_labeled_values(&self, out: &mut Vec<(String, String)>) {
        if let (Some(label), Some(value)) = (&self.label, &self.value) {
            if !value.is_empty() {
                out.push((label.clone(), value.clone()));
            }
        }
        for child in &self.children {
            child.collect_labeled_values(out);
        }
    }

    /// Collect all text content from this node and descendants.
    fn collect_content_text(&self, out: &mut String) {
        if !self.is_significant() {
            return;
        }
        if let (Some(label), Some(value)) = (&self.label, &self.value) {
            if !value.is_empty() {
                out.push_str(&format!("{label}: {value}\n"));
            }
        } else if let Some(value) = &self.value {
            if !value.is_empty() {
                out.push_str(value);
                out.push('\n');
            }
        }
        for child in &self.children {
            child.collect_content_text(out);
        }
    }
}

impl A11yTree {
    /// All (label, value) pairs in the tree — labeled UI elements with values.
    pub fn labeled_values(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        self.root.collect_labeled_values(&mut pairs);
        pairs
    }

    /// All text content as a single string — for content change comparison.
    pub fn content_text(&self) -> String {
        let mut text = String::new();
        self.root.collect_content_text(&mut text);
        text
    }
}
```

- [ ] **Step 5: Wire up module files**

```rust
// crates/alvum-capture/src/accessibility/mod.rs
pub mod node;
pub mod differ;
pub mod walker;
```

```rust
// crates/alvum-capture/src/lib.rs
pub mod accessibility;
```

- [ ] **Step 6: Run tests, commit**

Run: `cargo test -p alvum-capture accessibility::node`
Expected: 3 tests PASS

```bash
git add crates/alvum-capture/
git commit -m "feat(capture): add A11yNode and A11yTree types"
```

---

### Task 4: Generic A11y Differ

The core logic: compare two a11y tree snapshots and emit semantic events. Pure Rust, fully testable, no macOS dependencies.

**Files:**
- Create: `crates/alvum-capture/src/accessibility/differ.rs`

- [ ] **Step 1: Write failing tests for all 5 detection patterns**

```rust
// crates/alvum-capture/src/accessibility/differ.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::node::{A11yNode, A11yTree};

    fn leaf(role: &str, label: Option<&str>, value: Option<&str>) -> A11yNode {
        A11yNode {
            role: role.into(),
            label: label.map(Into::into),
            value: value.map(Into::into),
            children: vec![],
        }
    }

    fn tree(app: &str, window: &str, url: Option<&str>, children: Vec<A11yNode>) -> A11yTree {
        A11yTree {
            app: app.into(),
            window_title: window.into(),
            url: url.map(Into::into),
            root: A11yNode {
                role: "AXWindow".into(),
                label: None,
                value: None,
                children,
            },
        }
    }

    // Pattern 1: App focus change
    #[test]
    fn detect_app_focus_change() {
        let prev = tree("VS Code", "main.rs", None, vec![]);
        let curr = tree("Linear", "INGEST-342", None, vec![]);
        let events = diff(&prev, &curr);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], SemanticEvent::AppFocus { app, window, .. }
            if app == "Linear" && window == "INGEST-342"));
    }

    // Pattern 1b: Window change within same app
    #[test]
    fn detect_window_change_same_app() {
        let prev = tree("VS Code", "main.rs", None, vec![]);
        let curr = tree("VS Code", "lib.rs", None, vec![]);
        let events = diff(&prev, &curr);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], SemanticEvent::WindowChange { app, from, to, .. }
            if app == "VS Code" && from == "main.rs" && to == "lib.rs"));
    }

    // Pattern 2: Labeled value changed
    #[test]
    fn detect_field_changed() {
        let prev = tree("Linear", "INGEST-342", None, vec![
            leaf("AXStaticText", Some("Status"), Some("In Progress")),
            leaf("AXStaticText", Some("Priority"), Some("High")),
        ]);
        let curr = tree("Linear", "INGEST-342", None, vec![
            leaf("AXStaticText", Some("Status"), Some("Backlog")),
            leaf("AXStaticText", Some("Priority"), Some("High")),
        ]);
        let events = diff(&prev, &curr);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], SemanticEvent::FieldChanged { field, from, to, .. }
            if field == "Status" && from == "In Progress" && to == "Backlog"));
    }

    // Pattern 3: Text content changed
    #[test]
    fn detect_text_content_changed() {
        let prev = tree("VS Code", "main.rs", None, vec![
            A11yNode {
                role: "AXTextArea".into(),
                label: Some("Editor".into()),
                value: Some("fn main() {\n    println!(\"hello\");\n}".into()),
                children: vec![],
            },
        ]);
        let curr = tree("VS Code", "main.rs", None, vec![
            A11yNode {
                role: "AXTextArea".into(),
                label: Some("Editor".into()),
                value: Some("fn main() {\n    println!(\"hello\");\n    println!(\"world\");\n}".into()),
                children: vec![],
            },
        ]);
        let events = diff(&prev, &curr);

        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], SemanticEvent::TextChanged { detail, .. }
            if detail.contains("line")));
    }

    // Pattern 4: Navigation (URL change)
    #[test]
    fn detect_navigation() {
        let prev = tree("Safari", "GitHub", Some("https://github.com/foo"), vec![]);
        let curr = tree("Safari", "Docs.rs", Some("https://docs.rs/bar"), vec![]);
        let events = diff(&prev, &curr);

        assert!(events.iter().any(|e| matches!(e, SemanticEvent::Navigated { from, to, .. }
            if from.contains("github.com") && to.contains("docs.rs"))));
    }

    // Pattern 5: Structural change — node appeared
    #[test]
    fn detect_node_appeared() {
        let prev = tree("VS Code", "main.rs", None, vec![
            leaf("AXTextArea", Some("Editor"), Some("code")),
        ]);
        let curr = tree("VS Code", "main.rs", None, vec![
            leaf("AXTextArea", Some("Editor"), Some("code")),
            leaf("AXGroup", Some("Terminal"), Some("$ cargo build")),
        ]);
        let events = diff(&prev, &curr);

        assert!(events.iter().any(|e| matches!(e, SemanticEvent::NodeAppeared { detail, .. }
            if detail.contains("Terminal"))));
    }

    // Pattern 5b: Structural change — node disappeared
    #[test]
    fn detect_node_disappeared() {
        let prev = tree("Mail", "Inbox", None, vec![
            leaf("AXGroup", Some("Compose"), Some("Draft email")),
        ]);
        let curr = tree("Mail", "Inbox", None, vec![]);
        let events = diff(&prev, &curr);

        assert!(events.iter().any(|e| matches!(e, SemanticEvent::NodeDisappeared { detail, .. }
            if detail.contains("Compose"))));
    }

    // No changes = no events
    #[test]
    fn no_events_when_identical() {
        let tree1 = tree("VS Code", "main.rs", None, vec![
            leaf("AXStaticText", Some("Line"), Some("42")),
        ]);
        let tree2 = tree1.clone();
        let events = diff(&tree1, &tree2);
        assert!(events.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alvum-capture accessibility::differ`
Expected: FAIL — `diff` function not defined

- [ ] **Step 3: Implement the generic differ**

```rust
// crates/alvum-capture/src/accessibility/differ.rs

use chrono::Utc;
use std::collections::HashMap;

use alvum_core::event::SemanticEvent;
use crate::accessibility::node::A11yTree;

/// Compare two a11y tree snapshots and produce semantic change events.
/// Implements 5 universal detection patterns — no per-app logic.
pub fn diff(prev: &A11yTree, curr: &A11yTree) -> Vec<SemanticEvent> {
    let now = Utc::now();
    let mut events = Vec::new();

    // Pattern 1: App or window changed
    if prev.app != curr.app {
        events.push(SemanticEvent::AppFocus {
            ts: now,
            app: curr.app.clone(),
            window: curr.window_title.clone(),
        });
        return events; // App switch — don't diff content across apps
    }

    if prev.window_title != curr.window_title {
        events.push(SemanticEvent::WindowChange {
            ts: now,
            app: curr.app.clone(),
            from: prev.window_title.clone(),
            to: curr.window_title.clone(),
        });
    }

    // Pattern 4: Navigation (URL changed within same app)
    if let (Some(prev_url), Some(curr_url)) = (&prev.url, &curr.url) {
        if prev_url != curr_url {
            events.push(SemanticEvent::Navigated {
                ts: now,
                app: curr.app.clone(),
                from: prev_url.clone(),
                to: curr_url.clone(),
            });
        }
    }

    // Pattern 2: Labeled value changes
    let prev_values: HashMap<String, String> = prev
        .labeled_values()
        .into_iter()
        .collect();
    let curr_values: HashMap<String, String> = curr
        .labeled_values()
        .into_iter()
        .collect();

    for (label, curr_val) in &curr_values {
        if let Some(prev_val) = prev_values.get(label) {
            if prev_val != curr_val {
                events.push(SemanticEvent::FieldChanged {
                    ts: now,
                    app: curr.app.clone(),
                    field: label.clone(),
                    from: prev_val.clone(),
                    to: curr_val.clone(),
                });
            }
        }
    }

    // Pattern 3: Text content changed (large text areas)
    let prev_text = prev.content_text();
    let curr_text = curr.content_text();
    if prev_text != curr_text && !prev_text.is_empty() && !curr_text.is_empty() {
        let prev_lines = prev_text.lines().count();
        let curr_lines = curr_text.lines().count();
        let diff_lines = (curr_lines as isize - prev_lines as isize).unsigned_abs();
        let change_ratio = if prev_text.len() > 0 {
            levenshtein_ratio(&prev_text, &curr_text)
        } else {
            1.0
        };

        // Only emit if change is significant (>5% of content)
        if change_ratio > 0.05 {
            let detail = if curr_lines > prev_lines {
                format!("~{diff_lines} lines added")
            } else if curr_lines < prev_lines {
                format!("~{diff_lines} lines removed")
            } else {
                "content modified".to_string()
            };
            events.push(SemanticEvent::TextChanged {
                ts: now,
                app: curr.app.clone(),
                detail,
            });
        }
    }

    // Pattern 5: Structural changes — significant nodes appeared or disappeared
    let prev_sig = collect_significant_labels(prev);
    let curr_sig = collect_significant_labels(curr);

    for (label, description) in &curr_sig {
        if !prev_sig.contains_key(label) {
            events.push(SemanticEvent::NodeAppeared {
                ts: now,
                app: curr.app.clone(),
                detail: description.clone(),
            });
        }
    }
    for (label, description) in &prev_sig {
        if !curr_sig.contains_key(label) {
            events.push(SemanticEvent::NodeDisappeared {
                ts: now,
                app: curr.app.clone(),
                detail: description.clone(),
            });
        }
    }

    events
}

/// Collect labels of significant nodes as a map of label → description.
fn collect_significant_labels(tree: &A11yTree) -> HashMap<String, String> {
    let mut map = HashMap::new();
    collect_sig_recursive(&tree.root, &mut map);
    map
}

fn collect_sig_recursive(node: &crate::accessibility::node::A11yNode, map: &mut HashMap<String, String>) {
    if node.is_significant() {
        if let Some(label) = &node.label {
            let description = match &node.value {
                Some(val) if !val.is_empty() => {
                    // Truncate long values
                    let truncated = if val.len() > 80 { &val[..80] } else { val.as_str() };
                    format!("{label}: {truncated}")
                }
                _ => label.clone(),
            };
            map.insert(label.clone(), description);
        }
    }
    for child in &node.children {
        collect_sig_recursive(child, map);
    }
}

/// Approximate change ratio between two strings.
/// Returns 0.0 for identical, 1.0 for completely different.
/// Uses line-based comparison for efficiency (not character-level levenshtein).
fn levenshtein_ratio(a: &str, b: &str) -> f64 {
    let a_lines: Vec<&str> = a.lines().collect();
    let b_lines: Vec<&str> = b.lines().collect();
    let max_len = a_lines.len().max(b_lines.len());
    if max_len == 0 {
        return 0.0;
    }
    let common = a_lines
        .iter()
        .zip(b_lines.iter())
        .filter(|(a, b)| a == b)
        .count();
    1.0 - (common as f64 / max_len as f64)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alvum-capture accessibility::differ`
Expected: 8 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/alvum-capture/src/accessibility/differ.rs
git commit -m "feat(capture): add generic a11y differ with 5 detection patterns"
```

---

### Task 5: Screen Capture (ScreenCaptureKit + WebP)

**Files:**
- Create: `crates/alvum-capture/src/screen.rs`
- Modify: `crates/alvum-capture/Cargo.toml`

- [ ] **Step 1: Add dependencies**

```toml
# crates/alvum-capture/Cargo.toml — add to [dependencies]
screencapturekit = "1.5"
webp = "0.3"
image = "0.25"
```

- [ ] **Step 2: Write the screen capture module**

```rust
// crates/alvum-capture/src/screen.rs

use anyhow::{Context, Result};
use image::RgbaImage;
use screencapturekit::{
    sc_content_filter::SCContentFilter,
    sc_display::SCDisplay,
    sc_shareable_content::SCShareableContent,
    sc_screenshot::SCScreenshot,
    sc_stream_configuration::SCStreamConfiguration,
};
use std::path::Path;

/// Capture a screenshot of the main display, save as WebP.
pub async fn capture_screenshot(output_path: &Path, quality: f32) -> Result<()> {
    let content = SCShareableContent::get().await
        .context("failed to get shareable content — is screen recording permission granted?")?;
    let display = content
        .displays
        .into_iter()
        .next()
        .context("no displays found")?;

    let filter = SCContentFilter::new().with_display(&display);
    let config = SCStreamConfiguration::new()
        .set_width(display.width as u32)
        .set_height(display.height as u32)
        .set_pixel_format(screencapturekit::sc_stream_configuration::PixelFormat::BGRA8888);

    let image = SCScreenshot::new(&filter, &config).await
        .context("screenshot capture failed")?;

    let width = image.width as u32;
    let height = image.height as u32;
    let rgba_data = bgra_to_rgba(image.data);

    save_as_webp(&rgba_data, width, height, output_path, quality)?;
    Ok(())
}

fn bgra_to_rgba(bgra: Vec<u8>) -> Vec<u8> {
    let mut rgba = bgra;
    for chunk in rgba.chunks_exact_mut(4) {
        chunk.swap(0, 2); // swap B and R
    }
    rgba
}

fn save_as_webp(rgba: &[u8], width: u32, height: u32, path: &Path, quality: f32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let encoder = webp::Encoder::from_rgba(rgba, width, height);
    let webp_data = encoder.encode(quality);
    std::fs::write(path, &*webp_data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn bgra_to_rgba_swaps_channels() {
        let bgra = vec![10, 20, 30, 255]; // B=10, G=20, R=30, A=255
        let rgba = bgra_to_rgba(bgra);
        assert_eq!(rgba, vec![30, 20, 10, 255]); // R=30, G=20, B=10, A=255
    }

    #[test]
    fn save_webp_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.webp");
        // 2x2 red image
        let rgba = vec![255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255];
        save_as_webp(&rgba, 2, 2, &path, 80.0).unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alvum-capture screen`
Expected: 2 unit tests PASS. (The `capture_screenshot` function requires screen recording permission and a running display — test it manually or in integration tests.)

- [ ] **Step 4: Add to lib.rs, commit**

```rust
// crates/alvum-capture/src/lib.rs — add
pub mod screen;
```

```bash
git add crates/alvum-capture/
git commit -m "feat(capture): add ScreenCaptureKit screenshot capture + WebP encoding"
```

---

### Task 6: Accessibility Tree Walking (macOS API)

**Files:**
- Create: `crates/alvum-capture/src/accessibility/walker.rs`
- Modify: `crates/alvum-capture/Cargo.toml`

- [ ] **Step 1: Add dependencies**

```toml
# crates/alvum-capture/Cargo.toml — add to [dependencies]
accessibility = "0.2"
accessibility-sys = "0.2"
core-foundation = "0.10"
```

- [ ] **Step 2: Implement the a11y tree walker**

```rust
// crates/alvum-capture/src/accessibility/walker.rs

use accessibility::{AXAttribute, AXUIElement, TreeVisitor, TreeWalker};
use anyhow::{Context, Result};

use crate::accessibility::node::{A11yNode, A11yTree};

/// Walk the accessibility tree of the frontmost application.
pub fn walk_focused_app() -> Result<A11yTree> {
    let system = AXUIElement::system_wide();

    // Get the focused application
    let focused_app: AXUIElement = system
        .attribute(&AXAttribute::focused_application())
        .context("no focused application — is accessibility permission granted?")?;

    let app_title: String = focused_app
        .attribute(&AXAttribute::title())
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get the focused window
    let focused_window: AXUIElement = focused_app
        .attribute(&AXAttribute::focused_window())
        .unwrap_or(focused_app.clone());

    let window_title: String = focused_window
        .attribute(&AXAttribute::title())
        .unwrap_or_default();

    // Walk the tree from the focused window, pruning chrome
    let root = walk_element(&focused_window, 0, 6);

    // Try to extract URL (for browsers)
    let url = extract_url(&focused_window);

    Ok(A11yTree {
        app: app_title,
        window_title,
        url,
        root,
    })
}

const MAX_CHILDREN: usize = 50;

fn walk_element(element: &AXUIElement, depth: usize, max_depth: usize) -> A11yNode {
    let role: String = element
        .attribute(&AXAttribute::role())
        .unwrap_or_else(|_| "AXUnknown".to_string());

    let label: Option<String> = element
        .attribute(&AXAttribute::title())
        .ok()
        .or_else(|| element.attribute(&AXAttribute::description()).ok());

    let value: Option<String> = element
        .attribute(&AXAttribute::value())
        .ok()
        .map(|v: String| {
            // Truncate very long values (e.g., full editor content)
            if v.len() > 500 {
                format!("{}...", &v[..500])
            } else {
                v
            }
        });

    let children = if depth < max_depth {
        element
            .attribute(&AXAttribute::children())
            .unwrap_or_default()
            .into_iter()
            .take(MAX_CHILDREN)
            .map(|child: AXUIElement| walk_element(&child, depth + 1, max_depth))
            .collect()
    } else {
        vec![]
    };

    A11yNode {
        role,
        label,
        value,
        children,
    }
}

/// Try to extract URL from a browser window via the a11y tree.
fn extract_url(window: &AXUIElement) -> Option<String> {
    // Browsers typically have a text field with role AXTextField and
    // subrole AXURLField containing the current URL
    if let Ok(children) = window.attribute::<Vec<AXUIElement>>(&AXAttribute::children()) {
        for child in children {
            if let Ok(subrole) = child.attribute::<String>(&AXAttribute::new("AXSubrole")) {
                if subrole == "AXURLTextField" {
                    return child.attribute::<String>(&AXAttribute::value()).ok();
                }
            }
            // Recurse one level into toolbars
            if let Ok(role) = child.attribute::<String>(&AXAttribute::role()) {
                if role == "AXToolbar" {
                    if let Some(url) = extract_url(&child) {
                        return Some(url);
                    }
                }
            }
        }
    }
    None
}
```

- [ ] **Step 3: Write integration test (requires accessibility permission)**

```rust
// crates/alvum-capture/tests/capture_integration.rs

/// Integration tests require macOS accessibility permission.
/// Run with: cargo test -p alvum-capture --test capture_integration -- --ignored
#[test]
#[ignore] // requires accessibility permission
fn walk_focused_app_returns_tree() {
    let tree = alvum_capture::accessibility::walker::walk_focused_app().unwrap();
    assert!(!tree.app.is_empty());
    // The test runner itself is an app, so we should get something
    println!("App: {}, Window: {}", tree.app, tree.window_title);
    println!("Content text length: {}", tree.content_text().len());
}
```

- [ ] **Step 4: Run unit tests (should compile), commit**

Run: `cargo test -p alvum-capture` (non-ignored tests only)
Run manually: `cargo test -p alvum-capture --test capture_integration -- --ignored` (if permissions granted)

```bash
git add crates/alvum-capture/
git commit -m "feat(capture): add macOS accessibility tree walker"
```

---

### Task 7: Audio Processing (VAD + Opus + File Segmentation)

Shared audio processing module: takes raw PCM samples, runs VAD, encodes to Opus, segments into files on speech boundaries.

**Files:**
- Create: `crates/alvum-capture/src/audio/mod.rs`
- Create: `crates/alvum-capture/src/audio/processor.rs`

- [ ] **Step 1: Add dependencies**

```toml
# crates/alvum-capture/Cargo.toml — add to [dependencies]
silero-vad-rust = "6.2"
ort = "2.0.0-rc.12"
opus = "0.3"
hound = "3.5"            # WAV writing for tests
```

- [ ] **Step 2: Write failing test for VAD segmentation**

```rust
// crates/alvum-capture/src/audio/processor.rs

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn silence(samples: usize) -> Vec<f32> {
        vec![0.0; samples]
    }

    fn tone(samples: usize, freq: f32, sample_rate: f32) -> Vec<f32> {
        (0..samples)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate).sin() * 0.5)
            .collect()
    }

    #[test]
    fn processor_creates_no_files_for_silence() {
        let tmp = TempDir::new().unwrap();
        let mut proc = AudioProcessor::new(tmp.path().to_path_buf(), 16000).unwrap();

        // Feed 2 seconds of silence
        let samples = silence(32000);
        proc.process_samples(&samples).unwrap();
        proc.flush().unwrap();

        let files: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "opus"))
            .collect();
        assert_eq!(files.len(), 0, "silence should produce no audio files");
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p alvum-capture audio::processor`
Expected: FAIL — `AudioProcessor` not defined

- [ ] **Step 4: Implement AudioProcessor**

```rust
// crates/alvum-capture/src/audio/processor.rs

use anyhow::{Context, Result};
use chrono::Utc;
use std::path::PathBuf;
use tracing::debug;

/// Processes raw audio samples: runs VAD, accumulates speech segments,
/// encodes to Opus, and writes segmented files.
pub struct AudioProcessor {
    output_dir: PathBuf,
    sample_rate: usize,
    vad: silero_vad_rust::SileroVad,
    speech_buffer: Vec<f32>,
    is_speaking: bool,
    silence_frames: usize,
    /// Number of consecutive silent frames before closing a segment
    silence_threshold: usize,
}

impl AudioProcessor {
    pub fn new(output_dir: PathBuf, sample_rate: usize) -> Result<Self> {
        std::fs::create_dir_all(&output_dir)?;

        let vad = silero_vad_rust::SileroVad::new(
            silero_vad_rust::VadConfig {
                sample_rate: sample_rate as i64,
                ..Default::default()
            },
        ).context("failed to initialize Silero VAD")?;

        Ok(Self {
            output_dir,
            sample_rate,
            vad,
            speech_buffer: Vec::new(),
            is_speaking: false,
            silence_frames: 0,
            // ~1.5 seconds of silence ends a segment
            silence_threshold: (sample_rate * 3) / (2 * 512),
        })
    }

    /// Feed raw PCM f32 samples (mono, at configured sample_rate).
    /// Internally runs VAD, accumulates speech, writes segments.
    pub fn process_samples(&mut self, samples: &[f32]) -> Result<()> {
        // Process in VAD-sized chunks (512 samples for 16kHz)
        let chunk_size = 512;
        for chunk in samples.chunks(chunk_size) {
            if chunk.len() < chunk_size {
                break; // skip incomplete final chunk
            }

            let is_speech = self.vad.is_voice_segment(&chunk.to_vec())
                .unwrap_or(false);

            if is_speech {
                self.silence_frames = 0;
                if !self.is_speaking {
                    self.is_speaking = true;
                    debug!("speech started");
                }
                self.speech_buffer.extend_from_slice(chunk);
            } else if self.is_speaking {
                self.silence_frames += 1;
                // Keep buffering during short pauses
                self.speech_buffer.extend_from_slice(chunk);

                if self.silence_frames >= self.silence_threshold {
                    self.write_segment()?;
                    self.is_speaking = false;
                    self.silence_frames = 0;
                }
            }
        }
        Ok(())
    }

    /// Flush any remaining speech buffer to a file.
    pub fn flush(&mut self) -> Result<()> {
        if !self.speech_buffer.is_empty() && self.is_speaking {
            self.write_segment()?;
            self.is_speaking = false;
        }
        Ok(())
    }

    fn write_segment(&mut self) -> Result<()> {
        if self.speech_buffer.is_empty() {
            return Ok(());
        }

        let timestamp = Utc::now().format("%H-%M-%S");
        let path = self.output_dir.join(format!("{timestamp}.opus"));

        encode_opus(&self.speech_buffer, self.sample_rate, &path)?;
        debug!(path = %path.display(), samples = self.speech_buffer.len(), "wrote audio segment");

        self.speech_buffer.clear();
        Ok(())
    }
}

/// Encode f32 PCM samples to Opus and write to file.
fn encode_opus(samples: &[f32], sample_rate: usize, path: &std::path::Path) -> Result<()> {
    let mut encoder = opus::Encoder::new(
        sample_rate as u32,
        opus::Channels::Mono,
        opus::Application::Voip,
    ).context("failed to create Opus encoder")?;

    let frame_size = sample_rate / 50; // 20ms frames
    let mut encoded_data = Vec::new();

    for frame in samples.chunks(frame_size) {
        if frame.len() < frame_size {
            break;
        }
        let mut output = vec![0u8; 4000];
        let len = encoder.encode_float(frame, &mut output)
            .context("Opus encode failed")?;
        // Simple container: 2-byte length prefix + encoded data
        encoded_data.extend_from_slice(&(len as u16).to_le_bytes());
        encoded_data.extend_from_slice(&output[..len]);
    }

    std::fs::write(path, &encoded_data)?;
    Ok(())
}
```

- [ ] **Step 5: Wire up module**

```rust
// crates/alvum-capture/src/audio/mod.rs
pub mod processor;
pub mod mic;
pub mod system;
```

```rust
// crates/alvum-capture/src/lib.rs — add
pub mod audio;
```

- [ ] **Step 6: Run tests, commit**

Run: `cargo test -p alvum-capture audio::processor`
Expected: 1 test PASS

```bash
git add crates/alvum-capture/src/audio/
git commit -m "feat(capture): add audio processor with Silero VAD + Opus encoding"
```

---

### Task 8: Audio Capture Sources (Mic + System)

**Files:**
- Create: `crates/alvum-capture/src/audio/mic.rs`
- Create: `crates/alvum-capture/src/audio/system.rs`

- [ ] **Step 1: Add dependencies**

```toml
# crates/alvum-capture/Cargo.toml — add to [dependencies]
cpal = "0.17"
```

- [ ] **Step 2: Implement microphone capture**

```rust
// crates/alvum-capture/src/audio/mic.rs

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

use crate::audio::processor::AudioProcessor;

/// Captures microphone audio using cpal, feeds into AudioProcessor.
pub struct MicCapture {
    stream: Option<Stream>,
}

impl MicCapture {
    /// Start capturing from the default input device.
    /// Feeds samples into the provided AudioProcessor.
    pub fn start(processor: Arc<Mutex<AudioProcessor>>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("no input device available")?;

        info!(device = device.name().unwrap_or_default(), "starting mic capture");

        let config = StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(16000),
            buffer_size: cpal::BufferSize::Default,
        };

        let processor_err = processor.clone();
        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut proc) = processor.lock() {
                    if let Err(e) = proc.process_samples(data) {
                        error!(error = %e, "mic audio processing failed");
                    }
                }
            },
            move |err| {
                error!(error = %err, "mic stream error");
            },
            None,
        ).context("failed to build mic input stream")?;

        stream.play().context("failed to start mic stream")?;

        Ok(Self {
            stream: Some(stream),
        })
    }

    /// Stop capturing.
    pub fn stop(&mut self) {
        self.stream.take(); // dropping the stream stops it
    }
}
```

- [ ] **Step 3: Implement system audio capture**

```rust
// crates/alvum-capture/src/audio/system.rs

use anyhow::{Context, Result};
use std::sync::{Arc, Mutex};
use tracing::{error, info};

use crate::audio::processor::AudioProcessor;

/// Captures system audio (what you hear) via ScreenCaptureKit audio stream.
/// This is the Apple-blessed way to capture loopback audio on macOS 13+.
pub struct SystemAudioCapture {
    // Handle to the SCK stream — dropping it stops capture
    _stream: screencapturekit::sc_stream::SCStream,
}

impl SystemAudioCapture {
    /// Start capturing system audio.
    pub async fn start(processor: Arc<Mutex<AudioProcessor>>) -> Result<Self> {
        use screencapturekit::{
            sc_content_filter::SCContentFilter,
            sc_shareable_content::SCShareableContent,
            sc_stream::SCStream,
            sc_stream_configuration::SCStreamConfiguration,
        };

        let content = SCShareableContent::get().await
            .context("failed to get shareable content")?;
        let display = content.displays.into_iter().next()
            .context("no display found")?;

        let config = SCStreamConfiguration::new()
            .set_captures_audio(true)
            .set_excludes_current_process_audio(true)
            .set_channel_count(1)
            .set_sample_rate(16000);

        let filter = SCContentFilter::new().with_display(&display);

        let stream = SCStream::new(&filter, &config, move |audio_buffer| {
            // Convert audio buffer to f32 samples and feed to processor
            if let Ok(samples) = extract_f32_samples(&audio_buffer) {
                if let Ok(mut proc) = processor.lock() {
                    if let Err(e) = proc.process_samples(&samples) {
                        error!(error = %e, "system audio processing failed");
                    }
                }
            }
        });

        stream.start_capture().await
            .context("failed to start system audio capture")?;
        info!("system audio capture started");

        Ok(Self { _stream: stream })
    }
}

fn extract_f32_samples(buffer: &screencapturekit::sc_output_handler::AudioBuffer) -> Result<Vec<f32>> {
    // SCK provides audio in the configured format (f32, 16kHz, mono).
    // The AudioBuffer struct from screencapturekit exposes raw sample data.
    // If the crate's AudioBuffer type differs at implementation time, adapt
    // this function — it's the only SCK-audio-specific conversion point.
    Ok(buffer.data.clone())
}
```

- [ ] **Step 4: Commit**

Note: Both audio capture modules interact directly with hardware and macOS APIs. They're tested via integration tests (requires mic permission and screen recording permission).

```bash
git add crates/alvum-capture/src/audio/
git commit -m "feat(capture): add mic and system audio capture sources"
```

---

### Task 9: Event-Driven Trigger System

Orchestrates: when to capture, what to capture, and how to emit events.

**Files:**
- Create: `crates/alvum-capture/src/triggers.rs`

- [ ] **Step 1: Write failing test for trigger logic**

```rust
// crates/alvum-capture/src/triggers.rs

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_snapshot_on_app_focus() {
        let event = SemanticEvent::AppFocus {
            ts: Utc::now(),
            app: "Linear".into(),
            window: "INGEST-342".into(),
        };
        assert!(should_snapshot(&event));
    }

    #[test]
    fn should_not_snapshot_on_text_changed() {
        let event = SemanticEvent::TextChanged {
            ts: Utc::now(),
            app: "VS Code".into(),
            detail: "~3 lines added".into(),
        };
        assert!(!should_snapshot(&event));
    }

    #[test]
    fn should_snapshot_on_navigation() {
        let event = SemanticEvent::Navigated {
            ts: Utc::now(),
            app: "Safari".into(),
            from: "https://a.com".into(),
            to: "https://b.com".into(),
        };
        assert!(should_snapshot(&event));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alvum-capture triggers`
Expected: FAIL — `should_snapshot` not defined

- [ ] **Step 3: Implement trigger system**

```rust
// crates/alvum-capture/src/triggers.rs

use alvum_core::event::SemanticEvent;
use alvum_core::storage::append_jsonl;
use anyhow::Result;
use chrono::{NaiveDate, Utc};
use std::path::PathBuf;
use tracing::{debug, info};

use crate::accessibility::differ;
use crate::accessibility::node::A11yTree;
use crate::accessibility::walker;

/// Determines if a semantic event warrants saving a screenshot.
/// Screenshots are expensive — only take them at meaningful moments.
pub fn should_snapshot(event: &SemanticEvent) -> bool {
    matches!(
        event,
        SemanticEvent::AppFocus { .. }
            | SemanticEvent::Navigated { .. }
            | SemanticEvent::NodeAppeared { .. }
    )
}

/// The capture orchestrator. On each trigger, it:
/// 1. Walks the a11y tree of the focused app
/// 2. Diffs against the previous tree
/// 3. Emits semantic events to events.jsonl
/// 4. Optionally saves a screenshot
pub struct CaptureOrchestrator {
    events_path: PathBuf,
    snapshots_dir: PathBuf,
    previous_tree: Option<A11yTree>,
}

impl CaptureOrchestrator {
    pub fn new(events_path: PathBuf, snapshots_dir: PathBuf) -> Self {
        Self {
            events_path,
            snapshots_dir,
            previous_tree: None,
        }
    }

    /// Called on each trigger event. Returns the semantic events emitted.
    pub async fn on_trigger(&mut self) -> Result<Vec<SemanticEvent>> {
        // 1. Walk the current a11y tree
        let current_tree = match walker::walk_focused_app() {
            Ok(tree) => tree,
            Err(e) => {
                debug!(error = %e, "failed to walk a11y tree, skipping trigger");
                return Ok(vec![]);
            }
        };

        // 2. Diff against previous
        let events = match &self.previous_tree {
            Some(prev) => differ::diff(prev, &current_tree),
            None => {
                // First capture — emit app focus event
                vec![SemanticEvent::AppFocus {
                    ts: Utc::now(),
                    app: current_tree.app.clone(),
                    window: current_tree.window_title.clone(),
                }]
            }
        };

        // 3. Emit semantic events
        for event in &events {
            append_jsonl(&self.events_path, event)?;
        }

        // 4. Save screenshot if warranted
        let needs_snapshot = events.iter().any(should_snapshot);
        if needs_snapshot {
            let timestamp = Utc::now().format("%H-%M-%S");
            let path = self.snapshots_dir.join(format!("{timestamp}.webp"));
            if let Err(e) = crate::screen::capture_screenshot(&path, 80.0).await {
                debug!(error = %e, "screenshot capture failed, continuing");
            }
        }

        // 5. Update state
        self.previous_tree = Some(current_tree);

        if !events.is_empty() {
            debug!(count = events.len(), "emitted semantic events");
        }

        Ok(events)
    }
}
```

- [ ] **Step 4: Run tests, commit**

Run: `cargo test -p alvum-capture triggers`
Expected: 3 tests PASS

```rust
// crates/alvum-capture/src/lib.rs — add
pub mod triggers;
```

```bash
git add crates/alvum-capture/src/triggers.rs crates/alvum-capture/src/lib.rs
git commit -m "feat(capture): add event-driven trigger system and capture orchestrator"
```

---

### Task 10: Capture Daemon Assembly

Wires everything together: triggers, audio, orchestrator, lifecycle management.

**Files:**
- Create: `crates/alvum-capture/src/daemon.rs`

- [ ] **Step 1: Implement the daemon**

```rust
// crates/alvum-capture/src/daemon.rs

use alvum_core::config::AlvumConfig;
use alvum_core::storage::ensure_dir;
use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time;
use tracing::{error, info};

use crate::audio::mic::MicCapture;
use crate::audio::processor::AudioProcessor;
use crate::audio::system::SystemAudioCapture;
use crate::triggers::CaptureOrchestrator;

pub struct CaptureDaemon {
    config: AlvumConfig,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl CaptureDaemon {
    pub fn new(config: AlvumConfig) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            config,
            shutdown_tx,
            shutdown_rx,
        }
    }

    /// Start the capture daemon. Blocks until shutdown is signaled.
    pub async fn run(&self) -> Result<()> {
        let today = Local::now().date_naive();
        info!(date = %today, "starting capture daemon");

        // Create daily directories
        let capture_dir = self.config.capture_dir(today);
        ensure_dir(&self.config.snapshots_dir(today))?;
        ensure_dir(&self.config.audio_mic_dir(today))?;
        ensure_dir(&self.config.audio_system_dir(today))?;

        // Start audio processors
        let mic_processor = Arc::new(Mutex::new(
            AudioProcessor::new(self.config.audio_mic_dir(today), 16000)
                .context("failed to create mic audio processor")?,
        ));
        let sys_processor = Arc::new(Mutex::new(
            AudioProcessor::new(self.config.audio_system_dir(today), 16000)
                .context("failed to create system audio processor")?,
        ));

        // Start audio capture
        let mut mic = MicCapture::start(mic_processor.clone())
            .context("failed to start mic capture")?;
        let sys_capture = SystemAudioCapture::start(sys_processor.clone()).await
            .context("failed to start system audio capture")?;

        // Start screen capture orchestrator
        let mut orchestrator = CaptureOrchestrator::new(
            self.config.events_path(today),
            self.config.snapshots_dir(today),
        );

        // Main capture loop — trigger on interval (idle fallback)
        // In production, this is supplemented by NSWorkspace notifications
        // and ScreenCaptureKit frame diff callbacks
        let mut interval = time::interval(Duration::from_secs(30));
        let mut shutdown = self.shutdown_rx.clone();

        info!("capture daemon running");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = orchestrator.on_trigger().await {
                        error!(error = %e, "capture trigger failed");
                    }

                    // Check if day changed — rotate directories
                    let now = Local::now().date_naive();
                    if now != today {
                        info!(new_date = %now, "day changed, flushing");
                        // Flush audio processors
                        if let Ok(mut proc) = mic_processor.lock() {
                            let _ = proc.flush();
                        }
                        if let Ok(mut proc) = sys_processor.lock() {
                            let _ = proc.flush();
                        }
                        // In production, restart with new day's directories
                        break;
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        info!("shutdown signal received");
                        break;
                    }
                }
            }
        }

        // Cleanup
        mic.stop();
        if let Ok(mut proc) = mic_processor.lock() {
            proc.flush()?;
        }
        if let Ok(mut proc) = sys_processor.lock() {
            proc.flush()?;
        }

        info!("capture daemon stopped");
        Ok(())
    }

    /// Signal the daemon to shut down.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}
```

- [ ] **Step 2: Add to lib.rs**

```rust
// crates/alvum-capture/src/lib.rs — add
pub mod daemon;
```

- [ ] **Step 3: Write integration test**

```rust
// crates/alvum-capture/tests/capture_integration.rs — add

#[tokio::test]
#[ignore] // requires screen recording + accessibility + mic permissions
async fn daemon_starts_and_stops() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config = alvum_core::config::AlvumConfig::new(tmp.path().to_path_buf());
    let daemon = alvum_capture::daemon::CaptureDaemon::new(config);

    // Start daemon in background
    let daemon_ref = &daemon;
    let handle = tokio::spawn(async move {
        daemon_ref.run().await
    });

    // Let it run briefly
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    // Signal shutdown
    daemon.shutdown();

    // Wait for clean exit
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        handle,
    ).await;

    assert!(result.is_ok(), "daemon should shut down within 5 seconds");
}
```

- [ ] **Step 4: Run compilation check, commit**

Run: `cargo build -p alvum-capture`
Expected: compiles (runtime testing requires permissions)

```bash
git add crates/alvum-capture/
git commit -m "feat(capture): add capture daemon with lifecycle management"
```

---

## Implementation Notes

### macOS Permissions

The capture daemon requires three permissions granted via System Preferences > Privacy & Security:
- **Screen Recording** — for ScreenCaptureKit (screenshots + system audio)
- **Accessibility** — for AXUIElement tree walking
- **Microphone** — for cpal mic capture

In the Tauri app, these are requested via `Info.plist` entries. For development, grant them to Terminal.app or your IDE.

### SCK API Verification

The `screencapturekit` crate (v1.5.4) API used in this plan is based on documented capabilities. Verify the exact method signatures against the crate docs during implementation — particularly:
- `SCScreenshot` API for single frame capture
- Audio stream callback signature and `AudioBuffer` type
- `SCStreamConfiguration` builder methods for audio settings

### Audio Format

The Opus encoding uses a simple custom container (2-byte length prefix per frame). For V1 this is sufficient. A future task should migrate to the OGG/Opus container format via `libopusenc` for broader compatibility.

### NSWorkspace Notifications

The idle timer (30s interval) is the simplest trigger. For production, add NSWorkspace notification observers for immediate app-switch detection. This requires a running NSRunLoop, which Tauri provides. The `on_trigger` method is already designed to be called from any trigger source.
