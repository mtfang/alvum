# Health Connector

Sleep, heart rate, workouts, and adjacent biosignals as first-class evidence for the alignment engine. This spec defines the Apple Watch / iPhone HealthKit integration, the on-disk health observation shape, privacy model, and how health data feeds the decision graph, emergent-state detection, and morning briefing.

It backs the `heartRate`, `sleep`, and `workouts` signals surfaced on `/devices` for the Apple Watch and iPhone (see `~/git/alvum-frontend/devices.jsx:228-234` and the `signalDesc` map). The top-level spec does not mention health data; this spec adds it.

## Problem

The mockup treats the Apple Watch as a full fleet member with its own signal set (HR, sleep, workouts) yet the top-level spec has no type for any of it, no connector for Apple HealthKit, and no story for how body signals flow into alignment.

A complete alignment engine needs health data because:
- **Sleep quality is a causal input.** Poor sleep is a frequent root cause of next-day drift (skipped exercise, irritability, shortened attention spans). The decision graph is incomplete without it.
- **Exercise intentions need exercise evidence.** A "Run 3×/week" habit is directly validated by workout detection, not inferred from audio or location.
- **Heart rate is the cleanest physiological stress signal we can passively collect.** Sustained elevated resting HR or suppressed HRV is an `EmergentState` candidate comparable to the "burnout" example in the top-level spec.
- **Workouts carry location, participants (training partners detected via audio co-presence), and duration** — they connect three modalities with one authoritative record.

Adding health data also raises the stakes on privacy. HealthKit is arguably the most sensitive data source in the fleet. This spec treats health data with extra care (no cloud sync, no LLM exposure of raw values without user opt-in, aggregate-only for the briefing).

## Architecture

```
                         Apple HealthKit
   (iOS and watchOS authoritative store; mirrored via iCloud between devices)
                                │
        ┌───────────────────────┼────────────────────────┐
        │                       │                        │
        ▼                       ▼                        ▼
  iOS Companion           watchOS Companion       macOS HealthKit bridge
  (iPhone)                (Apple Watch)           (Mac can read some
                                                   HealthKit data if the
                                                   user grants it)
                                │
                                ▼
        ┌──────────────────────────────────────────────┐
        │  Health observations posted to The Box:      │
        │  POST /api/ingest/health                     │
        │  Content-Type: application/jsonl             │
        │  Each line = one HealthObservation           │
        └──────────────────────────────────────────────┘
                                │
                                ▼
        ┌──────────────────────────────────────────────┐
        │ alvum-connector-health (on The Box)          │
        │  - validates device token (device-fleet spec)│
        │  - dedups by (source, uuid) — HealthKit      │
        │    records are uniquely identifiable         │
        │  - writes to capture/YYYY-MM-DD/health/*.jsonl│
        │  - emits Observation records into the        │
        │    pipeline with metadata.health payload     │
        └──────────────────────────────────────────────┘
                                │
                                ▼
             Pipeline: Align (evidence), Link (emergent states),
             Brief (sleep / training summary), /knowledge
```

HealthKit is the source of truth. The alvum iOS and watchOS companions read from HealthKit (never write to it), translate to `HealthObservation`, and push to The Box on a cadence (push immediately for meaningful events: workout-ended, sleep-session-complete; batch every ~30 min for continuous streams like HR).

The Mac-as-capture-device path is weak (HealthKit on Mac is read-only for a subset of types and requires iCloud sync) — it's a fallback for users who don't have the phone companion set up, not the primary path.

## Data Model

### HealthObservation

