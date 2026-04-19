# Agency Layer: Proactive Next-Actions, Search, Absence Detection, and Life-Phase Awareness

Four product surfaces that turn alvum from a reflective diary into an active partner. They share architectural substrate (the decision graph, knowledge corpus, and capture stream are already in place), dependency ordering (all require Phases A–C to be shipped), and the same privacy posture (local-first, no cloud).

- **Now** — the inverse prompt. "What should I do right now?"
- **Find** — retrospective semantic search. "When did I last talk to mom?"
- **Quiet zones** — negative-space detection. "You haven't mentioned Theo in 3 weeks."
- **Life phase** — crisis/reset awareness. The engine reads the room.

This spec covers all four together because isolating them would duplicate half the architecture. Each has its own UI surface, data-model additions, and pipeline touchpoints.

## Why Now

Phases A–C give alvum the data, the alignment engine, and the product surface (Electron app + `/briefing`, `/intentions`, `/timeline`, `/decisions`, `/devices`). That surface is **observational** — the user consults alvum to see what happened. The Agency Layer is what makes the product feel alive instead of archival. It's also what drives daily-active use: without these surfaces, alvum is a 5-minute-a-day app (morning briefing + evening check-in). With them, every meeting prep, every open block, every question about the past becomes an alvum moment.

Each feature is independently shippable. Build order (below) prioritizes by effort-to-impact, not by theme coherence.

## Shared Architecture

All four features read from the same substrate — no new capture, no new processor. They differ in when they run:

```
                      ┌─────────────────────────────────────────┐
                      │ Existing substrate (Phases A–C)         │
                      │  - decisions/index.jsonl                │
                      │  - knowledge/{entities,patterns,facts}  │
                      │  - intentions.json                       │
                      │  - days/YYYY-MM-DD.json                 │
                      │  - capture/YYYY-MM-DD/*                 │
                      │  - health/ location/ …                  │
                      └──────────────┬──────────────────────────┘
                                     │
       ┌─────────────────────────────┼─────────────────────────────┐
       │                             │                             │
  ┌────▼────┐              ┌─────────▼─────────┐          ┌───────▼───────┐
  │ On-demand│              │ Nightly (in the  │          │ Triggered     │
  │ query    │              │ existing overnight│          │ (manual or by │
  │          │              │  pipeline)        │          │  signal)      │
  │ Now      │              │ Quiet-zones       │          │ Life-phase    │
  │ Find     │              │ detector          │          │ state changes │
  └──────────┘              └───────────────────┘          └───────────────┘
```

**Now** and **Find** are interactive — the user queries, the system responds in < 2 seconds. Both run on-demand against local indexes/graphs; neither needs the overnight pipeline to have run for today's data.

**Quiet zones** is a derived output of the nightly pipeline — one new stage that runs after Link, before Brief.

**Life phase** is state — a single small file (`life_phase.json`) that multiple pipeline stages read to decide their tone. Phase changes can be triggered by the user manually or proposed by a detector that runs daily.

All four are **local-only**. If the user is on `Privacy: Full` (§ Model Architecture — Privacy Modes), nothing about these features leaves The Box. If they're on `Hybrid` or `Best Quality`, the Find and Now features may make Claude API calls for LLM synthesis; Quiet-zones and Life-phase detection are always local.

## Feature 1: Now — the Inverse Prompt

### Problem

Alvum is passive today. It observes all day, reflects in the morning, prompts in the evening. The **moment of choice** — when the user is actually deciding what to do next — has no alvum surface. That moment is where intention drifts into habit or gets reinforced; it's where alvum is most valuable *and* currently absent.

### Core idea

A `/now` route and menubar widget that, on query, produces a ranked list of 1–3 next-action proposals. Each proposal is grounded in the user's current situation:

> **Run now.** You haven't run in 4 days; half-marathon training trend is declining. Weather is 62°F, no meetings until 14:00 — you have a 90-minute block. Prospect Park loop (5.2 mi) is 8 min from you.
>
> **Send Priya the design doc.** Promised Friday; she's out Monday. You drafted it at 4:12pm yesterday and closed without sending. 20-minute task.
>
> **Cook at home.** You planned dinner with Elise; yesterday you ordered takeout, broke the streak. Grocery list has 4 items.

