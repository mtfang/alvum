# Capture Orchestration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify capture sources under a single `alvum capture` command with config-driven source management and a `CaptureSource` trait.

**Architecture:** New `CaptureSource` trait in alvum-core. Config gains `[capture.*]` and `[processors.*]` sections. Audio and screen crates implement the trait. CLI orchestrator spawns all enabled sources concurrently with shared shutdown signal.

**Tech Stack:** Rust, `async-trait`, `tokio::sync::watch` (shutdown), existing alvum crates.

---

## File Structure

```
alvum/
├── crates/
│   ├── alvum-core/                    (modified)
│   │   ├── Cargo.toml                 add tokio + async-trait deps
│   │   └── src/
│   │       ├── lib.rs                 add pub mod capture
│   │       ├── capture.rs             NEW — CaptureSource trait
│   │       └── config.rs              add capture/processors maps, migration
│   ├── alvum-capture-audio/           (modified)
│   │   ├── Cargo.toml                 add alvum-core + async-trait deps
│   │   └── src/
│   │       ├── lib.rs                 add pub mod source
│   │       └── source.rs             NEW — AudioMicSource, AudioSystemSource
│   ├── alvum-capture-screen/          (modified)
│   │   ├── Cargo.toml                 add async-trait dep
│   │   └── src/
│   │       ├── lib.rs                 add pub mod source
│   │       └── source.rs             NEW — ScreenSource
│   └── alvum-cli/                     (modified)
│       └── src/
│           └── main.rs                replace Record+CaptureScreen with Capture, update Extract
```

---

### Task 1: CaptureSource Trait + Config Update

Add the `CaptureSource` trait to alvum-core. Update `AlvumConfig` to support `[capture.*]` and `[processors.*]` sections alongside existing `[connectors.*]`. Add migration for old `[connectors.audio]`.

**Files:**
- Create: `crates/alvum-core/src/capture.rs`
- Modify: `crates/alvum-core/src/lib.rs`
- Modify: `crates/alvum-core/src/config.rs`
- Modify: `crates/alvum-core/Cargo.toml`

- [ ] **Step 1: Add tokio and async-trait dependencies to alvum-core**

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
tokio.workspace = true
async-trait = "0.1"
toml = "0.8"
dirs = "6"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `crates/alvum-core/src/capture.rs` with the CaptureSource trait**

```rust
//! Trait contract for capture sources — always-on daemons that write raw data
//! to the capture directory. Each source runs until the shutdown signal fires.

use anyhow::Result;
use std::path::Path;
use tokio::sync::watch;

/// A capture source that runs continuously and writes files to the capture directory.
/// Sources own a subdirectory under `capture_dir` (e.g., `audio/mic/`, `screen/`).
/// They must exit cleanly when the shutdown receiver transitions to `true`.
#[async_trait::async_trait]
pub trait CaptureSource: Send + Sync {
    /// Unique name matching the config key (e.g., "audio-mic", "screen").
    fn name(&self) -> &str;

    /// Run the capture loop. Blocks until shutdown signal fires or an error occurs.
    /// Implementations must flush any buffered data before returning.
    async fn run(&self, capture_dir: &Path, shutdown: watch::Receiver<bool>) -> Result<()>;
}
```

- [ ] **Step 3: Register the capture module in `crates/alvum-core/src/lib.rs`**

Add `pub mod capture;` after the existing module declarations:

```rust
//! Core domain types for alvum: data references, artifacts, observations, decisions,
//! and storage primitives.
//!
//! Data flows through three layers:
//! - [`data_ref::DataRef`] — what connectors produce (file pointers)
//! - [`artifact::Artifact`] — what processors produce (typed output layers)
//! - [`observation::Observation`] — what the pipeline consumes (text for LLM reasoning)

pub mod artifact;
pub mod capture;
pub mod config;
pub mod data_ref;
pub mod decision;
pub mod observation;
pub mod storage;
```

- [ ] **Step 4: Add `CaptureSourceConfig` and `ProcessorConfig` to config.rs**

Add these structs below the existing `ConnectorConfig`:

```rust
/// Configuration for a capture source (always-on daemon).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSourceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Source-specific settings as key-value pairs.
    #[serde(flatten)]
    pub settings: HashMap<String, toml::Value>,
}

/// Configuration for a processor (processing settings used during extract).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessorConfig {
    /// Processor-specific settings as key-value pairs.
    #[serde(flatten)]
    pub settings: HashMap<String, toml::Value>,
}
```

- [ ] **Step 5: Add capture and processors fields to AlvumConfig**

