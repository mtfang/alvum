# Episodic Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the episodic alignment layer — time blocks + context threads + relevance scoring — that sits between processors and the decision extraction pipeline, enabling cross-source disambiguation.

**Architecture:** New crate `alvum-episode` with two passes: Pass 1 deterministically buckets observations into 5-minute time blocks. Pass 2 sends the full day's blocks to an LLM that identifies concurrent context threads, classifies them, and scores relevance. The CLI gains a cross-source `alvum extract` mode (no `--source` flag) that threads all available data before extraction.

**Tech Stack:** Rust, `alvum-core` (Observation, storage), `alvum-pipeline` (LlmProvider for Pass 2), `chrono` (time arithmetic), `serde_json`.

---

## File Structure

```
crates/alvum-episode/
├── Cargo.toml
└── src/
    ├── lib.rs                  re-exports
    ├── types.rs                TimeBlock, ContextThread, ThreadingResult
    ├── time_block.rs           Pass 1: temporal quantization
    └── threading.rs            Pass 2: LLM-driven context thread detection
```

Modifications:
- `crates/alvum-cli/src/main.rs` — add cross-source extract mode
- `Cargo.toml` — add `alvum-episode` to workspace members

---

### Task 1: Episode Types

**Files:**
- Create: `crates/alvum-episode/Cargo.toml`
- Create: `crates/alvum-episode/src/lib.rs`
- Create: `crates/alvum-episode/src/types.rs`

- [ ] **Step 1: Create Cargo.toml**

```toml
# crates/alvum-episode/Cargo.toml
[package]
name = "alvum-episode"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
alvum-pipeline = { path = "../alvum-pipeline" }
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true
tracing.workspace = true
```

- [ ] **Step 2: Add to workspace**

In root `Cargo.toml`, add `"crates/alvum-episode"` to the `members` list.

- [ ] **Step 3: Write types.rs with tests**

```rust
// crates/alvum-episode/src/types.rs

//! Core types for episodic alignment: time blocks, context threads, and threading results.

use alvum_core::observation::Observation;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A fixed-duration window containing all observations from all sources.
/// Pass 1 output. Pure temporal quantization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBlock {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub observations: Vec<Observation>,
}

impl TimeBlock {
    /// Number of distinct sources in this block.
    pub fn source_count(&self) -> usize {
        let mut sources: Vec<&str> = self.observations.iter().map(|o| o.source.as_str()).collect();
        sources.sort();
        sources.dedup();
        sources.len()
    }

    /// Check if block contains observations from a specific source.
    pub fn has_source(&self, source: &str) -> bool {
        self.observations.iter().any(|o| o.source == source)
    }
}

/// A coherent context spanning one or more TimeBlocks.
/// Pass 2 output. Represents a continuous activity with relevance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextThread {
    pub id: String,
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub sources: Vec<String>,
    pub observations: Vec<Observation>,
    pub relevance: f32,
    pub relevance_signals: Vec<String>,
    /// Free-form classification. Convention: "conversation", "solo_work",
    /// "media_playback", "ambient", "transition" — any string valid.
    pub thread_type: String,
    pub metadata: Option<serde_json::Value>,
}

impl ContextThread {
    /// Duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        (self.end - self.start).num_milliseconds() as f64 / 1000.0
    }

    /// Whether this thread passes a relevance threshold.
    pub fn is_relevant(&self, threshold: f32) -> bool {
        self.relevance >= threshold
    }
}

/// Complete output of the episodic alignment process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadingResult {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub time_blocks: Vec<TimeBlock>,
    pub threads: Vec<ContextThread>,
    pub observation_count: usize,
    pub source_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ts: &str, source: &str, kind: &str, content: &str) -> Observation {
        Observation {
            ts: ts.parse().unwrap(),
            source: source.into(),
            kind: kind.into(),
            content: content.into(),
            metadata: None,
            media_ref: None,
        }
    }

    #[test]
    fn time_block_source_count() {
        let block = TimeBlock {
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:05:00Z".parse().unwrap(),
            observations: vec![
                obs("2026-04-11T10:00:15Z", "audio-mic", "speech", "hello"),
                obs("2026-04-11T10:00:20Z", "screen", "app_focus", "Zoom"),
                obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "world"),
            ],
        };
        assert_eq!(block.source_count(), 2);
        assert!(block.has_source("audio-mic"));
        assert!(block.has_source("screen"));
        assert!(!block.has_source("calendar"));
    }

    #[test]
    fn context_thread_relevance_filter() {
        let thread = ContextThread {
            id: "thread_001".into(),
            label: "Sprint Planning".into(),
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:30:00Z".parse().unwrap(),
            sources: vec!["audio-mic".into(), "screen".into()],
            observations: vec![],
            relevance: 0.8,
            relevance_signals: vec!["multi-source convergence".into()],
            thread_type: "conversation".into(),
            metadata: None,
        };
        assert!(thread.is_relevant(0.5));
        assert!(thread.is_relevant(0.8));
        assert!(!thread.is_relevant(0.9));
        assert!((thread.duration_secs() - 1800.0).abs() < 0.1);
    }

    #[test]
    fn roundtrip_time_block() {
        let block = TimeBlock {
            start: "2026-04-11T10:00:00Z".parse().unwrap(),
            end: "2026-04-11T10:05:00Z".parse().unwrap(),
            observations: vec![obs("2026-04-11T10:01:00Z", "git", "commit", "fix bug")],
        };
        let json = serde_json::to_string(&block).unwrap();
        let deserialized: TimeBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.observations.len(), 1);
    }

    #[test]
    fn roundtrip_context_thread() {
        let thread = ContextThread {
            id: "thread_001".into(),
            label: "TV Background".into(),
            start: "2026-04-11T10:05:00Z".parse().unwrap(),
            end: "2026-04-11T11:30:00Z".parse().unwrap(),
            sources: vec!["audio-mic".into()],
            observations: vec![],
            relevance: 0.1,
            relevance_signals: vec!["media dialogue detected".into()],
            thread_type: "media_playback".into(),
            metadata: Some(serde_json::json!({"show": "Breaking Bad"})),
        };
        let json = serde_json::to_string(&thread).unwrap();
        let deserialized: ContextThread = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.thread_type, "media_playback");
        assert_eq!(deserialized.relevance, 0.1);
    }
}
```

