# Top-Level Spec Progress Matrix

> **For agentic workers:** this is a roadmap/status document, not an executable TDD plan. Each non-trivial gap gets its own dedicated plan in `docs/superpowers/plans/` before implementation. Spec references use `§` + section title from `docs/superpowers/specs/2026-04-03-alvum-system-design.md` unless noted.

**Date:** 2026-04-18
**Spec of record:** `docs/superpowers/specs/2026-04-03-alvum-system-design.md`
**Supporting specs:** `2026-04-12-{capture-orchestration,screen-capture,episodic-alignment,actor-attribution}.md`

**App-shell platform decision (2026-04-18):** the spec names Tauri + a single `app/` targeting macOS + iOS + watchOS + visionOS. Committed direction is split:
- **macOS app → Electron.** Matches the prevailing industry pattern for complex AI desktop apps (Anthropic Claude, OpenAI Codex, Cursor, Linear, 1Password 8, VS Code, Figma). The Rust workspace is the spawned sidecar; the web UI served by `alvum-web` on `localhost:3741` is the render target.
- **iOS / watchOS / visionOS → native Swift, deferred.** Electron does not reach any Apple mobile/wearable surface, and watchOS/visionOS have no web-view path at all — they are SwiftUI-only by platform constraint. Both OpenAI and Anthropic staff separate native Swift + native Kotlin teams for their mobile apps; cross-platform mobile frameworks are absent from their public job postings. The Rust workspace will expose a stable library-level interface so the eventual native iOS app can embed it (via UniFFI or similar) rather than talk to a local HTTP server the way the Mac app will.
- **Spec update required:** the `app/` line in `docs/superpowers/specs/2026-04-03-alvum-system-design.md` (lines 75 + § Tauri App Shell) needs amendment to reflect this split. Treat this doc as the canonical source until that happens.

**Legend:** ✅ Done · 🟡 Partial · ⚪ Not started

---

## Executive Read

The Rust backend has executed through the spec's **V0 → V1** band and has partial V1.5 infrastructure. Twelve crates exist, ~9k LOC, ~98 tests. Capture daemons (screen, audio) run end-to-end; the connector/processor abstraction matches the spec; episodic alignment and the knowledge corpus are wired into the pipeline.

The gap to full spec parity is **larger than the crate count suggests** because the spec's product surface and alignment semantics barely exist in code:

- The **entire intention → alignment → evening-check-in loop** is absent (no `Intention`, `AlignmentReport`, `EmergentState`, `ModalConflict`, `BehavioralSignal`, `Evidence`, or `Commitment` types).
- **Decision outcome tracking** is absent (no `effects`, `actual_outcome`, `check_by`, `cascade_depth`).
- **App shell, web UI, wearable firmware, ingest endpoint, embedding layer, and managed tier** have zero lines of code.
- Pipeline stages per spec are Prepare → Transcribe → Fuse+Extract → Link → Align+Brief; we implement roughly Extract → Causal → Brief with episodic threading in front. Linking does not yet detect outcomes, manage states, or do retroactive cascade updates.

The project is a functioning **V1 decision extractor on top of real capture**, not yet the alignment engine the spec describes.

---

## Platform Decision — Evidence Base

Captured here so the app-shell decision isn't re-opened without new evidence.

