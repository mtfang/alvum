# Actor Attribution: Entity Tracking Across Sources

Three-layer attribution system that identifies WHO performed an action — self, person, agent, organization, or environment — by combining capture-time hints, processor enrichment, and threading-time cross-source resolution. The knowledge corpus serves as the identity backbone, and user audit corrections feed back for continuous improvement.

## Problem

Without attribution, the pipeline can't distinguish "I decided to defer the migration" from "Sarah told me to defer the migration." The `proposed_by` and `resolved_by` fields on Decision exist, but nothing reliably fills them. In a Zoom call with multiple speakers, system audio is one undifferentiated stream. The pipeline needs to attribute every observation and decision to the correct actor.

## Actor Types

Five actor kinds, already defined in `alvum-core::decision::ActorKind`:

| Actor Kind | What it is | Examples |
|---|---|---|
| **Self** | The user | Speaking into mic, typing, clicking, making choices |
| **Person** | Other humans | Meeting participants, people in conversation, collaborators |
| **Agent** | AI/automation tools | Claude, Copilot, GitHub Actions, CI systems, bots |
| **Organization** | Company/team collective | Policy decisions, org announcements, team norms |
| **Environment** | External events, systems | Notifications, cron jobs, weather, system alerts, reminders |

## Three-Layer Attribution

Attribution accumulates confidence through three pipeline layers. Each layer adds signals; later layers can override earlier ones.

### Layer 1: Capture Hints (confidence 0.2-0.4)

Raw source metadata. No intelligence, no model calls. Capture crates tag observations with the channel they came from.

