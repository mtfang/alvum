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

**V1 ships as a desktop app + wearable (DIY tier).** The "box" is your primary machine running an Electron shell over the local Rust services. The dedicated appliance and managed tier are future product expansions once the software is proven. The crate architecture supports both — nothing couples to a specific deployment model.

## Architecture Overview

```
alvum/
├── crates/
│   ├── alvum-core                  ← types, config, Connector/CaptureSource/Processor traits
│   ├── alvum-pipeline              ← align → distill → learn → link → brief (library functions)
│   ├── alvum-episode               ← episodic alignment (time blocks + context threading)
│   ├── alvum-knowledge             ← knowledge corpus extraction and storage
│   │
│   ├── alvum-connector-audio       ← audio connector (bundles mic+system capture + whisper)
│   ├── alvum-connector-screen      ← screen connector (bundles screen capture + vision/OCR)
│   ├── alvum-connector-claude      ← claude-code connector (session parser, no capture)
│   ├── alvum-connector-git         ← git connector (future)
│   ├── alvum-connector-wearable    ← ESP32 ingest endpoint (future)
│   │
│   ├── alvum-capture-audio         ← internal: mic + system audio primitives
│   ├── alvum-capture-screen        ← internal: screen capture primitive
│   ├── alvum-processor-audio       ← internal: whisper transcription
│   ├── alvum-processor-screen      ← internal: vision model + OCR
│   │
│   └── alvum-cli                   ← orchestrator: loads connectors, runs capture/extract
├── app/                            ← Electron app shell (cross-platform desktop)
└── firmware/                       ← ESP32 wearable firmware (separate build)
```

