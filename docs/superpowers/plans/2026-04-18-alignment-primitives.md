# Phase A — Alignment Primitives Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `alvum-core` with every data-model type named in `docs/superpowers/specs/2026-04-03-alvum-system-design.md` § Data Model, plus a typed storage-paths module rooted at `~/.alvum/` per `docs/superpowers/specs/2026-04-18-storage-layout.md` (authoritative). Nothing downstream in the progress matrix (alignment engine, web UI, Electron shell, check-in loop) can start until these types exist.

**Architecture:** Pure additions to `crates/alvum-core` plus one new module in `crates/alvum-knowledge`, and a single refactor of `Decision` and `CausalLink` to match the spec. Each new type lives in its own module, exported through `lib.rs`, with serde round-trip tests. Storage paths are resolved through a new `paths::AlvumPaths` struct rooted at `$HOME/.alvum` with three lifecycle buckets (`capture/`, `generated/`, `runtime/`) per the authoritative storage-layout spec. The only crates outside `alvum-core` that change are `alvum-pipeline` (callers of `Decision` / `CausalLink`) and `alvum-knowledge` (new `PlaceAttributes` module).

**Scope covers the core-spec § Data Model plus the data types from the three 2026-04-18 sub-specs** (`device-fleet`, `location-map`, `health-connector`) that alignment directly consumes: `LocationObservation` and friends, `HealthObservation` and friends, and `PlaceAttributes` for place entities. The full `Device` type and device-fleet ingest plumbing are **not** in Phase A — they ride with the app-shell work in Phase C.

**Tech stack:** Rust 2024 edition, `serde`/`serde_json` (already workspace deps), `chrono` with `serde` feature (already a dep, used with `NaiveDate` and `DateTime<Utc>`), `dirs = 6` (already a dep in `alvum-core`), `tempfile` (dev-dep, used for path tests).

**Conventions to match (observed in existing `alvum-core`):**
- `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]` on all value types.
- `#[serde(rename_all = "snake_case")]` on enums.
- `#[serde(default, skip_serializing_if = "Option::is_none")]` on optional fields.
- `#[serde(default)]` on `Vec` fields. Reuse `deserialize_null_as_empty_vec` from `decision.rs` where LLM output is the source (tolerates `null` → `[]`).
- Tests live in the same file under `#[cfg(test)] mod tests`.

**Source of truth for type shapes:** `docs/superpowers/specs/2026-04-03-alvum-system-design.md` § Data Model (lines ~283-712). Every struct and enum below is copied from the spec with minor pragmatic adjustments noted inline.

**Known spec deviations (document in code comments):**
1. `Decision.timestamp: DateTime<Utc>` instead of `Decision.date: NaiveDate`. Preserves intra-day ordering we already have. `.date_naive()` recovers the spec's `date`. Comment on the field.
2. Rename current `Decision.source: String` (which is actually the connector name) → `Decision.connector: String`, so the new `source: DecisionSource` enum matches the spec verbatim.
3. Decision IDs stay as `String` (currently `"dec_001"`), not `Ulid`. Changing ID type is out of Phase A scope — deferred until it causes a real problem.

**Commit cadence:** one commit per step marked "Commit" below. Do not batch commits. Run `cargo test -p <affected-crate>` before every commit; do not commit if tests or clippy fail.

---

## Task 1: Paths Module

Resolves `~/.alvum/` and the three-bucket layout defined in `docs/superpowers/specs/2026-04-18-storage-layout.md` (authoritative — supersedes the top-level spec's § Storage tree).

**Files:**
- Create: `crates/alvum-core/src/paths.rs`
- Modify: `crates/alvum-core/src/lib.rs` (add `pub mod paths;`)

- [ ] **Step 1: Add failing test.**

Create `crates/alvum-core/src/paths.rs` with this content:

```rust
//! On-disk layout rooted at ~/.alvum/. Three lifecycle buckets:
//! - capture/   — raw ingest (ground truth; kept indefinitely)
//! - generated/ — LLM-derived + user-stated data (back up)
//! - runtime/   — operational state (binary, logs, tokens, caches)
//!
//! Authority: docs/superpowers/specs/2026-04-18-storage-layout.md

use anyhow::{Context, Result};
use chrono::NaiveDate;
use std::path::{Path, PathBuf};

/// Reverse-DNS bundle identifier. Not used for path resolution (we root at ~/.alvum
/// regardless of platform) but reserved for future use (e.g., launchd labels,
/// UserAgent strings, URL schemes). Exposed here so there's exactly one place
/// that owns this name.
pub const APP_ID: &str = "com.alvum.app";

/// Resolves every storage path under the ~/.alvum/ tree. Use `default_root()`
/// for the real location; `with_root()` for tests (pass a TempDir path).
#[derive(Debug, Clone)]
pub struct AlvumPaths {
    root: PathBuf,
}

impl AlvumPaths {
    /// Default root: `$HOME/.alvum`. Same on every platform.
    pub fn default_root() -> Result<Self> {
        let root = dirs::home_dir()
            .context("could not determine home directory")?
            .join(".alvum");
        Ok(Self { root })
    }

    /// Test-friendly constructor. Pass any directory (typically a TempDir path).
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn version_file(&self) -> PathBuf {
        self.root.join("VERSION")
    }

    // =============================================================
    // capture/ — GROUND TRUTH (kept indefinitely; back up)
    // =============================================================

    pub fn capture_root(&self) -> PathBuf {
        self.root.join("capture")
    }

    pub fn capture_dir(&self, date: NaiveDate) -> PathBuf {
        self.capture_root().join(date.format("%Y-%m-%d").to_string())
    }

    /// Top-level semantic events for the day (cross-source a11y differ output).
    pub fn events_file(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("events.jsonl")
    }

    /// Legacy flat-file fallback for location. Prefer the structured paths in
    /// Task 11 (`location_raw_file`, `location_fused_file`).
    pub fn location_file(&self, date: NaiveDate) -> PathBuf {
        self.capture_dir(date).join("location.jsonl")
    }

    // =============================================================
    // generated/ — CURRENT DERIVATION + USER-STATED STATE (back up)
    // =============================================================

    pub fn generated_dir(&self) -> PathBuf {
        self.root.join("generated")
    }

    // --- day extractions ---
    pub fn day_file(&self, date: NaiveDate) -> PathBuf {
        self.generated_dir().join("days").join(format!("{}.json", date.format("%Y-%m-%d")))
    }

    // --- decision graph ---
    pub fn decisions_index(&self) -> PathBuf {
        self.generated_dir().join("decisions").join("index.jsonl")
    }
    pub fn decisions_open(&self) -> PathBuf {
        self.generated_dir().join("decisions").join("open.jsonl")
    }
    pub fn decisions_states(&self) -> PathBuf {
        self.generated_dir().join("decisions").join("states.jsonl")
    }

    // --- knowledge corpus ---
    pub fn knowledge_entities(&self) -> PathBuf {
        self.generated_dir().join("knowledge").join("entities.jsonl")
    }
    pub fn knowledge_patterns(&self) -> PathBuf {
        self.generated_dir().join("knowledge").join("patterns.jsonl")
    }
    pub fn knowledge_facts(&self) -> PathBuf {
        self.generated_dir().join("knowledge").join("facts.jsonl")
    }

    // --- episodic memory ---
    pub fn episodes_dir(&self, date: NaiveDate) -> PathBuf {
        self.generated_dir().join("episodes").join(date.format("%Y-%m-%d").to_string())
    }
    pub fn threads_file(&self, date: NaiveDate) -> PathBuf {
        self.episodes_dir(date).join("threads.json")
    }
    pub fn time_blocks_file(&self, date: NaiveDate) -> PathBuf {
        self.episodes_dir(date).join("time_blocks.json")
    }

    // --- briefings (per-date directory, not flat .md file) ---
    pub fn briefing_dir(&self, date: NaiveDate) -> PathBuf {
        self.generated_dir().join("briefings").join(date.format("%Y-%m-%d").to_string())
    }
    pub fn briefing_file(&self, date: NaiveDate) -> PathBuf {
        self.briefing_dir(date).join("briefing.md")
    }

    // --- check-ins (questions + responses; user-stated, kept forever) ---
    pub fn checkins_dir(&self, date: NaiveDate) -> PathBuf {
        self.generated_dir().join("checkins").join(date.format("%Y-%m-%d").to_string())
    }
    pub fn checkin_questions_file(&self, date: NaiveDate) -> PathBuf {
        self.checkins_dir(date).join("questions.json")
    }
    pub fn checkin_responses_file(&self, date: NaiveDate) -> PathBuf {
        self.checkins_dir(date).join("responses.jsonl")
    }

    // --- single-file user state ---
    pub fn intentions_file(&self) -> PathBuf {
        self.generated_dir().join("intentions.json")
    }
    pub fn life_phase_file(&self) -> PathBuf {
        self.generated_dir().join("life_phase.json")
    }

    // =============================================================
    // runtime/ — OPERATIONAL STATE (regenerable; never back up)
    // =============================================================

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.runtime_dir().join("bin")
    }

    pub fn config_file(&self) -> PathBuf {
        self.runtime_dir().join("config.toml")
    }

    pub fn email_file(&self) -> PathBuf {
        self.runtime_dir().join("email.txt")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.runtime_dir().join("logs")
    }

    pub fn log_file(&self, name: &str) -> PathBuf {
        self.logs_dir().join(name)
    }

    pub fn devices_registry(&self) -> PathBuf {
        self.runtime_dir().join("devices").join("registry.json")
    }
    pub fn devices_tokens(&self) -> PathBuf {
        self.runtime_dir().join("devices").join("tokens.json")
    }
    pub fn device_heartbeats(&self, device_id: &str) -> PathBuf {
        self.runtime_dir().join("devices").join("heartbeats").join(format!("{device_id}.jsonl"))
    }

    pub fn embeddings_file(&self, name: &str) -> PathBuf {
        self.runtime_dir().join("embeddings").join(format!("{name}.idx"))
    }

    pub fn geocode_cache(&self) -> PathBuf {
        self.runtime_dir().join("cache").join("geocode").join("h3-r9.jsonl")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths() -> (AlvumPaths, TempDir) {
        let tmp = TempDir::new().unwrap();
        let p = AlvumPaths::with_root(tmp.path());
        (p, tmp)
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    // --- bucket roots ---

    #[test]
    fn three_buckets_exist_off_root() {
        let (p, _t) = paths();
        assert!(p.capture_root().ends_with("capture"));
        assert!(p.generated_dir().ends_with("generated"));
        assert!(p.runtime_dir().ends_with("runtime"));
    }

    #[test]
    fn version_file_at_root() {
        let (p, _t) = paths();
        assert!(p.version_file().ends_with("VERSION"));
    }

    // --- capture/ ---

    #[test]
    fn capture_dir_uses_iso_date_format() {
        let (p, _t) = paths();
        assert!(p.capture_dir(d("2026-04-03")).ends_with("capture/2026-04-03"));
    }

    #[test]
    fn events_and_location_under_capture_dir() {
        let (p, _t) = paths();
        let date = d("2026-04-03");
        assert_eq!(p.events_file(date), p.capture_dir(date).join("events.jsonl"));
        assert_eq!(p.location_file(date), p.capture_dir(date).join("location.jsonl"));
    }

    // --- generated/ ---

    #[test]
    fn day_file_lives_under_generated() {
        let (p, _t) = paths();
        assert!(p.day_file(d("2026-04-03")).ends_with("generated/days/2026-04-03.json"));
    }

    #[test]
    fn decisions_triplet_under_generated() {
        let (p, _t) = paths();
        assert!(p.decisions_index().ends_with("generated/decisions/index.jsonl"));
        assert!(p.decisions_open().ends_with("generated/decisions/open.jsonl"));
        assert!(p.decisions_states().ends_with("generated/decisions/states.jsonl"));
    }

    #[test]
    fn knowledge_triplet_under_generated() {
        let (p, _t) = paths();
        assert!(p.knowledge_entities().ends_with("generated/knowledge/entities.jsonl"));
        assert!(p.knowledge_patterns().ends_with("generated/knowledge/patterns.jsonl"));
        assert!(p.knowledge_facts().ends_with("generated/knowledge/facts.jsonl"));
    }

    #[test]
    fn episode_files_under_generated_and_dated() {
        let (p, _t) = paths();
        let date = d("2026-04-03");
        assert_eq!(p.threads_file(date), p.episodes_dir(date).join("threads.json"));
        assert_eq!(p.time_blocks_file(date), p.episodes_dir(date).join("time_blocks.json"));
        assert!(p.episodes_dir(date).ends_with("generated/episodes/2026-04-03"));
    }

    #[test]
    fn briefing_is_a_directory_not_a_flat_file() {
        let (p, _t) = paths();
        let date = d("2026-04-03");
        assert!(p.briefing_dir(date).ends_with("generated/briefings/2026-04-03"));
        assert_eq!(p.briefing_file(date), p.briefing_dir(date).join("briefing.md"));
    }

    #[test]
    fn checkins_split_questions_and_responses() {
        let (p, _t) = paths();
        let date = d("2026-04-03");
        assert_eq!(p.checkin_questions_file(date), p.checkins_dir(date).join("questions.json"));
        assert_eq!(p.checkin_responses_file(date), p.checkins_dir(date).join("responses.jsonl"));
        assert!(p.checkins_dir(date).ends_with("generated/checkins/2026-04-03"));
    }

    #[test]
    fn single_file_state_under_generated() {
        let (p, _t) = paths();
        assert!(p.intentions_file().ends_with("generated/intentions.json"));
        assert!(p.life_phase_file().ends_with("generated/life_phase.json"));
    }

    // --- runtime/ ---

    #[test]
    fn runtime_holds_bin_config_email_logs() {
        let (p, _t) = paths();
        assert!(p.bin_dir().ends_with("runtime/bin"));
        assert!(p.config_file().ends_with("runtime/config.toml"));
        assert!(p.email_file().ends_with("runtime/email.txt"));
        assert!(p.logs_dir().ends_with("runtime/logs"));
        assert_eq!(p.log_file("briefing.err"), p.logs_dir().join("briefing.err"));
    }

    #[test]
    fn devices_paths_under_runtime() {
        let (p, _t) = paths();
        assert!(p.devices_registry().ends_with("runtime/devices/registry.json"));
        assert!(p.devices_tokens().ends_with("runtime/devices/tokens.json"));
        assert!(p.device_heartbeats("pin").ends_with("runtime/devices/heartbeats/pin.jsonl"));
    }

    #[test]
    fn embeddings_and_cache_under_runtime() {
        let (p, _t) = paths();
        assert!(p.embeddings_file("decisions").ends_with("runtime/embeddings/decisions.idx"));
        assert!(p.geocode_cache().ends_with("runtime/cache/geocode/h3-r9.jsonl"));
    }

    // --- root resolution ---

    #[test]
    fn default_root_ends_with_dot_alvum() {
        let p = AlvumPaths::default_root().unwrap();
        assert!(p.root().ends_with(".alvum"));
    }

    #[test]
    fn app_id_is_the_canonical_reverse_dns() {
        // Not used in path resolution anymore, but reserved for launchd/URL/UA use.
        assert_eq!(APP_ID, "com.alvum.app");
    }
}
```

- [ ] **Step 2: Wire the module into `lib.rs`.**

Edit `crates/alvum-core/src/lib.rs` — add `pub mod paths;` in alphabetical order:

```rust
pub mod artifact;
pub mod capture;
pub mod config;
pub mod connector;
pub mod data_ref;
pub mod decision;
pub mod llm;
pub mod observation;
pub mod paths;
pub mod processor;
pub mod storage;
pub mod util;
```

- [ ] **Step 3: Run tests — expect pass.**

```bash
cargo test -p alvum-core paths::
```

Expected: all 14 tests pass.

- [ ] **Step 4: Clippy clean.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

Expected: no warnings.

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/paths.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add AlvumPaths for spec-mandated storage layout"
```

---

## Task 2: Evidence Module

`Evidence`, `Confidence`, `ModalClaim`, `ModalConflict`, `ConflictType` — the structural backbone for cross-modal contradiction detection (§ Data Model — Multi-Modal Evidence and Conflicts).

**Files:**
- Create: `crates/alvum-core/src/evidence.rs`
- Modify: `crates/alvum-core/src/lib.rs` (add `pub mod evidence;`)

- [ ] **Step 1: Write the module with tests.**

Create `crates/alvum-core/src/evidence.rs`:

```rust
//! Evidence chains and modal conflicts. See § Data Model — Multi-Modal Evidence and Conflicts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One piece of evidence attached to a Decision, Event, or AlignmentItem.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evidence {
    /// Name of the capture source ("claude-code", "audio-mic", "screen", "wearable-audio", ...).
    /// Kept as String to match `Observation.source` — free-form, not an enum.
    pub source: String,
    pub timestamp: DateTime<Utc>,
    pub description: String,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// A single-modality claim participating in a ModalConflict.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModalClaim {
    pub source: String,
    pub description: String,
    pub timestamp: DateTime<Utc>,
}

