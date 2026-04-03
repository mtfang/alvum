# Alvum System Design

An alignment engine that measures the distance between stated intentions and observed reality across all life domains. Captures multi-modal data (audio, screen, camera), processes overnight, builds a causal decision graph with emergent states, and delivers proactive briefings that surface cross-domain butterfly effects.

The core insight: instead of the human prompting the agent, the agent prompts the human — grounded in the trailing directed graph of decisions that led them to this point.

## Product Vision

Two tiers, same core system. The managed layer is additive — core functionality works air-gapped.

### DIY (Batteries Included)

**Buy the hardware. Own your data. Run everything locally.**

- **The Wearable** — a clip-on device (ESP32-S3, camera + mic) that captures your physical world. You wear it, it records, it syncs when you're home.
- **The Box** — a dedicated home appliance (Mac Mini-class, Apple Silicon) that runs everything: capture daemon for desktop, overnight pipeline, local LLMs, fine-tuned models, the web UI. All processing happens here. Nothing leaves your network.
- **The App** — installed on the box, runs the web UI accessible from any device on your local network. Also installs on your daily-driver Mac for desktop capture.

One-time hardware purchase. Free software updates. No cloud, no accounts. For privacy-conscious and security-conscious users who want full control.

### Fully Managed (Subscription)

**Rent the hardware. We handle everything. Guaranteed upgrades.**