Connector crates are the user-facing unit. They compose capture and processor primitives into complete plugins. Internal capture/processor crates remain (they're reusable building blocks), but users only interact with connectors.

### Connectors: The User-Facing Plugin Concept

A **Connector** is what the user adds and manages — a complete plugin that owns a data source end-to-end, from raw capture through processing to LLM-ready observations. The user thinks in terms of connectors: "I have an audio connector, a screen connector, a Claude Code connector."

Under the hood, a connector is a bundle composed of two primitives:

- **Capture**: always-on daemons or one-shot importers that produce raw data files (`DataRef` JSONL)
- **Processor**: interprets raw data into LLM-readable `Observation` objects

The user doesn't see this composition — they see one connector per data source. Capture and processor are the matrix of reusable primitives that connector implementers compose.

```
USER SEES                  UNDER THE HOOD
                           ┌────────────────────────────┬──────────────────────────┐
                           │ CAPTURE (primitive)        │ PROCESSOR (primitive)    │
[connectors.audio]         │ AudioMicSource             │ WhisperProcessor         │
[connectors.screen]        │ ScreenSource               │ VisionProcessor or OCR   │
[connectors.claude-code]   │ ClaudeSessionImporter      │ (identity — already text)│
[connectors.git]           │ GitLogImporter             │ DiffProcessor            │
                           └────────────────────────────┴──────────────────────────┘

All connectors produce Observations → fed to the pipeline (align → distill → link → brief).
```

### The Three Layers

Data flows through three independently extensible layers:

**Capture** (produces `DataRef` — file pointers). Daemons that run continuously (audio, screen) or one-shot importers that read existing data (Claude sessions, git log).

**Process** (produces `Observation` — text content + metadata). Reads DataRefs by type and interprets into LLM-readable form. Multiple processors can handle the same DataRef, each adding different output (text description, embedding vector, structured analysis).

**Pipeline** (reasons over observations). Episodic alignment, decision extraction, causal linking, briefing. Source-agnostic — it reads Observations regardless of which connector produced them.

The **Connector** bundles one or more capture sources with one or more processors. It's the deployment unit and the user's unit of configuration.

### DataRef — What Connectors Produce

A connector is any executable that writes JSONL to stdout. Each line is a pointer to a file:

```rust
struct DataRef {
    ts: DateTime<Utc>,
    source: String,           // connector name
    path: String,             // file path
    mime: String,             // MIME type
    metadata: Option<Value>,  // connector-specific context
}
```

```json
{"ts":"2026-04-11T10:15:00Z","source":"audio-mic","path":"capture/audio/mic/10-15-00.opus","mime":"audio/opus"}
{"ts":"2026-04-11T10:15:02Z","source":"screen","path":"capture/snapshots/10-15-00.webp","mime":"image/webp"}
{"ts":"2026-04-11T09:00:00Z","source":"claude-code","path":"session.jsonl","mime":"application/x-jsonl"}
```

### Artifact — What Processors Produce

A processor reads a DataRef, processes the file, and produces an Artifact with typed output layers:

```rust
struct Artifact {
    data_ref: DataRef,                              // always linked to source file
    layers: HashMap<String, serde_json::Value>,     // typed outputs, open-ended
}

trait Processor: Send + Sync {
    fn name(&self) -> &str;
    fn supported_mimes(&self) -> &[&str];
    fn process(&self, data: &DataRef) -> Result<Vec<Artifact>>;
}
```

Layers are namespaced strings. Convention:

| Layer key | Contains | Used by |
|---|---|---|
| `text` | Human/LLM-readable content | Pipeline (decision extraction) |
| `embedding` | Vector + model + dimensions | Embedding index (retrieval) |
| `structured` | Parsed data (timestamps, speakers, entities) | Direct queries, analysis |
| `media` | Ref to transformed media (resized image, extracted audio track) | Multimodal embedding |

A single file can go through **multiple processors**, each adding layers:

```
DataRef: 10-15-00.opus (audio/opus)
  ├─ WhisperProcessor
  │   layers: {
  │     "text": "I think we should defer the migration",
  │     "structured": {"segments": [...], "language": "en"}
  │   }
  ├─ GeminiEmbeddingProcessor (future)
  │   layers: {
  │     "embedding": {"model": "gemini-embedding-2", "vector": [...], "dims": 768}
  │   }
  └─ SentimentProcessor (future)
      layers: {
        "structured.sentiment": {"emotion": "anxious", "confidence": 0.82}
      }
```

### Connector Configuration

Users configure connectors — each connector entry in config.toml represents one complete data-source plugin. The connector internally declares which capture primitives and processors it uses; the user's config is about *what* data they want captured and *how* that source should behave, not about the internals.

```toml
[connectors.audio]
enabled = true
mic = true                 # enable mic capture
system = true              # enable system audio capture
whisper_model = "~/.alvum/runtime/models/ggml-base.bin"

[connectors.screen]
enabled = true
vision = "local"           # local | api | ocr | off
idle_interval_secs = 30

[connectors.claude-code]
enabled = true
session_dir = "~/.claude/projects"

[connectors.git]           # future
enabled = false
repos = ["~/code/alvum", "~/code/work"]
since = "7d"
```

Each connector advertises its capabilities to the user: the audio connector shows mic/system toggles and model path; the screen connector shows vision mode and idle interval; the claude-code connector shows session directory. Under the hood, each composes capture + processor internals appropriately.

`alvum capture` starts all enabled connectors that have capture daemons (audio, screen). `alvum extract` runs all enabled connectors, pulling observations through each's processor, and feeds the unified observation stream to the pipeline.

### Internal Primitives (for connector implementers)

When building a new connector (Rust crate, eventually external executable), you compose two traits:

```rust
/// CaptureSource — writes raw DataRefs to the capture directory.
/// Used by always-on daemons (audio, screen) or one-shot importers (Claude sessions, git log).
#[async_trait]
pub trait CaptureSource: Send + Sync {
    fn name(&self) -> &str;
    async fn run(&self, capture_dir: &Path, shutdown: watch::Receiver<bool>) -> Result<()>;
}

/// Processor — reads DataRefs, produces Observations.
/// Matched to DataRefs by source name or MIME type.
#[async_trait]
pub trait Processor: Send + Sync {
    fn name(&self) -> &str;
    fn handles(&self) -> &[&str];  // source names or MIME patterns this processor handles
    async fn process(&self, data_refs: &[DataRef], capture_dir: &Path) -> Result<Vec<Observation>>;
}

/// Connector — the user-facing plugin. Bundles capture + processor.
pub trait Connector: Send + Sync {
    fn name(&self) -> &str;
    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>>;
    fn processors(&self) -> Vec<Box<dyn Processor>>;
    fn from_config(settings: &HashMap<String, toml::Value>) -> Result<Self> where Self: Sized;
}
```

Example: the `AudioConnector` owns `AudioMicSource` + `AudioSystemSource` for capture, and `WhisperProcessor` for processing. The `ScreenConnector` owns `ScreenSource` for capture, and either `VisionProcessor`, `OllamaVisionProcessor`, or `OcrProcessor` depending on config. The `ClaudeCodeConnector` has no capture daemon (reads existing sessions) and its processor is a simple JSONL parser.

This split gives us matrix flexibility internally: a new processor (e.g., multimodal embedding) can be reused across multiple connectors; a new capture primitive can be paired with existing processors.

### Pipeline Stages

```
Gather → Process → Align → Distill → Learn → Link → Brief
                     ↑                   ↓
                     └── knowledge corpus ──┘  (feedback loop)
```

| Stage | What it does | Crate |
|---|---|---|
| **Capture** | Connectors' capture sources write DataRefs (file pointers) | alvum-connector-* (uses alvum-capture-*) |
| **Process** | Connectors' processors produce Observations | alvum-connector-* (uses alvum-processor-*) |
| **Align** | Time blocks + context threading + relevance scoring | alvum-episode |
| **Distill** | LLM reads high-relevance threads, extracts decisions | alvum-pipeline |
| **Learn** | Extract entities, relationships, patterns, facts → update knowledge corpus | alvum-knowledge |
| **Link** | LLM connects decisions causally, using knowledge corpus for context | alvum-pipeline |
| **Brief** | LLM generates proactive briefing, referencing knowledge corpus | alvum-pipeline |

The **Align** stage is new (episodic alignment — see `docs/superpowers/specs/2026-04-12-episodic-alignment.md`). It sits between processing and extraction, filtering noise and establishing cross-source context.

The **Learn** stage is new (knowledge extraction). It runs after decision distillation, extracting entities, relationships, and patterns as a side effect. The corpus feeds back into alignment (relevance scoring) and linking (causal context) on subsequent runs.

**V0 (done):** Claude Code connector → pipeline (distill → link → brief).

**V1 (current):** Audio capture + Whisper processor + pipeline.

**V1.5 (next):** Episodic alignment + knowledge corpus. Cross-source threading, relevance scoring, accumulated knowledge.

**V2+:** Additional connectors, processors, embeddings. Knowledge corpus grows richer with each new source.

- **Language**: Rust throughout
- **Platform**: cross-platform desktop first (macOS, then Windows/Linux)
- **Distribution**: Electron app packages (.dmg/.msi/.AppImage) with auto-update
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
    source: IntentionSource,
    confirmed: bool,               // user has explicitly validated this intention
    last_relevant: Option<NaiveDate>, // last date evidence of engagement was observed
}