- [ ] **Step 4: Write lib.rs**

```rust
// crates/alvum-episode/src/lib.rs
//! Episodic alignment: time blocks + context threads.
//!
//! Two-pass system that groups observations from all sources into time-aligned
//! blocks (Pass 1), then traces coherent context threads across blocks and
//! scores relevance (Pass 2). The pipeline extracts decisions only from
//! high-relevance threads.

pub mod types;
pub mod time_block;
pub mod threading;
```

Create placeholders:
```rust
// crates/alvum-episode/src/time_block.rs
// Implemented in Task 2

// crates/alvum-episode/src/threading.rs
// Implemented in Task 3
```

- [ ] **Step 5: Run tests, commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p alvum-episode`
Expected: 4 tests PASS

```bash
git add Cargo.toml crates/alvum-episode/
git commit -m "feat(episode): add TimeBlock, ContextThread, ThreadingResult types"
```

---

### Task 2: Pass 1 — Time Block Assembly

Pure Rust, deterministic, no LLM. Sorts observations by timestamp and buckets into fixed-width windows.

**Files:**
- Modify: `crates/alvum-episode/src/time_block.rs`

- [ ] **Step 1: Write tests first**

```rust
// crates/alvum-episode/src/time_block.rs

//! Pass 1: Temporal quantization. Bucket all observations into fixed-duration time blocks.

use alvum_core::observation::Observation;
use chrono::{DateTime, Duration, Utc};

use crate::types::TimeBlock;

/// Bucket observations into fixed-duration time blocks.
/// Empty blocks (no observations) are omitted.
pub fn assemble_time_blocks(
    observations: &[Observation],
    block_duration: Duration,
) -> Vec<TimeBlock> {
    if observations.is_empty() {
        return vec![];
    }

    let mut sorted: Vec<&Observation> = observations.iter().collect();
    sorted.sort_by_key(|o| o.ts);

    let earliest = sorted.first().unwrap().ts;
    let latest = sorted.last().unwrap().ts;

    // Align block start to the block boundary before the earliest observation
    let block_secs = block_duration.num_seconds();
    let epoch_secs = earliest.timestamp();
    let block_start_epoch = (epoch_secs / block_secs) * block_secs;
    let mut current_start = DateTime::<Utc>::from_timestamp(block_start_epoch, 0).unwrap();

    let mut blocks = Vec::new();

    while current_start <= latest {
        let current_end = current_start + block_duration;

        let block_obs: Vec<Observation> = sorted.iter()
            .filter(|o| o.ts >= current_start && o.ts < current_end)
            .cloned()
            .cloned()
            .collect();

        if !block_obs.is_empty() {
            blocks.push(TimeBlock {
                start: current_start,
                end: current_end,
                observations: block_obs,
            });
        }

        current_start = current_end;
    }

    blocks
}

