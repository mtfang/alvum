# Connector Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce `Processor` and `Connector` traits to match the existing `CaptureSource` trait, extract the extraction pipeline into a reusable library function, and refactor `cmd_extract` to be a generic dispatcher driven by the connector registry.

**Architecture:** Add `Processor` trait alongside `CaptureSource`. Add `Connector` trait that bundles them. Move `cmd_extract`'s 350-line pipeline into `alvum-pipeline::extract::extract_and_pipeline()` as a library function that takes `Vec<Box<dyn Connector>>`. The CLI becomes a thin dispatcher that loads connectors from config and calls the library.

**Tech Stack:** Rust, `async-trait`, existing alvum crates.

---

## Why this refactor

Currently:
- `CaptureSource` is a clean trait in alvum-core
- Processors are standalone functions with incompatible signatures
- `cmd_extract` hardcodes which processor runs for which source (audio → whisper, screen → vision)
- Adding a new processor requires modifying the CLI

After refactor:
- `Processor` trait in alvum-core, same pattern as `CaptureSource`
- `Connector` trait bundles capture + processor(s) as a user-facing plugin
- `cmd_extract` iterates connectors → calls their processors → unified observation stream
- New connectors register themselves; CLI doesn't know specific implementations

---

## File Structure

```
alvum/
├── crates/
│   ├── alvum-core/                    (modified)
│   │   └── src/
│   │       ├── capture.rs             (existing) CaptureSource trait
│   │       ├── processor.rs           NEW Processor trait
│   │       └── connector.rs           NEW Connector trait
│   ├── alvum-pipeline/                (modified)
│   │   └── src/
│   │       └── extract.rs             NEW extract_and_pipeline library function
│   ├── alvum-connector-audio/         NEW crate
│   │   └── src/
│   │       └── lib.rs                 AudioConnector implementing Connector
│   ├── alvum-connector-screen/        NEW crate
│   │   └── src/
│   │       └── lib.rs                 ScreenConnector implementing Connector
│   ├── alvum-connector-claude/        (existing, modified)
│   │   └── src/
│   │       └── lib.rs                 add ClaudeCodeConnector impl
│   └── alvum-cli/                     (modified)
│       └── src/
│           └── main.rs                thin dispatcher using connectors
```

---

### Task 1: Processor Trait in alvum-core

Add `Processor` trait matching the `CaptureSource` pattern. Every processor becomes a trait impl.

**Files:**
- Create: `crates/alvum-core/src/processor.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Create `crates/alvum-core/src/processor.rs`**

```rust
//! Processor trait: reads DataRefs, produces Observations.
//!
//! Processors interpret raw captured data (audio files, screenshots, etc.) into
//! LLM-readable Observation objects. They are paired with capture sources inside
//! a Connector.

use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

use crate::data_ref::DataRef;
use crate::observation::Observation;

/// A processor reads DataRefs and produces Observations.
#[async_trait]
pub trait Processor: Send + Sync {
    /// Unique name (e.g., "whisper", "vision-local", "ocr").
    fn name(&self) -> &str;

    /// Which sources or MIME types this processor handles.
    /// Examples: ["audio-mic", "audio-system"] or ["image/png"].
    fn handles(&self) -> Vec<String>;