enum IntentionKind {
    Mission,      // core values, identity — emerges over months
    Goal,         // time-bound target
    Habit,        // recurring intention
    Commitment,   // promise to someone (auto-extracted from audio)
}

enum IntentionSource {
    UserDefined,   // typed in manually on /intentions page
    CheckIn,       // emerged from evening check-in dialogue
    Inferred,      // observed from behavior patterns, awaiting confirmation
    Extracted,     // auto-extracted from audio (commitments to others)
}
```

### Intention Capture UX

Intentions enter the system through conversation, not forms. The primary path is **progressive dialogue** via the evening check-in and morning briefing. Manual entry is the fallback for users who already know what they want.

**Onboarding (30 seconds):**
```
Step 1: Select your domains (checkboxes, sensible defaults)
Step 2: "Anything you're working toward right now?" (free text, skip OK)
Done. Capture starts immediately.
```

No mission statements. No goal-setting wizard. The system starts observing and asks questions later when it has something to ask about.

**Week 1-2 (progressive intention building):**

The evening check-in gains a new question type: **intention probes**. These are grounded in observed behavior, not abstract:

```
"You spent 4 hours on the API project today — is that a current priority?"
  [Yes, it's my focus]  [No, I got pulled in]  [It's complicated]

"You had Gym on your calendar at 6pm but stayed at the office.
 Is getting to the gym regularly something you're working toward?"
  [Yes, I want to go 3x/week]  [Not right now]  [I keep meaning to]