Update the `AlvumConfig` struct:

```rust
/// Top-level config structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlvumConfig {
    #[serde(default)]
    pub pipeline: PipelineConfig,
    #[serde(default)]
    pub connectors: HashMap<String, ConnectorConfig>,
    #[serde(default)]
    pub capture: HashMap<String, CaptureSourceConfig>,
    #[serde(default)]
    pub processors: HashMap<String, ProcessorConfig>,
}
```

- [ ] **Step 6: Add accessor methods for capture and processors on AlvumConfig**

Add these methods inside the existing `impl AlvumConfig` block, after `enabled_connectors()`:

```rust
    /// Get a capture source config by name. Returns None if not configured.
    pub fn capture_source(&self, name: &str) -> Option<&CaptureSourceConfig> {
        self.capture.get(name)
    }

    /// Get a capture source setting as a string.
    pub fn capture_setting(&self, source: &str, key: &str) -> Option<String> {
        self.capture.get(source)?
            .settings.get(key)?
            .as_str()
            .map(|s| s.to_string())
    }

    /// Get all enabled capture sources.
    pub fn enabled_capture_sources(&self) -> Vec<(&str, &CaptureSourceConfig)> {
        self.capture.iter()
            .filter(|(_, c)| c.enabled)
            .map(|(name, config)| (name.as_str(), config))
            .collect()
    }

    /// Get a processor config by name.
    pub fn processor(&self, name: &str) -> Option<&ProcessorConfig> {
        self.processors.get(name)
    }

    /// Get a processor setting as a string.
    pub fn processor_setting(&self, processor: &str, key: &str) -> Option<String> {
        self.processors.get(processor)?
            .settings.get(key)?
            .as_str()
            .map(|s| s.to_string())
    }
```

- [ ] **Step 7: Add config migration from `[connectors.audio]` to `[capture.*]`**

Add a `migrate` method on `AlvumConfig` and call it from `load()` and `load_from()`:

```rust
    /// Migrate deprecated config formats to current.
    /// - `[connectors.audio]` → `[capture.audio-mic]` + `[capture.audio-system]`
    fn migrate(&mut self) {
        if self.connectors.contains_key("audio") && self.capture.is_empty() {
            eprintln!(
                "warning: [connectors.audio] is deprecated, migrate to [capture.audio-mic] and [capture.audio-system]"
            );

            let audio_connector = self.connectors.get("audio").unwrap();

            // Create audio-mic capture source from the old connector
            let mut mic_settings = HashMap::new();
            mic_settings.insert("device".into(), toml::Value::String("default".into()));
            mic_settings.insert("chunk_duration_secs".into(), toml::Value::Integer(60));
            self.capture.insert("audio-mic".into(), CaptureSourceConfig {
                enabled: audio_connector.enabled,
                settings: mic_settings,
            });

            // Create audio-system capture source
            let mut sys_settings = HashMap::new();
            sys_settings.insert("device".into(), toml::Value::String("default".into()));
            self.capture.insert("audio-system".into(), CaptureSourceConfig {
                enabled: audio_connector.enabled,
                settings: sys_settings,
            });

            // Create screen capture source (not from connector, but add default)
            let mut screen_settings = HashMap::new();
            screen_settings.insert("idle_interval_secs".into(), toml::Value::Integer(30));
            self.capture.insert("screen".into(), CaptureSourceConfig {
                enabled: true,
                settings: screen_settings,
            });
        }
    }
```

Then update both load methods to call `migrate()` before returning:

```rust
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read config: {}", path.display()))?;
            let mut config: Self = toml::from_str(&content)
                .with_context(|| format!("failed to parse config: {}", path.display()))?;
            config.migrate();
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let mut config: Self = toml::from_str(&content)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;
        config.migrate();
        Ok(config)
    }
```

- [ ] **Step 8: Update `Default` impl for `AlvumConfig` to include capture and processors**

Replace the existing `Default` impl:

```rust
impl Default for AlvumConfig {
    fn default() -> Self {
        let mut connectors = HashMap::new();

        // Claude Code connector - enabled by default
        let mut claude_settings = HashMap::new();
        claude_settings.insert("session_dir".into(), toml::Value::String(
            dirs::home_dir()
                .map(|h| h.join(".claude/projects").to_string_lossy().into_owned())
                .unwrap_or_else(|| "~/.claude/projects".into())
        ));
        claude_settings.insert("auto_detect_latest".into(), toml::Value::Boolean(true));
        connectors.insert("claude-code".into(), ConnectorConfig {
            enabled: true,
            settings: claude_settings,
        });

        // Capture sources
        let mut capture = HashMap::new();

        let mut mic_settings = HashMap::new();
        mic_settings.insert("device".into(), toml::Value::String("default".into()));
        mic_settings.insert("chunk_duration_secs".into(), toml::Value::Integer(60));
        capture.insert("audio-mic".into(), CaptureSourceConfig {
            enabled: true,
            settings: mic_settings,
        });

        let mut sys_settings = HashMap::new();
        sys_settings.insert("device".into(), toml::Value::String("default".into()));
        capture.insert("audio-system".into(), CaptureSourceConfig {
            enabled: true,
            settings: sys_settings,
        });

        let mut screen_settings = HashMap::new();
        screen_settings.insert("idle_interval_secs".into(), toml::Value::Integer(30));
        capture.insert("screen".into(), CaptureSourceConfig {
            enabled: true,
            settings: screen_settings,
        });

        // Processors (empty by default — no model paths to assume)
        let processors = HashMap::new();

        Self {
            pipeline: PipelineConfig::default(),
            connectors,
            capture,
            processors,
        }
    }
}
```

- [ ] **Step 9: Update existing tests and add new ones**

Replace the test module in `crates/alvum-core/src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_claude_connector() {
        let config = AlvumConfig::default();
        assert!(config.connectors.contains_key("claude-code"));
        assert!(config.connector("claude-code").unwrap().enabled);
    }

    #[test]
    fn default_config_has_capture_sources() {
        let config = AlvumConfig::default();
        assert!(config.capture.contains_key("audio-mic"));
        assert!(config.capture.contains_key("audio-system"));
        assert!(config.capture.contains_key("screen"));
        assert!(config.capture_source("audio-mic").unwrap().enabled);
    }

    #[test]
    fn default_config_no_audio_connector() {
        // Old [connectors.audio] is removed from defaults
        let config = AlvumConfig::default();
        assert!(!config.connectors.contains_key("audio"));
    }

    #[test]
    fn roundtrip_toml() {
        let config = AlvumConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AlvumConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.pipeline.provider, "cli");
        assert!(parsed.connectors.contains_key("claude-code"));
        assert!(parsed.capture.contains_key("audio-mic"));
        assert!(parsed.capture.contains_key("screen"));
    }

    #[test]
    fn enabled_connectors_filters() {
        let mut config = AlvumConfig::default();
        // Only claude-code connector in defaults now
        config.connectors.get_mut("claude-code").unwrap().enabled = false;
        let enabled = config.enabled_connectors();
        assert_eq!(enabled.len(), 0);
    }

    #[test]
    fn enabled_capture_sources_filters() {
        let mut config = AlvumConfig::default();
        config.capture.get_mut("audio-system").unwrap().enabled = false;
        let enabled = config.enabled_capture_sources();
        assert_eq!(enabled.len(), 2);
        assert!(enabled.iter().any(|(name, _)| *name == "audio-mic"));
        assert!(enabled.iter().any(|(name, _)| *name == "screen"));
    }

    #[test]
    fn capture_setting_returns_value() {
        let config = AlvumConfig::default();
        let device = config.capture_setting("audio-mic", "device");
        assert_eq!(device, Some("default".into()));
    }

    #[test]
    fn missing_capture_source_returns_none() {
        let config = AlvumConfig::default();
        assert!(config.capture_source("nonexistent").is_none());
    }

    #[test]
    fn processor_setting_returns_value() {
        let mut config = AlvumConfig::default();
        let mut audio_proc = HashMap::new();
        audio_proc.insert("whisper_model".into(), toml::Value::String("/path/to/model.bin".into()));
        config.processors.insert("audio".into(), ProcessorConfig {
            settings: audio_proc,
        });
        assert_eq!(
            config.processor_setting("audio", "whisper_model"),
            Some("/path/to/model.bin".into())
        );
    }

    #[test]
    fn migration_from_old_audio_connector() {
        // Simulate old config with [connectors.audio]
        let toml_str = r#"
[pipeline]
provider = "cli"
model = "claude-sonnet-4-6"
output_dir = "output"

[connectors.audio]
enabled = true
capture_dir = "capture"

[connectors.claude-code]
enabled = true
session_dir = "~/.claude/projects"
"#;
        let config: AlvumConfig = toml::from_str(toml_str).unwrap();
        // Before migration, capture is empty
        assert!(config.capture.is_empty());

        // After migration via load path
        let mut config = config;
        config.migrate();
        assert!(config.capture.contains_key("audio-mic"));
        assert!(config.capture.contains_key("audio-system"));
        assert!(config.capture.contains_key("screen"));
        assert!(config.capture_source("audio-mic").unwrap().enabled);
    }

    #[test]
    fn migration_skipped_when_capture_already_configured() {
        let toml_str = r#"
[pipeline]
provider = "cli"

[connectors.audio]
enabled = true
capture_dir = "capture"

[capture.audio-mic]
enabled = true
device = "Rode NT-USB"
chunk_duration_secs = 120
"#;
        let mut config: AlvumConfig = toml::from_str(toml_str).unwrap();
        config.migrate();
        // Migration should not overwrite existing capture config
        assert_eq!(
            config.capture_setting("audio-mic", "device"),
            Some("Rode NT-USB".into())
        );
        // Should not have added audio-system (migration skipped entirely)
        assert!(!config.capture.contains_key("audio-system"));
    }
}
```