    /// Process the given DataRefs into Observations.
    /// `capture_dir` is the root of the capture directory for resolving relative paths.
    async fn process(
        &self,
        data_refs: &[DataRef],
        capture_dir: &Path,
    ) -> Result<Vec<Observation>>;
}
```

- [ ] **Step 2: Add `pub mod processor;` to `crates/alvum-core/src/lib.rs`**

Add after the existing `pub mod capture;` line.

- [ ] **Step 3: Add unit test**

Append to `crates/alvum-core/src/processor.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_ref::DataRef;
    use chrono::Utc;

    struct DummyProcessor;

    #[async_trait]
    impl Processor for DummyProcessor {
        fn name(&self) -> &str { "dummy" }
        fn handles(&self) -> Vec<String> { vec!["test".into()] }
        async fn process(&self, _refs: &[DataRef], _dir: &Path) -> Result<Vec<Observation>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn processor_trait_is_implementable() {
        let p = DummyProcessor;
        assert_eq!(p.name(), "dummy");
        assert_eq!(p.handles(), vec!["test".to_string()]);
        let result = p.process(&[], std::path::Path::new("/tmp")).await.unwrap();
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 4: Run tests**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p alvum-core
```

Expected: 31 passing (30 existing + 1 new).

- [ ] **Step 5: Commit**

```bash
git add crates/alvum-core/src/processor.rs crates/alvum-core/src/lib.rs && git commit -m "feat(core): add Processor trait matching CaptureSource pattern"
```

---

### Task 2: Connector Trait in alvum-core

Add `Connector` trait that bundles capture sources + processors. This is the user-facing plugin contract.

**Files:**
- Create: `crates/alvum-core/src/connector.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Create `crates/alvum-core/src/connector.rs`**

```rust
//! Connector trait: the user-facing plugin concept.
//!
//! A Connector is what the user adds and manages. Internally, it bundles one or
//! more capture sources (daemons or importers) with one or more processors
//! (which interpret raw data into Observations).

use anyhow::Result;
use std::collections::HashMap;

use crate::capture::CaptureSource;
use crate::processor::Processor;

/// A Connector bundles capture sources and processors into a complete plugin.
pub trait Connector: Send + Sync {
    /// Unique name (e.g., "audio", "screen", "claude-code").
    fn name(&self) -> &str;

    /// Capture sources owned by this connector. May be empty for one-shot
    /// importers that don't run as daemons (e.g., claude-code).
    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>>;

    /// Processors owned by this connector. Each handles specific sources
    /// or MIME types produced by this connector's capture sources.
    fn processors(&self) -> Vec<Box<dyn Processor>>;
}

/// Helper for building connectors from config settings.
pub trait ConnectorBuilder {
    type Output: Connector;
    fn build(settings: &HashMap<String, toml::Value>) -> Result<Self::Output>;
}
```

- [ ] **Step 2: Add `pub mod connector;` to `crates/alvum-core/src/lib.rs`**

- [ ] **Step 3: Run tests**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p alvum-core
```

Expected: all tests still pass.

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-core/src/connector.rs crates/alvum-core/src/lib.rs && git commit -m "feat(core): add Connector trait for user-facing plugin bundling"
```

---

### Task 3: Audio Connector Crate

Create `alvum-connector-audio` that bundles `AudioMicSource` + `AudioSystemSource` + a new `WhisperProcessor` into a `Connector`.

**Files:**
- Create: `crates/alvum-connector-audio/Cargo.toml`
- Create: `crates/alvum-connector-audio/src/lib.rs`
- Create: `crates/alvum-connector-audio/src/processor.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "alvum-connector-audio"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
alvum-capture-audio = { path = "../alvum-capture-audio" }
alvum-processor-audio = { path = "../alvum-processor-audio" }
anyhow.workspace = true
async-trait = "0.1"
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
toml = "0.8"
```

- [ ] **Step 2: Add to workspace Cargo.toml members list**

Add `"crates/alvum-connector-audio"` to the workspace members.

- [ ] **Step 3: Create `src/processor.rs` — WhisperProcessor wrapping existing transcriber**

```rust
//! WhisperProcessor — implements the Processor trait using alvum-processor-audio.

use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct WhisperProcessor {
    model_path: PathBuf,
}

impl WhisperProcessor {
    pub fn new(model_path: PathBuf) -> Self {
        Self { model_path }
    }
}

#[async_trait]
impl Processor for WhisperProcessor {
    fn name(&self) -> &str {
        "whisper"
    }

    fn handles(&self) -> Vec<String> {
        vec!["audio-mic".into(), "audio-system".into(), "audio-wearable".into()]
    }

    async fn process(
        &self,
        data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        if !self.model_path.exists() {
            anyhow::bail!("Whisper model not found: {}", self.model_path.display());
        }

        info!(
            model = %self.model_path.display(),
            refs = data_refs.len(),
            "whisper processing"
        );

        // Run blocking whisper work on a blocking task
        let model_path = self.model_path.clone();
        let refs = data_refs.to_vec();
        tokio::task::spawn_blocking(move || {
            alvum_processor_audio::transcriber::process_audio_data_refs(&model_path, &refs)
        })
        .await
        .context("whisper task panicked")?
    }
}
```

- [ ] **Step 4: Create `src/lib.rs` — AudioConnector bundling capture + processor**

```rust
//! AudioConnector — user-facing plugin bundling audio capture + whisper processing.

pub mod processor;

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::processor::Processor;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use processor::WhisperProcessor;

pub struct AudioConnector {
    mic_enabled: bool,
    system_enabled: bool,
    mic_device: Option<String>,
    system_device: Option<String>,
    chunk_duration_secs: u32,
    whisper_model: Option<PathBuf>,
}

impl AudioConnector {
    pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let mic_enabled = settings.get("mic")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let system_enabled = settings.get("system")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let mic_device = settings.get("mic_device")
            .and_then(|v| v.as_str())
            .map(String::from);
        let system_device = settings.get("system_device")
            .and_then(|v| v.as_str())
            .map(String::from);
        let chunk_duration_secs = settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .map(|n| n as u32)
            .unwrap_or(60);
        let whisper_model = settings.get("whisper_model")
            .and_then(|v| v.as_str())
            .map(|s| {
                // Expand ~
                if let Some(stripped) = s.strip_prefix("~/") {
                    if let Some(home) = dirs::home_dir() {
                        return home.join(stripped);
                    }
                }
                PathBuf::from(s)
            });

        Ok(Self {
            mic_enabled,
            system_enabled,
            mic_device,
            system_device,
            chunk_duration_secs,
            whisper_model,
        })
    }
}

impl Connector for AudioConnector {
    fn name(&self) -> &str {
        "audio"
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        let mut sources: Vec<Box<dyn CaptureSource>> = Vec::new();

        if self.mic_enabled {
            let mut mic_settings = HashMap::new();
            if let Some(ref d) = self.mic_device {
                mic_settings.insert("device".into(), toml::Value::String(d.clone()));
            }
            mic_settings.insert("chunk_duration_secs".into(),
                toml::Value::Integer(self.chunk_duration_secs as i64));
            sources.push(Box::new(
                alvum_capture_audio::source::AudioMicSource::from_config(
                    &alvum_core::config::CaptureSourceConfig {
                        enabled: true,
                        settings: mic_settings,
                    }
                )
            ));
        }

        if self.system_enabled {
            let mut sys_settings = HashMap::new();
            if let Some(ref d) = self.system_device {
                sys_settings.insert("device".into(), toml::Value::String(d.clone()));
            }
            sources.push(Box::new(
                alvum_capture_audio::source::AudioSystemSource::from_config(
                    &alvum_core::config::CaptureSourceConfig {
                        enabled: true,
                        settings: sys_settings,
                    }
                )
            ));
        }

        sources
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        match &self.whisper_model {
            Some(path) => vec![Box::new(WhisperProcessor::new(path.clone()))],
            None => vec![],
        }
    }
}
```

NOTE: This step requires that `alvum-core` has a `dirs` dependency accessible, or we add it to the connector. Add `dirs = "6"` to the Cargo.toml if not already pulled in.

- [ ] **Step 5: Add `dirs = "6"` to `crates/alvum-connector-audio/Cargo.toml` dependencies**

- [ ] **Step 6: Run tests**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-connector-audio
```

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/alvum-connector-audio/ && git commit -m "feat(connector): add alvum-connector-audio bundling audio capture + whisper"
```

---

### Task 4: Screen Connector Crate

Create `alvum-connector-screen` that bundles `ScreenSource` + a vision/OCR processor selected by config.

**Files:**
- Create: `crates/alvum-connector-screen/Cargo.toml`
- Create: `crates/alvum-connector-screen/src/lib.rs`
- Create: `crates/alvum-connector-screen/src/processor.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create Cargo.toml**

```toml
[package]
name = "alvum-connector-screen"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
alvum-capture-screen = { path = "../alvum-capture-screen" }
alvum-processor-screen = { path = "../alvum-processor-screen" }
alvum-pipeline = { path = "../alvum-pipeline" }
anyhow.workspace = true
async-trait = "0.1"
tracing.workspace = true
serde.workspace = true
serde_json.workspace = true
toml = "0.8"
```

- [ ] **Step 2: Add workspace member**

Add `"crates/alvum-connector-screen"` to root Cargo.toml.

- [ ] **Step 3: Create `src/processor.rs`**

```rust
//! VisionProcessor / OcrProcessor — implements the Processor trait for screen DataRefs.

use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use alvum_pipeline::llm::LlmProvider;
use alvum_processor_screen::VisionMode;
use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

pub struct ScreenProcessor {
    mode: VisionMode,
    provider: Option<Arc<dyn LlmProvider>>,
}

impl ScreenProcessor {
    pub fn new(mode: VisionMode, provider: Option<Arc<dyn LlmProvider>>) -> Self {
        Self { mode, provider }
    }
}

#[async_trait]
impl Processor for ScreenProcessor {
    fn name(&self) -> &str {
        match self.mode {
            VisionMode::Local => "vision-local",
            VisionMode::Api => "vision-api",
            VisionMode::Ocr => "ocr",
            VisionMode::Off => "screen-off",
        }
    }

    fn handles(&self) -> Vec<String> {
        vec!["screen".into()]
    }

    async fn process(
        &self,
        data_refs: &[DataRef],
        capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        match self.mode {
            VisionMode::Local | VisionMode::Api => {
                let provider = self.provider.as_ref()
                    .ok_or_else(|| anyhow::anyhow!("vision mode requires LlmProvider"))?;
                alvum_processor_screen::describe::process_screen_data_refs(
                    provider.as_ref(),
                    data_refs,
                    capture_dir,
                ).await
            }
            VisionMode::Ocr => {
                alvum_processor_screen::ocr::process_screen_data_refs_ocr(
                    data_refs,
                    capture_dir,
                )
            }
            VisionMode::Off => Ok(vec![]),
        }
    }
}
```

- [ ] **Step 4: Create `src/lib.rs`**

```rust
//! ScreenConnector — user-facing plugin bundling screen capture + vision/OCR.

pub mod processor;

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::processor::Processor;
use alvum_pipeline::llm::LlmProvider;
use alvum_processor_screen::VisionMode;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

use processor::ScreenProcessor;

pub struct ScreenConnector {
    idle_interval_secs: u64,
    vision_mode: VisionMode,
    provider: Option<Arc<dyn LlmProvider>>,
}

impl ScreenConnector {
    pub fn from_config(
        settings: &HashMap<String, toml::Value>,
        provider: Option<Arc<dyn LlmProvider>>,
    ) -> Result<Self> {
        let idle_interval_secs = settings.get("idle_interval_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(30) as u64;
        let vision_str = settings.get("vision")
            .and_then(|v| v.as_str())
            .unwrap_or("local");
        let vision_mode = VisionMode::from_str(vision_str)
            .unwrap_or(VisionMode::Local);

        Ok(Self {
            idle_interval_secs,
            vision_mode,
            provider,
        })
    }
}

impl Connector for ScreenConnector {
    fn name(&self) -> &str {
        "screen"
    }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        let mut settings = HashMap::new();
        settings.insert("idle_interval_secs".into(),
            toml::Value::Integer(self.idle_interval_secs as i64));
        vec![Box::new(
            alvum_capture_screen::source::ScreenSource::from_config(&settings)
        )]
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(ScreenProcessor::new(
            self.vision_mode,
            self.provider.clone(),
        ))]
    }
}
```

- [ ] **Step 5: Add `VisionMode: Copy + Clone` derives if not already present**

Check `crates/alvum-processor-screen/src/lib.rs`. `VisionMode` should already derive `Copy, Clone`. If not, add them.

- [ ] **Step 6: Build**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-connector-screen
```

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/alvum-connector-screen/ && git commit -m "feat(connector): add alvum-connector-screen bundling screen capture + vision/OCR"
```

---

### Task 5: Claude Code Connector — add Connector impl

The existing `alvum-connector-claude` crate just has a parser. Add a `ClaudeCodeConnector` that implements the trait.

**Files:**
- Modify: `crates/alvum-connector-claude/Cargo.toml`
- Modify: `crates/alvum-connector-claude/src/lib.rs`
- Create: `crates/alvum-connector-claude/src/connector.rs`

- [ ] **Step 1: Add dependencies**

In `crates/alvum-connector-claude/Cargo.toml`, add:
```toml
async-trait = "0.1"
toml = "0.8"
tracing.workspace = true
chrono.workspace = true
```
(Some may already be present.)

- [ ] **Step 2: Create `src/connector.rs`**

```rust
//! ClaudeCodeConnector — reads Claude Code session files (no capture daemon).

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::info;

pub struct ClaudeCodeConnector {
    session_dir: PathBuf,
    before_ts: Option<chrono::DateTime<chrono::Utc>>,
}

impl ClaudeCodeConnector {
    pub fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> {
        let session_dir = settings.get("session_dir")
            .and_then(|v| v.as_str())
            .map(|s| {
                if let Some(stripped) = s.strip_prefix("~/") {
                    if let Some(home) = dirs::home_dir() {
                        return home.join(stripped);
                    }
                }
                PathBuf::from(s)
            })
            .unwrap_or_else(|| {
                dirs::home_dir()
                    .map(|h| h.join(".claude/projects"))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        Ok(Self {
            session_dir,
            before_ts: None,
        })
    }

    pub fn with_before(mut self, before: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        self.before_ts = before;
        self
    }
}

impl Connector for ClaudeCodeConnector {
    fn name(&self) -> &str { "claude-code" }

    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        vec![] // no capture daemon — reads existing sessions
    }

    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(ClaudeCodeProcessor {
            session_dir: self.session_dir.clone(),
            before_ts: self.before_ts,
        })]
    }
}