| Source | Hint | Confidence |
|---|---|---|
| `audio-mic` | `self` (likely user, could be someone nearby) | 0.3 |
| `audio-system` | `unknown_person` (someone on a remote call) | 0.2 |
| `screen` | `self` (user is viewing, didn't necessarily cause change) | 0.4 |

Hints are stored in `Observation.metadata` under an `actor_hints` array:
```json
{"actor_hints": [{"actor": "self", "kind": "self", "confidence": 0.3, "signal": "mic_channel"}]}
```

### Layer 2: Processor Enrichment (confidence 0.4-0.7)

Vision model and transcription add identity context. The processor reads the screenshot or audio segment and infers who is acting.

**Screen processor signals:**
- Active speaker indicator in video calls: "Zoom showing Sarah Chen highlighted" → `{actor: "sarah_chen", kind: "person", confidence: 0.6}`
- AI tool output visible: "Claude Code terminal with AI response" → `{actor: "claude", kind: "agent", confidence: 0.7}`
- Bot messages: "Slack bot message from deploy-bot" → `{actor: "deploy_bot", kind: "agent", confidence: 0.8}`
- System notification: "#engineering-alerts: pipeline failed" → `{actor: "ci_system", kind: "environment", confidence: 0.6}`

**Audio processor signals (future):**
- Speaker diarization: voice embedding clustering labels segments as Speaker A, Speaker B
- Voice enrollment: match against known voiceprints (not MVP)

Processor enrichment appends to the `actor_hints` array — does not overwrite capture hints.

### Layer 3: Threading Resolution (confidence 0.7-0.95)

The threading LLM sees all sources in context + the knowledge corpus. It performs final attribution by fusing signals across observations within each time block.

**Resolution examples:**

| Signals | Resolution |
|---|---|
| System audio speech at 10:01 + screen shows Sarah as active speaker + calendar says "1:1 with Sarah Chen" | Person: sarah_chen, confidence 0.9 |
| Mic audio at 10:00 + screen shows user's app in focus + no other speakers | Self, confidence 0.9 |
| Code appeared in editor + screen shows Claude Code + prompt was typed moments before | Agent: claude, confidence 0.85 |
| Slack notification appeared + no user interaction at that timestamp | Environment, confidence 0.8 |
| Status field changed on screen during meeting + unclear who did it | Unknown, confidence 0.3 |

The threading prompt already receives the knowledge corpus via `format_for_llm()`. Known entities (with names, types, relationships) help the LLM resolve ambiguous references: "Sarah" in audio → matches Entity `sarah_chen` in corpus.

## Entity Resolution

The knowledge corpus is the identity backbone. Resolution process:

1. **Match against known entities.** When the LLM encounters a name or identifier, check the corpus. Entity `sarah_chen` has name "Sarah Chen", aliases might include "Sarah", "SC".
2. **Cross-source correlation.** Calendar attendee "Sarah Chen" + screen showing "Sarah C." + system audio female voice → same entity.
3. **Create new entities.** First encounter with an unknown person creates a new Entity in the corpus with whatever signals are available. Subsequent encounters refine the entity.
4. **Merge duplicates.** User audit can merge entities that the system created separately ("Sarah" and "Sarah Chen" are the same person).

The `entity_names()` method on `KnowledgeCorpus` already injects known names into the threading prompt for recognition. As the corpus grows, attribution accuracy improves without model changes.

## Attribution Flow Through the Pipeline

No new types needed — attribution uses existing data model fields:

```
Capture
  Observation.metadata.actor_hints: [
    {"actor": "self", "kind": "self", "confidence": 0.3, "signal": "mic_channel"}
  ]
      ↓
Processor enrichment
  Observation.metadata.actor_hints: [
    {"actor": "self", "kind": "self", "confidence": 0.3, "signal": "mic_channel"},
    {"actor": "sarah_chen", "kind": "person", "confidence": 0.6, "signal": "screen_active_speaker"}
  ]
      ↓
Threading resolution
  ContextThread.metadata.speakers: ["self", "sarah_chen"]
  ContextThread.metadata.primary_actor: "self"
      ↓
Decision extraction
  Decision.proposed_by: ActorAttribution {
    actor: Actor { name: "sarah_chen", kind: Person },
    confidence: 0.85
  }
  Decision.resolved_by: ActorAttribution {
    actor: Actor { name: "self", kind: Self_ },
    confidence: 0.9
  }
```

## Agent Detection

Agents are the easiest actor type — identifiable by app identity and interaction patterns.

| Signal | Attribution | Confidence |
|---|---|---|
| Screen shows Claude Code with AI response | Agent: "claude" | 0.8 |
| Screen shows GitHub Actions / CI output | Agent: "github_actions" | 0.9 |
| Screen shows Copilot suggestion appearing | Agent: "copilot" | 0.8 |
| Code appeared in editor without typing | Agent: unknown | 0.5 |
| Automated Slack message (bot icon) | Agent: identified by bot name | 0.9 |
| Automated email (noreply address) | Agent/Environment: identified by sender | 0.8 |

The vision model naturally detects these from screenshots — AI tool interfaces have distinctive visual patterns.

## Organization + Environment Detection

**Organization:** Implicit in source context. The threading LLM infers organizational attribution from:
- Message source: `#company-announcements` Slack channel, `hr@company.com` email
- Content pattern: policy language, team-wide directives, org-level decisions
- No single person attributed as author

**Environment:** Detected by absence of human or agent interaction:
- System notifications (calendar reminders, OS alerts)
- Automated monitoring (CI failures, deploy alerts)
- External events (weather, news) if captured
- Observation timestamp has no user activity before it

## User Audit Loop

Attribution is a best guess. The user corrects mistakes, and corrections compound:

1. **Briefing presents attributions.** Each decision shows who proposed and who resolved.
2. **User flags errors.** "This was Sarah's idea, not mine" or "I said this, not the system."
3. **Corrections update knowledge corpus.**
   - Entity associations refined (voice patterns → entity, screen name variants → entity)
   - Relationship corrections (Sarah manages me, not the reverse)
   - New entities created from corrections
4. **Future pipeline runs benefit.** Corrected corpus injected into threading prompt → better attribution next time.

No voice enrollment, no ML retraining. The knowledge corpus is the learning mechanism — human corrections are the training signal.

## Implementation Scope

### What changes

**Threading prompt** (`alvum-episode/src/threading.rs`):
- Add attribution instructions to the system prompt
- Request `speakers` and `primary_actor` in thread metadata output
- Request `actor_hints` per observation assignment when confidence is high

**Decision extraction prompt** (`alvum-pipeline/src/distill.rs`):
- Already asks for `proposed_by` and `resolved_by` — enhance prompt to use actor hints from thread metadata
- Pass thread-level speaker information as context

**Screen processor** (`alvum-processor-screen`):
- Vision model prompt asks to identify active speakers, AI tool output, bot messages
- Appends actor hints to Observation metadata

**Capture crates** (`alvum-capture-audio`, `alvum-capture-screen`):
- Tag observations with source-based actor hints in metadata
- Minimal change: add `actor_hints` to metadata on observation creation

**Knowledge corpus** (`alvum-knowledge`):
- Already has Entity with relationships — no structural changes
- Entity resolution logic used during threading (already injected via `format_for_llm()`)

### What does NOT change

- `ActorAttribution`, `Actor`, `ActorKind` types — already correct
- `Decision` model — already has `proposed_by`, `resolved_by` with confidence
- Episodic alignment types — `ContextThread` metadata already supports arbitrary JSON
- Storage — JSONL format handles new metadata fields without schema migration
- `Observation` — metadata is already `Option<serde_json::Value>`, actor hints are just new keys

### No new crates

Attribution is a cross-cutting concern that enhances existing crates, not a standalone module. The logic lives in:
- Capture crates (hint tagging)
- Processor crates (enrichment)
- Threading prompt (resolution)
- Extraction prompt (final attribution)