One record per captured sample or session. Modeled as an enum because the underlying shapes are fundamentally different.

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HealthObservation {
    /// HealthKit UUID if from HealthKit; otherwise a stable hash.
    /// Used for dedup across devices (watch + phone mirror same sample).
    pub uuid: String,
    pub ts: DateTime<Utc>,
    pub source: HealthSource,
    /// Originating device id from the fleet.
    pub device_id: String,
    pub kind: HealthKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthSource {
    /// Apple HealthKit.
    HealthKit,
    /// Strava (pulled via alvum-connector-strava; workout-only).
    Strava,
    /// Manual entry in the UI.
    Manual,
    /// Wearable pin's HR sensor (future — not all pins have one).
    WearableSensor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HealthKind {
    /// A single heart-rate measurement.
    HeartRate {
        bpm: u32,
        context: HeartRateContext,
    },
    /// Heart rate variability (SDNN, in milliseconds). Derived; cadence is
    /// typically ~1×/5min on Watch.
    Hrv {
        sdnn_ms: f32,
    },
    /// A completed sleep session (from bedtime to wake).
    Sleep(SleepSession),
    /// A completed workout.
    Workout(WorkoutSession),
    /// Step count over a period.
    Steps {
        count: u32,
        period_s: u32,
    },
    /// Standing hours (Apple Watch "stand" ring).
    StandHours {
        hours: u32,
        period_s: u32,
    },
    /// Resting energy (calories) over a period.
    RestingEnergy {
        kcal: f32,
        period_s: u32,
    },
    /// Active energy.
    ActiveEnergy {
        kcal: f32,
        period_s: u32,
    },
    /// VO2Max (cardio fitness).
    Vo2Max {
        ml_per_kg_min: f32,
    },
    /// Respiratory rate.
    RespiratoryRate {
        breaths_per_min: f32,
    },
    /// Blood oxygen saturation (pulse oximetry).
    BloodOxygen {
        saturation: f32,  // [0.0, 1.0]
    },
    /// Mindful minutes (Apple "Mindfulness" sessions).
    MindfulMinutes {
        duration_s: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeartRateContext {
    /// Resting (user not active, typically at rest for > 5 min).
    Resting,
    /// During a workout session.
    Active,
    /// General background sample (Watch default cadence).
    Background,
}
```

### SleepSession

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SleepSession {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    #[serde(default)]
    pub stages: Vec<SleepStage>,
    /// Optional aggregate: minutes in each stage. Redundant with `stages` but
    /// some sources provide only this (older Watch, third-party apps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<SleepSummary>,
    /// Whether this session is the user's main sleep of the night
    /// (vs. a nap). HealthKit provides this distinction.
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
    /// HealthKit's "inBed" (pre-sleep, post-wake in bed).
    InBed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SleepSummary {
    pub awake_s: u32,
    pub rem_s: u32,
    pub core_s: u32,
    pub deep_s: u32,
    pub in_bed_s: u32,
    /// Percentage of in-bed time spent asleep. [0.0, 1.0].
    pub efficiency: f32,
}
```

### WorkoutSession

```rust
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
    /// Optional elevation gain in meters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_gain_m: Option<f32>,
    /// Cross-reference to a LocationObservation if the workout had route data.
    /// Points at `location::fused.jsonl[index]` on the same date.
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
    /// Catch-all for HealthKit activities we haven't named yet.
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct HeartRateZones {
    /// Seconds in each zone. Zones per Apple's default scheme (%max HR).
    /// Z1: <60%, Z2: 60-70%, Z3: 70-80%, Z4: 80-90%, Z5: >90%.
    pub z1_s: u32,
    pub z2_s: u32,
    pub z3_s: u32,
    pub z4_s: u32,
    pub z5_s: u32,
}
```

## Storage

```
~/.alvum/
└── capture/YYYY-MM-DD/
    └── health/
        ├── hr.jsonl             ← continuous HR + HRV samples
        ├── activity.jsonl       ← steps, energy, stand, respiratory, spo2, vo2max
        ├── sleep.jsonl          ← one line per SleepSession
        ├── workouts.jsonl       ← one line per WorkoutSession
        └── mindful.jsonl        ← MindfulMinutes sessions
```

Each file is append-only JSONL of `HealthObservation`. One file per health domain rather than a single file, because:
- HR volume (~1 sample/5s background, higher during workouts) is 1-2 orders of magnitude larger than other categories.
- Read patterns differ — sleep/workouts are session-oriented and rare; HR is continuous and dense.

### Retention

- `hr.jsonl` (continuous HR, HRV): 90 days at full resolution; downsample to 1-per-minute at day 91 and keep forever.
- `activity.jsonl`: forever.
- `sleep.jsonl` / `workouts.jsonl`: forever.
- `mindful.jsonl`: forever.

Per the storage-layout spec, default retention for `capture/` is indefinite. Health data benefits from that default: trend detection (resting HR over months, HRV over weeks) requires the long tail. Users pruning aggressively can carve out health categories in `runtime/config.toml` `[retention.capture.health]` if disk-pressured.

### Dedup

HealthKit samples have stable UUIDs. The connector dedups on insertion by maintaining a per-day in-memory set of seen UUIDs (bounded; flushed per-day). Duplicate submissions (same UUID from both iPhone and Watch via iCloud mirroring) are silently discarded after the first write.

## Pipeline Integration

### As Observation

Each HealthObservation becomes an `Observation` with `kind = "health"` and structured metadata. The `content` field (always a string per Observation contract) is a short human-readable summary:

```json
{
  "ts": "2026-04-18T06:52:14Z",
  "source": "watch",
  "kind": "health",
  "content": "45m run · 7.1 km · avg HR 162 · 4'12/km pace",
  "metadata": {
    "health": {
      "kind": "workout",
      "activity": "running",
      "distance_m": 7100,
      "avg_hr_bpm": 162,
      "route_ref": 3
    }
  },
  "device_id": "watch"
}
```

This rides the existing Observation pipeline with no shape change. The LLM never sees raw per-second HR samples; it sees the session summary. Continuous streams (HR, HRV) are not converted to Observations; they're loaded directly by the alignment stage when computing stress signals.

### Alignment evidence

The alignment stage loads workouts and sleep for the day explicitly:

- **Exercise intentions.** `WorkoutActivity` matches against the intention's description (fuzzy + canonical mapping: "gym" → `StrengthTraining`, "run" → `Running`, "yoga" → `Yoga`). A matching workout within the intention's time window is strong evidence of alignment.
- **Sleep intentions.** A "sleep 7h" habit checks against `SleepSession.summary.in_bed_s - summary.awake_s`. "In bed by 10pm" checks `start` timestamp.
- **Implicit sleep-driven alignment.** Poor sleep (efficiency < 0.75 or < 5h total) is surfaced in the briefing with a note about likely next-day impact, even without a stated sleep intention.

### Emergent-state inputs

The Link stage (§ Pipeline — Stage 4) runs emergent-state detection with health inputs:

| Signal pattern | Candidate `EmergentState` |
|---|---|
| 5+ consecutive days resting HR > baseline + 5 bpm | "Elevated physiological stress" |
| 7-day rolling avg sleep efficiency < 0.75 | "Sleep debt" |
| HRV trending down 10% week-over-week | "Recovery debt" |
| Workout frequency dropped by 50% vs. trailing 4-week | "Training decline" |
| Resting HR > baseline + 10 AND HRV dropped AND sleep < 6h | "Acute overload" — higher-confidence composite |

Baselines are per-user rolling 28-day medians, computed lazily from `hr.jsonl` and `sleep.jsonl`. They're not stored as state; recomputed when needed to avoid drift.

### Briefing surface

The morning briefing (§ Pipeline — Brief) may include a "Body" section when relevant:

> **Body.** Slept 5h 42m last night (efficiency 72%). Resting HR 64 bpm, up from 58 baseline. This is the fifth light-sleep night in a row.

Appears when: any active `EmergentState` in the body domain, OR the sleep/HR deviation breaches a threshold. Otherwise the section is omitted — briefing fatigue matters.

### Knowledge-corpus facts

The Learn stage (§ Pipeline — Stage "Learn") extracts durable facts from health patterns:

- "User tends to sleep best on nights without late meetings" (correlation pattern)
- "User trains hardest on Tuesdays and Fridays" (routine)

These become `Fact` entries in `knowledge/facts.jsonl` per the top-level spec. They're emerged, not entered.

## Privacy Model

Health data is treated with stricter defaults than the rest of the capture layer:

1. **No raw per-sample data to the LLM.** Workouts and sleep are summarized before they reach any LLM prompt. Continuous HR/HRV samples never appear in prompts — only derived statistics ("resting HR elevated 8 bpm vs. baseline").
2. **No cloud sync of raw health files.** Even when the managed tier ships (§ Managed Tier), `capture/*/health/hr.jsonl` and `activity.jsonl` are excluded from encrypted backup by default. User may opt in per-file type.
3. **HealthKit authorization per type.** The iOS companion requests HealthKit types individually. User may grant workouts + sleep without granting heart rate. The `/devices` page reflects which HealthKit types are actually authorized vs. requested.
4. **Redaction on export.** Any briefing export, decision share, or screenshot redacts specific HR / sleep values unless the user explicitly unlocks. Aggregate trends ("you slept poorly this week") are exportable; specific numbers are not, without a per-export toggle.
5. **No use for model training outside fine-tuning.** The fine-tuning pipeline (§ Fine-Tuning) excludes health data by default; a separate opt-in permits it. Health data never leaves the device for RL reward-model training (§ RL/RLHF) in the V6 design.
6. **Watch offline ≠ capture gap.** When the Watch loses network, it buffers locally and syncs later. Health observations for a window when the Watch was offline still arrive, but their `device_id` is `watch` and the `/devices` page notes the gap — the user can choose to discard the backfill.

## Authorization flow

First-time setup via the iOS companion:

1. User adds the iPhone as a device via the device-fleet pairing flow.
2. iOS companion presents a HealthKit types request screen, one category at a time:
   - **Workouts** — "Tracks exercise sessions and route data for workout alignment."
   - **Sleep** — "Reads sleep sessions to correlate with next-day decisions."
   - **Heart rate** — "Background and resting heart rate for stress pattern detection."
   - **Activity** — "Steps, stand hours, energy burn for day-shape context."
   - **Body measurements** — opt-in, not default.
3. User grants per category (iOS's standard HealthKit authorization UI).
4. Companion begins the initial 30-day backfill (one-shot) then moves to incremental.

User can revoke any category at any time from `/devices` → iPhone → HealthKit permissions, or from iOS Settings. Revoking stops future ingest; historical ingested data stays unless the user also taps "Forget health data from this device" which deletes `capture/*/health/` entries with matching `device_id`.

## iOS / watchOS Companion Responsibilities

**iPhone companion:**
- Monitors HealthKit in background via `HKObserverQuery` with background delivery.
- Translates HealthKit samples to `HealthObservation` on device.
- Posts batches to `POST /api/ingest/health` on The Box when on the home network. Buffers up to 30 days offline.
- Does NOT cache health values in a way that would persist if the iPhone is lost — HealthKit itself is the durable store; the companion holds only an in-flight batch.

**Apple Watch companion:**
- For V1.5, the Watch does not talk directly to The Box. HealthKit mirrors to the iPhone, which ships the data. Watch app's only role is showing status ("alvum is active") and forwarding manual-capture affordances ("log a decision").
- Future (V2+): a Watch-direct path for real-time HR streaming, used for live stress detection and in-the-moment briefings. Out of V1.5 scope.

## Connector Crate

`crates/alvum-connector-health/`:
- Depends on `alvum-core` and the device-fleet types.
- Provides `HealthConnector` implementing `Connector`.
- Receives JSONL payloads at `/api/ingest/health` (mounted by `alvum-web`), validates the device bearer token, writes to `capture/<date>/health/<category>.jsonl`, emits `Observation` records for sessions (workouts, sleep) and suppresses Observations for continuous streams (HR/HRV — loaded directly by alignment stage instead).

Companion apps (`app/ios-native/`, `app/watchos-native/` per the progress-matrix app-shell split) own the HealthKit read path; this crate owns the ingest, storage, and pipeline-facing side.

## Open Questions

- **Watch-direct vs. phone-mediated ingest.** We chose phone-mediated for simplicity in V1.5. The cost is latency: live stress-based interventions (meditation prompt when HR spikes during a meeting) need watch-direct, which requires a WatchConnectivity-to-phone push or a direct Wi-Fi path. Defer.
- **Source trust when phone and watch mirror.** HealthKit dedups by UUID; we rely on that. If a sample has no UUID (rare), we dedup by `(ts, kind, bpm)` — collisions are possible but cost is acceptable (at worst, two near-identical HR samples 10s apart).
- **Third-party integration priority.** Oura, Whoop, and Garmin have no HealthKit mirror for some of their signals (Oura tags, Whoop strain). Each needs a pull connector. V1.5 ships HealthKit-only; Oura is the likely V2 addition.
- **Cycle tracking.** HealthKit menstrual cycle data is a strong alignment signal for anyone who tracks it, but is in a separate permission category with additional sensitivity. Treat as explicit opt-in at a tier below the default Workouts/Sleep/HR prompts. Not covered in V1.5.
- **Weight and body composition.** Simple to ingest; unclear if they feed alignment directly. Treat as opportunistic — store if granted, don't build logic around them until a product need emerges.

## Phase / Milestone

| Component | Phase |
|---|---|
| `HealthObservation`, `HealthKind`, `SleepSession`, `WorkoutSession` in `alvum-core` | A (alignment primitives) |
| `AlvumPaths::health_*(date, category)` path helpers | A |
| `crates/alvum-connector-health` (ingest, storage, Observation emission) | C (parallel with `/devices`) |
| `POST /api/ingest/health` endpoint in `alvum-web` | C |
| iOS HealthKit reader in `app/ios-native/` | E (after iOS companion scaffolding) |
| watchOS companion status surface (no data ingest) | E |
| Alignment evidence from workouts / sleep | B |
| Emergent-state detection (sleep debt, elevated HR, HRV drop) | B |
| Body section in morning briefing | C |
| Knowledge-corpus fact extraction from health patterns | C |

Phase A gets the types so alignment-engine work in Phase B can consume them. The ingest pathway (companion + connector + endpoint) lives in Phase C / E per the iOS native split.

## Relationship to Other Specs

- **Top-level spec:** adds a § Data Model — Health subsection and introduces `alvum-connector-health` to § Architecture. Updates § Pipeline — Brief to include the conditional Body section.
- **`2026-04-18-device-fleet.md`:** every HealthObservation has a `device_id` resolving to an entry in the device registry. Companion pairing and HealthKit authorization flow through the device-fleet rails.
- **`2026-04-18-location-map.md`:** `WorkoutSession.route_ref` points to a `LocationObservation` entry produced by the same workout via HealthKit route data. The location spec owns the route shape; this spec owns the workout shape and back-reference.
- **`2026-04-18-alignment-primitives.md` (Phase A plan):** extended to include `HealthObservation` and supporting types in `alvum-core`.