struct ClaudeCodeProcessor {
    session_dir: PathBuf,
    before_ts: Option<chrono::DateTime<chrono::Utc>>,
}

#[async_trait]
impl Processor for ClaudeCodeProcessor {
    fn name(&self) -> &str { "claude-code-parser" }

    fn handles(&self) -> Vec<String> {
        vec!["claude-code".into()]
    }

    async fn process(
        &self,
        _data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> Result<Vec<Observation>> {
        // Claude-code is a one-shot connector: ignore data_refs, read session files directly
        if !self.session_dir.exists() {
            return Ok(vec![]);
        }

        info!(dir = %self.session_dir.display(), "scanning claude sessions");

        let mut observations = Vec::new();
        for entry in walkdir::WalkDir::new(&self.session_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    let session_obs = crate::parser::parse_session_filtered(path, self.before_ts)?;
                    observations.extend(session_obs);
                }
            }
        }

        info!(obs = observations.len(), "loaded claude observations");
        Ok(observations)
    }
}
```

- [ ] **Step 3: Add `walkdir` to dependencies**

```toml
walkdir = "2"
dirs = "6"
```

- [ ] **Step 4: Update lib.rs**

In `crates/alvum-connector-claude/src/lib.rs`, add:
```rust
pub mod connector;
pub use connector::ClaudeCodeConnector;
```

- [ ] **Step 5: Build**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-connector-claude
```

