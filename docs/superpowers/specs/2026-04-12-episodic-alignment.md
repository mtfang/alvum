# Episodic Alignment: Time Blocks + Context Threads

Cross-source temporal alignment and relevance scoring for multi-modal observations. This is the missing layer between processors (which understand individual files) and the pipeline (which extracts decisions). It answers: "what was actually happening, and does it matter?"

## Problem

Observations from different sources arrive independently. A 30-second audio transcript doesn't know that Netflix was on screen, or that a calendar event was active, or that the user was at a coffee shop. Without cross-context, the pipeline can't distinguish:

- Your conversation from TV dialogue
- Your self-talk from someone else's conversation nearby
- A work decision from a routine transaction
- A meeting you're in from a meeting happening at the next table

The solution: assemble observations from ALL sources into time-aligned episodes, trace coherent context threads across time, and score each thread's relevance before feeding it to the pipeline.

## Architecture

Two passes over all observations from all sources:

```
ALL observations (every source, every time)
    │
    ▼  Pass 1: Time Block Assembly (deterministic, no LLM)
    
TimeBlocks — fixed 5-minute windows, all observations bucketed by timestamp
    │
    ▼  Pass 2: Context Threading (single LLM call, full-day context)
    
ContextThreads — coherent activities traced across blocks,
                  relevance-scored, classified, labeled
    │
    ├──► ALL threads saved as episodic evidence (threads.json)
    │
    ▼  Filter by relevance
    
High-relevance threads only → Pipeline (extract decisions)
```

## Data Model

### TimeBlock

A fixed-duration window containing all observations from all sources. Pure temporal quantization — no intelligence, no filtering.

```rust
struct TimeBlock {
    /// Start of this time window.
    start: DateTime<Utc>,
    /// End of this time window (start + block_duration).
    end: DateTime<Utc>,
    /// All observations that fall within this window, from any source.
    observations: Vec<Observation>,
}
```

### ContextThread

A coherent context that spans one or more TimeBlocks. Represents a continuous activity — a meeting, a coding session, a TV show, a commute. Multiple threads can overlap in time (TV playing while coding). Each observation belongs to exactly one thread.

```rust
struct ContextThread {
    /// Sequential identifier.
    id: String,
    /// Human-readable label ("Sprint Planning with Sarah", "Netflix background").
    label: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    /// Which data sources contribute to this thread.
    sources: Vec<String>,
    /// All observations assigned to this thread.
    observations: Vec<Observation>,
    /// 0.0 = noise, 1.0 = definitely contains actionable content.
    relevance: f32,
    /// Why this relevance score ("multi-source convergence", "calendar match", "media dialogue detected").
    relevance_signals: Vec<String>,
    /// Free-form classification. Convention: "conversation", "solo_work",
    /// "media_playback", "ambient", "transition", "phone_call" — any string valid.
    thread_type: String,
    /// Structured metadata (participants, meeting title, show name, project, etc.).
    metadata: Option<serde_json::Value>,
}
```

### ThreadingResult

The complete output of the episodic alignment process for a time period.

```rust
struct ThreadingResult {
    /// Time range covered.
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    /// Pass 1 output.
    time_blocks: Vec<TimeBlock>,
    /// Pass 2 output.
    threads: Vec<ContextThread>,
    /// Total observations processed.
    observation_count: usize,
    /// How many sources contributed data.
    source_count: usize,
}
```

## Pass 1: Time Block Assembly

Deterministic. No LLM. Takes all observations, sorts by timestamp, buckets into fixed-duration windows.

**Algorithm:**

1. Sort all observations by timestamp.
2. Find earliest and latest timestamps.
3. Create blocks from earliest to latest at 5-minute intervals.
4. Place each observation in the block that contains its timestamp.
5. Drop empty blocks (no observations).

**Parameters:**
- `block_duration`: 5 minutes (configurable). Too short fragments threads, too long blurs concurrent contexts.

**Properties:**
- Every observation lands in exactly one block.
- Nothing is discarded.
- Output is deterministic given the same input.
- No LLM cost.