/// A conflict between what was stated and what was observed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModalConflict {
    pub stated: ModalClaim,
    pub observed: ModalClaim,
    pub conflict_type: ConflictType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConflictType {
    /// "said X, did Y"
    SayVsDo,
    /// Intention says X, behavior shows Y
    IntendVsDo,
    /// Believes X about self, evidence shows Y
    SelfPerception,
    /// Calendar/plan said X, day went Y
    PlanVsReality,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn evidence_roundtrip() {
        let e = Evidence {
            source: "audio-mic".into(),
            timestamp: ts("2026-04-11T10:15:00Z"),
            description: "Said 'I'll go to the gym at 6pm'".into(),
            confidence: Confidence::High,
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: Evidence = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, e);
    }

    #[test]
    fn confidence_serializes_snake_case() {
        let json = serde_json::to_string(&Confidence::High).unwrap();
        assert_eq!(json, "\"high\"");
        let json = serde_json::to_string(&Confidence::Medium).unwrap();
        assert_eq!(json, "\"medium\"");
    }

    #[test]
    fn modal_conflict_roundtrip() {
        let c = ModalConflict {
            stated: ModalClaim {
                source: "audio-mic".into(),
                description: "Gym at 6pm".into(),
                timestamp: ts("2026-04-11T09:00:00Z"),
            },
            observed: ModalClaim {
                source: "location".into(),
                description: "Still at office at 20:00".into(),
                timestamp: ts("2026-04-11T20:00:00Z"),
            },
            conflict_type: ConflictType::IntendVsDo,
        };
        let json = serde_json::to_string(&c).unwrap();
        let parsed: ModalConflict = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn conflict_type_variants_all_snake_case() {
        for (v, expected) in [
            (ConflictType::SayVsDo, "\"say_vs_do\""),
            (ConflictType::IntendVsDo, "\"intend_vs_do\""),
            (ConflictType::SelfPerception, "\"self_perception\""),
            (ConflictType::PlanVsReality, "\"plan_vs_reality\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), expected);
        }
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Edit `crates/alvum-core/src/lib.rs` — insert `pub mod evidence;` alphabetically (after `pub mod decision;`):

```rust
pub mod decision;
pub mod evidence;
pub mod llm;
```

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core evidence::
```

Expected: 4 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/evidence.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add Evidence, ModalConflict, and supporting types"
```

---

## Task 3: EmergentState Module

Persistent conditions that accumulate from many decisions (§ Data Model — Emergent States).

**Files:**
- Create: `crates/alvum-core/src/state.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/state.rs`:

```rust
//! Emergent states — persistent conditions that accumulate from many decisions and
//! transmit butterfly effects across domains. See § Data Model — Emergent States.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmergentState {
    pub id: String,
    pub description: String,
    pub domain: String,
    /// 0.0 to 1.0
    pub intensity: f32,
    #[serde(default)]
    pub contributing_decisions: Vec<String>,
    pub first_detected: NaiveDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<NaiveDate>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn emergent_state_roundtrip() {
        let s = EmergentState {
            id: "state_burnout_01".into(),
            description: "Sustained late-night work triggering sleep debt".into(),
            domain: "Health".into(),
            intensity: 0.72,
            contributing_decisions: vec!["dec_018".into(), "dec_027".into()],
            first_detected: d("2026-03-15"),
            resolved: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        let parsed: EmergentState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn resolved_state_serializes_date() {
        let s = EmergentState {
            id: "state_crunch_01".into(),
            description: "Pre-launch crunch".into(),
            domain: "Career".into(),
            intensity: 0.9,
            contributing_decisions: vec![],
            first_detected: d("2026-03-01"),
            resolved: Some(d("2026-04-01")),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"resolved\":\"2026-04-01\""));
    }

    #[test]
    fn missing_resolved_deserializes_as_none() {
        let json = r#"{
            "id": "state_01",
            "description": "x",
            "domain": "Health",
            "intensity": 0.5,
            "contributing_decisions": [],
            "first_detected": "2026-04-01"
        }"#;
        let s: EmergentState = serde_json::from_str(json).unwrap();
        assert!(s.resolved.is_none());
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Edit `crates/alvum-core/src/lib.rs` — insert `pub mod state;` alphabetically (after `pub mod processor;`, before `pub mod storage;`):

```rust
pub mod processor;
pub mod state;
pub mod storage;
```

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core state::
```

Expected: 3 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/state.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add EmergentState"
```

---

## Task 4: BehavioralSignal Module

Silent decision indicators detected from screen/camera behavior (§ Data Model — Behavioral Signals).

**Files:**
- Create: `crates/alvum-core/src/behavior.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/behavior.rs`:

```rust
//! Behavioral signals — silent decision indicators from screen + camera behavior.
//! See § Data Model — Behavioral Signals and § Silent Decision Detection.

use crate::evidence::Evidence;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BehavioralSignal {
    pub timestamp: DateTime<Utc>,
    pub signal_type: BehavioralSignalType,
    pub description: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BehavioralSignalType {
    /// compose-without-send, cart-without-purchase
    AbortedAction,
    /// opened task, immediately switched away
    AvoidancePattern,
    /// checked the same thing multiple times
    RepetitiveVisit,
    /// broke own deep work without external trigger
    SelfInterruption,
    /// gradual shift from planned activity
    AttentionDrift,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::Confidence;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn behavioral_signal_roundtrip() {
        let s = BehavioralSignal {
            timestamp: ts("2026-04-11T14:30:00Z"),
            signal_type: BehavioralSignalType::AbortedAction,
            description: "Opened email compose, typed 2 lines, discarded".into(),
            evidence: vec![Evidence {
                source: "screen".into(),
                timestamp: ts("2026-04-11T14:30:15Z"),
                description: "events.jsonl: compose window opened then closed".into(),
                confidence: Confidence::High,
            }],
        };
        let json = serde_json::to_string(&s).unwrap();
        let parsed: BehavioralSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, s);
    }

    #[test]
    fn signal_type_snake_case() {
        for (v, expected) in [
            (BehavioralSignalType::AbortedAction, "\"aborted_action\""),
            (BehavioralSignalType::AvoidancePattern, "\"avoidance_pattern\""),
            (BehavioralSignalType::RepetitiveVisit, "\"repetitive_visit\""),
            (BehavioralSignalType::SelfInterruption, "\"self_interruption\""),
            (BehavioralSignalType::AttentionDrift, "\"attention_drift\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), expected);
        }
    }

    #[test]
    fn default_evidence_is_empty() {
        let json = r#"{
            "timestamp": "2026-04-11T10:00:00Z",
            "signal_type": "aborted_action",
            "description": "x"
        }"#;
        let s: BehavioralSignal = serde_json::from_str(json).unwrap();
        assert!(s.evidence.is_empty());
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Edit `crates/alvum-core/src/lib.rs` — insert `pub mod behavior;` alphabetically after `pub mod artifact;`:

```rust
pub mod artifact;
pub mod behavior;
pub mod capture;
```

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core behavior::
```

Expected: 3 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/behavior.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add BehavioralSignal"
```

---

## Task 5: Intention Module

The reference signal the alignment engine measures behavior against (§ Data Model — Intentions).

**Files:**
- Create: `crates/alvum-core/src/intention.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/intention.rs`:

```rust
//! Intentions — the reference signal that observed behavior is measured against.
//! See § Data Model — Intentions and § Intention Capture UX.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Intention {
    pub id: String,
    pub kind: IntentionKind,
    pub description: String,
    pub domain: String,
    pub active: bool,
    pub created: NaiveDate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_date: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cadence: Option<Cadence>,
    pub source: IntentionSource,
    /// User has explicitly validated this intention.
    #[serde(default)]
    pub confirmed: bool,
    /// Last date evidence of engagement was observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_relevant: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntentionKind {
    /// Core values, identity — emerges over months
    Mission,
    /// Time-bound target
    Goal,
    /// Recurring intention
    Habit,
    /// Promise to someone (auto-extracted from audio)
    Commitment,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntentionSource {
    /// Typed in manually on /intentions page
    UserDefined,
    /// Emerged from evening check-in dialogue
    CheckIn,
    /// Observed from behavior patterns, awaiting confirmation
    Inferred,
    /// Auto-extracted from audio (commitments to others)
    Extracted,
}

/// Habit frequency. See § Supporting Types.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cadence {
    pub times: u32,
    pub period: Period,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Daily,
    Weekly,
    Monthly,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn habit_with_cadence_roundtrip() {
        let i = Intention {
            id: "int_gym".into(),
            kind: IntentionKind::Habit,
            description: "Gym 3x/week".into(),
            domain: "Health".into(),
            active: true,
            created: d("2026-04-01"),
            target_date: None,
            cadence: Some(Cadence { times: 3, period: Period::Weekly }),
            source: IntentionSource::CheckIn,
            confirmed: true,
            last_relevant: Some(d("2026-04-10")),
        };
        let json = serde_json::to_string(&i).unwrap();
        let parsed: Intention = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn goal_with_target_date() {
        let i = Intention {
            id: "int_migration".into(),
            kind: IntentionKind::Goal,
            description: "Ship migration by end of month".into(),
            domain: "Career".into(),
            active: true,
            created: d("2026-04-01"),
            target_date: Some(d("2026-04-30")),
            cadence: None,
            source: IntentionSource::UserDefined,
            confirmed: true,
            last_relevant: None,
        };
        let json = serde_json::to_string(&i).unwrap();
        assert!(json.contains("\"target_date\":\"2026-04-30\""));
    }

    #[test]
    fn intention_kinds_snake_case() {
        for (v, expected) in [
            (IntentionKind::Mission, "\"mission\""),
            (IntentionKind::Goal, "\"goal\""),
            (IntentionKind::Habit, "\"habit\""),
            (IntentionKind::Commitment, "\"commitment\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), expected);
        }
    }

    #[test]
    fn intention_sources_snake_case() {
        assert_eq!(serde_json::to_string(&IntentionSource::UserDefined).unwrap(), "\"user_defined\"");
        assert_eq!(serde_json::to_string(&IntentionSource::CheckIn).unwrap(), "\"check_in\"");
        assert_eq!(serde_json::to_string(&IntentionSource::Inferred).unwrap(), "\"inferred\"");
        assert_eq!(serde_json::to_string(&IntentionSource::Extracted).unwrap(), "\"extracted\"");
    }

    #[test]
    fn cadence_periods_snake_case() {
        for (v, expected) in [
            (Period::Daily, "\"daily\""),
            (Period::Weekly, "\"weekly\""),
            (Period::Monthly, "\"monthly\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), expected);
        }
    }

    #[test]
    fn missing_optional_fields_deserialize() {
        let json = r#"{
            "id": "int_x",
            "kind": "goal",
            "description": "x",
            "domain": "Career",
            "active": true,
            "created": "2026-04-01",
            "source": "user_defined"
        }"#;
        let i: Intention = serde_json::from_str(json).unwrap();
        assert!(i.target_date.is_none());
        assert!(i.cadence.is_none());
        assert!(!i.confirmed);
        assert!(i.last_relevant.is_none());
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Edit `crates/alvum-core/src/lib.rs` — insert `pub mod intention;` alphabetically after `pub mod evidence;`:

```rust
pub mod evidence;
pub mod intention;
pub mod llm;
```

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core intention::
```

Expected: 6 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/intention.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add Intention with kind, source, and cadence"
```

---

## Task 6: Decision & CausalLink Refactor

Six sub-commits. Each changes `crates/alvum-core/src/decision.rs` and (in places) `crates/alvum-pipeline/src/causal.rs` / `distill.rs` / storage tests. Every sub-commit must leave the workspace green (`cargo test` passes).

**Files:**
- Modify: `crates/alvum-core/src/decision.rs`
- Modify: `crates/alvum-core/src/storage.rs` (tests reference Decision literals)
- Modify: `crates/alvum-pipeline/src/causal.rs`
- Touch: any other pipeline file that constructs `Decision` literals (grep in Step 1 below).

### 6.1 Rename `source: String` → `connector: String`

Semantics: current field is actually the connector name. The spec's `source` is a different thing (the modality enum). Rename first to free up the name.

- [ ] **Step 1: Enumerate callers.**

```bash
grep -rn '\.source\b\|source:' crates/ --include='*.rs' | grep -v 'observation\|Observation\|data_ref\|DataRef\|artifact\|Artifact' | grep -v '^crates/alvum-core/src/decision'
```

Record the files that touch `Decision.source` or construct `Decision { source: ..., }`. Expected set: `crates/alvum-core/src/storage.rs` (tests), `crates/alvum-pipeline/src/distill.rs`, `crates/alvum-pipeline/src/briefing.rs`, `crates/alvum-pipeline/src/causal.rs` (maybe not — verify).

- [ ] **Step 2: Rename the field in `decision.rs`.**

In `crates/alvum-core/src/decision.rs`, change:

```rust
pub source: String,
```

to:

```rust
/// The connector that produced the observations this decision was extracted from
/// (e.g., "claude-code", "audio-mic", "screen"). Per-run source tracking.
pub connector: String,
```

In the same file, update every `source: "..."` in the test module (lines ~121-242) to `connector: "..."`.

- [ ] **Step 3: Update each caller.**

For every file from Step 1:
- Find `source: "..."` in a `Decision { ... }` literal → change to `connector: "..."`.
- Find `dec.source` or `decision.source` field access → change to `dec.connector` / `decision.connector`.
- Leave `obs.source` (Observation) untouched — that's a different type.

Commands to help locate:

```bash
grep -n 'Decision {' crates/alvum-pipeline/src/*.rs
grep -n '\.source' crates/alvum-pipeline/src/*.rs
```

- [ ] **Step 4: Run the full workspace tests.**

```bash
cargo test --workspace
```

Expected: all prior tests pass.

- [ ] **Step 5: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 6: Commit.**

```bash
git add -u
git commit -m "refactor(core): rename Decision.source to Decision.connector"
```

### 6.2 `Decision.timestamp: String` → `DateTime<Utc>`

- [ ] **Step 1: Add a failing test.**

In `crates/alvum-core/src/decision.rs` tests module, add:

```rust
#[test]
fn timestamp_roundtrips_as_iso8601() {
    use chrono::{DateTime, Utc};
    let dec = Decision {
        id: "dec_001".into(),
        timestamp: "2026-04-02T04:35:00Z".parse::<DateTime<Utc>>().unwrap(),
        summary: "x".into(),
        reasoning: None,
        alternatives: vec![],
        domain: "x".into(),
        connector: "claude-code".into(),
        proposed_by: self_attr(0.9),
        status: DecisionStatus::ActedOn,
        resolved_by: Some(self_attr(0.9)),
        causes: vec![],
        tags: vec![],
        expected_outcome: None,
    };
    let json = serde_json::to_string(&dec).unwrap();
    assert!(json.contains("\"timestamp\":\"2026-04-02T04:35:00Z\""));
    let back: Decision = serde_json::from_str(&json).unwrap();
    assert_eq!(back.timestamp, dec.timestamp);
}
```

- [ ] **Step 2: Run — expect compile failure because `timestamp: String` doesn't parse from `&str`.**

```bash
cargo test -p alvum-core decision::tests::timestamp_roundtrips_as_iso8601
```

Expected: compile error.

- [ ] **Step 3: Change the field type.**

Add `use chrono::{DateTime, Utc};` at the top of `decision.rs` if not present. Change:

```rust
pub timestamp: String,
```

to:

```rust
pub timestamp: chrono::DateTime<chrono::Utc>,
```

- [ ] **Step 4: Update existing literals in the same file.**

Every existing test in `decision.rs` constructs `Decision { timestamp: "..." }`. Change each to `timestamp: "...".parse().unwrap()`.

Also update `crates/alvum-core/src/storage.rs` test module (lines ~59-88) similarly: `timestamp: "...".parse().unwrap()`.

- [ ] **Step 5: Update pipeline callers.**

```bash
grep -rn 'timestamp: "' crates/alvum-pipeline/
```

For each hit inside a `Decision { ... }` literal, change to `timestamp: "...".parse().unwrap()` (or whatever idiom matches — may need `use chrono::{DateTime, Utc};`).

If the pipeline constructs `Decision.timestamp` from an LLM string, wrap the parse: replace `timestamp: raw_string.to_string()` with `timestamp: raw_string.parse().context("decision timestamp")?` and propagate the `Result`.

- [ ] **Step 6: Run full workspace tests.**

```bash
cargo test --workspace
```

Expected: all green.

- [ ] **Step 7: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 8: Commit.**

```bash
git add -u
git commit -m "refactor(core): Decision.timestamp becomes DateTime<Utc>"
```

### 6.3 Add `CausalMechanism` enum and update `CausalLink`

- [ ] **Step 1: Add failing test.**

In `crates/alvum-core/src/decision.rs` tests module, add:

```rust
#[test]
fn causal_mechanism_serializes_snake_case() {
    for (v, expected) in [
        (CausalMechanism::Direct, "\"direct\""),
        (CausalMechanism::ResourceCompetition, "\"resource_competition\""),
        (CausalMechanism::EmotionalInfluence, "\"emotional_influence\""),
        (CausalMechanism::Precedent, "\"precedent\""),
        (CausalMechanism::Constraint, "\"constraint\""),
        (CausalMechanism::Accumulation, "\"accumulation\""),
    ] {
        assert_eq!(serde_json::to_string(&v).unwrap(), expected);
    }
}

#[test]
fn causal_link_with_cross_domain() {
    let link = CausalLink {
        from_id: "dec_003".into(),
        mechanism: CausalMechanism::ResourceCompetition,
        strength: CausalStrength::Primary,
        cross_domain: Some(("Career".into(), "Health".into())),
    };
    let json = serde_json::to_string(&link).unwrap();
    let back: CausalLink = serde_json::from_str(&json).unwrap();
    assert_eq!(back, link);
}
```

- [ ] **Step 2: Run — compile error.**

```bash
cargo test -p alvum-core decision::tests::causal_mechanism
```

- [ ] **Step 3: Add the enum and extend `CausalLink`.**

In `crates/alvum-core/src/decision.rs`, add after the `CausalStrength` enum:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CausalMechanism {
    /// "because of X, I did Y"
    Direct,
    /// X consumed time/energy that Y needed
    ResourceCompetition,
    /// X created a feeling that shaped Y
    EmotionalInfluence,
    /// X set a pattern that Y followed
    Precedent,
    /// X eliminated options, forcing Y
    Constraint,
    /// X contributed to a state that triggered Y
    Accumulation,
}
```

Change `CausalLink`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CausalLink {
    pub from_id: String,
    pub mechanism: CausalMechanism,
    pub strength: CausalStrength,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_domain: Option<(String, String)>,
}
```

- [ ] **Step 4: Update existing `CausalLink` literals in `decision.rs` tests.**

The existing `roundtrip_decision_with_actors` test constructs `CausalLink { mechanism: "direct".into(), ... }` — change to `mechanism: CausalMechanism::Direct,` and add `cross_domain: None`.

Similarly update the `serialize_causal_link` test: `mechanism: CausalMechanism::Direct` replaces `"User pushback on oversimplification".into()`. Update the assertion to check `"direct"` string.

Actually the existing test relies on a free-text mechanism. Keep the original mechanism description and shift it into a new field if needed — but the spec's mechanism is the enum. Drop the free-text aspect; the LLM will classify into one of the enum values. This is a deliberate semantic shift.

- [ ] **Step 5: Update pipeline caller `causal.rs`.**

In `crates/alvum-pipeline/src/causal.rs:44-51`, `CausalLinkRaw` has `mechanism: String`. Change it to:

```rust
#[derive(Debug, Deserialize)]
struct CausalLinkRaw {
    from_id: String,
    mechanism: CausalMechanism,   // was: String
    strength: CausalStrength,
    #[serde(default)]
    cross_domain: Option<(String, String)>,
}
```

Add `use alvum_core::decision::CausalMechanism;` at the top.

At line ~92, where `CausalLink` is constructed, update the field list:

```rust
dec.causes.push(CausalLink {
    from_id: link.from_id.clone(),
    mechanism: link.mechanism,
    strength: link.strength.clone(),
    cross_domain: link.cross_domain.clone(),
});
```

- [ ] **Step 6: Update the LLM prompt in `causal.rs`.**

The prompt (around lines 10-40 of `causal.rs`) currently tells the LLM to produce a free-text `mechanism`. Change the relevant prompt section to enumerate the six valid values:

```
- mechanism: one of "direct", "resource_competition", "emotional_influence", "precedent", "constraint", "accumulation"
```

And update the example JSON:

```
"causes": [
  {"from_id": "dec_003", "mechanism": "constraint", "strength": "primary", "cross_domain": null},
  {"from_id": "dec_001", "mechanism": "precedent", "strength": "background", "cross_domain": null}
]
```

Add a short description of each mechanism so the LLM picks correctly. Use the same comments as on the enum variants.

- [ ] **Step 7: Run the full workspace tests.**

```bash
cargo test --workspace
```

Expected: all green.

- [ ] **Step 8: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 9: Commit.**

```bash
git add -u
git commit -m "refactor(core): CausalLink uses CausalMechanism enum and cross_domain"
```

### 6.4 Add `DecisionSource` enum and `source` field

- [ ] **Step 1: Add the enum and field.**

In `crates/alvum-core/src/decision.rs`, after `CausalMechanism`:

```rust
/// Modality by which the decision was detected.
/// Distinct from `connector` which names the capture source.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// From conversation transcript
    Spoken,
    /// Inferred from behavioral observation
    Revealed,
    /// From evening check-in response
    Explained,
}
```

Add a field to `Decision` (alphabetically near `connector` is fine; but keep field order stable for serde — append to the end of existing fields for now, before any Vec fields that already have serde defaults):

```rust
pub source: DecisionSource,
```

- [ ] **Step 2: Add a failing test.**

```rust
#[test]
fn decision_source_snake_case() {
    assert_eq!(serde_json::to_string(&DecisionSource::Spoken).unwrap(), "\"spoken\"");
    assert_eq!(serde_json::to_string(&DecisionSource::Revealed).unwrap(), "\"revealed\"");
    assert_eq!(serde_json::to_string(&DecisionSource::Explained).unwrap(), "\"explained\"");
}
```

- [ ] **Step 3: Update every Decision literal in the codebase.**

Every test / pipeline caller that constructs a `Decision { ... }` must now include `source: DecisionSource::<variant>`. For the MVP extractor, the LLM only produces `Spoken` (from claude-code transcript) — default to that in the extraction prompt.

Grep:
```bash
grep -rn 'Decision {' crates/ --include='*.rs'
```

For each hit, add `source: DecisionSource::Spoken` (Claude-Code extracted decisions all start as Spoken).

Add `use alvum_core::decision::DecisionSource;` where needed.

- [ ] **Step 4: Update the distill prompt.**

In `crates/alvum-pipeline/src/distill.rs`, the JSON schema sent to the LLM must now include `source` as one of `"spoken"`, `"revealed"`, `"explained"`. For Claude-Code derived decisions, hard-code `"spoken"` in the post-processing (don't rely on the LLM to know which source it came from — the pipeline knows from context).

If the LLM schema has `source` already (serving a different purpose), rename that to `origin_context` or drop it; the new spec-aligned `source` field takes precedence.

- [ ] **Step 5: Run full tests.**

```bash
cargo test --workspace
```

- [ ] **Step 6: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 7: Commit.**

```bash
git add -u
git commit -m "feat(core): add DecisionSource enum to Decision"
```

### 6.5 Add Decision outcome-tracking fields

Add `effects`, `contributing_states`, `check_by`, `actual_outcome`, `cascade_depth`, `cross_domain_effects`. All default empty/None so existing serialized decisions still deserialize.

- [ ] **Step 1: Add failing test.**

```rust
#[test]
fn decision_outcome_fields_default_empty_when_missing() {
    // Old on-disk decision written before these fields existed should still load.
    let json = r#"{
        "id": "dec_old",
        "timestamp": "2026-04-02T04:35:00Z",
        "summary": "x",
        "reasoning": null,
        "alternatives": [],
        "domain": "x",
        "connector": "claude-code",
        "proposed_by": {"actor": {"name": "user", "kind": "self"}, "confidence": 0.9},
        "status": "acted_on",
        "resolved_by": null,
        "causes": [],
        "tags": [],
        "expected_outcome": null,
        "source": "spoken"
    }"#;
    let dec: Decision = serde_json::from_str(json).unwrap();
    assert!(dec.effects.is_empty());
    assert!(dec.contributing_states.is_empty());
    assert!(dec.check_by.is_none());
    assert!(dec.actual_outcome.is_none());
    assert!(dec.cascade_depth.is_none());
    assert!(dec.cross_domain_effects.is_empty());
}
```

- [ ] **Step 2: Run — compile error.**

- [ ] **Step 3: Add fields to `Decision`.**

Insert after `causes`:

```rust
/// Downstream decisions that this one caused. Filled in over time as effects become observable.
#[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
pub effects: Vec<CausalLink>,

/// IDs of emergent states active at decision time.
#[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
pub contributing_states: Vec<String>,

/// When the expected outcome should be checked.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub check_by: Option<chrono::NaiveDate>,

/// What actually happened. Filled in once observed.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub actual_outcome: Option<String>,

/// Count of downstream decisions in the causal chain. Filled in by Link stage.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cascade_depth: Option<u32>,

/// Names of life domains this decision affected besides its own.
#[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
pub cross_domain_effects: Vec<String>,
```

- [ ] **Step 4: Update every existing Decision literal.**

Existing test cases in `decision.rs` and `storage.rs` construct `Decision` literals without these fields — they'll compile-error because of the missing fields. Easiest path: add defaults to each literal:

```rust
effects: vec![],
contributing_states: vec![],
check_by: None,
actual_outcome: None,
cascade_depth: None,
cross_domain_effects: vec![],
```

Alternative: create a `Decision::new_minimal()` test helper. YAGNI for now — add the fields inline.

- [ ] **Step 5: Run tests.**

```bash
cargo test --workspace
```

- [ ] **Step 6: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 7: Commit.**

```bash
git add -u
git commit -m "feat(core): add outcome-tracking fields to Decision"
```

### 6.6 Add Decision evidence, conflicts, participants

Final Decision refactor step. Depends on Task 2 (Evidence module) already merged.

- [ ] **Step 1: Add failing test.**

```rust
#[test]
fn decision_with_evidence_and_conflicts_roundtrips() {
    use crate::evidence::{Confidence, ConflictType, Evidence, ModalClaim, ModalConflict};
    let dec = Decision {
        id: "dec_042".into(),
        timestamp: "2026-04-11T20:00:00Z".parse().unwrap(),
        summary: "Skipped gym".into(),
        reasoning: Some("Stayed at office".into()),
        alternatives: vec![],
        domain: "Health".into(),
        connector: "screen".into(),
        proposed_by: self_attr(0.9),
        status: DecisionStatus::Ignored,
        resolved_by: None,
        causes: vec![],
        tags: vec![],
        expected_outcome: None,
        source: DecisionSource::Revealed,
        effects: vec![],
        contributing_states: vec![],
        check_by: None,
        actual_outcome: None,
        cascade_depth: None,
        cross_domain_effects: vec![],
        participants: vec![],
        evidence: vec![Evidence {
            source: "location".into(),
            timestamp: "2026-04-11T20:00:00Z".parse().unwrap(),
            description: "Still at office".into(),
            confidence: Confidence::High,
        }],
        conflicts: vec![ModalConflict {
            stated: ModalClaim {
                source: "audio-mic".into(),
                description: "Gym at 6pm".into(),
                timestamp: "2026-04-11T09:00:00Z".parse().unwrap(),
            },
            observed: ModalClaim {
                source: "location".into(),
                description: "Office at 20:00".into(),
                timestamp: "2026-04-11T20:00:00Z".parse().unwrap(),
            },
            conflict_type: ConflictType::IntendVsDo,
        }],
    };
    let json = serde_json::to_string(&dec).unwrap();
    let back: Decision = serde_json::from_str(&json).unwrap();
    assert_eq!(back, dec);
}
```

- [ ] **Step 2: Run — compile error.**

- [ ] **Step 3: Add fields to `Decision`.**

Insert at the end (after `cross_domain_effects`):

```rust
/// Who else took part — names or roles, free-form.
#[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
pub participants: Vec<String>,

/// Evidence chain supporting this decision.
#[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
pub evidence: Vec<crate::evidence::Evidence>,

/// Cross-modal contradictions detected at decision time.
#[serde(default, deserialize_with = "deserialize_null_as_empty_vec")]
pub conflicts: Vec<crate::evidence::ModalConflict>,
```

- [ ] **Step 4: Update every existing Decision literal again.**

Add `participants: vec![]`, `evidence: vec![]`, `conflicts: vec![]` to each literal that now fails to compile.

- [ ] **Step 5: Run tests.**

```bash
cargo test --workspace
```

- [ ] **Step 6: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 7: Commit.**

```bash
git add -u
git commit -m "feat(core): add participants, evidence, and conflicts to Decision"
```

---

## Task 7: Supporting Day-Level Types

`Event`, `Commitment`, `ActivityBlock`, `CapturePayload`, `ActivityKind` — the building blocks of `DayExtraction`. (§ Data Model — Supporting Types.)

**Files:**
- Create: `crates/alvum-core/src/day_support.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/day_support.rs`:

```rust
//! Supporting value types used by DayExtraction.
//! See § Data Model — Supporting Types.

use crate::evidence::Evidence;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// Like Decision but without outcome tracking or causal links.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub summary: String,
    pub domain: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

/// Auto-extracted promise: who you promised, what, by when, fulfilled status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Commitment {
    pub id: String,
    pub to: String,
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<NaiveDate>,
    #[serde(default)]
    pub fulfilled: bool,
    /// When extracted.
    pub extracted: DateTime<Utc>,
    /// User explicitly confirmed this commitment in evening check-in.
    #[serde(default)]
    pub confirmed: bool,
}

/// A time range with a classified activity type and references to its capture data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActivityBlock {
    pub id: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub kind: ActivityKind,
    /// Paths to captured data (audio chunks, frames, events.jsonl ranges, ...).
    #[serde(default)]
    pub capture_refs: Vec<String>,
    /// Optional human/LLM-readable summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Meeting,
    DeepWork,
    Transit,
    Conversation,
    Idle,
    /// Unclassified/other — prefer a specific kind when possible.
    Other,
}

/// Capture payload variants — either a file-on-disk reference or inline structured data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CapturePayload {
    /// Pointer to a file on disk.
    FilePath { path: String, mime: String },
    /// Inline JSON data (small, self-contained events).
    Inline { value: serde_json::Value },
    /// Inline text (e.g., accessibility snapshot summary).
    Text { content: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn event_roundtrip() {
        let e = Event {
            id: "evt_001".into(),
            timestamp: ts("2026-04-11T10:00:00Z"),
            summary: "Morning standup".into(),
            domain: "Career".into(),
            evidence: vec![],
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn commitment_with_deadline() {
        let c = Commitment {
            id: "com_001".into(),
            to: "Sarah".into(),
            what: "Send migration spec".into(),
            by: Some(NaiveDate::from_ymd_opt(2026, 4, 15).unwrap()),
            fulfilled: false,
            extracted: ts("2026-04-11T10:00:00Z"),
            confirmed: true,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"by\":\"2026-04-15\""));
        let back: Commitment = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn activity_block_roundtrip() {
        let b = ActivityBlock {
            id: "blk_001".into(),
            start: ts("2026-04-11T09:00:00Z"),
            end: ts("2026-04-11T10:00:00Z"),
            kind: ActivityKind::Meeting,
            capture_refs: vec!["capture/2026-04-11/audio/mic/09-00-00.opus".into()],
            summary: Some("Weekly team sync".into()),
        };
        let json = serde_json::to_string(&b).unwrap();
        let back: ActivityBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn capture_payload_tagged_variants() {
        let p1 = CapturePayload::FilePath {
            path: "capture/2026-04-11/audio/mic/10-00-00.opus".into(),
            mime: "audio/opus".into(),
        };
        let j1 = serde_json::to_string(&p1).unwrap();
        assert!(j1.contains("\"type\":\"file_path\""));

        let p2 = CapturePayload::Text { content: "hello".into() };
        let j2 = serde_json::to_string(&p2).unwrap();
        assert!(j2.contains("\"type\":\"text\""));

        let p3 = CapturePayload::Inline { value: serde_json::json!({"k": 1}) };
        let j3 = serde_json::to_string(&p3).unwrap();
        assert!(j3.contains("\"type\":\"inline\""));

        let back: CapturePayload = serde_json::from_str(&j1).unwrap();
        assert_eq!(back, p1);
    }

    #[test]
    fn activity_kinds_snake_case() {
        assert_eq!(serde_json::to_string(&ActivityKind::DeepWork).unwrap(), "\"deep_work\"");
        assert_eq!(serde_json::to_string(&ActivityKind::Meeting).unwrap(), "\"meeting\"");
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Insert `pub mod day_support;` alphabetically after `pub mod data_ref;`:

```rust
pub mod data_ref;
pub mod day_support;
pub mod decision;
```

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core day_support::
```

Expected: 5 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/day_support.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add Event, Commitment, ActivityBlock, CapturePayload"
```

---

## Task 8: Alignment Module

`AlignmentReport`, `AlignmentItem`, `AlignmentStatus`, `Trend`. (§ Data Model — Alignment.)

**Files:**
- Create: `crates/alvum-core/src/alignment.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/alignment.rs`:

```rust
//! Alignment reports — the output of comparing intentions against observed reality.
//! See § Data Model — Alignment.

use crate::evidence::Evidence;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlignmentReport {
    pub date: NaiveDate,
    #[serde(default)]
    pub items: Vec<AlignmentItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AlignmentItem {
    pub intention_id: String,
    pub status: AlignmentStatus,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    pub trend: Trend,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streak: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AlignmentStatus {
    Aligned,
    Drifting { gap_description: String },
    Violated { description: String },
    NoEvidence,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Trend {
    Improving,
    Declining,
    Stable,
    InsufficientData,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn aligned_status_serializes_cleanly() {
        let s = AlignmentStatus::Aligned;
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"status":"aligned"}"#);
    }

    #[test]
    fn drifting_status_carries_gap() {
        let s = AlignmentStatus::Drifting { gap_description: "Skipped 2 of 3".into() };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"status\":\"drifting\""));
        assert!(json.contains("Skipped 2 of 3"));
        let back: AlignmentStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn alignment_item_roundtrip() {
        let item = AlignmentItem {
            intention_id: "int_gym".into(),
            status: AlignmentStatus::Drifting { gap_description: "1 of 3 this week".into() },
            evidence: vec![],
            trend: Trend::Declining,
            streak: Some(-2),
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: AlignmentItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back, item);
    }

    #[test]
    fn alignment_report_roundtrip() {
        let r = AlignmentReport {
            date: d("2026-04-11"),
            items: vec![],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: AlignmentReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn trend_variants_snake_case() {
        assert_eq!(serde_json::to_string(&Trend::Improving).unwrap(), "\"improving\"");
        assert_eq!(serde_json::to_string(&Trend::InsufficientData).unwrap(), "\"insufficient_data\"");
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Insert `pub mod alignment;` alphabetically after `pub mod artifact;` — but careful, that collides with where you put `behavior`. The final order should be:

```rust
pub mod alignment;
pub mod artifact;
pub mod behavior;
pub mod capture;
pub mod config;
pub mod connector;
pub mod data_ref;
pub mod day;
pub mod day_support;
pub mod decision;
pub mod evidence;
pub mod health;
pub mod intention;
pub mod llm;
pub mod location;
pub mod observation;
pub mod paths;
pub mod processor;
pub mod state;
pub mod storage;
pub mod util;
```

(Tasks 9, 11, and 12 — `day`, `location`, `health` — insert later; listed here so the final canonical order is unambiguous.)

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core alignment::
```

Expected: 5 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/alignment.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add AlignmentReport, AlignmentItem, AlignmentStatus, Trend"
```

---

## Task 9: DayExtraction Module

Brings the day-level types together into a single serializable document (§ Data Model — Day Extraction).

**Files:**
- Create: `crates/alvum-core/src/day.rs`
- Modify: `crates/alvum-core/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/day.rs`:

```rust
//! Pipeline's complete output for a single day.
//! See § Data Model — Day Extraction.

use crate::alignment::AlignmentReport;
use crate::behavior::BehavioralSignal;
use crate::day_support::{ActivityBlock, Commitment, Event};
use crate::decision::Decision;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DayExtraction {
    pub date: NaiveDate,
    #[serde(default)]
    pub activity_blocks: Vec<ActivityBlock>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub decisions: Vec<Decision>,
    #[serde(default)]
    pub commitments: Vec<Commitment>,
    #[serde(default)]
    pub behavioral_signals: Vec<BehavioralSignal>,
    pub alignment: AlignmentReport,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn empty_day_extraction_roundtrip() {
        let dx = DayExtraction {
            date: d("2026-04-11"),
            activity_blocks: vec![],
            events: vec![],
            decisions: vec![],
            commitments: vec![],
            behavioral_signals: vec![],
            alignment: AlignmentReport {
                date: d("2026-04-11"),
                items: vec![],
            },
        };
        let json = serde_json::to_string(&dx).unwrap();
        let back: DayExtraction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, dx);
    }

    #[test]
    fn partial_day_from_legacy_json_deserializes() {
        // A day file written before some arrays existed should still load.
        let json = r#"{
            "date": "2026-04-11",
            "alignment": {"date": "2026-04-11"}
        }"#;
        let dx: DayExtraction = serde_json::from_str(json).unwrap();
        assert!(dx.decisions.is_empty());
        assert!(dx.events.is_empty());
        assert!(dx.commitments.is_empty());
    }
}
```

- [ ] **Step 2: Wire into `lib.rs`.**

Insert `pub mod day;` alphabetically after `pub mod data_ref;`, before `pub mod day_support;`:

```rust
pub mod data_ref;
pub mod day;
pub mod day_support;
```

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-core day::
```

Expected: 2 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-core/src/day.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add DayExtraction (pipeline's per-day output shape)"
```

---

## Task 10: Storage Migration in CLI

Move the CLI from writing to `./output/` to writing to the `~/.alvum/` layout via `AlvumPaths` (per `docs/superpowers/specs/2026-04-18-storage-layout.md`). Pipeline writers land in `generated/`; no repo-relative paths remain. Support an override for tests and development (`--output <path>` substitutes the root).

**Files:**
- Modify: `crates/alvum-cli/src/main.rs`
- Modify: `crates/alvum-pipeline/src/extract.rs` (or wherever the output dir is threaded through)
- Modify: `crates/alvum-core/src/config.rs` (update default `output_dir` semantics)
- Add end-to-end test: `crates/alvum-cli/tests/storage_paths.rs` (new file) OR add to an existing test if one exists.

- [ ] **Step 1: Inspect current output-dir plumbing.**

```bash
grep -rn 'output_dir\|output/' crates/alvum-cli/src/ crates/alvum-pipeline/src/ crates/alvum-core/src/config.rs
```

Note every place `output_dir: PathBuf` is passed. Expected path: CLI takes `--output` flag → threaded into pipeline → used to construct `decisions.jsonl` / `briefing.md` / `threads.json` file paths.

- [ ] **Step 2: Change default output dir in `config.rs`.**

In `crates/alvum-core/src/config.rs`, replace:

```rust
fn default_output_dir() -> PathBuf { PathBuf::from("output") }
```

with:

```rust
fn default_output_dir() -> PathBuf {
    // The pipeline writes briefings; point default_output_dir at where they land.
    // Per storage-layout spec: ~/.alvum/generated/briefings/.
    crate::paths::AlvumPaths::default_root()
        .map(|p| p.generated_dir().join("briefings"))
        .unwrap_or_else(|_| PathBuf::from("output"))
}
```

- [ ] **Step 3: Add failing test for the new default.**

In `crates/alvum-core/src/config.rs` tests module, add:

```rust
#[test]
fn default_output_dir_points_to_generated_briefings() {
    // Per storage-layout spec, pipeline output lands at ~/.alvum/generated/briefings.
    // Fallback to "output" only if dirs::home_dir() somehow fails (CI edge case).
    let config = AlvumConfig::default();
    let s = config.pipeline.output_dir.to_string_lossy().to_string();
    assert!(
        s.ends_with("generated/briefings") || s == "output",
        "unexpected default output_dir: {s}"
    );
}
```

- [ ] **Step 4: Run tests.**

```bash
cargo test -p alvum-core config::
```

Expected: pass.

- [ ] **Step 5: Migrate pipeline writers to use AlvumPaths-derived paths.**

In `crates/alvum-pipeline/src/extract.rs` (and any other writer), replace literal subpaths with `AlvumPaths` method calls. Example:

Before:
```rust
let out = output_dir.join("decisions.jsonl");
```

After:
```rust
use alvum_core::paths::AlvumPaths;
let paths = AlvumPaths::with_root(output_dir);
let out = paths.decisions_index();
```

Do this for at minimum:
- `decisions.jsonl` → `paths.decisions_index()` (now `generated/decisions/index.jsonl`)
- Flat `briefing.md` → `paths.briefing_file(today)` (now `generated/briefings/<date>/briefing.md`)
- `threads.json` → `paths.threads_file(today)` (now `generated/episodes/<date>/threads.json`)
- `knowledge/entities.jsonl` etc. → `paths.knowledge_entities()` etc. (now under `generated/`)

Where `today` comes from: use the most recent timestamp in the observations, falling back to `chrono::Utc::now().date_naive()`.

**Note on the `--output` flag**: when the user passes `--output <path>`, that path is treated as the `AlvumPaths` *root*, not as a flat output directory. `AlvumPaths::with_root(&override_path)` creates a paths helper rooted there; briefings land at `<override>/generated/briefings/<date>/briefing.md`. This keeps dev overrides (and tests) structurally identical to production installs.

- [ ] **Step 6: Add an end-to-end smoke test.**

Create `crates/alvum-cli/tests/storage_paths.rs`:

```rust
//! End-to-end smoke test: run CLI `extract` against a TempDir override and assert
//! that output files land in the spec-mandated layout under the override root.

use std::process::Command;
use tempfile::TempDir;

#[test]
#[ignore = "requires claude CLI auth; enable manually"]
fn extract_writes_to_spec_paths() {
    let tmp = TempDir::new().unwrap();
    let bin = env!("CARGO_BIN_EXE_alvum-cli");

    // We assume there's a fixture session. If not, this test is skipped; see ignore attribute above.
    let fixture = "tests/fixtures/session.jsonl";
    if !std::path::Path::new(fixture).exists() {
        eprintln!("skipping: no session fixture at {fixture}");
        return;
    }

    let out = Command::new(bin)
        .arg("extract")
        .arg("--session").arg(fixture)
        .arg("--output").arg(tmp.path())
        .output()
        .expect("run cli");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    // Override root is $tmp; expected layout from the storage-layout spec:
    //   $tmp/generated/decisions/index.jsonl
    //   $tmp/generated/briefings/<date>/briefing.md
    let generated = tmp.path().join("generated");
    assert!(generated.join("decisions").join("index.jsonl").exists(),
        "no decisions index at generated/decisions/index.jsonl");
    assert!(
        generated.join("briefings").read_dir().map_or(false, |mut it| it.next().is_some()),
        "no briefing directory produced at generated/briefings/"
    );
}
```

Mark it `#[ignore]` because it needs `claude` auth to run end-to-end. The point is to have it compile and be runnable manually; gated so CI stays clean.

- [ ] **Step 7: Run the workspace tests.**

```bash
cargo test --workspace
```

Expected: all green; the ignored test does not run.

- [ ] **Step 8: Clippy.**

```bash
cargo clippy --workspace -- -D warnings
```

- [ ] **Step 9: Manual verification.**

```bash
cargo run -p alvum-cli -- extract \
    --session ~/.claude/projects/<some-project>/<some-session>.jsonl \
    --output /tmp/alvum-smoke-test
```

Then:

```bash
ls /tmp/alvum-smoke-test
# Expected: generated/ (containing decisions/, briefings/, episodes/, knowledge/)
ls /tmp/alvum-smoke-test/generated
# Expected: decisions/  briefings/  episodes/  knowledge/
```

- [ ] **Step 10: Commit.**

```bash
git add -u
git add crates/alvum-cli/tests/storage_paths.rs
git commit -m "feat(cli): write outputs to spec-mandated AlvumPaths layout"
```

---

## Task 11: Location Primitives Module

Types the alignment engine consumes to reason about where the user was. Per `docs/superpowers/specs/2026-04-18-location-map.md` § Data Model. Fusion logic, third-party integration (Strava, HealthKit routes), and rendering live in later phases; **only the types land in Phase A**.

**Files:**
- Create: `crates/alvum-core/src/location.rs`
- Modify: `crates/alvum-core/src/lib.rs`
- Modify: `crates/alvum-core/src/paths.rs` (add location path helpers)

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/location.rs`:

```rust
//! Location observations: point / route / transit.
//! See `docs/superpowers/specs/2026-04-18-location-map.md` § Data Model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LocationObservation {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub kind: LocationKind,
    pub source: LocationSource,
    /// Which device produced the raw data (if from a device; None for pulled sources).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// Human-readable label ("Prospect Park loop", "Home · Park Slope").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Free-form detail ("5.2 mi · 8'42\" pace · 312 ft gain").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub confidence: LocationConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocationKind {
    Point {
        at: GeoPoint,
        /// PlaceEntity id if matched against the knowledge corpus.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place_id: Option<String>,
    },
    Route {
        points: Vec<GeoPoint>,
        mode: MotionMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        distance_m: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gain_m: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        moving_s: Option<u32>,
    },
    Transit {
        origin: GeoPoint,
        destination: GeoPoint,
        steps: Vec<TransitStep>,
        total_duration_s: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MotionMode {
    Walk,
    Run,
    Bike,
    Drive,
    /// Motorcycle, scooter, etc.
    Motor,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransitStep {
    pub kind: TransitStepKind,
    pub label: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<f32>,
    /// Transit line identifier ("B", "Q", "M62").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stops: Option<u32>,
    #[serde(default)]
    pub points: Vec<GeoPoint>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransitStepKind {
    Walk,
    Subway,
    Bus,
    Train,
    Drive,
    Bike,
    Transfer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationSource {
    Iphone,
    Mac,
    Wearable,
    CarPlay,
    Strava,
    HealthKit,
    PhotoExif,
    Manual,
    Inferred,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LocationConfidence {
    High,
    Medium,
    Low,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn pt(lat: f64, lon: f64) -> GeoPoint {
        GeoPoint { lat, lon, ts: None, accuracy_m: None }
    }

    #[test]
    fn point_location_roundtrip() {
        let obs = LocationObservation {
            start: ts("2026-04-11T09:00:00Z"),
            end: ts("2026-04-11T17:30:00Z"),
            kind: LocationKind::Point {
                at: pt(40.7589, -73.9851),
                place_id: Some("place_office_midtown".into()),
            },
            source: LocationSource::Mac,
            device_id: Some("mbp".into()),
            label: Some("Office · Midtown".into()),
            detail: None,
            confidence: LocationConfidence::High,
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: LocationObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
    }

    #[test]
    fn route_location_roundtrip() {
        let obs = LocationObservation {
            start: ts("2026-04-11T06:05:00Z"),
            end: ts("2026-04-11T06:52:00Z"),
            kind: LocationKind::Route {
                points: vec![pt(40.6602, -73.9690), pt(40.6610, -73.9700)],
                mode: MotionMode::Run,
                distance_m: Some(8370.0),
                gain_m: Some(95.0),
                moving_s: Some(2722),
            },
            source: LocationSource::Strava,
            device_id: None,
            label: Some("Prospect Park loop".into()),
            detail: Some("5.2 mi · 8'42\" pace".into()),
            confidence: LocationConfidence::High,
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: LocationObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
        assert!(json.contains("\"kind\":\"route\""));
        assert!(json.contains("\"mode\":\"run\""));
    }

    #[test]
    fn transit_location_roundtrip() {
        let obs = LocationObservation {
            start: ts("2026-04-11T08:15:00Z"),
            end: ts("2026-04-11T09:00:00Z"),
            kind: LocationKind::Transit {
                origin: pt(40.6664, -73.9828),
                destination: pt(40.7580, -73.9855),
                total_duration_s: 2700,
                steps: vec![
                    TransitStep {
                        kind: TransitStepKind::Walk,
                        label: "Walk to 7 Av station".into(),
                        start: ts("2026-04-11T08:15:00Z"),
                        end: ts("2026-04-11T08:21:00Z"),
                        distance_m: Some(480.0),
                        line: None,
                        stops: None,
                        points: vec![],
                    },
                    TransitStep {
                        kind: TransitStepKind::Subway,
                        label: "B train → Atlantic Av–Barclays".into(),
                        start: ts("2026-04-11T08:21:00Z"),
                        end: ts("2026-04-11T08:29:00Z"),
                        distance_m: None,
                        line: Some("B".into()),
                        stops: Some(2),
                        points: vec![],
                    },
                ],
            },
            source: LocationSource::Iphone,
            device_id: Some("iphone".into()),
            label: Some("Park Slope → Midtown".into()),
            detail: Some("2 lines · 4 stops · 42 min".into()),
            confidence: LocationConfidence::High,
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: LocationObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
    }

    #[test]
    fn location_source_snake_case() {
        assert_eq!(serde_json::to_string(&LocationSource::Iphone).unwrap(), "\"iphone\"");
        assert_eq!(serde_json::to_string(&LocationSource::CarPlay).unwrap(), "\"car_play\"");
        assert_eq!(serde_json::to_string(&LocationSource::HealthKit).unwrap(), "\"health_kit\"");
        assert_eq!(serde_json::to_string(&LocationSource::PhotoExif).unwrap(), "\"photo_exif\"");
    }

    #[test]
    fn transit_step_kind_snake_case() {
        for (v, expected) in [
            (TransitStepKind::Walk, "\"walk\""),
            (TransitStepKind::Subway, "\"subway\""),
            (TransitStepKind::Transfer, "\"transfer\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), expected);
        }
    }

    #[test]
    fn motion_mode_snake_case() {
        assert_eq!(serde_json::to_string(&MotionMode::Walk).unwrap(), "\"walk\"");
        assert_eq!(serde_json::to_string(&MotionMode::Unknown).unwrap(), "\"unknown\"");
    }
}
```

- [ ] **Step 2: Add location path helpers to `paths.rs`.**

In `crates/alvum-core/src/paths.rs`, add inside `impl AlvumPaths`:

```rust
// --- location capture (raw per-source, pulled from third parties, fused) ---

pub fn location_raw_dir(&self, date: NaiveDate) -> PathBuf {
    self.capture_dir(date).join("location").join("raw")
}

pub fn location_raw_file(&self, date: NaiveDate, source: &str) -> PathBuf {
    // e.g. location_raw_file(d, "iphone") -> .../location/raw/iphone.jsonl
    self.location_raw_dir(date).join(format!("{source}.jsonl"))
}

pub fn location_pulled_dir(&self, date: NaiveDate) -> PathBuf {
    self.capture_dir(date).join("location").join("pulled")
}

pub fn location_fused_file(&self, date: NaiveDate) -> PathBuf {
    self.capture_dir(date).join("location").join("fused.jsonl")
}
```

Add corresponding tests to `paths.rs` tests module:

```rust
#[test]
fn location_paths() {
    let (p, _t) = paths();
    let date = d("2026-04-11");
    assert!(p.location_raw_dir(date).ends_with("capture/2026-04-11/location/raw"));
    assert!(p.location_raw_file(date, "iphone").ends_with("location/raw/iphone.jsonl"));
    assert!(p.location_pulled_dir(date).ends_with("location/pulled"));
    assert!(p.location_fused_file(date).ends_with("location/fused.jsonl"));
}
```

- [ ] **Step 3: Wire `location` into `lib.rs`.**

Insert `pub mod location;` alphabetically after `pub mod llm;`:

```rust
pub mod llm;
pub mod location;
pub mod observation;
```

- [ ] **Step 4: Run tests.**

```bash
cargo test -p alvum-core location::
cargo test -p alvum-core paths::location_paths
```

Expected: 6 location tests + 1 new path test pass.

- [ ] **Step 5: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 6: Commit.**

```bash
git add crates/alvum-core/src/location.rs crates/alvum-core/src/paths.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add LocationObservation types and capture paths"
```

---

## Task 12: Health Primitives Module

Types for Apple Watch / iPhone HealthKit ingest. Per `docs/superpowers/specs/2026-04-18-health-connector.md` § Data Model. Ingest plumbing, HealthKit reader, and companion apps live in later phases; **only the types and paths land in Phase A**.

**Files:**
- Create: `crates/alvum-core/src/health.rs`
- Modify: `crates/alvum-core/src/lib.rs`
- Modify: `crates/alvum-core/src/paths.rs` (add health path helpers)

- [ ] **Step 1: Write the module.**

Create `crates/alvum-core/src/health.rs`:

```rust
//! Health observations from Apple HealthKit (iPhone + Apple Watch).
//! See `docs/superpowers/specs/2026-04-18-health-connector.md` § Data Model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthObservation {
    /// HealthKit UUID or stable hash. Used for dedup across mirroring devices.
    pub uuid: String,
    pub ts: DateTime<Utc>,
    pub source: HealthSource,
    /// Originating device id (from the device fleet).
    pub device_id: String,
    pub kind: HealthKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthSource {
    HealthKit,
    Strava,
    Manual,
    WearableSensor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HealthKind {
    HeartRate {
        bpm: u32,
        context: HeartRateContext,
    },
    Hrv {
        sdnn_ms: f32,
    },
    Sleep(SleepSession),
    Workout(WorkoutSession),
    Steps {
        count: u32,
        period_s: u32,
    },
    StandHours {
        hours: u32,
        period_s: u32,
    },
    RestingEnergy {
        kcal: f32,
        period_s: u32,
    },
    ActiveEnergy {
        kcal: f32,
        period_s: u32,
    },
    Vo2Max {
        ml_per_kg_min: f32,
    },
    RespiratoryRate {
        breaths_per_min: f32,
    },
    BloodOxygen {
        /// [0.0, 1.0]
        saturation: f32,
    },
    MindfulMinutes {
        duration_s: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeartRateContext {
    Resting,
    Active,
    Background,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SleepSession {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub stages: Vec<SleepStage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SleepSummary>,
    pub is_main_sleep: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SleepStage {
    pub kind: SleepStageKind,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SleepStageKind {
    Awake,
    Rem,
    Core,
    Deep,
    InBed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SleepSummary {
    pub awake_s: u32,
    pub rem_s: u32,
    pub core_s: u32,
    pub deep_s: u32,
    pub in_bed_s: u32,
    /// Fraction of in-bed time spent asleep. [0.0, 1.0].
    pub efficiency: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkoutSession {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub activity: WorkoutActivity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_energy_kcal: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_hr_bpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_hr_bpm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zones: Option<HeartRateZones>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_gain_m: Option<f32>,
    /// Index into `location::fused.jsonl` on the same date when HealthKit provided a route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_ref: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkoutActivity {
    Running,
    Walking,
    Cycling,
    Swimming,
    StrengthTraining,
    Hiit,
    Yoga,
    Hiking,
    Rowing,
    Elliptical,
    Dance,
    CrossTraining,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct HeartRateZones {
    pub z1_s: u32,
    pub z2_s: u32,
    pub z3_s: u32,
    pub z4_s: u32,
    pub z5_s: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn heart_rate_observation_roundtrip() {
        let obs = HealthObservation {
            uuid: "hk-uuid-abc".into(),
            ts: ts("2026-04-11T08:00:00Z"),
            source: HealthSource::HealthKit,
            device_id: "watch".into(),
            kind: HealthKind::HeartRate { bpm: 62, context: HeartRateContext::Resting },
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: HealthObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
        assert!(json.contains("\"kind\":\"heart_rate\""));
        assert!(json.contains("\"context\":\"resting\""));
    }

    #[test]
    fn sleep_session_roundtrip() {
        let obs = HealthObservation {
            uuid: "hk-sleep-1".into(),
            ts: ts("2026-04-11T07:15:00Z"),
            source: HealthSource::HealthKit,
            device_id: "watch".into(),
            kind: HealthKind::Sleep(SleepSession {
                start: ts("2026-04-10T23:10:00Z"),
                end: ts("2026-04-11T07:05:00Z"),
                stages: vec![
                    SleepStage { kind: SleepStageKind::InBed, start: ts("2026-04-10T23:10:00Z"), end: ts("2026-04-10T23:35:00Z") },
                    SleepStage { kind: SleepStageKind::Core, start: ts("2026-04-10T23:35:00Z"), end: ts("2026-04-11T01:50:00Z") },
                    SleepStage { kind: SleepStageKind::Deep, start: ts("2026-04-11T01:50:00Z"), end: ts("2026-04-11T02:40:00Z") },
                    SleepStage { kind: SleepStageKind::Rem, start: ts("2026-04-11T05:30:00Z"), end: ts("2026-04-11T06:15:00Z") },
                ],
                summary: Some(SleepSummary {
                    awake_s: 900, rem_s: 2700, core_s: 11100, deep_s: 3000, in_bed_s: 28500, efficiency: 0.82,
                }),
                is_main_sleep: true,
            }),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: HealthObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
    }

    #[test]
    fn workout_session_with_zones_and_route_ref() {
        let obs = HealthObservation {
            uuid: "hk-wk-1".into(),
            ts: ts("2026-04-11T06:52:00Z"),
            source: HealthSource::HealthKit,
            device_id: "watch".into(),
            kind: HealthKind::Workout(WorkoutSession {
                start: ts("2026-04-11T06:05:00Z"),
                end: ts("2026-04-11T06:52:00Z"),
                activity: WorkoutActivity::Running,
                distance_m: Some(7100.0),
                active_energy_kcal: Some(520.0),
                avg_hr_bpm: Some(162),
                max_hr_bpm: Some(181),
                zones: Some(HeartRateZones {
                    z1_s: 120, z2_s: 900, z3_s: 1200, z4_s: 540, z5_s: 60,
                }),
                elevation_gain_m: Some(95.0),
                route_ref: Some(3),
            }),
        };
        let json = serde_json::to_string(&obs).unwrap();
        let back: HealthObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, obs);
    }

    #[test]
    fn health_kind_tag_snake_case() {
        let hrv = HealthObservation {
            uuid: "u".into(),
            ts: ts("2026-04-11T08:00:00Z"),
            source: HealthSource::HealthKit,
            device_id: "watch".into(),
            kind: HealthKind::Hrv { sdnn_ms: 48.2 },
        };
        let json = serde_json::to_string(&hrv).unwrap();
        assert!(json.contains("\"kind\":\"hrv\""));

        let vo2 = HealthObservation {
            uuid: "u".into(),
            ts: ts("2026-04-11T08:00:00Z"),
            source: HealthSource::HealthKit,
            device_id: "watch".into(),
            kind: HealthKind::Vo2Max { ml_per_kg_min: 46.2 },
        };
        let json = serde_json::to_string(&vo2).unwrap();
        assert!(json.contains("\"kind\":\"vo2_max\""));
    }

    #[test]
    fn workout_activity_snake_case() {
        assert_eq!(serde_json::to_string(&WorkoutActivity::StrengthTraining).unwrap(), "\"strength_training\"");
        assert_eq!(serde_json::to_string(&WorkoutActivity::Hiit).unwrap(), "\"hiit\"");
        assert_eq!(serde_json::to_string(&WorkoutActivity::CrossTraining).unwrap(), "\"cross_training\"");
    }

    #[test]
    fn sleep_stage_kind_snake_case() {
        assert_eq!(serde_json::to_string(&SleepStageKind::Rem).unwrap(), "\"rem\"");
        assert_eq!(serde_json::to_string(&SleepStageKind::InBed).unwrap(), "\"in_bed\"");
    }
}
```

- [ ] **Step 2: Add health path helpers to `paths.rs`.**

In `crates/alvum-core/src/paths.rs`, add inside `impl AlvumPaths`:

```rust
// --- health capture (separate files per category per the health spec) ---

pub fn health_dir(&self, date: NaiveDate) -> PathBuf {
    self.capture_dir(date).join("health")
}

pub fn health_hr_file(&self, date: NaiveDate) -> PathBuf {
    self.health_dir(date).join("hr.jsonl")
}

pub fn health_activity_file(&self, date: NaiveDate) -> PathBuf {
    self.health_dir(date).join("activity.jsonl")
}

pub fn health_sleep_file(&self, date: NaiveDate) -> PathBuf {
    self.health_dir(date).join("sleep.jsonl")
}

pub fn health_workouts_file(&self, date: NaiveDate) -> PathBuf {
    self.health_dir(date).join("workouts.jsonl")
}

pub fn health_mindful_file(&self, date: NaiveDate) -> PathBuf {
    self.health_dir(date).join("mindful.jsonl")
}
```

Add a test to `paths.rs` tests module:

```rust
#[test]
fn health_paths() {
    let (p, _t) = paths();
    let date = d("2026-04-11");
    assert!(p.health_dir(date).ends_with("capture/2026-04-11/health"));
    assert!(p.health_hr_file(date).ends_with("health/hr.jsonl"));
    assert!(p.health_sleep_file(date).ends_with("health/sleep.jsonl"));
    assert!(p.health_workouts_file(date).ends_with("health/workouts.jsonl"));
    assert!(p.health_activity_file(date).ends_with("health/activity.jsonl"));
    assert!(p.health_mindful_file(date).ends_with("health/mindful.jsonl"));
}
```

- [ ] **Step 3: Wire `health` into `lib.rs`.**

Insert `pub mod health;` alphabetically after `pub mod evidence;`:

```rust
pub mod evidence;
pub mod health;
pub mod intention;
```

- [ ] **Step 4: Run tests.**

```bash
cargo test -p alvum-core health::
cargo test -p alvum-core paths::health_paths
```

Expected: 6 health tests + 1 new path test pass.

- [ ] **Step 5: Clippy.**

```bash
cargo clippy -p alvum-core -- -D warnings
```

- [ ] **Step 6: Commit.**

```bash
git add crates/alvum-core/src/health.rs crates/alvum-core/src/paths.rs crates/alvum-core/src/lib.rs
git commit -m "feat(core): add HealthObservation types and capture paths"
```

---

## Task 13: PlaceAttributes in `alvum-knowledge`

Structured attributes for `Entity` entries with `entity_type: "place"`. Per `docs/superpowers/specs/2026-04-18-location-map.md` § Data Model — PlaceEntity. Lives in `alvum-knowledge` because that's where `Entity` lives; references `GeoPoint` from `alvum-core::location`.

**Files:**
- Create: `crates/alvum-knowledge/src/place.rs`
- Modify: `crates/alvum-knowledge/src/lib.rs`

- [ ] **Step 1: Write the module.**

Create `crates/alvum-knowledge/src/place.rs`:

```rust
//! Structured attributes for Entity entries with entity_type = "place".
//! Stored as the entity's `attributes` field (serde_json::Value) per the
//! existing `alvum-knowledge::types::Entity` shape.
//!
//! See `docs/superpowers/specs/2026-04-18-location-map.md` § Data Model.

use alvum_core::location::GeoPoint;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaceAttributes {
    /// Canonical point at the place (centroid if polygon).
    pub at: GeoPoint,
    /// Radius in meters treated as "the same place" for dwell detection.
    #[serde(default = "default_place_radius_m")]
    pub radius_m: f32,
    /// Optional category hint ("home", "office", "gym", "coffee_shop", "park").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Privacy class. Default `Public`; users mark home and sensitive places explicitly.
    #[serde(default)]
    pub privacy: PlacePrivacy,
}

fn default_place_radius_m() -> f32 { 50.0 }

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlacePrivacy {
    /// Normal — raw coordinates usable anywhere.
    #[default]
    Public,
    /// Obfuscate in any UI export, map screenshot, or outbound sync.
    Sensitive,
    /// Never render raw coordinates; always show label only.
    /// Fusion suppresses raw lat/lon for points inside this place's radius.
    Hidden,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(lat: f64, lon: f64) -> GeoPoint {
        GeoPoint { lat, lon, ts: None, accuracy_m: None }
    }

    #[test]
    fn place_attributes_roundtrip() {
        let p = PlaceAttributes {
            at: geo(40.6602, -73.9690),
            radius_m: 75.0,
            category: Some("park".into()),
            privacy: PlacePrivacy::Public,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: PlaceAttributes = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn default_radius_when_missing() {
        let json = r#"{
            "at": {"lat": 40.7580, "lon": -73.9855},
            "category": "office"
        }"#;
        let p: PlaceAttributes = serde_json::from_str(json).unwrap();
        assert_eq!(p.radius_m, 50.0);
        assert_eq!(p.privacy, PlacePrivacy::Public);
    }

    #[test]
    fn privacy_variants_snake_case() {
        assert_eq!(serde_json::to_string(&PlacePrivacy::Public).unwrap(), "\"public\"");
        assert_eq!(serde_json::to_string(&PlacePrivacy::Sensitive).unwrap(), "\"sensitive\"");
        assert_eq!(serde_json::to_string(&PlacePrivacy::Hidden).unwrap(), "\"hidden\"");
    }

    #[test]
    fn hidden_place_keeps_shape() {
        let p = PlaceAttributes {
            at: geo(40.6710, -73.9800),
            radius_m: 150.0,
            category: Some("home".into()),
            privacy: PlacePrivacy::Hidden,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: PlaceAttributes = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert!(json.contains("\"privacy\":\"hidden\""));
    }
}
```

- [ ] **Step 2: Wire into `alvum-knowledge/src/lib.rs`.**

Add `pub mod place;` alongside the existing module declarations. Exact placement depends on the current `lib.rs` contents — run `cat crates/alvum-knowledge/src/lib.rs` to see, then insert alphabetically.

- [ ] **Step 3: Run tests.**

```bash
cargo test -p alvum-knowledge place::
```

Expected: 4 tests pass.

- [ ] **Step 4: Clippy.**

```bash
cargo clippy -p alvum-knowledge -- -D warnings
```

- [ ] **Step 5: Commit.**

```bash
git add crates/alvum-knowledge/src/place.rs crates/alvum-knowledge/src/lib.rs
git commit -m "feat(knowledge): add PlaceAttributes for place-typed entities"
```

---

## Self-Review Checklist

Run this before considering Phase A done:

- [ ] **Spec coverage — top-level spec § Data Model** — every type has a Rust struct/enum with at least one round-trip test:
  - Decision (with outcome, evidence, conflicts, participants, source, connector) ✓ Task 6
  - CausalLink (CausalMechanism, cross_domain) ✓ Task 6.3
  - EmergentState ✓ Task 3
  - Evidence, Confidence, ModalClaim, ModalConflict, ConflictType ✓ Task 2
  - Intention, IntentionKind, IntentionSource, Cadence, Period ✓ Task 5
  - BehavioralSignal, BehavioralSignalType ✓ Task 4
  - AlignmentReport, AlignmentItem, AlignmentStatus, Trend ✓ Task 8
  - Entity/Relationship/Pattern/Fact — already in `alvum-knowledge` per progress-matrix row 20. Out of Phase A scope.
  - Event, Commitment, ActivityBlock, CapturePayload ✓ Task 7
  - DayExtraction ✓ Task 9

- [ ] **Spec coverage — sub-specs 2026-04-18** — types the alignment engine consumes:
  - LocationObservation, LocationKind (Point/Route/Transit), GeoPoint, MotionMode, TransitStep, TransitStepKind, LocationSource, LocationConfidence ✓ Task 11
  - HealthObservation, HealthKind (all 12 variants), HealthSource, HeartRateContext, SleepSession, SleepStage, SleepStageKind, SleepSummary, WorkoutSession, WorkoutActivity, HeartRateZones ✓ Task 12
  - PlaceAttributes, PlacePrivacy ✓ Task 13
  - Device, DeviceKind, DeviceSignals, DevicePermissions, DeviceCapabilities, Heartbeat — **NOT** in Phase A; they ride with the device-fleet work in Phase C.

- [ ] **Storage layout** — `AlvumPaths` resolves every subtree named in § Storage. ✓ Task 1

- [ ] **Known spec deviations documented in code** — `Decision.timestamp` vs. `date`, `Decision.connector` vs. `source`, `Decision.id: String` vs. `Ulid`. ✓ Comments added in Tasks 1 & 6.

- [ ] **`cargo test --workspace`** green.

- [ ] **`cargo clippy --workspace -- -D warnings`** green.

- [ ] **Progress matrix updated** — edit `docs/superpowers/plans/2026-04-18-top-level-spec-progress-matrix.md` to flip rows 14, 14b, 15, 16, 17, 18, 19, 21, 22 from 🟡/⚪ to ✅. Add a dated Update entry using the Rolling Update Template.

- [ ] **No `./output/` writes** — grep for `"output"` as a path literal in the codebase; if any remain, they must be test fixtures or have a documented reason.

---

## Out of Phase A Scope (documented here to prevent scope creep)

- Intention storage loader/saver (Task 5 adds the type only; a separate module in Phase B or C handles read/write of `intentions.json`).
- The LLM-facing JSON schemas for decisions and alignment reports — Phase B refactors those prompts.
- Migrating on-disk old decisions to the new Decision shape — existing test fixtures using `"output/"` should be rewritten or marked deprecated. Real on-disk outputs from prior runs are in `./output/` (gitignored); users should re-run rather than migrate.
- Wearable-specific `Evidence.source` values — those get added when Phase E lands.
- `ExtractionResult` at the bottom of `decision.rs` — leave unchanged; it's an MVP artifact that may be replaced by `DayExtraction` in Phase B.
- **Device fleet types** (`Device`, `DeviceKind`, `DeviceSignals`, `DevicePermissions`, `DeviceCapabilities`, `Heartbeat`) — Phase C (with the Electron app shell and the pairing / heartbeat endpoints in `alvum-web`). Phase A's Decision and Observation do NOT yet carry `device_id` attribution; that's added in Phase C when the Device type itself lands.
- **Location fusion logic** — the Prepare-stage implementation that merges iPhone + Strava + Mac + wearable streams into `fused.jsonl`. Phase B. Phase A only introduces the types and the paths where fusion will eventually write.
- **HealthKit reader** — the iOS / watchOS companion code that translates HealthKit queries into `HealthObservation` payloads. Phase E (iOS native companion work). Phase A only introduces the types and the paths where ingest will eventually write.
- **Strava / HealthKit pull connectors** — one-shot pull jobs that populate `capture/<date>/location/pulled/*`. Phase E or later.
- **PlaceEntity creation flow** — how `PlaceAttributes` get attached to an `Entity` (user-driven vs. auto-detected from dwell clusters). Phase B / C.
- **Observation `device_id` / `device_kind` attribution fields** — explicitly deferred to Phase C so they land together with the Device type itself. Until then, `Observation.source` (connector name) is the only attribution.