- [ ] **Step 6: Commit**

```bash
git add crates/alvum-connector-claude/ && git commit -m "feat(connector): add ClaudeCodeConnector implementing Connector trait"
```

---

### Task 6: Extract Library Function

Move the pipeline logic out of `cmd_extract` into a reusable library function in `alvum-pipeline`.

**Files:**
- Create: `crates/alvum-pipeline/src/extract.rs`
- Modify: `crates/alvum-pipeline/src/lib.rs`
- Modify: `crates/alvum-pipeline/Cargo.toml`

- [ ] **Step 1: Add dependencies**

In `crates/alvum-pipeline/Cargo.toml`, add:
```toml
alvum-episode = { path = "../alvum-episode" }
alvum-knowledge = { path = "../alvum-knowledge" }
```

- [ ] **Step 2: Create `src/extract.rs`**

```rust
//! extract_and_pipeline — the full observation → decision pipeline as a library function.
//!
//! Takes a set of connectors, runs their processors, does episodic alignment,
//! extracts decisions, links causally, generates briefing, and updates the
//! knowledge corpus. Returns the complete extraction result.

use alvum_core::config::AlvumConfig;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::decision::ExtractionResult;
use alvum_core::observation::Observation;
use alvum_core::storage;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

use crate::llm::LlmProvider;

pub struct ExtractConfig {
    pub capture_dir: PathBuf,
    pub output_dir: PathBuf,
    pub relevance_threshold: f32,
}

pub struct ExtractOutput {
    pub observations: Vec<Observation>,
    pub threading: alvum_episode::types::ThreadingResult,
    pub result: ExtractionResult,
}

/// Run the full extraction pipeline for a set of connectors.
pub async fn extract_and_pipeline(
    connectors: Vec<Box<dyn Connector>>,
    provider: Arc<dyn LlmProvider>,
    config: ExtractConfig,
) -> Result<ExtractOutput> {
    std::fs::create_dir_all(&config.output_dir)?;

    let mut all_observations: Vec<Observation> = Vec::new();

    // 1. Each connector's processors produce Observations from its DataRefs
    for connector in &connectors {
        let connector_name = connector.name();
        let processors = connector.processors();

        for processor in processors {
            let handles = processor.handles();
            info!(connector = connector_name, processor = processor.name(), handles = ?handles, "running processor");

            // Gather DataRefs for this processor's handles from capture directory
            let data_refs = gather_data_refs_for_handles(&config.capture_dir, &handles)?;

            if data_refs.is_empty() {
                info!(processor = processor.name(), "no data refs found, skipping");
                continue;
            }

            match processor.process(&data_refs, &config.capture_dir).await {
                Ok(obs) => {
                    info!(processor = processor.name(), count = obs.len(), "processor produced observations");
                    all_observations.extend(obs);
                }
                Err(e) => {
                    warn!(processor = processor.name(), error = %e, "processor failed, continuing");
                }
            }
        }
    }

    // 2. Save unified transcript
    let transcript_path = config.output_dir.join("transcript.jsonl");
    // Truncate and rewrite
    let _ = std::fs::remove_file(&transcript_path);
    for obs in &all_observations {
        storage::append_jsonl(&transcript_path, obs)?;
    }
    info!(path = %transcript_path.display(), count = all_observations.len(), "saved transcript");

    if all_observations.is_empty() {
        anyhow::bail!("no observations produced by any connector");
    }

    // 3. Load knowledge corpus for context-aware threading
    let knowledge_dir = config.output_dir.join("knowledge");
    let corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

    // 4. Episodic alignment
    info!("running episodic alignment");
    let threading = alvum_episode::threading::align_episodes(
        provider.as_ref(),
        &all_observations,
        chrono::Duration::minutes(5),
        Some(&corpus),
    ).await?;

    let threads_path = config.output_dir.join("threads.json");
    std::fs::write(&threads_path, serde_json::to_string_pretty(&threading)?)?;

    // 5. Filter relevant threads
    let relevant: Vec<&alvum_episode::types::ContextThread> = threading.threads.iter()
        .filter(|t| t.is_relevant(config.relevance_threshold))
        .collect();

    let relevant_observations: Vec<Observation> = relevant.iter()
        .flat_map(|t| t.observations.clone())
        .collect();

    // 6. Extract decisions
    info!(count = relevant_observations.len(), "extracting decisions");
    let mut decisions = crate::distill::extract_decisions(provider.as_ref(), &relevant_observations).await?;

    // 7. Link causally
    if !decisions.is_empty() {
        crate::causal::link_decisions(provider.as_ref(), &mut decisions).await?;
    }

    // 8. Briefing
    let briefing = if !decisions.is_empty() {
        crate::briefing::generate_briefing(provider.as_ref(), &decisions).await?
    } else {
        String::from("No decisions found.")
    };

    // 9. Save decisions + briefing
    let decisions_path = config.output_dir.join("decisions.jsonl");
    let _ = std::fs::remove_file(&decisions_path);
    for dec in &decisions {
        storage::append_jsonl(&decisions_path, dec)?;
    }
    std::fs::write(config.output_dir.join("briefing.md"), &briefing)?;

    let result = ExtractionResult {
        session_id: "cross-source".into(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        decisions: decisions.clone(),
        briefing: briefing.clone(),
    };
    std::fs::write(
        config.output_dir.join("extraction.json"),
        serde_json::to_string_pretty(&result)?,
    )?;

    // 10. Knowledge extraction (best-effort — don't fail pipeline on this)
    match alvum_knowledge::extract::extract_knowledge(provider.as_ref(), &relevant_observations, &corpus).await {
        Ok(new_knowledge) => {
            let mut updated = corpus;
            updated.merge(new_knowledge);
            if let Err(e) = alvum_knowledge::store::save(&knowledge_dir, &updated) {
                warn!(error = %e, "failed to save knowledge corpus");
            }
        }
        Err(e) => warn!(error = %e, "knowledge extraction failed, skipping"),
    }

    Ok(ExtractOutput {
        observations: all_observations,
        threading,
        result,
    })
}

/// Gather DataRefs from the capture directory for the given handles.
/// Handles are source names (e.g., "audio-mic", "screen") or MIME types.
fn gather_data_refs_for_handles(
    capture_dir: &Path,
    handles: &[String],
) -> Result<Vec<DataRef>> {
    let mut data_refs = Vec::new();

    for handle in handles {
        match handle.as_str() {
            "audio-mic" => {
                let dir = capture_dir.join("audio").join("mic");
                data_refs.extend(scan_audio_dir(&dir, "audio-mic")?);
            }
            "audio-system" => {
                let dir = capture_dir.join("audio").join("system");
                data_refs.extend(scan_audio_dir(&dir, "audio-system")?);
            }
            "audio-wearable" => {
                let dir = capture_dir.join("audio").join("wearable");
                data_refs.extend(scan_audio_dir(&dir, "audio-wearable")?);
            }
            "screen" => {
                let captures_path = capture_dir.join("screen").join("captures.jsonl");
                if captures_path.exists() {
                    let refs: Vec<DataRef> = storage::read_jsonl(&captures_path)
                        .context("failed to read screen captures.jsonl")?;
                    data_refs.extend(refs);
                }
            }
            "claude-code" => {
                // ClaudeCodeProcessor handles this directly, ignoring data_refs
                // Emit a single dummy ref so the processor runs
                data_refs.push(DataRef {
                    ts: chrono::Utc::now(),
                    source: "claude-code".into(),
                    path: "".into(),
                    mime: "application/x-jsonl".into(),
                    metadata: None,
                });
            }
            other => {
                warn!(handle = other, "unknown handle, no DataRefs gathered");
            }
        }
    }

    Ok(data_refs)
}

fn scan_audio_dir(dir: &Path, source: &str) -> Result<Vec<DataRef>> {
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut refs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "wav" || ext == "opus" {
            let mime = if ext == "wav" { "audio/wav" } else { "audio/opus" };
            refs.push(DataRef {
                ts: chrono::Utc::now(),
                source: source.into(),
                path: path.to_string_lossy().into_owned(),
                mime: mime.into(),
                metadata: None,
            });
        }
    }
    Ok(refs)
}

/// Config-driven construction of connectors from AlvumConfig.
/// Returns enabled connectors, skipping those that fail to construct.
pub fn connectors_from_config(
    config: &AlvumConfig,
    provider: Arc<dyn LlmProvider>,
) -> Vec<Box<dyn Connector>> {
    let mut connectors: Vec<Box<dyn Connector>> = Vec::new();

    for (name, cfg) in &config.connectors {
        if !cfg.enabled {
            continue;
        }

        match name.as_str() {
            "audio" => {
                match alvum_connector_audio::AudioConnector::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => warn!(name, error = %e, "failed to build audio connector"),
                }
            }
            "screen" => {
                match alvum_connector_screen::ScreenConnector::from_config(&cfg.settings, Some(provider.clone())) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => warn!(name, error = %e, "failed to build screen connector"),
                }
            }
            "claude-code" => {
                match alvum_connector_claude::ClaudeCodeConnector::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => warn!(name, error = %e, "failed to build claude-code connector"),
                }
            }
            other => {
                warn!(name = other, "unknown connector, skipping");
            }
        }
    }

    connectors
}
```