"You've mentioned the migration to 3 different people this week.
 Is shipping it a goal, or just something on your mind?"
  [It's a goal — by end of month]  [Just on my mind]  [It's someone else's problem]
```

Each response creates or refines an intention:
- "Yes, I want to go 3x/week" → creates Habit { description: "Gym", cadence: 3x/week, source: CheckIn, confirmed: true }
- "Not right now" → no intention created (or marks existing one inactive)
- "It's complicated" → creates Intention { confirmed: false } for follow-up

**Intention probe generation** is a pipeline stage: after extracting decisions and behavioral signals, the system identifies patterns that look like they SHOULD have an intention behind them but don't. The probe asks the user to make the implicit explicit.

**Week 3+ (full alignment mode):**

Enough intentions are established for meaningful alignment reports. The morning briefing now shows intention-vs-reality gaps. The evening check-in continues refining — catching new intentions, confirming inferred ones, and flagging stale ones.

**Stale intention detection:**

```
"You set 'Learn Spanish by December' 3 months ago. You haven't
 spent time on it in 6 weeks. Still relevant?"
  [Yes, recommit]  [Deprioritize]  [Remove]
```

Intentions that haven't been engaged with (no behavioral evidence) for a configurable period get flagged. The system doesn't silently drop them — it asks. The response is itself a decision that enters the graph.

**Commitment extraction (automatic):**

Commitments to others are extracted from audio transcripts by the pipeline:
- "I'll have the spec to Sarah by Friday" → creates Commitment { to: "Sarah", by: Friday, source: Extracted, confirmed: false }
- Surfaced in the evening check-in for confirmation: "Did you mean to commit to this?"
- Tracked against observed behavior: did you send the spec?

**Mission/values (emergent, not forced):**

Missions and core values are never asked for upfront. They emerge over months from the pattern of goals and habits. After 2-3 months, the system might observe:

```
"Your goals cluster around three themes: health, being present for family,
 and building things. These look like core values. Want to name them?"
```

This is the inverse of traditional goal-setting apps: instead of top-down (mission → goals → habits), intentions build bottom-up (behavior → habits → goals → mission emerges).

**Manual fallback (/intentions page):**

Always available for users who prefer explicit entry. Four sections:
- Mission: free text, rarely changes
- Goals: time-bound targets with domain
- Habits: recurring intentions with cadence
- Commitments: auto-extracted, user confirms/dismisses

Progress bars on habits are computed from capture data — the system observes whether you went to the gym, it doesn't ask you to check a box.

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

### Knowledge Corpus

Accumulated facts, entities, relationships, and behavioral patterns extracted from observations over time. This is the system's long-term semantic memory — what it "knows" about the person's life, independent of any specific episode or decision.

The knowledge corpus is both OUTPUT (extracted by the pipeline) and INPUT (fed back into every pipeline stage for context). It's the feedback loop that makes the system smarter over time.

```rust
/// A known entity in the person's life.
struct Entity {
    id: String,
    name: String,
    /// Free-form type: "person", "project", "place", "organization",
    /// "tool", "concept" — any string.
    entity_type: String,
    /// What the system knows about this entity.
    description: String,
    /// How this entity relates to others.
    relationships: Vec<Relationship>,
    /// When this entity was first observed.
    first_seen: NaiveDate,
    /// When this entity was last referenced in observations.
    last_seen: NaiveDate,
    /// Structured attributes (role, location, status, etc.).
    attributes: Option<serde_json::Value>,
}

/// A relationship between two entities.
struct Relationship {
    target_id: String,
    /// Free-form: "manages", "reports_to", "blocks", "part_of",
    /// "married_to", "lives_at" — any string.
    relation: String,
    /// When this relationship was established or last confirmed.
    last_confirmed: NaiveDate,
}

/// A recurring behavioral pattern observed over time.
struct Pattern {
    id: String,
    description: String,
    /// How many times this pattern has been observed.
    occurrences: u32,
    /// When first and last observed.
    first_seen: NaiveDate,
    last_seen: NaiveDate,
    /// Domains this pattern affects.
    domains: Vec<String>,
    /// Decision IDs that exemplify this pattern.
    evidence: Vec<String>,
}

/// A persistent fact about the person's life.
struct Fact {
    id: String,
    content: String,
    /// Free-form category: "routine", "preference", "constraint", "context".
    category: String,
    /// When learned.
    learned: NaiveDate,
    /// When last confirmed by observation.
    last_confirmed: NaiveDate,
    /// Source that established this fact.
    source: String,
}
```

**How the corpus feeds back into the pipeline:**

| Pipeline stage | What it gets from the corpus |
|---|---|
| Relevance scoring | Entity names → "Sarah" in audio is high-relevance because she's the user's manager |
| Thread classification | Entity relationships → audio about "the migration" + screen showing Linear = same project thread |
| Decision extraction | Known entities + relationships → richer context for the LLM to identify decisions |
| Causal linking | Patterns → "this matches your deferral-under-pressure pattern" |
| Briefing generation | Everything → the agent references established facts, not rediscovering them |
| Intention probes | Entities + patterns → ask about specific people, projects, and behaviors |

**Extraction:** The pipeline extracts knowledge as a side effect of decision extraction. Each pipeline run:
1. Reads the existing corpus for context
2. Extracts any new entities, relationships, patterns, or facts from today's observations
3. Updates or confirms existing entries (refreshes `last_seen` / `last_confirmed`)
4. Flags stale entries (not referenced in 30+ days)

**The corpus grows incrementally.** Day 1: sparse. Month 3: rich. Year 1: comprehensive. It never needs to be rebuilt — each pipeline run adds to it.

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

All data lives as files on disk under a single root: **`~/.alvum/`**. No database in V1.

**Authoritative layout spec: `docs/superpowers/specs/2026-04-18-storage-layout.md`.** That document owns the tree and the per-file semantics; this section is a summary of the shape for orientation only. If this summary ever disagrees with the storage-layout spec, the storage-layout spec wins.

Three top-level lifecycle buckets:

```
~/.alvum/
├── VERSION                                # data-layout version (integer)
├── capture/                               # GROUND TRUTH — raw ingest; kept indefinitely
│   └── YYYY-MM-DD/
│       ├── audio/ screen/ location/ health/ frames/
│       └── events.jsonl
├── generated/                             # CURRENT DERIVATION — LLM output + user-stated state
│   ├── decisions/    (index, open, states)
│   ├── knowledge/    (entities, patterns, facts)
│   ├── days/         (YYYY-MM-DD.json = DayExtraction)
│   ├── episodes/     (YYYY-MM-DD/{threads,time_blocks}.json)
│   ├── briefings/    (YYYY-MM-DD/briefing.md)
│   ├── checkins/     (YYYY-MM-DD/{questions.json, responses.jsonl})
│   ├── intentions.json
│   └── life_phase.json
└── runtime/                               # OPERATIONAL — regenerable; never back up
    ├── bin/alvum
    ├── config.toml
    ├── email.txt
    ├── logs/
    ├── devices/    (registry, tokens, heartbeats — Phase C+)
    ├── embeddings/ (V2+)
    └── cache/
```

### Retention Policy

- **`capture/` (ground truth): kept indefinitely** by default. Raw data is the re-extraction fuel when models improve — a year of 2026 audio produces a materially better decision graph when re-run through 2028's models. Users can opt into per-source pruning via `runtime/config.toml` `[retention.capture]` if disk-pressured; evidence-linked media stays by default.
- **`generated/`: forever.** Small, partly re-derivable from `capture/` + a better model, partly user-stated (intentions, check-in responses, confirmed facts) and irreplaceable.
- **`runtime/`: prune as needed.** Logs rotate by their owning subsystem; caches and indexes regenerate; tokens re-issued on re-pairing.

Backup rule: one line — `rsync -av --delete --exclude runtime/ ~/.alvum/ <backup-target>/`. See the storage-layout spec for the full policy and the re-extraction workflow.

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

The desktop app's embedded HTTP server accepts uploads from the ESP32:

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

Served by embedded axum server on localhost:3741. Opened via Electron BrowserWindow from tray/menu bar.

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

A short document from an advisor who knows your history. The briefing's character changes as intentions build up:

- **Week 1-2 (learning phase):** Surfaces behavioral patterns and decisions without alignment scoring. "Here's what you spent time on" — observation, not judgment. Asks questions that build the intention model.
- **Week 3+ (full alignment mode):** Shows intention-vs-reality gaps per domain. Alignment scores. Trend lines. "Your gym attendance dropped from 3x to 1x this month."

Sections: alignment (intentions vs. reality per domain), decisions (new decisions with causal context), open threads (approaching deadlines, unresolved topics), cascade alerts (butterfly effects), state warnings (active emergent states), stale intention flags. Evidence citations link to the timeline view.

### Evening Check-In (`/checkin`)

2-4 questions, two types:
- **Behavioral probes** (existing): grounded in specific observed actions ("you deleted that email — what was behind that?")
- **Intention probes** (new): grounded in observed patterns that lack a stated intention ("you've been to the gym 3 times this week — is that a goal you're building?")

Each question has: text input, voice recording option, multiple choice shortcuts, skip button. Low friction — 60-90 seconds total. Responses feed into both the decision graph (as explained decisions) and the intention model (creating or refining intentions).

### Intentions (`/intentions`)

Four sections: Mission (free text, rarely changes), Goals (time-bound targets), Habits (recurring intentions with observed progress bars — computed from capture data, not self-reported), Commitments (auto-extracted from audio, user confirms/dismisses/edits).

### Decision Log (`/decisions`)

Timeline list with filtering and search. Clicking a decision shows its narrative causal chain — the story tracing causes backward and effects forward, with pattern analysis. Generated by the LLM, not a visual graph.

### Timeline (`/timeline/:date`)

Raw day view. Semantic events, transcript segments, and snapshot thumbnails interleaved chronologically. The evidence room — briefings and decisions link here when citing specific moments.

## Electron App Shell (alvum-app)

Thin wrapper (~200 lines). Manages system-level concerns only.

**Responsibilities:**
- Tray/menu bar icon (always visible, capture status indicator)
- Native window for web UI (opens on tray click, points to localhost:3741)
- App lifecycle (start on login, background operation)
- OS permissions (screen recording, microphone, accessibility, location)
- Auto-update (Electron updater)
- Desktop packaging and code signing/notarization per OS
- Spawns: capture daemon, web server, pipeline scheduler

**Does not contain:** business logic, web UI rendering, data storage management. All of that lives in the crates.

### Performance constraints for the app shell

- Keep Electron main process minimal; all heavy compute remains in Rust crates/sidecars.
- Prefer streaming IPC and file-backed artifacts over large JSON payloads between renderer and backend.
- Use process isolation (`contextIsolation`, sandboxed renderer, no Node in renderer) to keep UI responsive and secure.
- Budget cold-start latency and steady-state memory explicitly in release criteria.

### Explicit non-goal

- Do **not** use Tauri for the long-term app shell roadmap (maturity and cross-platform compatibility risk for our target product scope).

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

## Embedding Strategy

### Architecture

Embeddings provide **retrieval** when the decision graph outgrows LLM context. They sit between the pipeline output and the LLM reasoning stages as an optional layer — skipped when data fits in context, activated when it doesn't.

```rust
trait EmbeddingProvider: Send + Sync {
    async fn embed_text(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_image(&self, image: &[u8]) -> Result<Vec<f32>>;
    async fn embed_audio(&self, audio: &[u8]) -> Result<Vec<f32>>;
    fn dimensions(&self) -> usize;
    fn supported_modalities(&self) -> Vec<Modality>;
}

enum Modality { Text, Image, Audio, Video }
```

### Multimodal Embedding: Why and When

The pipeline converts most data to text (transcripts, descriptions, semantic events). Text-only embeddings cover ~95% of retrieval needs because all modalities converge to text before reaching the embedding layer.

Multimodal embeddings become valuable for **raw media retrieval** — searching by visual or auditory similarity without relying on text descriptions:

- "Find camera frames that look like a whiteboard" (raw image → image embedding)
- "Find moments where I sounded stressed" (raw audio → audio embedding, compared against stress-prosody examples)
- "Find screenshots showing this error message" (text query → matched against raw screenshot embeddings)

These queries bypass the text-description bottleneck — the answer might exist in a frame the vision model described poorly, or an audio clip that transcription missed nuance on.

### Provider Tiers (Matches Product Tiers)

| Tier | Text | Text + Image | Text + Image + Audio | Notes |
|---|---|---|---|---|
| **MVP** | None | None | None | Data fits in LLM context |
| **DIY (local)** | Nomic Embed Text v1.5 (93M, CPU) | Nomic Embed Vision (shared space, 93M) | ONE-PEACE (Apache 2.0, 4B, GPU) | Fully private, runs on Apple Silicon |
| **Managed (cloud)** | Gemini Embedding 001 | Gemini Embedding 2 | Gemini Embedding 2 | All modalities in one API |

Model choice per tier in config:

```json
{
  "embeddings": {
    "provider": "nomic-local",
    "model": "nomic-embed-text-v1.5",
    "dimensions": 768,
    "multimodal": false
  }
}
```

Upgrade path: `"multimodal": true` switches to a multimodal provider (Nomic Vision for local, Gemini Embedding 2 for cloud). Same vector store, same retrieval interface — the embedding backend changes transparently.

### What Gets Embedded

Every artifact the pipeline produces gets an embedding (when the embedding layer is active):

| Artifact | Embedding type | When |
|---|---|---|
| Decision summary | Text | Always (when embeddings active) |
| Event summary | Text | Always |
| Activity block summary | Text | Always |
| Day summary | Text | Always |
| Observation content | Text | Always |
| Raw camera frames | Image (multimodal) | When multimodal enabled |
| Raw audio segments | Audio (multimodal) | When multimodal enabled |
| Screenshots | Image (multimodal) | When multimodal enabled |

All embeddings share the same vector space — a text query can match against text, image, or audio embeddings, enabling true cross-modal retrieval.

### Storage

Embeddings are derived artifacts (regenerable from `generated/` + a re-embed pass), so they live under `runtime/`:

```
~/.alvum/runtime/embeddings/
├── decisions.idx           ← vector index over decision embeddings
├── events.idx              ← vector index over event embeddings
├── observations.idx        ← vector index over observation embeddings
└── media.idx               ← multimodal index (frames, audio, screenshots)
```

Vector store options: `usearch` (Rust-native, lightweight), `hnswlib`, or SQLite with `sqlite-vss`. No heavy infrastructure — a single-file index per collection.

### Retrieval Flow

When the causal analysis or briefing stages need historical context:

```
LLM needs: "decisions similar to today's migration deferral"
    │
    ▼
Embed query: "deferred infrastructure work under time pressure"
    │
    ▼
Vector search: top-K nearest decisions from decisions.idx
    │
    ▼
Retrieved decisions injected into LLM prompt as context
    │
    ▼
LLM reasons over current + retrieved decisions
```

When multimodal is active, the same flow works for raw media:

```
LLM needs: "find the whiteboard from that planning meeting"
    │
    ▼
Embed query as text → search media.idx (text vs image embeddings)
    │
    ▼
Retrieved frame paths → inject frame descriptions + thumbnails into prompt
```

## Growth Path

Each version triggered by observed limitations, not anticipated need.

| Version | Adds | Trigger |
|---|---|---|
| V0 | Claude Code connector + pipeline + decision graph + briefing (MVP) | Initial build — validate core with conversation logs |
| V0.5 | Audio capture + Whisper transcription processor | V0 validated, first real capture source |
| V1 | Episodic alignment (time blocks + context threads) + knowledge corpus | Audio captures noise (TV, ambient) — need cross-context filtering and accumulated knowledge |
| V1.5 | Desktop capture + wearable connectors + alignment engine + evening check-in | Episodic layer validated, ready for multi-modal capture |
| V2 | Text embeddings for retrieval | Knowledge corpus + decisions exceed LLM context (~6 months) |
| V2.5 | Local model support (Ollama) | Users want full privacy mode |
| V3 | Multimodal embeddings (raw image/audio search) | Text descriptions prove insufficient for retrieval quality |
| V3.5 | Multi-device support | Second machine, shared data via Syncthing |
| V4 | Fine-tuned extraction model | 6+ months of validated data, ready to personalize |
| V5 | Structured storage (SQLite/DuckDB) | Vector index + JSONL not scaling for complex queries |
| V6 | RL-trained briefing optimization | Enough user feedback data to train a reward model |