**Desktop AI apps in the wild (verified April 2026):**
- **Anthropic Claude desktop** — Electron. Claude Code lead Boris Cherny: "Some of the engineers working on the app worked on Electron back in the day, so preferred building non-natively. It's also a nice way to share code so we're guaranteed that features across web and desktop have the same look and feel." ([HN, Feb 2026](https://news.ycombinator.com/item?id=47104973))
- **OpenAI Codex desktop (Feb 2026)** — Electron 40 + Node.js + React/Jotai + better-sqlite3 + node-pty, wrapping a 51MB Rust CLI. ([Simon Willison](https://simonwillison.net/2026/Feb/2/introducing-the-codex-app/))
- **OpenAI ChatGPT for Mac (May 2024)** — fully native, not Electron, not Catalyst. ([Javi, OpenAI](https://x.com/Javi/status/1790074965112328538))
- **OpenAI ChatGPT for Windows (Oct 2024)** — Electron. ([Windows Latest](https://www.windowslatest.com/2024/10/18/i-tried-the-official-chatgpt-app-for-windows-11-its-just-an-electron-based-chrome-wrapper/))
- **Linear desktop** — Electron, wraps their React web app. ([Linear changelog](https://linear.app/changelog/2019-04-25-linear-desktop-app))
- **1Password 8** — Electron + React UI + Rust backend. ([1Password blog](https://blog.1password.com/1password-8-the-story-so-far/))
- **Figma desktop** — Electron shell + BrowserView + WebGL + WebAssembly core. ([Figma blog](https://www.figma.com/blog/introducing-browserview-for-electron/))
- **VS Code / Cursor / Slack / Discord / Notion / Signal** — all Electron.

**Mobile AI apps:**
- **OpenAI**: iOS is Swift + UIKit/SwiftUI, Android is Kotlin. Dedicated "core Swift platform" and "core Kotlin-based platform" infra teams. ([OpenAI ChatGPT iOS](https://openai.com/careers/software-engineer-ios-san-francisco/), [OpenAI Android infra](https://openai.com/careers/android-engineer-chatgpt-mobile-infrastructure-san-francisco/))
- **Anthropic**: iOS is Swift/UIKit/SwiftUI (7+ years required), Android is Kotlin/Jetpack Compose (7+ years). ([Anthropic iOS](https://www.anthropic.com/careers/jobs/4572744008), [Anthropic Android](https://www.anthropic.com/careers/jobs/4899511008))
- Neither company lists React Native, Flutter, or any cross-platform framework in their mobile JDs.

**Read of the evidence:** Complex cross-platform AI tools → Electron desktop is the default and well-accepted. Apple mobile surfaces → unambiguously native, even at the companies that accept Electron on desktop. Alvum's Mac app (complex tool, Rust backend, needs menu bar + auto-update + OS permission flows) is the Codex / Claude / Linear / 1Password shape, not the consumer ChatGPT Mac shape. Electron is defensible.

**What would re-open this decision:** a hard requirement for deep Apple-ecosystem integration the Electron shell can't deliver (e.g., Continuity, Focus modes, Handoff, visionOS coexistence with the desktop). Catch this in the Phase C plan; don't speculate now.

---

## Progress Matrix — Spec Section by Spec Section

| # | Spec section | Status | Evidence | Gap |
|---|---|:-:|---|---|
| 1 | § Architecture — Workspace layout (crates) | 🟡 | 12/16 crates present; `alvum-web`, `alvum-graph`, `alvum-connector-git`, `alvum-connector-wearable` absent. `app/desktop-electron/` (Phase C) and `firmware/` (Phase E) absent. `app/ios-native/` etc. deferred post-V1.5 per row 43b. | Scaffold missing crates when each epic starts; keep YAGNI until used |
| 2 | § Architecture — Connector/CaptureSource/Processor traits | ✅ | `alvum-core/src/{connector,capture,processor}.rs` | Externalize as stable plugin ABI once a 3rd-party connector appears |
| 3 | § Architecture — DataRef + Artifact + layers | ✅ | `alvum-core/src/{data_ref,artifact}.rs` | None |
| 4 | § Architecture — Connector config (TOML) | ✅ | `alvum-core/src/config.rs` → `~/.config/alvum/config.toml`; CLI dispatch in `alvum-cli/src/main.rs` | Add per-connector `enabled` audit + validation errors for malformed TOML |
| 5 | § Pipeline — Gather (capture) | ✅ | `alvum-capture-{audio,screen}`, connector bridges | — |
| 6 | § Pipeline — Process (processors) | ✅ | `alvum-processor-{audio,screen}` (Whisper + vision/OCR) | — |
| 7 | § Pipeline — Align (episodic) | ✅ | `alvum-episode/src/threading.rs`, wired in `alvum-pipeline/src/extract.rs` | Add regression corpus; verify "relevance scoring fed by knowledge corpus" feedback loop |
| 8 | § Pipeline — Distill (decision extraction) | ✅ | `alvum-pipeline/src/distill.rs` | — |
| 9 | § Pipeline — Learn (knowledge) | ✅ | `alvum-knowledge/src/{extract,store,types}.rs`; stores entities, patterns, facts; merges on subsequent runs | Add stale-entry flagging (not referenced in 30+ days); relationships field not fully extracted |
| 10 | § Pipeline — Link (causal) | 🟡 | `alvum-pipeline/src/causal.rs` fills `causes` | Missing: outcome detection against open decisions, retroactive `effects` updates, `cascade_depth`, butterfly flagging (§ Stage 4) |
| 11 | § Pipeline — Brief | ✅ | `alvum-pipeline/src/briefing.rs` | Two-phase briefing (learning-phase vs. full-alignment-mode) requires intentions — blocked by row 15 |
| 12 | § Pipeline — Prepare stage (audio dedup, frame dedup, timeline skeleton → `prepared.json`) | ⚪ | None | Build `alvum-pipeline::prepare` (§ Stage 1) |
| 13 | § Data Model — Life Domains (user-defined strings) | ✅ | `Decision.domain: String` (free-form) | Onboarding seeds defaults + `/intentions` page edits (blocked by app shell) |
| 14 | § Data Model — Decision (full spec shape) | 🟡 | `crates/alvum-core/src/decision.rs:14` — `id, timestamp, summary, reasoning, alternatives, domain, source, proposed_by, status, resolved_by, causes, tags, expected_outcome` | Missing: `effects`, `contributing_states`, `check_by`, `actual_outcome`, `cascade_depth`, `cross_domain_effects`, `conflicts`, `evidence`, `participants`, `source: DecisionSource` enum (currently `String`). `timestamp` is `String` not `DateTime<Utc>` |
| 14b | § Data Model — CausalLink (mechanism enum, cross_domain) | 🟡 | `crates/alvum-core/src/decision.rs:69` has `from_id`, `mechanism: String`, `strength` | `mechanism` should be the `CausalMechanism` enum (Direct, ResourceCompetition, EmotionalInfluence, Precedent, Constraint, Accumulation); `cross_domain: Option<(String, String)>` field missing. Fold into Phase A work-unit 1. |
| 15 | § Data Model — Intention (+IntentionKind, IntentionSource, Cadence) | ⚪ | No type exists | Build `alvum-core::intention` + storage (`intentions.json`) — prerequisite for alignment |
| 16 | § Data Model — Evidence, ModalClaim, ModalConflict, ConflictType | ⚪ | No types exist | Add to `alvum-core`; populated from Fuse+Extract |
| 17 | § Data Model — EmergentState | ⚪ | No type exists | Add to `alvum-core`; lifecycle owned by Link stage (§ Stage 4 item 4) |
| 18 | § Data Model — BehavioralSignal (+BehavioralSignalType) | ⚪ | No type exists | Produced by Fuse+Extract; consumed by evening check-in |
| 19 | § Data Model — AlignmentReport / AlignmentItem / AlignmentStatus / Trend | ⚪ | No type exists | Produced by Stage 5 (Align+Brief) |
| 20 | § Data Model — Knowledge Corpus (Entity, Relationship, Pattern, Fact) | 🟡 | `alvum-knowledge/src/types.rs` has entity/pattern/fact | `Relationship` struct with `last_confirmed` + bidirectional relationship extraction not fully implemented |
| 21 | § Data Model — DayExtraction (activity_blocks, events, decisions, commitments, behavioral_signals, alignment) | ⚪ | Pipeline outputs `decisions.jsonl` + `briefing.md` only | Build once upstream types exist (rows 15-19); becomes `days/YYYY-MM-DD.json` |
| 22 | § Storage — directory layout | 🟡 | `output/` (gitignored) only; no `capture/` → `days/` → `decisions/` → `knowledge/` → `episodes/` → `intentions.json` → `briefings/` → `checkin_questions/` hierarchy | Implement per-spec paths under `~/.alvum/`; migrate from `./output/` |
| 23 | § Storage — retention policy (30-day raw, forever for refined) | ⚪ | Not implemented | Scheduled pruner that preserves evidence-linked media |
| 24 | § Capture — Desktop screen triggers (app focus, frame diff, idle, clipboard, URL) | 🟡 | App-focus + idle implemented; SCK frame diff + clipboard + browser-URL triggers absent | Extend trigger set per `specs/2026-04-12-screen-capture.md` |
| 25 | § Capture — Semantic change events (generic a11y differ, `events.jsonl`) | ⚪ | No a11y integration, no `events.jsonl` writer | Implement in `alvum-capture-screen`; 5 universal detection patterns |
| 26 | § Capture — Desktop audio (mic + system + wearable streams) | 🟡 | Mic + system via `alvum-capture-audio`; wearable absent | Gated on wearable arrival |
| 27 | § Capture — CoreLocation `location.jsonl` | ⚪ | Not implemented | Small addition once storage layout lands |
| 28 | § Capture — Wearable ingest endpoint (`POST /api/ingest` via axum, mDNS discovery) | ⚪ | No endpoint | Requires web server (row 34) |
| 29 | § Multi-Modal Fusion — per-block LLM reasoning with fusion rules | ⚪ | Extraction operates on flat observations; no per-block two-tier model strategy | Refactor `distill.rs` to segment by activity block + route cheap/strong by classification (§ Stage 3) |
| 30 | § Noise Filtering — Level 1 (capture-time: a11y pruning, audio VAD, frame pHash) | 🟡 | Audio VAD in `alvum-processor-audio/src/vad.rs`; a11y pruning and frame pHash absent | Add when wearable lands |
| 31 | § Noise Filtering — Level 2 (prepare stage) | ⚪ | Blocked by row 12 | Same work |
| 32 | § Noise Filtering — Level 3 (LLM context curation, per-block token budgets) | ⚪ | Prompts don't enforce budgets | Add budget enforcement in `distill.rs` |
| 33 | § Silent Decision Detection (aborted actions, avoidance, repetitive visits, self-interruption) | ⚪ | No detector | Runs on capture events + timeline; emits `BehavioralSignal`s |
| 34 | § Web UI — axum server on `localhost:3741` | ⚪ | Crate absent | Build `alvum-web` with routes `/`, `/briefing/:date`, `/checkin`, `/intentions`, `/timeline/:date`, `/decisions`, `/settings`, `/api/*` |
| 35 | § Web UI — Morning briefing page (learning-phase vs. alignment-mode) | ⚪ | Briefing is `.md` only | Render HTML with evidence citations → `/timeline/:date` |
| 36 | § Web UI — Evening check-in (behavioral + intention probes, voice/text/MC) | ⚪ | Not started | Depends on `BehavioralSignal`s (18) + intention-probe stage |
| 37 | § Web UI — `/intentions` page | ⚪ | Not started | Depends on `Intention` type (15) |
| 38 | § Web UI — `/decisions` and `/timeline/:date` views | ⚪ | Not started | Depends on stored day extractions + decision graph file format |
| 39 | § Intention Capture UX — progressive dialogue through check-ins | ⚪ | Not started | Blocked on rows 15, 18, 36 |
| 40 | § Intention Capture UX — commitment extraction from audio | ⚪ | Not started | Adds `Commitment` type + extraction prompt to distill |
| 41 | § Intention Capture UX — stale-intention detection | ⚪ | Not started | Small addition once intentions exist |
| 42 | § Intention Capture UX — onboarding (domain select + free-text goals) | ⚪ | Not started | Part of app shell onboarding |
| 43 | § App Shell — macOS (Electron, replaces spec's Tauri) | ⚪ | No `app/` directory | Menu bar icon, native window → `localhost:3741`, macOS permission flows (screen recording, mic, a11y, location), auto-update, DMG + code signing, spawns capture daemons + web server + pipeline scheduler. Explicit cold-start / RSS / renderer frame-time budgets enforced in CI. **Desktop-only** — see row 43b for Apple mobile/wearable surfaces. |
| 43b | App Shell — iOS / watchOS / visionOS (native Swift, deferred) | ⚪ | Not started | Separate native apps written in Swift/SwiftUI, embedding the Rust workspace as a library (UniFFI or similar FFI) rather than using Electron. watchOS + visionOS have no web-view path; iOS could in principle use a WKWebView shell but parity with watch/vision argues for unified native. Scheduled as a post-V1.5 expansion — core alignment loop ships on Mac first. |
| 44 | § Wearable (`firmware/`) | ⚪ | No `firmware/` directory | ESP32-S3 + OV2640 + SPH0645 + BMI270; Opus audio, adaptive-rate WebP frames, mDNS sync |
| 45 | § Model Architecture — `ModelProvider` trait with capabilities/cost/privacy | 🟡 | `alvum-core::llm::LlmProvider` exists with cli/api/ollama impls; lacks `capabilities()`, `vision()`, cost/privacy tiering | Extend trait per spec; add vision path |
| 46 | § Model Architecture — Privacy modes (full/hybrid/best) + per-stage model config | ⚪ | One provider for all stages | Add `models.{transcription,extraction_routine,extraction_rich,linking,briefing,frame_description}` in config |
| 47 | § Fine-Tuning Pipeline (LoRA/QLoRA on Apple Silicon MLX after 6 months) | ⚪ | Not started | Design-deferred per spec; artifacts already training-data-shaped |
| 48 | § RL/RLHF for briefing quality (engagement signals → reward model) | ⚪ | No interaction logging | Deferred, but add interaction event logging **now** so data accumulates from day one |
| 49 | § Embedding Strategy — provider trait, MVP "no embeddings" | ✅ | Consistent with MVP posture | — |
| 50 | § Embedding Strategy — text embeddings + vector store (usearch / hnswlib / sqlite-vss) | ⚪ | Not started | Trigger per spec: "corpus + decisions exceed LLM context (~6 months)" — defer |
| 51 | § Embedding Strategy — multimodal embeddings | ⚪ | Not started | V3 per growth path |
| 52 | § Managed Tier — encrypted cloud sync (ChaCha20-Poly1305, Argon2) | ⚪ | Not started | V1 ships DIY-only per spec |

---

## Version Alignment (vs. spec's Growth Path)

| Version | Spec trigger | Status |
|---|---|:-:|
| **V0** — Claude Code connector + pipeline + briefing | Initial build | ✅ |
| **V0.5** — Audio capture + Whisper | V0 validated | ✅ |
| **V1** — Episodic alignment + knowledge corpus | Audio noise + cross-context | ✅ (backend; product surface missing) |
| **V1.5** — Desktop capture + wearable + alignment engine + check-in | Episodic validated | 🟡 (desktop capture built; alignment engine + check-in + wearable not) |
| **V2** — Text embeddings for retrieval | Context exhaustion | ⚪ |
| **V2.5** — Local models (Ollama) | Privacy demand | 🟡 (Ollama provider exists; per-stage routing missing) |
| **V3** — Multimodal embeddings | Text descriptions insufficient | ⚪ |
| **V3.5** — Multi-device via Syncthing | Second machine | ⚪ |
| **V4** — Fine-tuned extraction | 6+ months of data | ⚪ |
| **V5** — Structured storage (SQLite/DuckDB) | Query scaling | ⚪ |
| **V6** — RL-trained briefing | Enough feedback | ⚪ (start logging now) |

**We are mid-V1.5.** Reaching full V1.5 is the focused near-term target.

---

## Delivery Phases

Each phase yields working software. Phase A unblocks everything else because it defines the types the app, UI, and pipeline all consume.

### Phase A — Alignment Primitives (type + storage foundation)

**Goal:** make the spec's data model real in code so the rest of the spec can be implemented against it.

Rows closed: 14, 15, 16, 17, 18, 19, 21, 22.

**Work units:**
1. Extend `alvum-core/src/decision.rs` with `effects`, `contributing_states`, `check_by`, `actual_outcome`, `cascade_depth`, `cross_domain_effects`, `conflicts`, `evidence`, `participants`. Convert `source: String` → `DecisionSource` enum. Convert `timestamp: String` → `DateTime<Utc>`.
2. Create `alvum-core/src/intention.rs` with `Intention`, `IntentionKind`, `IntentionSource`, `Cadence`.
3. Create `alvum-core/src/evidence.rs` with `Evidence`, `Confidence`, `ModalClaim`, `ModalConflict`, `ConflictType`.
4. Create `alvum-core/src/state.rs` with `EmergentState`.
5. Create `alvum-core/src/behavior.rs` with `BehavioralSignal`, `BehavioralSignalType`.
6. Create `alvum-core/src/alignment.rs` with `AlignmentReport`, `AlignmentItem`, `AlignmentStatus`, `Trend`.
7. Create `alvum-core/src/day.rs` with `DayExtraction` and supporting `ActivityBlock`, `Commitment`, `Event`.
8. Create `alvum-core/src/paths.rs` resolving `~/.alvum/` with the full spec directory tree; migrate writers away from `./output/`.
9. Serde round-trip tests per type.

**Exit criteria:** every type in § Data Model exists with round-trip tests; CLI still works; storage paths match spec.

**Dedicated plan to write before executing:** `docs/superpowers/plans/<date>-alignment-primitives.md`.

### Phase B — Alignment Engine (behavior-side of the loop)

**Goal:** turn capture + extraction into intention-vs-reality alignment.

Rows closed: 10, 12, 19 (output), 29, 33, 40.

**Work units:**
1. Implement `alvum-pipeline::prepare` (audio dedup, frame pHash clustering, activity-block segmentation → `prepared.json`).
2. Refactor `distill.rs` to run per-activity-block with two-tier model routing (cheap vs. strong by block classification).
3. Extend `causal.rs` to Stage-4 spec: outcome detection against open decisions, retroactive `effects` + `cascade_depth`, butterfly flagging, `EmergentState` lifecycle (create/intensify/resolve).
4. Add `alvum-pipeline::alignment` that computes `AlignmentReport` per active intention from the day's evidence.
5. Add `alvum-pipeline::signals` for behavioral-signal detection.
6. Add `alvum-pipeline::commitments` to extract `Commitment` entries from audio during distill.
7. Regression corpus: curated day under `tests/fixtures/days/` + expected extractions for CI.

**Exit criteria:** `alvum extract` produces a full `DayExtraction` including `AlignmentReport` and `BehavioralSignal`s; regression corpus locks quality.

**Dedicated plan:** `docs/superpowers/plans/<date>-alignment-engine.md`.

### Phase C — Product Surface (web UI + app shell)

**Goal:** a non-CLI user can operate daily.

Rows closed: 34, 35, 36, 37, 38, 42, 43, and 13 via onboarding. **Row 43b (iOS/watch/vision native apps) is explicitly out of Phase C scope** — deferred post-V1.5.

**Work units:**
1. Create `crates/alvum-web` (axum) with routes `/`, `/briefing/:date`, `/checkin`, `/intentions`, `/timeline/:date`, `/decisions`, `/settings`, `/api/*`. Embedded server on `localhost:3741`. The web UI must be mobile-viewport-responsive so future native iOS can optionally embed a WKWebView during early development if useful.
2. Create `app/` Electron shell for **macOS only**: menu bar icon, native window pointing at `localhost:3741`, macOS permission prompts (screen recording, mic, a11y, location), auto-update, DMG packaging + code signing. Enforce performance budgets in CI: cold start, RSS, renderer frame-time. The Rust workspace is spawned as a child process (capture daemons + web server + pipeline scheduler).
3. Onboarding flow: domain selection (defaults Health / Family / Career / Finances / Creative) + single free-text goal prompt.
4. Evening-check-in generation stage: 2-3 questions from today's `BehavioralSignal`s and unlabelled patterns; writes `checkin_questions/<date>.json`.
5. Intention-probe generation: patterns lacking a stated intention surface as check-in questions.
6. Interaction logging from day one (briefing item engage/dismiss, check-in answer/skip) — future RLHF training data per § RL/RLHF.

**Exit criteria:** daily loop — open app → see briefing → answer check-in → see intentions updated — works end-to-end on one Mac with Electron perf budgets passing in CI.

**Dedicated plan:** `docs/superpowers/plans/<date>-product-surface.md`.

### Phase D — Capture Completeness

**Goal:** match § Capture fully on macOS so the alignment engine has the inputs it was designed for.

Rows closed: 24, 25, 27, 30 (capture-time additions).

**Work units:**
1. Extend `alvum-capture-screen` with ScreenCaptureKit frame-diff trigger, clipboard (NSPasteboard), browser-URL a11y trigger.
2. Implement generic a11y differ (5 patterns) producing `events.jsonl` per day.
3. macOS Vision-framework OCR fallback when a11y text is below threshold.
4. CoreLocation significant-change monitoring → `location.jsonl`.
5. A11y tree pruning + frame pHash dedup at capture time.

**Exit criteria:** a real day's capture produces the full on-disk layout from § Storage.

**Dedicated plan:** `docs/superpowers/plans/<date>-capture-completeness.md`.

### Phase E — Wearable

**Goal:** realize the clip-on ESP32 pathway.

Rows closed: 26, 28, 44.

**Work units:**
1. Create `crates/alvum-connector-wearable` with audio + frame ingest handlers mounted into `alvum-web`.
2. mDNS advertisement of `_alvum._tcp.local`.
3. Prepare-stage dedup integration: mic-vs-wearable SNR per 5-min window; wearable-frame suppression when concurrent screen capture exists.
4. Create `firmware/` ESP32-S3 project (Opus continuous audio, adaptive-rate WebP frames, WiFi sync).

**Exit criteria:** a day's capture can include wearable audio/frames merged into the timeline.

**Dedicated plan:** `docs/superpowers/plans/<date>-wearable.md`.

### Phase F — Model Architecture Upgrade

**Goal:** match § Model Architecture (privacy presets, per-stage routing, local vision).

Rows closed: 45, 46.

**Work units:**
1. Extend `LlmProvider` with `capabilities()`, `vision()`; add `VisionRequest/Response` types.
2. Add `models.*` per-stage config; CLI honors per-stage selection.
3. Add privacy-mode presets (full / hybrid / best) selectable via settings UI.
4. Add LLaVA-style local vision via Ollama for frame description.

**Exit criteria:** user flips privacy mode in settings → every stage routes accordingly, no code changes.

### Phases G+ — Deferred by design

V2 text embeddings, V3 multimodal embeddings, V3.5 multi-device, V4 fine-tuning, V5 structured storage, V6 RL-trained briefing — all deferred until their spec-defined triggers fire. Do not pre-build.

---

## Immediate Next Actions

1. **Write the Phase A plan** (`alignment-primitives`). Nothing downstream can start without the types.
2. **Introduce `Intention` + onboarding defaults** — the smallest slice of Phase A that unlocks Phase B's alignment computation.
3. **Fix `Decision.timestamp` → `DateTime<Utc>`** (known limitation); fold into Phase A work-unit 1.
4. **Stop writing to `./output/`; start writing to `~/.alvum/` per § Storage** — Phase A work-unit 8. All downstream phases assume this layout.
5. **Amend the top-level spec** to reflect the committed app-shell split: Electron for macOS, native Swift for iOS/watchOS/visionOS (deferred). Spec lines 75 + § Tauri App Shell are now stale. Fold `app/` into the workspace as `app/desktop-electron/` to leave room for `app/ios-native/` etc. later.
6. **Add interaction-event logging skeleton** in the Phase C plan — cheap now, accumulates the V6 reward dataset from day one.

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Types sprawl before UI consumes them (Phase A ships dead code) | Med | Med | Keep Phase A strictly aligned to what B/C need; no speculative fields |
| Alignment engine produces unreliable reports without intention data | High | High | Gate alignment mode in UI until enough intentions are confirmed (spec's "Week 3+" rule) |
| Two-tier model routing makes extraction cost unpredictable | Med | Med | Cost budget per day in config; fail loudly when exceeded |
| Electron shell misses perf budgets on older hardware | Med | Med | Budgets enforced in CI from Phase C day 1 (cold start, RSS, renderer frame-time). If budgets slip materially, options are: (a) tighten the renderer, (b) move the hot path to the Rust sidecar, (c) switch to Tauri 2.x (stable since Oct 2024; would only change the shell, not the Rust workspace or web UI). Full rewrite to native Swift is the nuclear option and defers the Mac ship. |
| Mobile parity (iOS/watch/vision) is a separate large project and may slip indefinitely | High | Med | Treat as post-V1.5. Keep the Rust workspace library-friendly (stable FFI surface) so native mobile can embed it. Don't let mobile-parity anxiety block the Electron Mac ship. |
| App-shell permission flows (screen recording, a11y) stall daily use | Med | High | Build permission audit + degraded-mode UI early |
| Wearable firmware timeline slips | High | Low | A-D deliver value without wearable; defer until prototype exists |
| V6 reward data unrecoverable if not logged now | Low | High | Interaction logging in Phase C from day one |
| Extraction quality regresses as prompts/types evolve | High | High | Regression corpus in Phase B is non-negotiable |

---

## Rolling Update Template

When revisiting this document, append a dated update below rather than editing the matrix — the matrix stays a baseline; updates show deltas.

```md
### Update YYYY-MM-DD
| Row | Was | Now | Delta | Notes |
|---|:-:|:-:|---|---|
| 10 | 🟡 | 🟡 | outcome detection shipped | still missing cascade_depth |
| 14 | 🟡 | ✅ | full Decision shape landed | closed Phase A work-unit 1 |
```