- [ ] **Step 10: Verify Task 1 compiles and tests pass**

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-core
```

- [ ] **Step 11: Commit Task 1**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-core/src/capture.rs crates/alvum-core/src/lib.rs crates/alvum-core/src/config.rs crates/alvum-core/Cargo.toml && git commit -m "feat: add CaptureSource trait and config sections for capture orchestration"
```

---

### Task 2: AudioMicSource + AudioSystemSource

Add `alvum-core` as a dependency to `alvum-capture-audio`. Create two `CaptureSource` implementations that wrap the existing `capture::start_capture` + `AudioEncoder` logic. The existing `Recorder` struct stays untouched.

**Files:**
- Modify: `crates/alvum-capture-audio/Cargo.toml`
- Create: `crates/alvum-capture-audio/src/source.rs`
- Modify: `crates/alvum-capture-audio/src/lib.rs`

- [ ] **Step 1: Add alvum-core and async-trait dependencies**

```toml
# crates/alvum-capture-audio/Cargo.toml
[package]
name = "alvum-capture-audio"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
async-trait = "0.1"
cpal = "0.17"
anyhow.workspace = true
tracing.workspace = true
tokio.workspace = true
chrono.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `crates/alvum-capture-audio/src/source.rs`**

```rust
//! CaptureSource implementations for audio: mic and system audio.
//! Each source independently manages one audio stream + encoder.

use alvum_core::capture::CaptureSource;
use alvum_core::config::CaptureSourceConfig;
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tracing::info;

use crate::capture::{self, SampleCallback, SAMPLE_RATE};
use crate::devices;
use crate::encoder::AudioEncoder;
use crate::recorder::make_chunked_callback;

/// Captures microphone audio. Reads `device` and `chunk_duration_secs` from config.
pub struct AudioMicSource {
    device_name: Option<String>,
    chunk_duration_secs: u32,
}

impl AudioMicSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let device_name = config.settings.get("device")
            .and_then(|v| v.as_str())
            .filter(|s| *s != "default")
            .map(|s| s.to_string());

        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;

        Self { device_name, chunk_duration_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for AudioMicSource {
    fn name(&self) -> &str {
        "audio-mic"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let mic_dir = capture_dir.join("audio").join("mic");
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let device = devices::get_input_device(self.device_name.as_deref())
            .context("failed to get mic device")?;

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(mic_dir, SAMPLE_RATE)?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "mic".into());
        let stream = capture::start_capture(&device, "mic", callback)?;

        info!("audio-mic source started");

        // Hold stream alive until shutdown
        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        // Flush remaining audio data
        drop(stream);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }

        info!("audio-mic source stopped");
        Ok(())
    }
}

/// Captures system audio output. Reads `device` from config.
/// Gracefully degrades if system audio device is not available.
pub struct AudioSystemSource {
    device_name: Option<String>,
    chunk_duration_secs: u32,
}