- [ ] **Step 3: Update `src/lib.rs`**

Add `pub mod extract;` to `crates/alvum-pipeline/src/lib.rs`.

NOTE: This creates a dependency from alvum-pipeline back to alvum-connector-* crates. This inverts the module tree. To avoid cyclic deps, the `connectors_from_config` function should live in the CLI, not in alvum-pipeline. Let me revise:

- [ ] **Step 4: Move `connectors_from_config` to CLI**

Remove the `connectors_from_config` function from `src/extract.rs`. It belongs in the CLI where all connector crates are already linked.

- [ ] **Step 5: Remove `alvum-connector-*` deps from alvum-pipeline**

They're not needed — `extract_and_pipeline` takes `Vec<Box<dyn Connector>>` as a parameter.

- [ ] **Step 6: Build**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-pipeline
```

- [ ] **Step 7: Commit**

```bash
git add crates/alvum-pipeline/ && git commit -m "feat(pipeline): add extract_and_pipeline library function for generic connector-driven extraction"
```

---

### Task 7: Refactor cmd_extract in CLI

Replace the 350-line `cmd_extract` with a thin dispatcher that loads connectors from config and calls the library function.

**Files:**
- Modify: `crates/alvum-cli/Cargo.toml`
- Modify: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Add connector deps**

In `crates/alvum-cli/Cargo.toml`, add:
```toml
alvum-connector-audio = { path = "../alvum-connector-audio" }
alvum-connector-screen = { path = "../alvum-connector-screen" }
```

(`alvum-connector-claude` should already be there.)

- [ ] **Step 2: Create `connectors_from_config` helper in main.rs**

After the imports, before main:

```rust
fn connectors_from_config(
    config: &alvum_core::config::AlvumConfig,
    provider: std::sync::Arc<dyn alvum_pipeline::llm::LlmProvider>,
) -> Vec<Box<dyn alvum_core::connector::Connector>> {
    let mut connectors: Vec<Box<dyn alvum_core::connector::Connector>> = Vec::new();

    for (name, cfg) in &config.connectors {
        if !cfg.enabled {
            continue;
        }

        match name.as_str() {
            "audio" => {
                match alvum_connector_audio::AudioConnector::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "screen" => {
                match alvum_connector_screen::ScreenConnector::from_config(&cfg.settings, Some(provider.clone())) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            "claude-code" => {
                match alvum_connector_claude::ClaudeCodeConnector::from_config(&cfg.settings) {
                    Ok(c) => connectors.push(Box::new(c)),
                    Err(e) => tracing::warn!(name = %name, error = %e, "failed to build connector"),
                }
            }
            other => {
                tracing::warn!(name = %other, "unknown connector type, skipping");
            }
        }
    }

    connectors
}
```

- [ ] **Step 3: Replace `cmd_extract` body**

Replace the body of `cmd_extract` with:

```rust
async fn cmd_extract(
    _source: Option<String>,       // legacy, ignored
    _session: Option<PathBuf>,     // legacy, ignored
    output: PathBuf,
    provider_name: String,
    model: String,
    _before: Option<String>,       // legacy
    capture_dir: Option<PathBuf>,
    _whisper_model: Option<PathBuf>, // now read from connector config
    relevance_threshold: f32,
    _vision: Option<String>,       // now read from connector config
) -> Result<()> {
    let capture_dir = capture_dir.context("--capture-dir required")?;
    let provider: std::sync::Arc<dyn alvum_pipeline::llm::LlmProvider> =
        alvum_pipeline::llm::create_provider(&provider_name, &model)?.into();

    let config = alvum_core::config::AlvumConfig::load()?;
    let connectors = connectors_from_config(&config, provider.clone());

    if connectors.is_empty() {
        println!("No connectors enabled. Check config.");
        return Ok(());
    }

    let names: Vec<&str> = connectors.iter().map(|c| c.name()).collect();
    println!("Running connectors: {}", names.join(", "));

    let extract_config = alvum_pipeline::extract::ExtractConfig {
        capture_dir,
        output_dir: output.clone(),
        relevance_threshold,
    };

    let result = alvum_pipeline::extract::extract_and_pipeline(
        connectors,
        provider,
        extract_config,
    ).await?;

    println!("\nExtracted {} decisions from {} observations across {} threads.",
        result.result.decisions.len(),
        result.observations.len(),
        result.threading.threads.len(),
    );
    println!("\nOutput: {}", output.display());
    println!("\n{}", "=".repeat(60));
    println!("{}", result.result.briefing);

    Ok(())
}
```

- [ ] **Step 4: Convert Box<dyn LlmProvider> to Arc where needed**

`create_provider` returns `Box<dyn LlmProvider>`. We need `Arc` for cloning. Either:
- Change `create_provider` to return `Arc`, or
- Convert: `let provider: Arc<dyn LlmProvider> = Arc::from(create_provider(...)?);`

Use the second approach to avoid breaking other callers.

- [ ] **Step 5: Build and fix any type errors**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-cli
```

- [ ] **Step 6: Test end-to-end**

```bash
alvum extract --capture-dir ./capture/2026-04-12 --output ./output/2026-04-12
```

Expected: full pipeline runs (audio + screen + claude-code) producing briefing, decisions, etc.

- [ ] **Step 7: Commit**

```bash
git add crates/alvum-cli/ && git commit -m "refactor(cli): replace cmd_extract body with extract_and_pipeline library call"
```

---

### Task 8: Final Verification + Cleanup

- [ ] **Step 1: Run full workspace test**

```bash
export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace
```

All tests must pass.

- [ ] **Step 2: Reinstall binary**

```bash
cargo install --path crates/alvum-cli
```

- [ ] **Step 3: Test the refactored pipeline**

```bash
alvum extract --capture-dir ./capture/2026-04-12 --output ./output/2026-04-12-refactor
```

Should produce equivalent output to the pre-refactor pipeline.

- [ ] **Step 4: Commit any cleanup**

```bash
git add -A && git commit -m "refactor: final cleanup after connector refactor" || echo "no changes"
```

---

## Verification Checklist

After all tasks complete:

- [ ] `cargo build --workspace` clean, zero warnings
- [ ] `cargo test --workspace` all passing
- [ ] `alvum extract --capture-dir X --output Y` produces equivalent pipeline output
- [ ] New connector can be added by: (a) creating a new crate, (b) implementing Connector, (c) adding one match arm in CLI
- [ ] No processor-specific logic in `cmd_extract` body

## Known limitations (out of scope)

- The external executable protocol from the original spec is still not built — this refactor only unifies the in-process Rust model
- `CronCreate`/scheduler is still missing — extract still runs manually
- The Connector trait has no `enabled()` method — enable state comes from config