/// Format time blocks as a text timeline for LLM consumption.
/// Used as input to Pass 2 (context threading).
pub fn format_blocks_for_llm(blocks: &[TimeBlock]) -> String {
    let mut parts = Vec::new();

    for (i, block) in blocks.iter().enumerate() {
        let start = block.start.format("%H:%M");
        let end = block.end.format("%H:%M");
        parts.push(format!("=== Block {} ({start}-{end}) ===", i));

        for obs in &block.observations {
            let ts = obs.ts.format("%H:%M:%S");
            let speaker = obs.speaker().map(|s| format!(" {s}:")).unwrap_or_default();
            let content = if obs.content.len() > 500 {
                format!("{}...", &obs.content[..500])
            } else {
                obs.content.clone()
            };
            parts.push(format!("[{ts}] [{source}/{kind}]{speaker} {content}",
                source = obs.source, kind = obs.kind));
        }

        parts.push(String::new()); // blank line between blocks
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(ts: &str, source: &str, kind: &str, content: &str) -> Observation {
        Observation {
            ts: ts.parse().unwrap(),
            source: source.into(),
            kind: kind.into(),
            content: content.into(),
            metadata: None,
            media_ref: None,
        }
    }

    #[test]
    fn empty_observations_produces_no_blocks() {
        let blocks = assemble_time_blocks(&[], Duration::minutes(5));
        assert!(blocks.is_empty());
    }

    #[test]
    fn single_observation_produces_one_block() {
        let observations = vec![
            obs("2026-04-11T10:02:30Z", "audio-mic", "speech", "hello"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].observations.len(), 1);
    }

    #[test]
    fn observations_in_same_window_group_together() {
        let observations = vec![
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "first"),
            obs("2026-04-11T10:03:00Z", "screen", "app_focus", "Zoom"),
            obs("2026-04-11T10:04:30Z", "audio-mic", "speech", "second"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].observations.len(), 3);
        assert_eq!(blocks[0].source_count(), 2);
    }

    #[test]
    fn observations_in_different_windows_produce_separate_blocks() {
        let observations = vec![
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "morning"),
            obs("2026-04-11T10:12:00Z", "audio-mic", "speech", "later"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].observations[0].content, "morning");
        assert_eq!(blocks[1].observations[0].content, "later");
    }

    #[test]
    fn empty_gaps_are_skipped() {
        let observations = vec![
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "early"),
            obs("2026-04-11T10:31:00Z", "audio-mic", "speech", "late"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        // 30 minutes apart with 5-min blocks = only 2 blocks (not 7 empty ones)
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn observations_sorted_regardless_of_input_order() {
        let observations = vec![
            obs("2026-04-11T10:04:00Z", "audio-mic", "speech", "second"),
            obs("2026-04-11T10:01:00Z", "audio-mic", "speech", "first"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks[0].observations[0].content, "first");
        assert_eq!(blocks[0].observations[1].content, "second");
    }

    #[test]
    fn cross_source_observations_in_same_block() {
        let observations = vec![
            obs("2026-04-11T10:00:15Z", "audio-mic", "speech", "let's defer"),
            obs("2026-04-11T10:00:15Z", "screen", "app_focus", "Zoom"),
            obs("2026-04-11T10:01:00Z", "calendar", "event", "Sprint Planning"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].source_count(), 3);
    }

    #[test]
    fn format_blocks_produces_readable_output() {
        let observations = vec![
            obs("2026-04-11T10:00:15Z", "audio-mic", "speech", "hello world"),
            obs("2026-04-11T10:00:20Z", "screen", "app_focus", "Zoom"),
        ];
        let blocks = assemble_time_blocks(&observations, Duration::minutes(5));
        let formatted = format_blocks_for_llm(&blocks);
        assert!(formatted.contains("=== Block 0"));
        assert!(formatted.contains("[audio-mic/speech]"));
        assert!(formatted.contains("[screen/app_focus]"));
        assert!(formatted.contains("hello world"));
    }
}
```

- [ ] **Step 2: Run tests, commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p alvum-episode time_block`
Expected: 7 tests PASS

```bash
git add crates/alvum-episode/src/time_block.rs
git commit -m "feat(episode): add Pass 1 time block assembly"
```

---

### Task 3: Pass 2 — LLM Context Threading

Send the formatted time blocks to an LLM that identifies concurrent threads, classifies them, and scores relevance.

**Files:**
- Modify: `crates/alvum-episode/src/threading.rs`

- [ ] **Step 1: Implement threading module**

```rust
// crates/alvum-episode/src/threading.rs

//! Pass 2: LLM-driven context thread detection.
//! Takes formatted time blocks and produces ContextThreads with relevance scores.

use alvum_core::observation::Observation;
use alvum_pipeline::llm::LlmProvider;
use alvum_pipeline::util::strip_markdown_fences;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use tracing::info;

use crate::time_block;
use crate::types::{ContextThread, ThreadingResult, TimeBlock};

const THREADING_SYSTEM_PROMPT: &str = r#"You are analyzing a full day of captured data from multiple sensors.
The data is organized into 5-minute time blocks, each containing
observations from various sources (audio transcripts, screen events,
location, calendar, etc.).

Identify CONTEXT THREADS — coherent, continuous activities that
may span multiple time blocks and may run concurrently.

For each thread, output:
- id: sequential (thread_001, thread_002, ...)
- label: human-readable name for this activity
- start: ISO 8601 timestamp (start of first relevant observation)
- end: ISO 8601 timestamp (end of last relevant observation)
- thread_type: free-form classification (e.g., "conversation", "solo_work",
  "media_playback", "ambient", "transition", "phone_call")
- sources: which data sources contribute to this thread
- observations: array of objects with {block_index, obs_index} identifying
  which observations belong to this thread
- relevance: 0.0 to 1.0
- relevance_signals: list of reasons for the score
- metadata: structured context if available (participants, meeting title, etc.)

THREADING RULES:
1. A time block can participate in MULTIPLE concurrent threads.
2. Each observation belongs to EXACTLY ONE thread. Disambiguate.
3. Trace threads across block boundaries — a meeting spanning
   10:00-10:30 is ONE thread across multiple blocks.
4. Split threads when the context genuinely changes.

RELEVANCE SCORING:
High (0.7-1.0):
  - Multi-source convergence (audio + screen + calendar corroborate)
  - Decision language ("let's do X", "I've decided", "we should")
  - Commitment language ("I'll have it by Friday")
  - References to the person's actual projects, people, goals

Medium (0.3-0.7):
  - Single-source conversation with work content
  - Solo work session with sparse self-talk
  - Thinking aloud about real topics

Low (0.0-0.3):
  - Media playback (TV, movies, podcasts, music)
  - Other people's conversations not involving the user
  - Routine transactions ("large coffee please")
  - Transit with no meaningful conversation

Output ONLY a JSON array of threads. No markdown, no explanation."#;

/// LLM response shape for a single thread.
#[derive(serde::Deserialize)]
struct ThreadRaw {
    id: String,
    label: String,
    start: String,
    end: String,
    thread_type: String,
    sources: Vec<String>,
    observations: Vec<ObsRef>,
    relevance: f32,
    relevance_signals: Vec<String>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct ObsRef {
    block_index: usize,
    obs_index: usize,
}

/// Run Pass 2: identify context threads from time blocks using an LLM.
pub async fn identify_threads(
    provider: &dyn LlmProvider,
    blocks: &[TimeBlock],
) -> Result<Vec<ContextThread>> {
    if blocks.is_empty() {
        return Ok(vec![]);
    }

    let formatted = time_block::format_blocks_for_llm(blocks);
    info!(blocks = blocks.len(), formatted_len = formatted.len(), "threading time blocks");

    let response = provider
        .complete(THREADING_SYSTEM_PROMPT, &formatted)
        .await
        .context("LLM threading call failed")?;

    let json_str = strip_markdown_fences(&response);
    let raw_threads: Vec<ThreadRaw> = serde_json::from_str(json_str).with_context(|| {
        format!("failed to parse threading response. First 500 chars:\n{}",
            &response[..response.len().min(500)])
    })?;

    // Resolve observation references into actual Observation objects
    let mut threads = Vec::new();
    for raw in raw_threads {
        let mut observations = Vec::new();
        for obs_ref in &raw.observations {
            if let Some(block) = blocks.get(obs_ref.block_index) {
                if let Some(obs) = block.observations.get(obs_ref.obs_index) {
                    observations.push(obs.clone());
                }
            }
        }

        let start = raw.start.parse::<DateTime<Utc>>().unwrap_or_else(|_| {
            observations.first().map(|o| o.ts).unwrap_or_else(Utc::now)
        });
        let end = raw.end.parse::<DateTime<Utc>>().unwrap_or_else(|_| {
            observations.last().map(|o| o.ts).unwrap_or_else(Utc::now)
        });

        threads.push(ContextThread {
            id: raw.id,
            label: raw.label,
            start,
            end,
            sources: raw.sources,
            observations,
            relevance: raw.relevance.clamp(0.0, 1.0),
            relevance_signals: raw.relevance_signals,
            thread_type: raw.thread_type,
            metadata: raw.metadata,
        });
    }

    info!(threads = threads.len(), "identified context threads");
    Ok(threads)
}

/// Full episodic alignment: Pass 1 + Pass 2.
pub async fn align_episodes(
    provider: &dyn LlmProvider,
    observations: &[Observation],
    block_duration: Duration,
) -> Result<ThreadingResult> {
    // Pass 1: time blocks
    let time_blocks = time_block::assemble_time_blocks(observations, block_duration);
    info!(blocks = time_blocks.len(), "assembled time blocks");

    // Pass 2: context threading
    let threads = identify_threads(provider, &time_blocks).await?;

    let mut sources: Vec<String> = observations.iter().map(|o| o.source.clone()).collect();
    sources.sort();
    sources.dedup();

    let start = time_blocks.first().map(|b| b.start).unwrap_or_else(Utc::now);
    let end = time_blocks.last().map(|b| b.end).unwrap_or_else(Utc::now);

    Ok(ThreadingResult {
        start,
        end,
        time_blocks,
        threads,
        observation_count: observations.len(),
        source_count: sources.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threading_prompt_contains_key_instructions() {
        assert!(THREADING_SYSTEM_PROMPT.contains("CONTEXT THREADS"));
        assert!(THREADING_SYSTEM_PROMPT.contains("relevance"));
        assert!(THREADING_SYSTEM_PROMPT.contains("EXACTLY ONE thread"));
        assert!(THREADING_SYSTEM_PROMPT.contains("media_playback"));
    }
}
```

- [ ] **Step 2: Run tests, commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p alvum-episode threading`
Expected: 1 test PASS (prompt content check). Full integration requires LLM — tested via CLI.

```bash
git add crates/alvum-episode/src/threading.rs
git commit -m "feat(episode): add Pass 2 LLM-driven context threading"
```

---

### Task 4: CLI Cross-Source Extract Mode

Add `alvum extract` without `--source` that gathers all available data, runs episodic alignment, and extracts decisions only from high-relevance threads.

**Files:**
- Modify: `crates/alvum-cli/Cargo.toml` (add alvum-episode dependency)
- Modify: `crates/alvum-cli/src/main.rs`

- [ ] **Step 1: Add dependency**

In `crates/alvum-cli/Cargo.toml`, add:
```toml
alvum-episode = { path = "../alvum-episode" }
```

- [ ] **Step 2: Update CLI — make source optional, add relevance threshold**

In `crates/alvum-cli/src/main.rs`, change the Extract command's source field from required to optional and add relevance threshold:

In the `Commands::Extract` variant, change:
```rust
        /// Data source: "claude" or "audio". Omit for cross-source threading.
        #[arg(long)]
        source: Option<String>,
```

Add:
```rust
        /// Minimum relevance score for threads to be sent to decision extraction (0.0-1.0)
        #[arg(long, default_value = "0.5")]
        relevance_threshold: f32,
```

Update the match arm in main() to pass the new args, and update `cmd_extract`'s signature to accept `source: Option<String>` and `relevance_threshold: f32`.

- [ ] **Step 3: Add cross-source mode to cmd_extract**

At the start of `cmd_extract`, after gathering observations, add the threading path:

```rust
    // If no specific source, run cross-source episodic alignment
    if source.is_none() {
        // Gather ALL available observations from all sources in capture_dir
        let capture_dir = capture_dir.context("--capture-dir required for cross-source mode")?;
        let mut all_observations = Vec::new();

        // Scan for audio files
        if let Some(ref model_path) = whisper_model {
            let mut audio_refs = Vec::new();
            for subdir in &["audio/mic", "audio/system", "audio/wearable"] {
                let dir = capture_dir.join(subdir);
                if dir.is_dir() {
                    for entry in std::fs::read_dir(&dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if ext == "wav" || ext == "opus" {
                            let source = format!("audio-{}", subdir.split('/').last().unwrap_or("unknown"));
                            let mime = if ext == "wav" { "audio/wav" } else { "audio/opus" };
                            audio_refs.push(alvum_core::data_ref::DataRef {
                                ts: chrono::Utc::now(),
                                source,
                                path: path.to_string_lossy().into_owned(),
                                mime: mime.into(),
                                metadata: None,
                            });
                        }
                    }
                }
            }
            if !audio_refs.is_empty() {
                info!(files = audio_refs.len(), "found audio files, transcribing");
                let audio_obs = alvum_processor_audio::transcriber::process_audio_data_refs(model_path, &audio_refs)?;
                all_observations.extend(audio_obs);
            }
        }

        // Scan for screen events
        let events_path = capture_dir.join("events.jsonl");
        if events_path.exists() {
            info!("loading screen events");
            let screen_obs: Vec<Observation> = alvum_core::storage::read_jsonl(&events_path)?;
            all_observations.extend(screen_obs);
        }

        // Save ALL as episodic evidence
        let transcript_path = output.join("transcript.jsonl");
        for obs in &all_observations {
            alvum_core::storage::append_jsonl(&transcript_path, obs)?;
        }
        info!(path = %transcript_path.display(), observations = all_observations.len(), "saved transcript");

        if all_observations.is_empty() {
            println!("No observations found in capture directory.");
            return Ok(());
        }

        // Episodic alignment: Pass 1 + Pass 2
        info!("running episodic alignment...");
        let result = alvum_episode::threading::align_episodes(
            provider.as_ref(),
            &all_observations,
            chrono::Duration::minutes(5),
        ).await?;

        // Save threading result
        let threads_path = output.join("threads.json");
        std::fs::write(&threads_path, serde_json::to_string_pretty(&result)?)?;
        info!(
            threads = result.threads.len(),
            blocks = result.time_blocks.len(),
            "episodic alignment complete"
        );

        // Filter to high-relevance threads
        let relevant: Vec<&alvum_episode::types::ContextThread> = result.threads.iter()
            .filter(|t| t.is_relevant(relevance_threshold))
            .collect();

        info!(
            total_threads = result.threads.len(),
            relevant = relevant.len(),
            threshold = relevance_threshold,
            "filtered by relevance"
        );

        if relevant.is_empty() {
            println!("✓ {} threads identified, none above relevance threshold {:.1}",
                result.threads.len(), relevance_threshold);
            println!("  threads: {}", threads_path.display());
            println!("  transcript: {}", transcript_path.display());
            for t in &result.threads {
                println!("    {} ({:.2}) — {}", t.id, t.relevance, t.label);
            }
            return Ok(());
        }

        // Collect observations from relevant threads for decision extraction
        let relevant_observations: Vec<Observation> = relevant.iter()
            .flat_map(|t| t.observations.clone())
            .collect();

        info!(observations = relevant_observations.len(), "observations from relevant threads");

        // Extract decisions from relevant observations only
        info!("extracting decisions from relevant threads...");
        let mut decisions =
            alvum_pipeline::distill::extract_decisions(provider.as_ref(), &relevant_observations).await?;
        info!(decisions = decisions.len(), "extracted");

        if !decisions.is_empty() {
            info!("analyzing causal links...");
            alvum_pipeline::causal::link_decisions(provider.as_ref(), &mut decisions).await?;
            let link_count: usize = decisions.iter().map(|d| d.causes.len()).sum();
            info!(links = link_count, "linked");

            info!("generating briefing...");
            let briefing =
                alvum_pipeline::briefing::generate_briefing(provider.as_ref(), &decisions).await?;

            for dec in &decisions {
                alvum_core::storage::append_jsonl(&decisions_path, dec)?;
            }
            std::fs::write(&briefing_path, &briefing)?;

            let extraction = alvum_core::decision::ExtractionResult {
                session_id: "cross-source".into(),
                extracted_at: chrono::Utc::now().to_rfc3339(),
                decisions: decisions.clone(),
                briefing: briefing.clone(),
            };
            std::fs::write(&extraction_path, serde_json::to_string_pretty(&extraction)?)?;

            println!("\n✓ {} threads → {} relevant → {} decisions",
                result.threads.len(), relevant.len(), decisions.len());
            println!("  threads:    {}", threads_path.display());
            println!("  decisions:  {}", decisions_path.display());
            println!("  briefing:   {}", briefing_path.display());
            println!("\n{}", "=".repeat(60));
            println!("{briefing}");
        } else {
            println!("✓ {} relevant threads, no decisions found.", relevant.len());
            println!("  threads: {}", threads_path.display());
        }

        return Ok(());
    }

    // Original single-source mode below (unchanged)
    let source = source.unwrap();
```

- [ ] **Step 3: Build and verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-cli`
Expected: compiles

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo run -p alvum-cli -- extract --help`
Expected: shows --source as optional, --relevance-threshold with default 0.5

- [ ] **Step 4: Commit**

```bash
git add crates/alvum-cli/ crates/alvum-episode/
git commit -m "feat(cli): add cross-source extract mode with episodic alignment"
```

---

### Task 5: Knowledge Corpus Types + Extraction

New crate `alvum-knowledge` that stores and extracts entities, patterns, and facts from pipeline output.

**Files:**
- Create: `crates/alvum-knowledge/Cargo.toml`
- Create: `crates/alvum-knowledge/src/lib.rs`
- Create: `crates/alvum-knowledge/src/types.rs`
- Create: `crates/alvum-knowledge/src/extract.rs`
- Create: `crates/alvum-knowledge/src/store.rs`

- [ ] **Step 1: Create crate**

```toml
# crates/alvum-knowledge/Cargo.toml
[package]
name = "alvum-knowledge"
version = "0.1.0"
edition = "2024"

[dependencies]
alvum-core = { path = "../alvum-core" }
alvum-pipeline = { path = "../alvum-pipeline" }
serde.workspace = true
serde_json.workspace = true
chrono.workspace = true
anyhow.workspace = true
tracing.workspace = true
```

Add `"crates/alvum-knowledge"` to workspace members.

- [ ] **Step 2: Implement types.rs**

```rust
// crates/alvum-knowledge/src/types.rs

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// A known entity in the person's life.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    pub id: String,
    pub name: String,
    /// Free-form: "person", "project", "place", "organization", "tool", etc.
    pub entity_type: String,
    pub description: String,
    pub relationships: Vec<Relationship>,
    pub first_seen: NaiveDate,
    pub last_seen: NaiveDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
}

/// A relationship between two entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Relationship {
    pub target_id: String,
    /// Free-form: "manages", "reports_to", "blocks", "part_of", etc.
    pub relation: String,
    pub last_confirmed: NaiveDate,
}