**A 16-hour day produces max 192 blocks.** Most will be sparse. Active periods (meetings, work sessions) will have dense blocks; idle time (sleep, away) will have gaps.

## Pass 2: Context Threading (LLM-Driven)

One LLM call per day over the full set of time blocks. Uses long-context reasoning to identify concurrent threads, classify them, and score relevance.

**Why LLM over heuristics:** The LLM with 200K context sees the full day at once. It can:
- Notice that "the migration" was discussed in the 10:00 meeting AND the 14:00 hallway chat — same decision thread, different times and participants
- Recognize scripted TV dialogue from conversational speech by content and style
- Understand that audio at 21:00 referencing fictional characters is entertainment, not decisions
- Detect that a brief self-talk moment ("I should really email Sarah about this") during a coding session is an intention signal worth surfacing
- Cross-reference audio against screen events, calendar, and location to disambiguate concurrent activities

Heuristics would reimplementing worse versions of what the LLM does natively with long context.

**Input format:** Time blocks formatted as an interleaved timeline:

```
=== Block 10:00-10:05 ===
[10:00:15] [audio-mic/speech_segment] "I think we should defer the migration until after the release"
[10:00:15] [screen/app_focus] Zoom — "Sprint Planning"
[10:01:00] [calendar/event] "Sprint Planning" 10:00-10:30, attendees: Sarah, James
[10:03:22] [audio-mic/speech_segment] "Yeah let's push to August"
[10:04:10] [screen/field_changed] Linear — Status: "In Progress" → "Backlog"

=== Block 10:05-10:10 ===
[10:05:00] [audio-mic/speech_segment] "Previously on Breaking Bad..."
[10:05:30] [screen/app_focus] Netflix
[10:07:00] [audio-mic/speech_segment] "I am the one who knocks"
[10:08:30] [screen/app_focus] VS Code — main.rs (brief switch)
[10:09:00] [screen/app_focus] Netflix (switched back)
```

**System prompt for threading:**

```
You are analyzing a full day of captured data from multiple sensors.
The data is organized into 5-minute time blocks, each containing
observations from various sources (audio transcripts, screen events,
location, calendar, etc.).

Identify CONTEXT THREADS — coherent, continuous activities that
may span multiple time blocks and may run concurrently.

For each thread, output:
- id: sequential (thread_001, thread_002, ...)
- label: human-readable name
- start/end: timestamps bounding the thread
- thread_type: free-form classification
- sources: which data sources contribute
- observation_indices: which observations belong to this thread
  (reference by block index + observation index within block)
- relevance: 0.0-1.0
- relevance_signals: list of reasons for the score
- metadata: structured context (participants, meeting title, etc.)

THREADING RULES:
1. A time block can participate in MULTIPLE concurrent threads.
2. Each observation belongs to EXACTLY ONE thread. Disambiguate.
3. Trace threads across block boundaries — a meeting that spans
   10:00-10:30 is ONE thread across 6 blocks, not 6 separate threads.
4. Split threads when the context genuinely changes (new meeting,
   different conversation, switched activities).

RELEVANCE SCORING:
High (0.7-1.0):
  - Multi-source convergence (audio + screen + calendar corroborate)
  - Decision language ("let's do X", "I've decided", "we should")
  - Commitment language ("I'll have it by Friday")
  - References to the person's actual projects, people, goals

Medium (0.3-0.7):
  - Single-source conversation with work content
  - Solo work session (screen activity, sparse self-talk)
  - Thinking aloud about real topics

Low (0.0-0.3):
  - Media playback (TV, movies, podcasts, music)
  - Other people's conversations (not involving the user)
  - Routine transactions ("large coffee please")
  - Transit with no meaningful conversation

Output ONLY a JSON array of threads. No markdown, no explanation.
```

**Cost:** ~$0.50-1.00 per day at Sonnet prices for a typical day's blocks.

## Relevance Scoring Detail

Relevance is scored by the LLM during threading but guided by explicit criteria in the prompt. The key signals:

| Signal | Relevance effect | Source needed |
|---|---|---|
| Calendar event matches audio | Strong positive | calendar + audio |
| Meeting app on screen + conversation audio | Strong positive | screen + audio |
| Multiple speakers taking turns | Moderate positive | audio |
| References to known entities (people, projects) | Moderate positive | audio + decision graph |
| Decision/commitment language | Strong positive | audio |
| Scripted dialogue style | Strong negative | audio |
| Single source, no corroboration | Moderate negative | any single source |
| Laugh track / music bed | Strong negative | audio |
| Location = home + time = evening + no calendar | Moderate negative | location + calendar |

**Single-source degradation:** When only audio is available (no screen, no calendar), relevance scoring is less confident. The LLM still analyzes content ("is this scripted dialogue or natural conversation?") but confidence is lower. A single-source conversation thread might score 0.5 instead of 0.8, meaning the pipeline treats it as medium-relevance and extracts with lower confidence.

**Relevance is not permanent.** A thread scored 0.3 today could be re-scored later when:
- The evening check-in reveals "that podcast influenced my thinking"
- New sources come online and retroactive cross-referencing is possible
- The decision graph grows and a previously-unrecognized entity becomes known

## Pipeline Integration

### New pipeline stage

The pipeline gains an "assemble" stage before extraction:

```
BEFORE:
  observations → extract decisions → link → brief

AFTER:
  observations → time blocks → LLM threading → context threads
       ↓                                            ↓
  save ALL as                              filter by relevance ≥ 0.5
  episodic evidence                               ↓
  (transcript.jsonl)              relevant threads → extract decisions → link → brief
                                                    ↓
                                            all threads saved
                                            (threads.json)
```

### CLI changes

The `--source` flag becomes optional. When omitted, `alvum extract` reads ALL available data in the capture directory and does cross-source threading:

```bash
# Cross-source threading (recommended)
alvum extract --capture-dir ./capture/2026-04-11 --output ./output

# Single-source extraction (backward compatible, no threading)
alvum extract --source claude --session <file> --output ./output
alvum extract --source audio --capture-dir <dir> --whisper-model <model> --output ./output
```

When `--source` is specified, threading is skipped (single-source mode). When omitted, all sources in the capture dir are gathered, processed, and threaded together.

### Output artifacts

```
output/
├── transcript.jsonl        ← ALL observations (episodic evidence, always saved)
├── time_blocks.json        ← Pass 1 output
├── threads.json            ← Pass 2 output (the episodic memory)
├── decisions.jsonl         ← from high-relevance threads only
├── briefing.md             ← morning briefing
└── extraction.json         ← full result
```

`threads.json` is the episodic memory. Each thread is an episode — labeled, classified, scored, linked to its observations. This is the artifact that future analysis (re-scoring, pattern detection, longitudinal trends) operates on.

## Bootstrapping: Audio-Only

When only audio is available (no screen capture yet):

- Pass 1 works identically (bucket audio observations into time blocks)
- Pass 2 gets audio-only blocks, so threading relies on:
  - Content analysis (is this scripted? natural? self-talk?)
  - Speaker patterns (multiple speakers = conversation)
  - Acoustic consistency (same environment = same thread)
  - Temporal gaps (silence > 5 min = thread boundary)
- Relevance scores are lower confidence (no cross-source corroboration)
- The system is transparent about this: `relevance_signals: ["single-source, audio-only — lower confidence"]`

When screen capture comes online later, existing audio observations can be **retroactively re-threaded** with cross-source context. The raw observations in `transcript.jsonl` are the source of truth; `threads.json` is always re-generable.

## Implementation Scope

### New crate: `alvum-episode`

```
crates/alvum-episode/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── time_block.rs      ← Pass 1: temporal quantization
    ├── threading.rs        ← Pass 2: LLM-driven thread detection
    └── types.rs            ← TimeBlock, ContextThread, ThreadingResult
```

Dependencies: `alvum-core` (types), `alvum-pipeline` (LlmProvider for Pass 2).

### CLI changes

- `alvum extract` without `--source` triggers cross-source threading
- New `--relevance-threshold` flag (default 0.5) controls which threads go to extraction
- Existing `--source` mode works as before (no threading)