Same wearable, same box, same software — but as a managed service:
- Hardware leased, swapped out for newer models on upgrade cycles
- End-to-end encrypted cloud backup (zero-knowledge — we can't read your data)
- Zero-touch software and model updates pushed remotely
- Remote health monitoring and support
- If your box dies, ship a new one, restore from encrypted backup

Monthly subscription. For users who want a worry-free appliance they never think about.

### Architectural Principle

Both tiers run identical software. The managed layer is an optional module:

```
┌──────────────────────────────────────────────────────────┐
│  MANAGED LAYER (optional, additive)                      │
│  Encrypted cloud sync │ Remote management │ Account      │
├──────────────────────────────────────────────────────────┤
│  CORE (identical in both tiers)                          │
│  capture → pipeline → graph → alignment → briefing      │
│  Works fully standalone. No cloud dependency.            │
└──────────────────────────────────────────────────────────┘
```

Cloud sync uses end-to-end encryption (ChaCha20-Poly1305, Argon2 key derivation). The encryption key lives on the user's device. The cloud stores opaque ciphertext — a server breach reveals nothing. Managed users can export their data and switch to DIY at any time.

**V1 ships as a macOS app + wearable (DIY tier).** The "box" is your Mac running the Tauri app. The dedicated appliance and managed tier are future product expansions once the software is proven. The crate architecture supports both — nothing couples to a specific deployment model.

## Architecture Overview

```
alvum/
├── crates/
│   ├── alvum-core        ← shared types, config, storage access
│   ├── alvum-capture     ← macOS screen + audio capture daemon
│   ├── alvum-pipeline    ← overnight batch: transcribe, extract, align
│   ├── alvum-graph       ← decision graph operations, state management
│   └── alvum-web         ← API + web UI + wearable ingest endpoint (axum)
├── app/                  ← Tauri shell (the distributable .app)
└── firmware/             ← ESP32 wearable firmware (separate build)
```

Wearable ingestion is an HTTP endpoint in alvum-web (`/api/ingest`), not a separate crate. The ESP32 POSTs standard multipart uploads — no custom protocol requiring its own crate.

- **Language**: Rust throughout
- **Platform**: macOS only (Apple Silicon + Intel)
- **Distribution**: Tauri app → signed .dmg with auto-update
- **UI**: Local web UI served by embedded axum server (localhost:3741)
- **Wearable**: ESP32-S3 with camera + mic, syncs over local network

## Data Model (alvum-core)

### Life Domains

User-defined string labels, not a fixed enum. Configured during onboarding, editable anytime. Seeded with sensible defaults (Health, Family, Career, Finances, Creative). Renaming and splitting/merging are string operations on JSONL files.

```rust
// Domain is just a string label — the user defines their own
type LifeDomain = String;
```

### Raw Capture

Everything captured during the day is a timestamped event with a source tag. The pipeline doesn't care where data came from — a wearable audio chunk and a desktop mic chunk both produce transcripts.

```rust
enum CaptureSource {
    Screen,
    Audio,
    Accessibility,
    WindowLog,
    WearableAudio,
    WearableFrame,
    Location,
}

struct CaptureEvent {
    id: Ulid,
    timestamp: DateTime<Utc>,
    source: CaptureSource,
    payload: CapturePayload,
}
```

### Decisions

A decision is a choice made, deferred, or revealed through behavior. Each carries multi-modal evidence, causal links with typed mechanisms, and outcome tracking.

```rust
struct Decision {
    id: Ulid,
    date: NaiveDate,
    summary: String,
    reasoning: Option<String>,
    alternatives: Vec<String>,
    participants: Vec<String>,
    domain: String,                     // user-defined life domain
    source: DecisionSource,
    evidence: Vec<Evidence>,
    conflicts: Vec<ModalConflict>,

    // Causal graph edges
    causes: Vec<CausalLink>,
    effects: Vec<CausalLink>,           // filled in as outcomes observed
    contributing_states: Vec<Ulid>,     // emergent states active at decision time

    // Outcome tracking
    open: bool,
    expected_outcome: Option<String>,
    check_by: Option<NaiveDate>,
    actual_outcome: Option<String>,

    // Magnitude (computed over time as effects become visible)
    cascade_depth: Option<u32>,
    cross_domain_effects: Vec<String>,

    tags: Vec<String>,
}

enum DecisionSource {
    Spoken,     // from conversation transcript
    Revealed,   // inferred from behavioral observation
    Explained,  // from evening check-in response
}
```

### Causal Links

Edges between decisions, events, and states carry mechanism and strength information to enable butterfly effect detection.

```rust
struct CausalLink {
    from: Ulid,
    to: Ulid,
    mechanism: CausalMechanism,
    strength: CausalStrength,
    cross_domain: Option<(String, String)>,
}

enum CausalMechanism {
    Direct,              // "because of X, I did Y"
    ResourceCompetition, // X consumed time/energy that Y needed
    EmotionalInfluence,  // X created a feeling that shaped Y
    Precedent,           // X set a pattern that Y followed
    Constraint,          // X eliminated options, forcing Y
    Accumulation,        // X contributed to a state that triggered Y
}

enum CausalStrength {
    Primary,       // THE cause
    Contributing,  // one of several factors
    Background,    // distant/indirect influence
}
```

### Emergent States

Persistent conditions that accumulate from many decisions and transmit butterfly effects across domains. Nobody decides to be burned out — it emerges from accumulated decisions and becomes the medium through which one domain affects another.

```rust
struct EmergentState {
    id: Ulid,
    description: String,
    domain: String,
    intensity: f32,                     // 0.0 to 1.0
    contributing_decisions: Vec<Ulid>,
    first_detected: NaiveDate,
    resolved: Option<NaiveDate>,
}
```

### Multi-Modal Evidence and Conflicts

Every decision and event carries its evidence chain. Cross-modal contradictions (said X, did Y) are the primary signal for the alignment engine.

```rust
struct Evidence {
    source: CaptureSource,
    timestamp: DateTime<Utc>,
    description: String,
    confidence: Confidence,
}

enum Confidence {
    High,
    Medium,
    Low,
}

struct ModalConflict {
    stated: ModalClaim,
    observed: ModalClaim,
    conflict_type: ConflictType,
}

struct ModalClaim {
    source: CaptureSource,
    description: String,
    timestamp: DateTime<Utc>,
}

enum ConflictType {
    SayVsDo,          // "said X, did Y"
    IntendVsDo,       // intention says X, behavior shows Y
    SelfPerception,   // believes X about self, evidence shows Y
    PlanVsReality,    // calendar/plan said X, day went Y
}
```

### Intentions

The reference signal that actions are measured against. Four kinds, all tied to user-defined domains.

```rust
struct Intention {
    id: Ulid,
    kind: IntentionKind,
    description: String,
    domain: String,
    active: bool,
    created: NaiveDate,
    target_date: Option<NaiveDate>,
    cadence: Option<Cadence>,
}

enum IntentionKind {
    Mission,      // core values, identity
    Goal,         // time-bound target
    Habit,        // recurring intention
    Commitment,   // promise to someone (auto-extracted)
}
```

### Behavioral Signals

Silent decision indicators detected from screen and camera behavior.

```rust
struct BehavioralSignal {
    timestamp: DateTime<Utc>,
    signal_type: BehavioralSignalType,
    description: String,
    evidence: Vec<Evidence>,
}

enum BehavioralSignalType {
    AbortedAction,        // compose-without-send, cart-without-purchase
    AvoidancePattern,     // opened task, immediately switched away
    RepetitiveVisit,      // checked the same thing multiple times
    SelfInterruption,     // broke own deep work without external trigger
    AttentionDrift,       // gradual shift from planned activity
}
```

### Alignment

Output of comparing intentions against observed reality.

```rust
struct AlignmentReport {
    date: NaiveDate,
    items: Vec<AlignmentItem>,
}

struct AlignmentItem {
    intention_id: Ulid,
    status: AlignmentStatus,
    evidence: Vec<Evidence>,
    trend: Trend,
    streak: Option<i32>,
}

enum AlignmentStatus {
    Aligned,
    Drifting { gap_description: String },
    Violated { description: String },
    NoEvidence,
}
```

### Supporting Types

These types are referenced above and defined during implementation. Brief descriptions:

- `CapturePayload` — enum: file path (audio, image) or inline data (JSON, text)
- `Cadence` — habit frequency: `{ times: u32, period: Period }` where Period is Daily/Weekly/Monthly
- `Trend` — enum: Improving, Declining, Stable, Insufficient Data
- `Event` — like Decision but without outcome tracking or causal links. Has id, timestamp, summary, domain, evidence.
- `Commitment` — extracted promise: who you promised, what, by when, fulfilled status
- `ActivityBlock` — a time range with classified type (Meeting, DeepWork, Transit, Conversation, Idle), containing references to all capture data within that range

### Day Extraction

The pipeline's complete output for a single day.

```rust
struct DayExtraction {
    date: NaiveDate,
    activity_blocks: Vec<ActivityBlock>,
    events: Vec<Event>,
    decisions: Vec<Decision>,
    commitments: Vec<Commitment>,
    behavioral_signals: Vec<BehavioralSignal>,
    alignment: AlignmentReport,
}
```

## Storage

All data lives as files on disk. No database in V1.

```
~/Library/Application Support/com.alvum.app/
├── capture/                              RAW — delete after 30 days
│   └── 2026-04-03/
│       ├── events.jsonl                  ← semantic change events (generic a11y diff)
│       ├── snapshots/                    ← full screenshots at key moments only
│       │   ├── 09-00-00.webp
│       │   └── 09-30-00.webp
│       ├── audio/
│       │   ├── mic/                      ← Mac microphone
│       │   ├── system/                   ← Mac system audio (remote call participants)
│       │   └── wearable/                 ← ESP32 mic
│       ├── frames/                       ← wearable camera (pHash deduped)
│       ├── location.jsonl
│       └── checkin.jsonl                 ← evening check-in responses
│
├── days/                                 REFINED — keep forever
│   └── 2026-04-03.json                  ← DayExtraction
│
├── decisions/                            THE GRAPH — keep forever, core asset
│   ├── index.jsonl                       ← all decisions, causally linked
│   ├── open.jsonl                        ← decisions awaiting outcomes
│   └── states.jsonl                      ← emergent states (active + resolved)
│
├── intentions.json                       ← user's missions, goals, habits
│
├── briefings/                            AGENT OUTPUT
│   └── 2026-04-03.md                    ← morning briefing
│
├── checkin_questions/                    AGENT OUTPUT
│   └── 2026-04-03.json                  ← evening check-in questions
│
└── config.json                           ← domains, API keys, schedule, preferences
```

### Retention Policy

- Raw capture (`capture/`): 30 days default, configurable. Audio and frames linked from decisions are kept longer.
- Refined data (`days/`): forever.
- Decision graph (`decisions/`): forever. This is the core asset.
- Briefings: forever.
- Snapshots linked from evidence chains: kept as long as the decision exists.

## Capture Layer (alvum-capture)

Always-on daemon. Observes and writes files. No processing, no intelligence, no network calls. Target: <5% CPU, <100MB RAM.

### Desktop Screen Capture

Event-driven, not polling. Captures a screenshot + accessibility tree snapshot atomically on each trigger.

| Trigger | macOS API | Debounce |
|---|---|---|
| App switch / window focus | `NSWorkspaceDidActivateApplicationNotification` | None |
| Significant visual change | `ScreenCaptureKit` frame diff (>5% of pixel count changed) | 3s poll interval |
| Idle fallback | Timer | Every 30s if no other trigger |
| Clipboard change | `NSPasteboard` change count | None |
| URL change in browser | Accessibility API on browser URL bar | 500ms |

### Semantic Change Events (Generic A11y Differ)

Instead of storing raw a11y tree snapshots or per-app diffs, the capture daemon generates **semantic change events** — human-readable, LLM-ready descriptions of what changed. One generic differ handles all apps by comparing a11y node roles, labels, and values.

Five universal detection patterns:

1. **App/window focus change** — different app or window title
2. **Labeled value changed** — any node with a label whose value differs ("Status": "In Progress" → "Backlog")
3. **Text content changed** — large text areas (editors, documents) with content delta size
4. **Navigation** — URL or window title changed within same app
5. **Structural change** — significant nodes appeared or disappeared (dialogs, panels, compose windows)

Output is `events.jsonl` — one line per change, self-contained, no reconstruction needed:

```jsonl
{"ts":"09:00","type":"app_focus","app":"VS Code","window":"api_spec.py"}
{"ts":"09:05","type":"text_changed","app":"VS Code","detail":"~3 lines added in editor"}
{"ts":"09:12","type":"node_appeared","app":"VS Code","detail":"Terminal panel: cargo build — compiled successfully"}
{"ts":"09:30","type":"app_focus","app":"Linear","window":"INGEST-342"}
{"ts":"09:30","type":"field_changed","app":"Linear","field":"Status","from":"In Progress","to":"Backlog"}
```

No per-app interpreter code. The a11y tree is already app-agnostic — roles and labels are semantic by design.

**Fallback:** If a11y tree content text is below a useful threshold for an app (canvas-rendered, bad a11y support), fall back to macOS Vision framework OCR on the screenshot. Self-adapting, no per-app classification needed.

### Screenshots

Stored as full images at meaningful moments only (app switch, navigation, meeting start). Not diffed. Maybe 30-60 per day at ~100-200KB each.

### Desktop Audio Capture

Three simultaneous streams via CoreAudio:

| Stream | Captures | Role |
|---|---|---|
| Mac microphone (`mic/`) | Your voice + room ambient | Redundant with wearable when at desk |
| Mac system audio (`system/`) | Remote call participants, video audio | Unique — wearable can't capture this |
| Wearable mic (`wearable/`) | Your voice + room ambient, portable | Unique when away from desk |

All streams go through VAD (Silero) to skip silence. Output: opus chunks segmented by voice activity.

### Location

CoreLocation significant-change monitoring (low power). Appended to `location.jsonl`.

### Wearable Ingestion

The Tauri app's HTTP server accepts uploads from the ESP32:

```
POST localhost:3741/api/ingest
Content-Type: multipart/form-data
source=wearable_audio|wearable_frame
```

Written to `capture/{date}/audio/wearable/` and `capture/{date}/frames/` in the same format as desktop capture. The ESP32 discovers the server via mDNS (`_alvum._tcp.local`). No cloud, no account, no pairing app.

### Capture Daemon Lifecycle

Spawned on app launch as a background thread. Registers for macOS event notifications, starts audio streams with VAD, starts CoreLocation, opens ingest endpoint. Flushes on sleep/quit, resumes on wake.

## Overnight Pipeline (alvum-pipeline)

Runs once per day (3am default, configurable). Five sequential stages.

### Stage 1: Prepare

No models. Pure data wrangling.

- **Audio dedup:** Detect temporal overlap between `mic/` and `wearable/` streams. Select higher-SNR source per 5-minute window. `system/` always included (unique signal).
- **Frame dedup:** Perceptual hash (pHash) all wearable frames. Cluster near-identical frames, keep sharpest. Drop wearable frames of screens when concurrent screen capture exists.
- **Build timeline skeleton:** Merge events.jsonl + location.jsonl + audio manifest + frame list chronologically. Segment into activity blocks by: location change, >5min silence gap, major app switch, significant scene change.

Output: `prepared.json` manifest with activity blocks and processing instructions.

### Stage 2: Transcribe + Describe

Model-heavy but mechanical. No reasoning.

- **Audio → text:** whisper-rs (whisper.cpp bindings). Process selected voice stream and system audio stream separately. Speaker diarization via embedding clustering.
- **Frame descriptions (selective):**
  - Text-heavy frames → macOS Vision framework OCR (free, local)
  - Complex/important frames (whiteboards, documents, novel scenes) → Claude Vision API
  - Generic frames → skip or one-word tag
  - Budget: ~30-50 vision API calls/day
- **Accessibility text:** Already structured from events.jsonl — no additional processing needed.

### Stage 3: Fuse + Extract

First LLM reasoning step. Per-activity-block, the model reads full multi-modal context and extracts structured output.

**Two-tier model strategy:**
- Routine blocks (solo desk work, transit) → cheap model (Haiku-class)
- Rich blocks (meetings, multi-modal) → strong model (Sonnet/Opus-class)

Classification: if block has transcript with >1 speaker or has both audio and screen activity, it's rich.

**Multi-modal fusion rules in the prompt:**
1. Resolve references: "this/that" → check visual context for referent
2. Detect contradictions: said vs. did → record both, don't flatten
3. Fill gaps: one modality silent, another has data → use available data
4. Weight actions over words: if stated priority conflicts with time allocation, actual behavior is truth
5. Confirm commitments: "I'll do X" in audio → check screen/camera for evidence of X

**Per-block output:** events, decisions, commitments, modal conflicts, behavioral signals.

### Stage 4: Link

Cross-day reasoning. Connect today's extractions to the existing decision graph.

The LLM receives today's extractions, open decisions, recent decisions (90 days), and active emergent states. It:

1. **Causal linking** — for each new decision, identify causes with mechanism and strength. Fill in `causes` field.
2. **Outcome detection** — scan today's events for anything that resolves an open decision. Close the loop with `actual_outcome`.
3. **Commitment tracking** — match today's actions against open commitments.
4. **State management** — create, intensify, or resolve emergent states based on accumulated patterns.
5. **Retroactive linking** — update past decisions' `effects` and `cascade_depth` as downstream effects become visible.
6. **Butterfly candidates** — flag small/trivial decisions that now show outsized downstream effects.

### Stage 5: Align + Brief

Final step. Compare today's observed reality against active intentions.

**Alignment analysis:** For each active intention, gather evidence from today's capture across all modalities. Compute alignment status, trend, and streak.

**Briefing generation:** Final LLM call. Produces morning briefing with:
- Alignment section (intentions vs. reality, per domain)
- Decisions section (new decisions with causal context and pattern matching)
- Open threads (commitments approaching deadlines, unresolved topics)
- Cascade alerts (butterfly effects, cross-domain propagation)
- State warnings (active emergent states and their root causes)

**Evening check-in generation:** 2-3 targeted questions based on behavioral signals observed today. Specific, grounded in evidence, answerable in 60 seconds.

### Pipeline Performance Budget

```
Stage 1: Prepare .............. ~2 min  (data wrangling, no models)
Stage 2: Transcribe ........... ~45 min (Whisper is the bottleneck)
Stage 3: Extract .............. ~10 min (LLM calls per block)
Stage 4: Link ................. ~3 min  (one LLM call)
Stage 5: Align + Brief ........ ~5 min  (two LLM calls)
Total ......................... ~65 min
```

### Pipeline Cost Budget

- Whisper: free (local, whisper.cpp)
- Frame descriptions: ~$0.50/day (30-50 Claude Vision calls)
- Extraction: ~$1-2/day (per-block LLM calls, mix of cheap + strong models)
- Linking + alignment + briefing: ~$0.50/day (3 strong model calls)
- **Total: ~$2-4/day**, or free with local models (slower)

## Multi-Modal Fusion

Fusion happens in the pipeline, not at capture time. Raw streams stay independent. The pipeline merges them into a unified timeline, then the LLM reasons across all modalities simultaneously.

### Resolution

One modality disambiguates another:
- "This one" in audio + camera frame showing finger on whiteboard Option B → referent resolved
- Garbled audio + screen showing Zoom chat message → screen fills in what was said
- No audio + screen shows email compose → screen reveals intent

### Contradiction (the valuable signal)

Modalities disagree:
- "Migration is my top priority" (audio) + 3% screen time on migration → silent deprioritization
- "Going to the gym" (audio) + location still at office at 8pm → intention violated
- "Great meeting, we're aligned" (audio) + elevated stress throughout → words don't match experience

Contradictions are surfaced to the alignment engine, not resolved. The gap IS the insight.

## Noise Filtering

Three levels, each reducing volume for the next.

### Level 1: Capture-Time

- **A11y tree pruning:** Strip known-static roles (MenuBar, Toolbar, StatusBar). Keep content-bearing nodes only.
- **A11y diffing:** Only emit a semantic event if >5% of content text changed since last capture.
- **Screenshot cropping:** Capture focused window content area only — exclude menu bar, title bar, dock.
- **Audio VAD:** Silero voice activity detection, skip silence.
- **Frame pHash:** Don't store wearable frames identical to previous.

### Level 2: Prepare Stage (overnight)

- **App session collapsing:** 200 captures of the same app → one session with N meaningful state changes.
- **Frame clustering:** pHash groups near-identical wearable frames, keep sharpest representative.
- **Audio quality triage:** ClearSpeech → transcribe. NoisySpeech → transcribe with low-confidence flag. BackgroundNoise/Music → skip, log duration only.

### Level 3: LLM Context Curation

- **Per-block token budget:** meetings get 8-12K tokens, deep work 3-5K, transit 500, idle 100.
- **Priority ordering:** transcript first (highest signal per token), then app session summaries, then frame descriptions.
- **Prompt suppression:** explicitly tell the LLM to ignore UI chrome, repeated app states, background audio, and routine mechanical actions.

## Silent Decision Detection

Most decisions are never spoken. They manifest as behavioral patterns captured by screen and camera.

### Three Decision Types

| Type | Source | Example |
|---|---|---|
| Spoken | Audio transcript | "Let's defer the migration" |
| Revealed | Screen + camera behavior | Spent 6 hours on Zillow (exploring moving) |
| Explained | Evening check-in | "I deleted that email because I wasn't ready" |

### Behavioral Signal Detection

The pipeline detects anomalies from screen events and camera frames:
- **Intention-action gaps:** stated priority vs. actual time allocation, calendar events missed
- **Aborted actions:** compose-without-send, cart-without-purchase, document opened repeatedly without editing
- **Attention patterns:** repeated visits to a topic, avoidance patterns, increasing time on a category
- **Behavioral transitions:** self-interrupted deep work, accelerating task switches

### Evening Check-In

2-3 targeted questions delivered in the web UI, grounded in specific behavioral observations from today. Responses (voice or text) are transcribed and become `DecisionSource::Explained` records in the decision graph. Over time, the system learns which behavioral patterns produce real decisions for this specific person and asks fewer, better questions.

## Web UI (alvum-web)

Served by embedded axum server on localhost:3741. Opened via Tauri native window from menu bar icon.

### Routes

| Route | Purpose |
|---|---|
| `/` | Today's morning briefing (landing page) |
| `/briefing/:date` | Historical briefings |
| `/checkin` | Evening check-in questions |
| `/intentions` | Manage missions, goals, habits, commitments |
| `/timeline/:date` | Raw day view (events, transcript, snapshots) |
| `/decisions` | Decision log with causal chain drill-down |
| `/settings` | Capture config, pipeline schedule, API keys, domains |
| `/api/*` | JSON API backing all views + wearable ingest |

### Morning Briefing (`/`)

A short document from an advisor who knows your history. Sections: alignment (intentions vs. reality per domain), decisions (new decisions with causal context), open threads (approaching deadlines, unresolved topics), cascade alerts (butterfly effects), state warnings (active emergent states). Evidence citations link to the timeline view.

### Evening Check-In (`/checkin`)

2-3 specific questions based on today's behavioral signals. Each has: text input, voice recording option, skip button. Low friction — 30-60 seconds total.

### Intentions (`/intentions`)

Four sections: Mission (free text, rarely changes), Goals (time-bound targets), Habits (recurring intentions with observed progress bars — computed from capture data, not self-reported), Commitments (auto-extracted from audio, user confirms/dismisses/edits).

### Decision Log (`/decisions`)

Timeline list with filtering and search. Clicking a decision shows its narrative causal chain — the story tracing causes backward and effects forward, with pattern analysis. Generated by the LLM, not a visual graph.

### Timeline (`/timeline/:date`)

Raw day view. Semantic events, transcript segments, and snapshot thumbnails interleaved chronologically. The evidence room — briefings and decisions link here when citing specific moments.

## Tauri App Shell (alvum-app)

Thin wrapper (~200 lines). Manages system-level concerns only.

**Responsibilities:**
- Menu bar icon (always visible, capture status indicator)
- Native window for web UI (opens on tray click, points to localhost:3741)
- App lifecycle (start on login, background operation)
- macOS permissions (screen recording, microphone, accessibility, location)
- Auto-update (Tauri built-in updater)
- DMG packaging and code signing
- Spawns: capture daemon, web server, pipeline scheduler

**Does not contain:** business logic, web UI rendering, data storage management. All of that lives in the crates.

## Wearable (firmware/)

ESP32-S3 based. Camera (OV2640) + MEMS microphone + IMU + microSD + WiFi/BLE.

### Form Factor

Magnetic shirt clip (~30x30x15mm). Clip to collar, pocket, or cap. One device, multiple positions — discover optimal placement empirically.

### Capture Behavior

- **Audio:** Continuous recording with on-device VAD. Opus encoding. Store to microSD, sync over WiFi.
- **Frames:** Adaptive rate. 1 per 10s default. Increase to 1 per 3-5s when IMU detects activity or faces detected. Decrease to 1 per 30-60s when idle. WebP encoding.
- **Sync:** Discovers alvum server via mDNS on local network. Uploads audio chunks and frames via HTTP POST to `/api/ingest`. Syncs when on WiFi (typically when charging overnight).

### Hardware BOM (prototype)

- ESP32-S3-DevKitC or ESP32-S3-EYE (~$15)
- OV2640 camera module
- MEMS microphone (SPH0645 or similar, I2S interface)
- BMI270 IMU
- 8-16GB microSD
- 300-400mAh LiPo
- 3D printed clip enclosure

## Model Architecture

### Provider Abstraction

All LLM calls go through a provider trait. No hardcoded API client anywhere in the pipeline.

```rust
trait ModelProvider: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
    async fn vision(&self, request: VisionRequest) -> Result<VisionResponse>;
    fn capabilities(&self) -> ModelCapabilities;
}

struct ModelCapabilities {
    max_context: usize,
    supports_vision: bool,
    supports_structured_output: bool,
    cost_tier: CostTier,           // Free, Cheap, Expensive
    privacy: PrivacyLevel,         // Local, Cloud
}

enum CostTier { Free, Cheap, Expensive }
enum PrivacyLevel { Local, Cloud }
```

Implementations:

| Provider | Use case | Privacy |
|---|---|---|
| `ClaudeProvider` | Cloud API (Opus, Sonnet, Haiku) | Cloud — data leaves machine |
| `OllamaProvider` | Local models (Llama, Qwen, Mistral) | Local — nothing leaves machine |
| `FineTunedProvider` | Custom fine-tuned models served locally | Local — trained on your data |
| `WhisperProvider` | Audio transcription (whisper.cpp) | Local — always local |
| `VisionFrameworkProvider` | macOS Vision OCR | Local — always local |

The pipeline config specifies which provider to use at each stage:

```json
{
  "models": {
    "transcription": "whisper-large-v3",
    "extraction_routine": "ollama:llama3.2",
    "extraction_rich": "claude:sonnet",
    "linking": "claude:opus",
    "briefing": "claude:opus",
    "frame_description": "ollama:llava"
  }
}
```

Users who want full privacy set every stage to a local provider. Users who want best quality use cloud for reasoning-heavy stages. Mix and match per stage based on privacy tolerance and quality needs.

### Privacy Modes

Three presets for the settings UI, covering the spectrum:

| Mode | Transcription | Extraction | Linking/Briefing | Frame description |
|---|---|---|---|---|
| **Full privacy** | Whisper (local) | Ollama (local) | Ollama (local) | LLaVA (local) |
| **Hybrid** | Whisper (local) | Ollama (local) | Claude API | Claude Vision |
| **Best quality** | Whisper (local) | Claude Haiku | Claude Opus | Claude Vision |

Transcription is always local (Whisper) — audio is the most sensitive data and whisper.cpp is already excellent. The privacy tradeoff is in the reasoning stages.

### Fine-Tuning Pipeline

After months of operation, the system accumulates labeled training data:

| Task | Training data source | Volume after 6 months |
|---|---|---|
| Decision extraction | Pipeline extractions validated by user (evening check-in confirms/corrects) | ~3,000-5,000 labeled decisions |
| Causal linking | Links confirmed through outcome observation (predicted → actual outcome match) | ~1,000-2,000 validated causal chains |
| Behavioral signal classification | Check-in responses labeling which signals were real decisions vs. noise | ~500-1,000 labeled signals |
| Briefing quality | Implicit feedback — which briefing items did the user engage with, which were dismissed | ~200-400 briefing sessions |

This data is already structured (it's the decision graph + day extractions). Fine-tuning a smaller model on this person-specific data produces a model that:

- Knows what THIS person considers a decision (vs. noise)
- Understands THIS person's causal patterns
- Recognizes THIS person's behavioral signatures
- Generates briefings calibrated to THIS person's attention and interests

**Fine-tuning workflow:**

```
User's data (6+ months)
    │
    ▼
Export training pairs from:
  - decisions/ (input: transcript → output: extracted decisions)
  - days/ (input: multi-modal context → output: events + decisions)
  - checkin responses (input: behavioral signal → output: was it a real decision?)
    │
    ▼
Fine-tune a small local model (e.g., Llama 3.2 8B, Qwen 2.5 7B)
  - LoRA or QLoRA for efficiency
  - Runs on Apple Silicon (MLX)
    │
    ▼
Replace cloud provider with fine-tuned local model
  - Full privacy: nothing leaves the machine
  - Lower cost: $0/day instead of $2-4/day
  - Personalized: better extraction quality for this specific person
```

### RL / RLHF for Briefing Quality

The morning briefing has a natural reward signal: **did the user act on it?**

Observable signals:
- User clicked through to evidence (engaged with the insight)
- User updated an intention after reading the briefing (it prompted reflection)
- User dismissed a briefing item (it wasn't useful)
- User answered the evening check-in question that was generated from a pattern (the pattern was real)
- User skipped the check-in question (the pattern was noise)

Over time, this creates preference data:

```
Briefing item A: user engaged → positive signal
Briefing item B: user dismissed → negative signal
Briefing item C: user acted on it, changed a goal → strong positive signal
```

This can train a reward model that scores potential briefing items by predicted user engagement. The briefing agent then optimizes for items the user will actually find valuable — learning to shut up about things this person doesn't care about and surface the patterns that drive real reflection.

Not V1. But the architecture collects the feedback data from day one (user interactions with the web UI are logged), so when we're ready to train, the data is there.

## Growth Path

Each version triggered by observed limitations, not anticipated need.

| Version | Adds | Trigger |
|---|---|---|
| V1 | Desktop capture + wearable audio/camera + overnight pipeline + decision graph + alignment engine + morning briefing + evening check-in | Initial build |
| V1.5 | Learnable noise filters | Pipeline extracts zero events from recurring app patterns |
| V2 | Intra-day query agent | Users need "what did I decide about X?" without waiting for overnight |
| V2.5 | Local model support (Ollama) | Users want full privacy mode |
| V3 | Multi-device support | Second machine, shared data via Syncthing |
| V4 | Fine-tuned extraction model | 6+ months of validated data, ready to personalize |
| V5 | Structured storage (SQLite/DuckDB) | decisions.jsonl exceeds LLM context window (~2+ years of data) |
| V6 | RL-trained briefing optimization | Enough user feedback data to train a reward model |
| V7 | Embedding-based semantic search | Keyword search proves insufficient for retrieval |