Each proposal has one-tap actions: [Do it] / [Schedule it] / [Not now — tell me why].

### Inputs

The proposal engine reads (all already captured or derivable):

| Input | Source |
|---|---|
| Active intentions + their current status | `intentions.json` + last `AlignmentReport` |
| Open commitments with deadlines | `DayExtraction.commitments` scanned across recent days |
| Calendar state (now + next event) | Existing calendar connector (or iOS HealthKit + EKEvent) |
| Current location | Most recent `LocationObservation`, via device-fleet heartbeats |
| Current physiological state | Today's `HealthObservation` — resting HR, HRV, sleep from last night |
| Time-of-day pattern for this user | Patterns in `knowledge/patterns.jsonl` |
| Life phase | `life_phase.json` |

### Data model

Transient — proposals are regenerated on demand, not persisted long-term. User responses (especially rejections with reason) *are* logged as Decisions, because they're decisions.

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextActionSuggestion {
    /// Stable for the lifetime of the proposal batch (e.g., sha256 of content).
    pub id: String,
    /// Generated at.
    pub ts: DateTime<Utc>,
    /// Short imperative title ("Run now").
    pub title: String,
    /// Two-sentence explanation grounded in cited state.
    pub reasoning: String,
    /// Intentions this proposal advances (by id).
    #[serde(default)]
    pub intention_refs: Vec<String>,
    /// Commitments this proposal fulfills (by id).
    #[serde(default)]
    pub commitment_refs: Vec<String>,
    /// Estimated duration.
    pub duration_min: u32,
    /// Suggested window (if the proposal is time-sensitive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
    /// Per-source citations so the user can verify claims.
    #[serde(default)]
    pub citations: Vec<Citation>,
    /// LLM-estimated relevance. Used for ordering and filtering.
    pub confidence: f32,  // [0.0, 1.0]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    /// What's being cited ("intention", "commitment", "decision", "pattern", "health").
    pub kind: String,
    pub ref_id: String,
    /// Optional inline snippet for UI display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}
```

### Architecture

New pipeline stage: `alvum-pipeline::propose`. NOT part of the nightly batch — it's called on demand by the web server. A scheduled version also runs every 30 min during active hours to refresh the menubar widget's cached top-1.

```
Request /now
    │
    ▼
Gather state (≤ 100ms):
  - active intentions w/ status
  - open commitments w/ deadlines
  - calendar next event
  - last location
  - last health snapshot
  - time-of-day pattern matches
  - life_phase
    │
    ▼
Route to LLM (routine-tier model by default — Haiku / local):
  - Prompt: "Given this state, propose 1–3 concrete next actions
    the user could take right now. Each must cite the state that
    motivated it. Rank by alignment leverage."
    │
    ▼
Parse → NextActionSuggestion list
    │
    ▼
Cache for ≤ 5 minutes (short-lived — state changes fast)
    │
    ▼
Return
```

**Cost control.** Cached 5 min. Menubar auto-refresh only runs during "active hours" (user-configurable, default 07:00–21:00). In `Full privacy` mode, routes to local model (Llama/Qwen) with a simpler prompt.

**User response → Decision.** When user taps [Do it], [Schedule it], [Not now], or [Skip with reason], the response becomes a Decision logged to `decisions/index.jsonl`:

- `proposed_by: Actor::Agent("alvum")`
- `resolved_by: Actor::Self_` with confidence reflecting the tap
- `status: ActedOn | Accepted | Rejected | Ignored`
- `source: DecisionSource::Explained` (since user explicitly chose)
- `connector: "now"`

This closes the loop: the proposal engine's own suggestions enter the decision graph and become future-alignment input. Over time, the briefing can say: "alvum proposed running 12 times this month; you did it 4 times."

### UI

- **`/now` route** (desktop web UI) — the primary surface. Top 3 proposals as cards. Each card: title, reasoning, citations (tap to drill into source), action buttons.
- **Menubar widget** (Electron shell) — top 1 proposal, ~80 chars wide. Clicking the widget opens `/now`.
- **iOS companion shortcut** — "Ask alvum what's next" — fetches from The Box over the local network, shows one proposal. Works offline if the phone was recently in contact with The Box (last cached response; ≥ 10 min old is marked stale).
- **Empty state** — when no proposal clears the confidence threshold: *"Nothing pressing. Use the time however you want."* (Deliberate — not a productivity tool.)

### Privacy

- Prompts never leave The Box in `Full privacy` mode.
- Citations include references but never raw audio/screen content — just typed evidence.
- User can disable `/now` entirely if they find it too pushy.

### Phase

**Phase G** (post-V1.5). Requires Phase C (app shell, web UI), B (alignment engine for intention scoring), and the health + location + device-fleet work.

---

## Feature 3: Find — Retrospective Semantic Search

### Problem

The top-level spec parks text embeddings at V2 with the trigger "corpus + decisions exceed LLM context (~6 months)." That's the wrong trigger. Search is a **day-one headline feature**, not a scaling afterthought. Questions like *"when did I last talk to mom?"*, *"what did we decide about the migration three weeks ago?"*, *"where was I when I had the encryption idea?"* — every alvum user will try to ask these within their first week. If the answer is "sorry, search ships in V2," the product feels incomplete.

### Core idea

A global omnibox (Cmd-K) and a `/search` route that answer natural-language queries over the full decision graph, knowledge corpus, capture stream, and day extractions. Answers are **synthesized narratives with citations**, not raw search-result lists. Raw matches are available under the fold.

Example: *"when did I last talk to mom?"* →

> You last spoke with her on April 6 at 18:22 — a 12-minute call about the lease renewal on the Vermont place. You called her; she was cooking. Open thread from that call: she asked you to forward the inspection report.
>
> [Play clip] [Open timeline] [All conversations with mom]

### Inputs

| Input | Source |
|---|---|
| Decision summaries | `decisions/index.jsonl` |
| Observations (transcripts, dialogue, screen events) | `capture/<date>/*/content or artifact layers` |
| Entities | `knowledge/entities.jsonl` |
| Facts | `knowledge/facts.jsonl` |
| Patterns | `knowledge/patterns.jsonl` |
| Day extractions | `days/<date>.json` |
| Briefings | `briefings/<date>.md` |
| Location observations | `capture/<date>/location/fused.jsonl` |

### Data model

No new persistent types. Search is a query function.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub matched_kind: SearchMatchKind,
    pub ref_id: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub snippet: String,
    pub relevance: f32,
    #[serde(default)]
    pub context_links: Vec<ContextLink>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchKind {
    Decision,
    Commitment,
    Observation,
    Entity,
    Fact,
    Pattern,
    ActivityBlock,
    BriefingItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextLink {
    pub kind: SearchMatchKind,
    pub ref_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchAnswer {
    pub query: String,
    pub narrative: String,          // LLM-synthesized answer
    pub citations: Vec<Citation>,   // reuses the Now-surface Citation type
    pub results: Vec<SearchResult>, // raw matches, ranked
}
```

### Architecture

Two-layer retrieval:

**Layer 1 — Structured index (deterministic, fast)** runs first:
- Entity-name matching ("mom" → Entity match → every Observation/Decision that references its id)
- Date-phrase parsing ("three weeks ago" → `[2026-03-28, 2026-04-03]` range filter)
- Explicit-type filters ("commitments to Priya" → filter `kind = Commitment AND to = "Priya"`)
- Location filters ("when I was at the park" → filter by PlaceEntity id)

**Layer 2 — Embedding search (fallback / fuzzy)** — only when Layer 1 returns low-confidence or the query is purely topical (*"conversations about encryption"*).

**Embedding index** built incrementally by the nightly pipeline:
- Embed: Decision.summary, Observation.content, Fact.content, ActivityBlock.summary.
- Store: `embeddings/*.usearch` (one file per matched-kind for cache locality).
- Model: text-only provider per `Privacy` mode (local `nomic-embed-text` on Full, Gemini Embedding on Managed).

**LLM synthesis**:
- Top K (K=8) matches from both layers are passed to the LLM with the query.
- Prompt: *"Answer the user's question directly in 2–3 sentences. Cite every claim with a reference id."*
- LLM output: `{narrative: str, citation_ids: [...]}`.

**Latency budget**: < 2 s end-to-end. Embedding search via `usearch` is ~10 ms for 100k vectors. LLM synthesis is the dominant cost — keep input under 4k tokens.

### UI

- **Cmd-K omnibox** (global in the Electron shell) — a large input with real-time suggestions from Layer 1 as you type. Press Enter for full synthesis.
- **`/search?q=...` route** — permalinkable. Answer + citations at top; raw result list below.
- **Result cards** — each result card has the matched content, its kind badge (Decision/Commitment/etc), timestamp, and three affordances: [Open] (route to the source view), [Play clip] (if audio exists), [Copy link].
- **History** — recent searches are stored in `capture/<date>/searches.jsonl` with a 30-day retention by default; user can disable logging per-search or globally.

### Privacy

- Query never leaves The Box in Full privacy mode.
- Search history is local and gitignored/excluded from managed-tier cloud sync.
- Results that reference Hidden PlaceEntities show only the label; raw coords are never in search output.
- A "don't log this search" toggle in the omnibox.

### Phase

**Phase G**. Structured-index layer can ship first (no embedding work needed); embedding layer is a follow-up. Both land well before the V2 trigger. Recommendation: **move text embeddings from V2 to V1.5 explicitly** — update the growth-path table in the top-level spec.

---

## Feature 4: Quiet Zones — Negative-Space Detection

### Problem

The decision graph and knowledge corpus describe what the user *did* think and do. What the user *didn't* think or do is equally load-bearing: the friend not called, the project abandoned, the domain neglected. No feature surfaces this. The failure mode this targets — "out of sight, out of mind" — is the root cause of most neglected relationships, stalled projects, and slow-motion burnouts.

### Core idea

A new nightly pipeline stage that detects meaningful *absences* — entities, intentions, domains, or communication patterns with established baseline frequency that have been quiet long enough to warrant attention. Absences surface as a dedicated briefing section (*"Quiet zones"*) and optional notifications.

Example briefing entries:

> **Theo** — you haven't mentioned him in 3 weeks. Typical baseline: ~daily at work. Last decision involving him: April 1 (migration risk review).
>
> **Finances** — no finances-domain decisions in 21 days. You usually log ~3 per month. [Is something paused, or did we miss it?]
>
> **Half-marathon training** — no workouts logged in 6 days. Goal is still active (target Oct 12).

### Inputs

All from existing derived data:

| Signal class | Detection logic | Input source |
|---|---|---|
| Entity silence | Entity with mention count > threshold in trailing 28 days, zero mentions in last N days (N = cadence-based) | `knowledge/entities.jsonl` + daily Observation stream |
| Intention disengagement | Active intention with zero linked evidence in trailing period appropriate to its cadence | `intentions.json` + `AlignmentReport` |
| Domain absence | Domain with zero Decisions in trailing period > 1.5 × median interval | `decisions/index.jsonl` grouped by domain |
| Communication pattern break | Detected pattern (daily communication with person X) that has been quiet for > 2 × typical gap | `knowledge/patterns.jsonl` |

### Data model

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NegativeSignal {
    pub id: String,
    pub detected_on: NaiveDate,
    pub kind: NegativeSignalKind,
    /// One-sentence description for UI.
    pub description: String,
    /// How long the gap has lasted.
    pub gap_days: u32,
    /// Expected frequency / cadence as a reference point.
    pub baseline_description: String,
    /// 0.0 = barely noteworthy, 1.0 = urgent.
    /// Severity = f(gap/baseline ratio, entity importance, intention activity).
    pub severity: f32,
    /// References to the underlying entity/intention/domain/pattern.
    pub references: Vec<Citation>,
    /// User acknowledgement state.
    #[serde(default)]
    pub acknowledgement: Option<Acknowledgement>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NegativeSignalKind {
    EntitySilence,
    IntentionDisengagement,
    DomainAbsence,
    CommunicationBreak,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Acknowledgement {
    pub action: AckAction,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AckAction {
    /// User confirmed the gap is intentional ("yes, I'm stepping back from X").
    Intentional,
    /// User wants to address the gap ("I'll reach out today").
    WillAct,
    /// "Actually I did engage — capture missed it." (Feedback signal.)
    FalsePositive,
    /// Don't show again for this subject.
    Silenced,
}
```

### Storage

```
~/.alvum/generated/
└── quiet_zones/
    └── YYYY-MM-DD/
        └── signals.jsonl         ← derived; regenerable by re-running the detector
```

Under `generated/` because this is LLM-derived output (the detector runs in the nightly Link stage). Regenerable from current `generated/` + `capture/` data, but the acknowledgements the user writes on top (see `Acknowledgement` struct) are user-stated — those must survive re-generation. The on-disk format keeps the acknowledgement inline with the signal so they're never split.

Acknowledgements are merged forward: a signal silenced yesterday stays silenced for the same subject tomorrow until the subject re-activates.

### Detection Algorithm (nightly, after Link stage)

```
For each Entity E in knowledge/entities.jsonl:
  baseline = rolling 28-day mention frequency
  if baseline > 3/week AND last_mention > (7 days):
    emit EntitySilence signal, severity = min(1.0, gap / (baseline_interval * 2))

For each active Intention I:
  expected_interval = cadence_interval(I)
  if last_evidence_for(I) > expected_interval * 1.5:
    emit IntentionDisengagement signal

For each Domain D in user's domain list:
  recent_decisions = decisions in D over last 28 days
  typical_interval = median gap between decisions in D
  current_gap = today - last_decision_in_D
  if current_gap > typical_interval * 1.5 AND recent_decisions.count >= 3:
    emit DomainAbsence signal

For each Pattern P classified as "regular communication":
  if gap_since_last_observation > pattern.typical_interval * 2:
    emit CommunicationBreak signal

Merge forward acknowledgements from trailing 7 days.
Suppress any signal with matching subject + Silenced ack.
Suppress any signal with matching subject + Intentional ack if < 14 days old.
Emit remaining signals to today's negative_signals.jsonl.
```

False-positive rate matters. Tune severity threshold in config; below threshold, signals exist in the file but don't appear in the briefing.

### UI

- **Briefing section: Quiet zones** — 0 to 3 top signals by severity. Each is a card with: description, baseline, gap, [Acknowledge] dropdown with the four ack actions, [Reach out] (drafts a message for the user to send if the signal is a person), [See history] (links to the entity/intention page).
- **Weekly review amplification** — Sunday digest includes a "what you didn't do this week" section drawn from the week's signals.
- **Entity / intention pages** — historical signals render as a sparse timeline showing silent periods.
- **Life-phase override** — during non-Steady phase (see Feature 5), quiet-zone severity is downgraded by half. Intentional breaks from routine aren't surfaced as alerts.

### Privacy

All local. `FalsePositive` acknowledgements are valuable capture-quality signal and are written to a local `capture_quality.jsonl` used for internal debugging — never exported or cloud-synced.

### Phase

**Phase G**. Depends on stable Knowledge Corpus (Phase A+) and mature pattern extraction (Phase C). Useful even with modest corpora: after ~3 weeks of use, entity mention baselines are meaningful.

---

## Feature 5: Life Phase — The Engine Reads the Room

### Problem

An alignment engine that cheerfully announces *"Gym streak broken"* during a family emergency is tone-deaf in a way that destroys trust. Real lives have weeks where everything goes sideways — a death, a breakup, a burnout, a move, a newborn — and alvum's default productivity voice is wrong for those weeks. Users churn during crises; the product needs to earn staying power by being the right tool at the right temperature.

### Core idea

A single stateful flag (`LifePhase`) that multiple pipeline stages and UI surfaces read to decide their tone. Phases:

- **Steady** — default. Normal alignment tracking, normal briefings.
- **In the thick of it** — active work crunch (user-declared or detected from sustained elevated-HR + long hours patterns). Alignment tracking continues but framed as *"here's what's still working"* instead of *"here's what's slipping."*
- **Big feelings** — life disruption (user-declared or detected from sleep-crash + HR-elevated + new dominant topic clusters + location anomalies). Alignment scoring paused; briefings shift to gentle prompts.
- **Recovering** — bridge state after a Big-feelings phase, gradually re-introducing alignment scoring over two weeks.

### Triggers

**Automatic (proposed, not imposed):**
- `Big feelings` candidate fires when any three of:
  - Sleep efficiency < 0.65 for 3+ consecutive nights
  - Resting HR > baseline + 8 bpm for 3+ days
  - New dominant topic cluster in audio (>30% of conversation time on a new entity in last 48h)
  - Location pattern deviation (> 2 σ outside normal weekly cluster)
  - User's own language patterns shift (pipeline detects emotional-distress markers in self-speech transcripts)
- `In the thick of it` candidate fires when:
  - > 10 hours/day at work locations for 4+ consecutive days
  - Sleep < 6 h for 4+ consecutive days with elevated HR
  - Workouts dropped > 50% vs. trailing month

**Manual** from `/settings → Life phase`:
- Four presets one tap each.
- Optional note (private — never shown elsewhere in the UI but carried in the phase record).
- Optional duration override (default 14 days; user can set 1–60 days).

Detected candidates never auto-activate. The briefing the next morning surfaces:

> Alvum noticed: your sleep dropped sharply this week and there's a lot of new conversation about [topic redacted at UI level — never shown out of context]. Would you like to shift to gentle mode? [Yes, two weeks] [Yes, one week] [I'm okay]

### Data model

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LifePhase {
    pub kind: LifePhaseKind,
    pub started: DateTime<Utc>,
    pub expires: Option<DateTime<Utc>>,
    pub triggers: Vec<PhaseTrigger>,
    /// User-private note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Whether the user set this manually or confirmed an automatic proposal.
    pub source: PhaseSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifePhaseKind {
    Steady,
    InTheThickOfIt,
    BigFeelings,
    Recovering,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhaseSource {
    UserManual,
    DetectorConfirmed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhaseTrigger {
    pub kind: PhaseTriggerKind,
    pub at: DateTime<Utc>,
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PhaseTriggerKind {
    SleepCrash,
    HrElevated,
    HrvDrop,
    TopicShift,
    LocationAnomaly,
    ExtendedWorkHours,
    WorkoutDrop,
    UserManual,
}
```

### Storage

```
~/.alvum/generated/
├── life_phase.json             ← current phase (single LifePhase)
└── life_phase_history.jsonl    ← append-only log of transitions
```

Under `generated/` — the current phase is user-stated state (set manually or detector-proposed + user-confirmed), so it belongs with `intentions.json` and confirmed knowledge-corpus entries. Never under `runtime/`: if someone restores from a backup, their recent declaration of "I'm in a crisis phase" must come back with them.

Transitioning from one phase to another writes the previous LifePhase to history with its `expires` populated, then writes the new one to `life_phase.json`. History is used only for the "review what happened" affordance after a phase ends.

### Effects across the product

Every stage and UI surface reads `life_phase.json`:

| Surface | Steady | In the thick of it | Big feelings | Recovering |
|---|---|---|---|---|
| Briefing tone | Normal — drift/alignment framing | "Here's what's still working." No penalties language. | Gentle prompts only. No alignment scoring shown. Light observations. | Soft re-introduction of alignment; streaks don't count gaps. |
| Intention scoring | Normal | Normal but de-emphasized in UI | Paused; intentions remain but not scored | Scoring shown but prior gap not penalized |
| Check-in questions | Behavioral + intention probes | Behavioral probes only | "What would help right now?" open-ended | Gentle behavioral probes; no intention probes for first 7 days |
| Quiet-zones (Feature 4) | Normal | Downgraded severity × 0.5 | Suppressed | Downgraded severity × 0.7 |
| Now (Feature 1) | All proposals | Rest-focused proposals gain priority | Restorative-only proposals | Restorative proposals emphasized |
| Notifications | Normal | Reduced (only deadlines) | Muted entirely | Minimal |

The phase flag is a single read in each prompt-building routine; adding phase-awareness is a cheap change per surface but produces a product that feels psychologically literate instead of mechanical.

### UI

- **Nav indicator** — a small colored dot next to the app name: green (Steady), amber (In the thick of it), blue (Big feelings), teal (Recovering). Hover shows the phase name + remaining duration.
- **`/settings → Life phase`** — current phase + toggle presets + optional duration + optional note.
- **Phase-end surface** — when a non-Steady phase expires, the next briefing includes a soft question: *"Gentle mode just ended. When you're ready, here's the shape of that two weeks."* Tap opens a read-only review of what happened during the phase (decisions, evidence, conflicts) without alignment scoring.

### Privacy

- `life_phase.json` is never cloud-synced, even on Managed tier.
- Detector-captured audio patterns that inform a `BigFeelings` proposal are flagged with a `phase_trigger: true` tag in the day file so the user can remove them if desired (e.g., a phone call about a death they want to wipe).
- Phase notes are never quoted in any LLM prompt.

### Detection Algorithm (nightly)

```
Read last 7 days of health, location, decisions, observations.

If current_phase.kind != Steady AND current_phase.expires < now:
  Write current_phase to history with observed end timestamp.
  Set current_phase to { Recovering, expires: now + 14d } if kind was BigFeelings,
  else Steady.

Score BigFeelings candidate from trigger combinations above.
Score InTheThickOfIt candidate similarly.

If current_phase.kind == Steady AND (BigFeelings score > threshold OR ITOI score > threshold):
  Write proposal to tomorrow's briefing; do NOT auto-activate.

If current_phase.kind != Steady AND all trigger conditions have relaxed:
  Emit "phase might be ending" hint into briefing; user decides.
```

Detector runs in the existing Link stage; no new pipeline topology.

### Phase

**Phase G**, with early partial landing in Phase C (the LifePhase type and the UI toggle land with the app shell; detector arrives with the health connector in Phase E / G).

---

## Cross-Feature Concerns

### Stable IDs (prerequisite)

All four features create or cite references that must survive overnight re-runs:
- Now proposals cite Decisions and Intentions
- Find surfaces Decisions, Commitments, Observations by id
- Quiet-zones cites Entities, Intentions, Patterns, Decisions
- Life-phase history references Decisions and Observations

The top-level spec doesn't guarantee Decision-id stability across pipeline re-runs. Without this guarantee, every feature here becomes fragile. **Spec addition required**: decisions get content-hash-derived IDs (stable as long as the extracted substance doesn't change), or a reconciler that maps old → new IDs on re-run. Resolve before shipping Feature 1.

### Life-phase is a filter, not a mode

Every Agency Layer feature must read `life_phase.json` at query time, not at storage time. This matters:
- A decision captured during `BigFeelings` stays a decision; nothing about its storage changes.
- What changes is *how it's surfaced*. Penalties-language is a render-time concern, not a data-model concern.

Otherwise switching back to Steady doesn't re-expose the data correctly.

### Shared `/api/*` endpoints

All four features export JSON under `/api/*`. Proposed routes:
- `GET /api/now` → `Vec<NextActionSuggestion>`
- `POST /api/now/:id/respond` → logs a Decision, invalidates cache
- `POST /api/search` → `SearchAnswer`
- `GET /api/negative-signals?date=…` → `Vec<NegativeSignal>`
- `POST /api/negative-signals/:id/ack` → records Acknowledgement
- `GET /api/life-phase` → `LifePhase`
- `PUT /api/life-phase` → transitions phase
- `GET /api/life-phase/history` → history entries

The iOS companion uses these identically — there's no mobile-specific agency protocol.

### Backed by local LLM when possible

All four features call LLMs. In Full privacy mode, they must route to local (Ollama). Browser cost constraint: local models are 5–20× slower than Haiku. Shipping local-only mode acceptably requires either:
- Smaller prompts (preferable — a Now proposal prompt should fit in 2k tokens)
- Caching (already planned for Now; extend to Quiet-zones and Life-phase detector)
- Async execution for non-interactive flows (Quiet-zones detector runs overnight already; Find's embedding layer is fast without an LLM; Now is the one that must be fast interactively)

## Spec Updates Needed

1. **Top-level spec § Web UI routes** — add `/now`, `/search`, `/life-phase` (or fold the last into `/settings`). Quiet-zones lives under `/briefing/:date` and `/weekly-review`.
2. **Top-level spec § Growth Path** — move text embeddings from V2 to V1.5 (Find depends on them; V2 trigger was miscalibrated).
3. **Top-level spec § Pipeline Stages** — add `Propose` stage (interactive) and a note that the Link stage now includes negative-signal detection.
4. **Progress matrix** — add four rows, one per Agency Layer feature, marked Phase G.
5. **`2026-04-18-alignment-primitives.md`** — no change; these features' types land in their own plans, not Phase A.
6. **Stable decision IDs** — new sub-spec or a short addition to the top-level spec § Data Model. Prerequisite for all four agency features.

## Implementation Plan Outline

Build order, prioritizing user-facing return on effort:

| Order | Feature | Effort | Unlock |
|---|---|---|---|
| 1 | **Find** (structured layer first) | ~2 weeks | Day-one utility. Works without embeddings. |
| 2 | **Life phase** (manual only, no detector) | ~1 week | Product empathy. Simple toggle + render-time filters. |
| 3 | **Find** (embedding layer) | ~2 weeks | Fuzzy / topical queries. Requires embedding infra. |
| 4 | **Quiet zones** (nightly detector + briefing section) | ~2 weeks | Unique-to-alvum value surface. |
| 5 | **Now** (manual query surface) | ~3 weeks | Highest-value feature but largest design + prompt work. |
| 6 | **Life phase detector** | ~1 week | Nice-to-have; auto-proposes a phase shift. |
| 7 | **Now** (menubar widget + iOS shortcut) | ~2 weeks | Converts Now from occasional-use to ambient. |

Total ~13 weeks for the full Agency Layer across Phase G. Phases 1–3 (Find + Life phase basic) is a 4–5 week ship that delivers a qualitatively different product.

Each ordered item gets its own dedicated implementation plan before execution — pattern established by Phase A (`alignment-primitives.md`) and the future Phase B/C/D/E plans.

## Open Questions

- **Proactive push vs. pull for Now.** Is the menubar widget enough, or does Now want to *push* notifications during open blocks ("you have 90 min; here's a high-leverage option")? Push is clearly higher-impact but higher-annoyance — probably opt-in per-block-type.
- **Find history sensitivity.** Should searches themselves be semi-permanent records? They reveal what the user is worried about (searching for "mom" a lot after a diagnosis). Default 30-day retention; explicit never-log toggle. Verify no accidental cloud sync.
- **Quiet-zones false positives during travel.** Everyone's routine breaks on vacation. Detector should downgrade severity during location-anomaly periods (i.e., when far from home). Add to the severity formula.
- **Life-phase detector training data.** Thresholds are guessed. Tune with user feedback: when users manually confirm / reject an auto-proposed phase, that's label-perfect training signal. Build the loop early.
- **Crisis features ≠ therapy features.** Being careful here. Alvum is not a mental-health tool; Life phase is a tone-aware alignment engine. If a user is in sustained distress, alvum's behavior is "be gentle" and *"maybe this is a time to talk to someone."* Explicit: the product does not diagnose, treat, or refer. Make sure the UI copy is aligned with this posture; involve a reviewer with mental-health product experience before shipping.

## Relationship to Other Specs

- **Top-level spec:** extends the product vision to include proactive agency; superseded in § Web UI routes; growth-path adjusted.
- **`2026-04-18-device-fleet.md`:** Now and Find can both be queried from any peer device (iPhone companion shortcut, Watch glance). The device-fleet auth / routing apply.
- **`2026-04-18-location-map.md`:** Now reads location; Find queries against location; Quiet-zones downgrades during location anomalies.
- **`2026-04-18-health-connector.md`:** Life-phase detector primarily reads health data; Now consumes physiological state; Find can query across health sessions.
- **`2026-04-18-alignment-primitives.md` (Phase A plan):** no change — Agency Layer types land in their own per-feature plans, after Phase A is shipped.