impl AudioSystemSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let device_name = config.settings.get("device")
            .and_then(|v| v.as_str())
            .filter(|s| *s != "default")
            .map(|s| s.to_string());

        let chunk_duration_secs = config.settings.get("chunk_duration_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(60) as u32;

        Self { device_name, chunk_duration_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for AudioSystemSource {
    fn name(&self) -> &str {
        "audio-system"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let sys_dir = capture_dir.join("audio").join("system");
        let samples_per_chunk = SAMPLE_RATE as usize * self.chunk_duration_secs as usize;

        let device = match devices::get_output_device(self.device_name.as_deref()) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "system audio device not found, source will not run");
                // Wait for shutdown instead of returning an error — other sources keep running
                while !*shutdown.borrow_and_update() {
                    if shutdown.changed().await.is_err() {
                        break;
                    }
                }
                return Ok(());
            }
        };

        let encoder = Arc::new(Mutex::new(AudioEncoder::new(sys_dir, SAMPLE_RATE)?));
        let callback = make_chunked_callback(encoder.clone(), samples_per_chunk, "system".into());

        let stream = match capture::start_capture(&device, "system", callback) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "system audio capture not available, source will not run");
                while !*shutdown.borrow_and_update() {
                    if shutdown.changed().await.is_err() {
                        break;
                    }
                }
                return Ok(());
            }
        };

        info!("audio-system source started");

        while !*shutdown.borrow_and_update() {
            if shutdown.changed().await.is_err() {
                break;
            }
        }

        drop(stream);
        if let Ok(mut enc) = encoder.lock() {
            let _ = enc.flush_segment();
        }

        info!("audio-system source stopped");
        Ok(())
    }
}
```

- [ ] **Step 3: Make `make_chunked_callback` public in recorder.rs**

In `crates/alvum-capture-audio/src/recorder.rs`, change the visibility of `make_chunked_callback` from private to `pub(crate)`:

```rust
/// Create a callback that writes audio in fixed-length chunks.
/// No VAD — every sample is recorded. Chunks are flushed every `samples_per_chunk` samples.
pub(crate) fn make_chunked_callback(
```

- [ ] **Step 4: Register the source module in lib.rs**

```rust
//! Audio capture daemon: records microphone and system audio in fixed-length chunks.
//!
//! Captures audio from configurable input/output devices, encodes as Opus, and
//! writes fixed-length chunk files to the capture directory. No VAD — every
//! sample is recorded. VAD and speech detection live in the processor layer.

pub mod devices;
pub mod capture;
pub mod encoder;
pub mod recorder;
pub mod source;
```

- [ ] **Step 5: Verify Task 2 compiles**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-capture-audio
```

- [ ] **Step 6: Commit Task 2**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-capture-audio/ && git commit -m "feat: add AudioMicSource and AudioSystemSource implementing CaptureSource"
```

---

### Task 3: ScreenSource

Implement `CaptureSource` for screen capture. Wraps the existing `daemon::run()` logic. The key difference: instead of blocking until the mpsc channel closes, the source checks the `watch` shutdown signal to exit cleanly.

**Files:**
- Modify: `crates/alvum-capture-screen/Cargo.toml`
- Create: `crates/alvum-capture-screen/src/source.rs`
- Modify: `crates/alvum-capture-screen/src/lib.rs`

- [ ] **Step 1: Add async-trait dependency**

```toml
# crates/alvum-capture-screen/Cargo.toml
[package]
name = "alvum-capture-screen"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
async-trait = "0.1"
image = { version = "0.25", default-features = false, features = ["png"] }
core-graphics = "0.24"
core-foundation = "0.10"
anyhow.workspace = true
tokio.workspace = true
tracing.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Create `crates/alvum-capture-screen/src/source.rs`**

```rust
//! CaptureSource implementation for screen capture.
//! Wraps the existing trigger + screenshot + writer pipeline, adding shutdown support.

use alvum_core::capture::CaptureSource;
use alvum_core::config::CaptureSourceConfig;
use anyhow::{bail, Context, Result};
use std::path::Path;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::screenshot;
use crate::trigger;
use crate::writer::ScreenWriter;

/// Captures screenshots of the active window on app focus changes and idle timer.
/// Reads `idle_interval_secs` from config (not yet passed to trigger — future enhancement).
pub struct ScreenSource {
    _idle_interval_secs: u32,
}

impl ScreenSource {
    pub fn from_config(config: &CaptureSourceConfig) -> Self {
        let idle_interval_secs = config.settings.get("idle_interval_secs")
            .and_then(|v| v.as_integer())
            .unwrap_or(30) as u32;

        Self { _idle_interval_secs: idle_interval_secs }
    }
}

#[async_trait::async_trait]
impl CaptureSource for ScreenSource {
    fn name(&self) -> &str {
        "screen"
    }

    async fn run(&self, capture_dir: &Path, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        // Check Screen Recording permission before starting
        match screenshot::check_screen_recording_permission() {
            Ok(true) => info!("Screen Recording permission verified"),
            Ok(false) => {
                let _ = std::process::Command::new("open")
                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
                    .spawn();

                bail!(
                    "Screen Recording permission not granted.\n\n\
                     alvum needs Screen Recording access to capture window screenshots.\n\
                     Opening System Settings > Privacy & Security > Screen Recording...\n\
                     Grant permission, then restart alvum capture."
                );
            }
            Err(e) => {
                warn!(error = %e, "could not verify Screen Recording permission, proceeding anyway");
            }
        }

        let writer = ScreenWriter::new(capture_dir.to_path_buf())
            .context("failed to create screen writer")?;

        let mut triggers = trigger::start_triggers()
            .context("failed to start screen triggers")?;

        info!(capture_dir = %capture_dir.display(), "screen source started");

        let mut capture_count: u64 = 0;

        loop {
            tokio::select! {
                // Shutdown signal takes priority
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                event = triggers.recv() => {
                    let Some(event) = event else { break };
                    match screenshot::capture_frontmost_window() {
                        Ok(Some(shot)) => {
                            match writer.save_screenshot(
                                &shot.png_bytes,
                                event.ts,
                                &shot.app_name,
                                &shot.window_title,
                                event.kind.as_str(),
                            ) {
                                Ok(_) => {
                                    capture_count += 1;
                                    info!(
                                        count = capture_count,
                                        app = %shot.app_name,
                                        trigger = event.kind.as_str(),
                                        "captured screenshot"
                                    );
                                }
                                Err(e) => warn!(error = %e, "failed to save screenshot"),
                            }
                        }
                        Ok(None) => {}
                        Err(e) => warn!(error = %e, "screenshot capture failed"),
                    }
                }
            }
        }

        info!(total = capture_count, "screen source stopped");
        Ok(())
    }
}
```

- [ ] **Step 3: Register the source module in lib.rs**

```rust
//! Screen capture daemon: captures active window screenshots on app focus change
//! and idle timer triggers.
//!
//! Captures are intentionally dumb — save PNG files and record DataRefs.
//! Interpretation (vision model) lives in alvum-processor-screen.

pub mod daemon;
pub mod screenshot;
pub mod source;
pub mod trigger;
pub mod writer;
```

- [ ] **Step 4: Verify Task 3 compiles**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-capture-screen
```

- [ ] **Step 5: Commit Task 3**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-capture-screen/ && git commit -m "feat: add ScreenSource implementing CaptureSource"
```

---

### Task 4: Unified `alvum capture` Command

Replace `Record` and `CaptureScreen` CLI commands with a single `Capture` command. The orchestrator loads config, applies overrides, creates sources, and manages concurrent execution with a shared shutdown signal.

**Files:**
- Modify: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Replace `Record` and `CaptureScreen` enum variants with `Capture`**

Remove the `Record` and `CaptureScreen` variants from the `Commands` enum. Add:

```rust
    /// Start all enabled capture sources (audio, screen)
    Capture {
        /// Capture directory (default: ./capture/<today>)
        #[arg(long)]
        capture_dir: Option<PathBuf>,
        /// Only start these sources (comma-separated, e.g., "audio-mic,screen")
        #[arg(long)]
        only: Option<String>,
        /// Disable these sources (comma-separated, e.g., "audio-system")
        #[arg(long)]
        disable: Option<String>,
    },
```

- [ ] **Step 2: Update the `match cli.command` block**

Remove the `Commands::Record { .. }` and `Commands::CaptureScreen { .. }` arms. Add:

```rust
        Commands::Capture { capture_dir, only, disable } => {
            cmd_capture(capture_dir, only, disable).await
        }
```

- [ ] **Step 3: Remove `cmd_record` and `cmd_capture_screen` functions**

Delete both functions entirely.

- [ ] **Step 4: Add the `cmd_capture` function and source registry**

```rust
/// Create a CaptureSource from a config key and its settings.
fn create_source(
    name: &str,
    config: &alvum_core::config::CaptureSourceConfig,
) -> Option<Box<dyn alvum_core::capture::CaptureSource>> {
    match name {
        "audio-mic" => Some(Box::new(
            alvum_capture_audio::source::AudioMicSource::from_config(config),
        )),
        "audio-system" => Some(Box::new(
            alvum_capture_audio::source::AudioSystemSource::from_config(config),
        )),
        "screen" => Some(Box::new(
            alvum_capture_screen::source::ScreenSource::from_config(config),
        )),
        unknown => {
            tracing::warn!(source = unknown, "unknown capture source in config, skipping");
            None
        }
    }
}

async fn cmd_capture(
    capture_dir: Option<PathBuf>,
    only: Option<String>,
    disable: Option<String>,
) -> Result<()> {
    let config = alvum_core::config::AlvumConfig::load()?;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let capture_dir = capture_dir
        .unwrap_or_else(|| PathBuf::from("capture").join(&today));

    std::fs::create_dir_all(&capture_dir)?;

    // Parse --only and --disable filters
    let only_set: Option<Vec<&str>> = only.as_deref()
        .map(|s| s.split(',').map(|s| s.trim()).collect());
    let disable_set: Vec<&str> = disable.as_deref()
        .map(|s| s.split(',').map(|s| s.trim()).collect())
        .unwrap_or_default();

    // Resolve which sources to start
    let mut sources: Vec<Box<dyn alvum_core::capture::CaptureSource>> = Vec::new();
    for (name, source_config) in config.enabled_capture_sources() {
        // Apply --only filter
        if let Some(ref only) = only_set {
            if !only.contains(&name) {
                continue;
            }
        }
        // Apply --disable filter
        if disable_set.contains(&name) {
            continue;
        }
        if let Some(source) = create_source(name, source_config) {
            sources.push(source);
        }
    }

    if sources.is_empty() {
        println!("No capture sources enabled. Check config or --only/--disable flags.");
        println!("Config: {}", alvum_core::config::config_path().display());
        return Ok(());
    }

    let source_names: Vec<&str> = sources.iter().map(|s| s.name()).collect();
    info!(
        dir = %capture_dir.display(),
        sources = ?source_names,
        "starting capture"
    );

    println!("Capturing: {}", source_names.join(", "));
    println!("Directory: {}", capture_dir.display());
    println!("Press Ctrl-C to stop.\n");

    // Shared shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Spawn each source as a tokio task
    let mut handles = Vec::new();
    for source in sources {
        let dir = capture_dir.clone();
        let rx = shutdown_rx.clone();
        let name = source.name().to_string();
        let handle = tokio::spawn(async move {
            if let Err(e) = source.run(&dir, rx).await {
                tracing::error!(source = %name, error = %e, "capture source failed");
            }
        });
        handles.push(handle);
    }

    // Wait for Ctrl-C
    tokio::signal::ctrl_c().await?;

    println!("\nStopping...");
    let _ = shutdown_tx.send(true);

    // Await all tasks (with a timeout to avoid hanging)
    let timeout = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        futures::future::join_all(handles),
    );

    match timeout.await {
        Ok(_) => {}
        Err(_) => tracing::warn!("some capture sources did not shut down within 5 seconds"),
    }

    println!("Done.");
    Ok(())
}
```

- [ ] **Step 5: Add `futures` dependency to alvum-cli**

The `join_all` call needs the `futures` crate. Alternatively, use a manual loop. The simpler approach avoids a new dependency:

Replace the `futures::future::join_all` timeout block with:

```rust
    // Await all tasks (with a timeout to avoid hanging)
    for handle in handles {
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            handle,
        ).await;
    }
```

This avoids needing a `futures` dependency. Each handle gets up to 5 seconds.

- [ ] **Step 6: Update the module-level doc comment in main.rs**

```rust
//! CLI entry point for alvum.
//!
//! Subcommands:
//! - `alvum capture` — start all enabled capture sources (audio, screen)
//! - `alvum devices` — list available audio devices
//! - `alvum extract` — extract decisions from data sources
//! - `alvum config-init` — initialize a default config file
//! - `alvum config-show` — show current configuration
//! - `alvum connectors` — list connectors and their status
```

- [ ] **Step 7: Update `cmd_devices` hint text**

Change the final println in `cmd_devices`:

```rust
    println!("\nUse [capture.audio-mic] device setting in config to select a device.");
```

- [ ] **Step 8: Verify Task 4 compiles**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-cli
```

- [ ] **Step 9: Commit Task 4**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-cli/ && git commit -m "feat: unify capture sources under single 'alvum capture' command"
```

---

### Task 5: Extract Reads Processor Config

Update `alvum extract` to read `[processors]` config for defaults instead of requiring CLI flags every time. CLI flags still override config values.

**Files:**
- Modify: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Update `cmd_extract` to read processor config**

At the top of `cmd_extract`, after loading the provider, add config-based defaults:

```rust
    let config = alvum_core::config::AlvumConfig::load()?;

    // Processor config provides defaults — CLI flags override
    let whisper_model = whisper_model.or_else(|| {
        config.processor_setting("audio", "whisper_model")
            .map(|s| {
                let expanded = if s.starts_with("~/") {
                    dirs::home_dir()
                        .map(|h| h.join(&s[2..]))
                        .unwrap_or_else(|| PathBuf::from(&s))
                } else {
                    PathBuf::from(&s)
                };
                expanded
            })
    });

    let vision = {
        let from_config = config.processor_setting("screen", "vision")
            .unwrap_or_else(|| "local".into());
        if vision == "local" {
            // "local" is the clap default — check if config overrides it
            from_config
        } else {
            // Explicit CLI flag takes precedence
            vision
        }
    };
```

Note: the `vision` parameter in `cmd_extract`'s signature uses `default_value = "local"`. To distinguish "user passed --vision" from "clap default", a cleaner approach is to make `vision` an `Option<String>`:

- [ ] **Step 2: Change vision CLI arg to Option**

In the `Extract` variant of `Commands`:

```rust
        /// Vision processing mode for screen captures: local, api, ocr, off
        #[arg(long)]
        vision: Option<String>,
```

Then in `cmd_extract`, resolve the final value:

```rust
    let vision = vision
        .or_else(|| config.processor_setting("screen", "vision"))
        .unwrap_or_else(|| "local".into());
```

- [ ] **Step 3: Use config for provider and model defaults too**

The `Extract` command already has `--provider` and `--model` with hardcoded defaults. Update them to use config:

```rust
        /// LLM provider: cli, api, ollama (default from config)
        #[arg(long)]
        provider: Option<String>,

        /// Model to use (default from config)
        #[arg(long)]
        model: Option<String>,
```

Then in `cmd_extract`:

```rust
    let config = alvum_core::config::AlvumConfig::load()?;
    let provider_name = provider.unwrap_or_else(|| config.pipeline.provider.clone());
    let model = model.unwrap_or_else(|| config.pipeline.model.clone());
```

- [ ] **Step 4: Update cmd_extract signature**

The full updated signature becomes:

```rust
async fn cmd_extract(
    source: Option<String>,
    session: Option<PathBuf>,
    output: PathBuf,
    provider: Option<String>,
    model: Option<String>,
    before: Option<String>,
    capture_dir: Option<PathBuf>,
    whisper_model: Option<PathBuf>,
    relevance_threshold: f32,
    vision: Option<String>,
) -> Result<()> {
```

And the match arm:

```rust
        Commands::Extract { source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold, vision } => {
            cmd_extract(source, session, output, provider, model, before, capture_dir, whisper_model, relevance_threshold, vision).await
        }
```

- [ ] **Step 5: Verify Task 5 compiles**

```bash
cd /Users/michael/git/alvum && cargo check -p alvum-cli
```

- [ ] **Step 6: Commit Task 5**

```bash
cd /Users/michael/git/alvum && git add crates/alvum-cli/ && git commit -m "feat: extract reads processor config for whisper_model and vision defaults"
```

---

### Task 6: Tests + Cleanup

Verify the full workspace compiles and all tests pass. Update doc comments. Remove stale references to old commands.

**Files:**
- Various (verification and doc cleanup)

- [ ] **Step 1: Full workspace build**

```bash
cd /Users/michael/git/alvum && cargo build
```

- [ ] **Step 2: Run all tests**

```bash
cd /Users/michael/git/alvum && cargo test
```

- [ ] **Step 3: Verify `alvum --help` shows the new command structure**

```bash
cd /Users/michael/git/alvum && cargo run -- --help
```

Expected output should list `capture`, `devices`, `extract`, `config-init`, `config-show`, `connectors`. Should NOT list `record` or `capture-screen`.

- [ ] **Step 4: Verify `alvum capture --help` shows source management flags**

```bash
cd /Users/michael/git/alvum && cargo run -- capture --help
```

Expected: `--capture-dir`, `--only`, `--disable` documented.

- [ ] **Step 5: Verify `alvum config-show` outputs the new config sections**

```bash
cd /Users/michael/git/alvum && cargo run -- config-show
```

Expected: output includes `[capture.audio-mic]`, `[capture.audio-system]`, `[capture.screen]` sections.

- [ ] **Step 6: Spot-check that old daemon.rs and recorder.rs still compile independently**

The existing `daemon::run()` and `Recorder::start()` should still work — they were not modified, only new code was added alongside them.

```bash
cd /Users/michael/git/alvum && cargo test -p alvum-capture-audio -p alvum-capture-screen
```

- [ ] **Step 7: Commit final cleanup**

```bash
cd /Users/michael/git/alvum && git add -A && git commit -m "chore: verify capture orchestration integration, update docs"
```