/// A recurring behavioral pattern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pattern {
    pub id: String,
    pub description: String,
    pub occurrences: u32,
    pub first_seen: NaiveDate,
    pub last_seen: NaiveDate,
    pub domains: Vec<String>,
    pub evidence: Vec<String>,
}

/// A persistent fact about the person's life.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Fact {
    pub id: String,
    pub content: String,
    /// Free-form: "routine", "preference", "constraint", "context".
    pub category: String,
    pub learned: NaiveDate,
    pub last_confirmed: NaiveDate,
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
                // Merge relationships
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
```

- [ ] **Step 3: Implement store.rs (load/save from knowledge/ directory)**

```rust
// crates/alvum-knowledge/src/store.rs

use anyhow::{Context, Result};
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
    // Overwrite (not append) — we save the full state each time
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = String::new();
    for item in items {
        content.push_str(&serde_json::to_string(item).context("failed to serialize")?);
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
```

- [ ] **Step 4: Implement extract.rs (LLM-driven knowledge extraction)**

```rust
// crates/alvum-knowledge/src/extract.rs

//! Extract entities, patterns, and facts from observations using an LLM.

use alvum_core::observation::Observation;
use alvum_pipeline::llm::LlmProvider;
use alvum_pipeline::util::strip_markdown_fences;
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
```

- [ ] **Step 5: Write lib.rs, add tempfile dev-dep**

```rust
// crates/alvum-knowledge/src/lib.rs
//! Knowledge corpus: accumulated entities, patterns, and facts.
//!
//! The system's long-term semantic memory. Extracted from observations,
//! fed back into every pipeline stage for context.

pub mod types;
pub mod extract;
pub mod store;
```

```toml
# Add to Cargo.toml [dev-dependencies]
tempfile = "3"
```

- [ ] **Step 6: Run tests, commit**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test -p alvum-knowledge`
Expected: 7 tests PASS (4 types + 2 store + 1 extract)

```bash
git add Cargo.toml crates/alvum-knowledge/
git commit -m "feat(knowledge): add knowledge corpus types, store, and extraction"
```

---

### Task 6: Wire Knowledge Corpus into Threading + Pipeline

Feed the knowledge corpus into the threading prompt (for better relevance scoring) and into the extraction pipeline.

**Files:**
- Modify: `crates/alvum-episode/Cargo.toml` (add alvum-knowledge dep)
- Modify: `crates/alvum-episode/src/threading.rs` (inject corpus into prompt)
- Modify: `crates/alvum-cli/Cargo.toml` (add alvum-knowledge dep)
- Modify: `crates/alvum-cli/src/main.rs` (load corpus, pass to threading, run knowledge extraction after decisions)

- [ ] **Step 1: Update alvum-episode to accept knowledge context**

In `crates/alvum-episode/Cargo.toml`, add:
```toml
alvum-knowledge = { path = "../alvum-knowledge" }
```

In `threading.rs`, update `identify_threads` and `align_episodes` to accept an optional knowledge corpus. Inject it into the user message before the time blocks:

```rust
pub async fn identify_threads(
    provider: &dyn LlmProvider,
    blocks: &[TimeBlock],
    knowledge: Option<&alvum_knowledge::types::KnowledgeCorpus>,
) -> Result<Vec<ContextThread>> {
    // ... existing code ...
    
    let mut user_message = String::new();
    
    // Inject knowledge corpus if available
    if let Some(corpus) = knowledge {
        let summary = corpus.format_for_llm();
        if !summary.is_empty() {
            user_message.push_str(&summary);
            user_message.push_str("\n\n");
        }
    }
    
    user_message.push_str(&formatted);
    
    let response = provider
        .complete(THREADING_SYSTEM_PROMPT, &user_message)
        // ... rest unchanged
```

Update `align_episodes` similarly to pass knowledge through.

- [ ] **Step 2: Wire into CLI**

In the cross-source mode of `cmd_extract`:

```rust
// Load existing knowledge corpus
let knowledge_dir = output.join("knowledge");
let mut corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

// Pass corpus to episodic alignment
let result = alvum_episode::threading::align_episodes(
    provider.as_ref(),
    &all_observations,
    chrono::Duration::minutes(5),
    Some(&corpus),
).await?;

// ... after decision extraction ...

// Extract new knowledge from relevant observations
info!("extracting knowledge...");
let new_knowledge = alvum_knowledge::extract::extract_knowledge(
    provider.as_ref(),
    &relevant_observations,
    &corpus,
).await?;
corpus.merge(new_knowledge);
alvum_knowledge::store::save(&knowledge_dir, &corpus)?;
```

- [ ] **Step 3: Build and verify**

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo build -p alvum-cli`
Expected: compiles

Run: `export PATH="$HOME/.cargo/bin:$PATH" && cargo test --workspace`
Expected: all tests pass

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: wire knowledge corpus into threading and pipeline"
```

---

## Implementation Notes

### LLM Cost

Pass 2 (context threading) is one LLM call per day. A typical day has ~50-100 non-empty time blocks. At Sonnet prices, ~$0.50-1.00 per day.

### Observation Indices

The LLM references observations by `{block_index, obs_index}`. The implementation resolves these back to actual `Observation` objects. Invalid indices are silently skipped (the LLM might hallucinate an index).

### Single-Source Backward Compatibility

When `--source` is specified, the existing single-source flow runs unchanged. No threading, no relevance scoring. This is backward compatible with all existing usage.

### Re-Threading

`threads.json` is always re-generable from `transcript.jsonl`. If the LLM improves, or new sources come online, delete `threads.json` and re-run.
