# Platform, Provider, and Briefing Gap Audit

**Date:** 2026-04-28
**Status:** Audit plus runtime-baseline documentation
**Spec lineage:** This document reviews the current codebase against the product
vision in `2026-04-03-alvum-system-design.md`, the MVP decision extractor plan,
the storage/layout specs, and the current production code paths. The companion
runtime spec `2026-04-28-component-extension-runtime.md` documents the external
HTTP extension platform implemented after this audit.

## Executive Read

Alvum already has the internal primitives needed for a developer platform:
`Connector`, `CaptureSource`, `Processor`, `DataRef`, `Observation`,
`Decision`, `Edge`, and `LlmProvider` exist and are wired through the CLI,
Electron shell, and overnight briefing pipeline.

The gap is not lack of primitives. The gap is that the primitives are still
first-party, in-process implementation details. A contributor can add a
connector or provider by editing the workspace, but there is no stable platform
surface that explains the component model, exposes a registry, declares
capabilities, validates config, or gives a third-party developer a clear
integration path.

The briefing and decision graph methodology is similarly real but under-
documented. The current system is a hierarchical tree pipeline:

```
DataRef -> Observation -> TimeBlock -> Thread -> Cluster -> Domain Decisions
       -> Decision Edges -> Day Briefing -> Knowledge update
```

That flow lives primarily in `crates/alvum-pipeline/src/tree/*` prompts and
orchestration. The older specs still describe an earlier Prepare / Transcribe /
Fuse / Link / Align model and older files such as `distill.rs`, `causal.rs`,
while the menubar Synthesis calendar now exposes a per-day Decision Graph view
backed by `decisions.jsonl` plus `tree/L4-edges.jsonl` when available.
and `briefing.rs`. The current implementation should be documented as the
source of truth, while the older vision should be treated as the future
alignment engine target.

## Reviewed Scope

This audit covered the repo from the public docs down to the runtime paths that
actually ship the daily briefing:

| Area | Current source of truth | Notes |
| --- | --- | --- |
| Product vision | `docs/superpowers/specs/2026-04-03-alvum-system-design.md` | Still the broadest product and architecture vision. Some crate names and pipeline descriptions are stale. |
| MVP | `docs/superpowers/plans/2026-04-03-mvp-decision-extractor.md` and `docs/superpowers/plans/2026-04-18-pmf-path.md` | MVP goal is trailing decision graph plus useful morning briefing. |
| Runtime operations | `AGENTS.md`, `scripts/build-deploy.sh`, `scripts/briefing.sh`, `app/main.js` | These are the practical truth for build, signing, capture, provider UI, and briefing triggers. |
| Storage | `docs/superpowers/specs/2026-04-18-storage-layout.md` plus current code paths | Storage spec is authoritative for lifecycle layout; current tree outputs are more detailed than the spec. |
| Core contracts | `crates/alvum-core/src/{connector,capture,processor,data_ref,observation,artifact,decision,llm,config}.rs` | Internal API is coherent, but not packaged as an external SDK. |
| Connectors | `crates/alvum-connector-*`, `crates/alvum-capture-*`, `crates/alvum-processor-*` | First-party connectors exist for audio, screen, Claude sessions, and Codex sessions. |
| Providers | `crates/alvum-pipeline/src/llm.rs`, `crates/alvum-cli/src/main.rs`, `app/main.js` | Provider implementations and UI are hardcoded. |
| Briefing methodology | `crates/alvum-pipeline/src/extract.rs`, `crates/alvum-pipeline/src/tree/*`, `crates/alvum-knowledge/src/*` | Actual methodology is implemented, observable, and checkpointed, but not yet explained in a public methodology doc. |

## Current Component Model

### Runtime Shape

The production path is:

```
launchd
  -> Alvum.app/Contents/MacOS/Alvum
       -> Contents/Resources/bin/alvum capture
       -> scripts/briefing.sh
            -> alvum extract
```

The Electron shell owns the macOS permission flow and spawns the signed Rust
binary. Terminal diagnostics are not equivalent to production capture because
TCC grants depend on the responsible app process.

Daily briefing generation starts in `scripts/briefing.sh`, backfills missing
days, and invokes `alvum extract` into:

```
~/.alvum/generated/briefings/YYYY-MM-DD/
```

The Electron popover and `alvum tail` both observe:

```
~/.alvum/runtime/briefing.progress
~/.alvum/runtime/pipeline.events
```

### Core Contracts

| Contract | Current role | Platform implication |
| --- | --- | --- |
| `DataRef` | Timestamped pointer to raw source data. | Connector ingestion boundary. Needs documented source naming, path, MIME, and metadata rules. |
| `Observation` | LLM-readable unit produced from one or more refs. | Pipeline boundary. Needs documented `kind`, metadata, evidence, and media conventions. |
| `Artifact` | Typed layer container for text, embedding, structured output. | Exists in core but is not the dominant processor output path today; docs should not imply it is required until the pipeline uses it. |
| `CaptureSource` | Long-running capture daemon primitive. | Useful for in-process connectors, but production capture currently has a separate hardcoded CLI source factory. |
| `Processor` | Converts `DataRef` batches to `Observation`s. | Main extension point for raw media or session sources. |
| `Connector` | Bundles capture sources, processors, expected sources, and ref gathering. | Correct abstraction, but not discoverable through a registry or external manifest. |
| `LlmProvider` | Text completion plus optional image completion. | Works for first-party providers, but lacks capability, privacy, cost, routing, and config schema metadata. |
| `Decision` | Decision node shape for briefing and graph views. | Current node contract is richer than older docs, including source, status, evidence, confidence, causes, and effects. |
| `Edge` | Rich relation between decisions or higher-level items. | Authoritative causal/alignment edge shape. `Decision.causes/effects` is a projection, not the full graph. |

### First-Party Connectors

| Connector | Current path | Source type | Notes |
| --- | --- | --- | --- |
| Audio | `crates/alvum-connector-audio` | Captured mic/system audio | Scans `capture/YYYY-MM-DD/audio/{mic,system}`. Processor requires a configured Whisper model. |
| Screen | `crates/alvum-connector-screen` | Screen captures JSONL plus images | Uses OCR or provider-backed vision depending on config. |
| Claude Code | `crates/alvum-connector-claude` | Existing session JSONL | Thin wrapper over generic `SessionConnector`. |
| Codex | `crates/alvum-connector-codex` | Existing rollout JSONL | Thin wrapper over generic `SessionConnector`. |

The generic `SessionConnector<S>` is the clearest existing model for a new
developer contribution. A developer implements `SessionSchema` with a source
name, directory, filename matcher, and line parser. That path works well for
JSONL session-style connectors, but it is not documented as the recommended
starter path.

## Gap 1: Developer Platform and Connector Integration

### What Exists

The spec concept is sound: a connector is the user-facing unit and composes
capture plus processing into source-agnostic observations. The code has this
shape:

```
Connector
  -> capture_sources()
  -> processors()
  -> gather_data_refs()
  -> expected_sources()

Processor
  -> handles(source)
  -> process(DataRef[]) -> Observation[]
```

Config is also structurally ready for multiple connectors:

```toml
[connectors.audio]
enabled = true

[connectors.screen]
enabled = true

[capture.audio-mic]
enabled = false
```

The pipeline already asks enabled connectors to gather refs, emits per-source
inventory events, routes refs to processors by `handles()`, retries processor
failures, and writes a partial briefing if one source fails.

### What A Developer Must Do Today

Adding a first-party connector today requires coordinated edits:

1. Create a new crate under `crates/alvum-connector-*`.
2. Add it to the root workspace.
3. Add it as an `alvum-cli` dependency.
4. Implement `Connector`; usually implement `Processor` and sometimes
   `CaptureSource`.
5. Add a `connectors_from_config()` match arm in `alvum-cli`.
6. If it captures continuously, add a separate `create_source()` match arm for
   `alvum capture`.
7. Add config defaults and migrations in `AlvumConfig`.
8. Update Electron config parsing and source toggles if it has app-visible
   settings.
9. Update scripts or status summaries if users need to see the source.
10. Add tests for parsing, gather, processing, and pipeline integration.

That is a workable internal workflow. It is not yet a developer platform.

### Platform Gaps

| Gap | Current impact | Target |
| --- | --- | --- |
| Connector registry only exists for external packages | Built-ins now have read-only virtual manifests, but core construction still uses CLI match statements. | Use the descriptor catalog as the construction source for built-in connectors. |
| Capture path partly split | External connector capture sources now enumerate through the extension registry; built-ins still use the hardcoded source factory. | Route production capture through connector descriptors for both built-in and external sources. |
| External ABI is new and low-level | Third-party packages can run as managed HTTP services, and `alvum extensions scaffold` now creates a starter Node package. There is not yet a published SDK/helper library. | Add conformance tests and helper libraries around `alvum.extension.json`. |
| Built-in manifest metadata is minimal | UI and docs can inspect core capture/processor/connector IDs, but not all settings, permissions, and storage paths yet. | Enrich built-in descriptors with settings schemas, permissions, storage paths, and capabilities. |
| Untyped config maps | Connector settings are flexible but not self-validating. | Typed per-connector config with validation errors surfaced in CLI, app, and `pipeline.events`. |
| Free-form metadata | `Observation.kind`, `source`, and metadata shape are manually coordinated strings. | Versioned source and observation conventions per connector. |
| Template is minimal | Contributors can scaffold a runnable starter package, but still need deeper tutorials for real captures/processors. | Add a session-connector tutorial and richer examples. |
| No conformance tests | There is no single command that says a connector is valid. | Test harness covering config defaults, ref gathering, processor output, expected-source warnings, and resume fingerprint behavior. |
| No privacy checklist | Connectors may handle highly sensitive raw data with no required declaration. | Every connector documents raw data written, retention class, permissions, and whether data leaves the device. |

### Target Developer Platform Shape

The implemented first milestone is a manifest-backed external package runtime:

```
alvum.extension.json
  -> capture components
  -> processor components
  -> analysis lenses
  -> connector-owned route matrix
  -> managed localhost HTTP service
```

That gives non-workspace contributors a way to build against stable JSON/HTTP
contracts without loading third-party code into the Rust process. Built-in
audio, screen, and session components now also expose read-only virtual
manifests through the same descriptor shape, so extension authors can route
against core component IDs such as `alvum.audio/audio-mic` without owning the
core capture lifecycle. Remaining platform work is to use that descriptor model
as the source of truth for:

- CLI connector construction.
- Capture daemon source construction.
- Config validation.
- Electron settings UI.
- Connector docs.
- Expected source inventory.
- Test harness discovery.
- Extension package scaffolds, examples, and conformance checks.

### Connector Contribution Checklist

A contributor-facing guide should make this the required checklist:

1. Pick connector class: session importer, raw capture source, processor-only,
   or combined capture-plus-processor connector.
2. Declare stable source IDs. Do not rename source IDs after release without a
   migration.
3. Document raw storage under `~/.alvum/capture/YYYY-MM-DD/<source>/`.
4. Implement `gather_data_refs()` with date-window filtering.
5. Implement `expected_sources()` so missing modality warnings are meaningful.
6. Implement processors that produce bounded, source-grounded `Observation`s.
7. Emit metadata needed for attribution, evidence, and privacy review.
8. Add config defaults, schema validation, and migration behavior.
9. Add parser/processor tests plus one pipeline smoke fixture.
10. Verify `alvum extract --capture-dir <date>` and `alvum tail --filter input_filter`.

## Gap 2: Provider Customization and Extension

### What Exists

`LlmProvider` currently supports:

- `complete(system, user_message)`
- `complete_with_image(system, user_message, image_path)` as an optional method
- `name()`

Implemented providers are:

| Provider | Current transport | Notes |
| --- | --- | --- |
| `claude-cli` | `claude -p` subprocess | Good local account path. |
| `codex-cli` | `codex exec` subprocess | Good Codex account path. |
| `anthropic-api` | HTTPS API | Supports image calls. |
| `bedrock` | AWS Bedrock SDK/API | Environment and AWS config driven. |
| `ollama` | Local HTTP API | Supports local text and image models. |
| `auto` | Fallback chain | Tries providers in a fixed order and classifies failures. |

The CLI exposes provider list, test, and set-active commands. Electron surfaces
provider status and lets the user choose a provider.

### Provider Gaps

| Gap | Current impact | Target |
| --- | --- | --- |
| Hardcoded factory | Adding a provider requires editing `alvum-pipeline/src/llm.rs`, `alvum-cli`, and app assumptions. | Provider registry with descriptors and constructors. |
| Config drift | `[pipeline].provider` can be written by UI, while briefing scripts and app runs still pass `--provider auto`. | One authoritative provider resolution path used by CLI, script, and app. |
| No capabilities | Text-only providers can be used where vision or structured JSON is expected. | Capabilities include text, vision, structured JSON reliability, max context, streaming, local/cloud, cost tier, and privacy tier. |
| No per-stage routing | Screen vision, threading, clustering, domain extraction, day briefing, and knowledge extraction share one provider. | Stage-specific model policy with fallback. |
| No provider config schema | Base URLs, env var names, API keys, timeouts, and default models are provider-specific code. | `[providers.<id>]` TOML schema validated by provider descriptor. |
| No conformance tests | Provider additions can parse text but fail JSON-heavy stages. | Provider test suite with text, strict JSON, retry, long-context, and image cases. |
| Naming drift | Spec says `ModelProvider`; code says `LlmProvider`. | Rename deliberately or document `LlmProvider` as the current implementation of the spec model-provider concept. |
| Weak UX contract | "Use provider" in the app does not guarantee scheduled briefings use it. | Provider picker must describe exact scope: active default, stage override, fallback, or one-run override. |

### Target Provider Model

The provider platform should be descriptor-backed:

```rust
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub display_name: &'static str,
    pub default_model: &'static str,
    pub capabilities: ModelCapabilities,
    pub settings: &'static [SettingDescriptor],
    pub availability: fn(&ProviderConfig) -> ProviderAvailability,
    pub constructor: fn(&ProviderConfig, Option<&str>) -> Result<Box<dyn LlmProvider>>,
}

pub struct ModelCapabilities {
    pub text: bool,
    pub vision: bool,
    pub structured_json: bool,
    pub max_context_tokens: Option<u32>,
    pub local: bool,
    pub cost_tier: CostTier,
    pub privacy_tier: PrivacyTier,
}
```

The config should separate provider definitions from stage routing:

```toml
[pipeline]
provider = "auto"
model = "claude-sonnet-4-6"

[providers.ollama]
base_url = "http://localhost:11434"
default_model = "llama3.2"

[providers.anthropic-api]
api_key_env = "ANTHROPIC_API_KEY"
default_model = "claude-sonnet-4-6"

[models]
thread = "claude-cli:claude-sonnet-4-6"
cluster = "claude-cli:claude-sonnet-4-6"
cluster_edges = "claude-cli:claude-sonnet-4-6"
domain = "claude-cli:claude-sonnet-4-6"
domain_edges = "claude-cli:claude-sonnet-4-6"
day = "claude-cli:claude-sonnet-4-6"
knowledge = "claude-cli:claude-sonnet-4-6"
screen_description = "ollama:llama3.2-vision"
```

The current `auto` fallback should remain, but it should become a policy rather
than a special provider implementation:

```
resolve stage model
  -> filter providers by required capabilities
  -> try configured provider
  -> classify auth / usage / transient failures
  -> fall back only when policy allows
  -> emit pipeline event for every switch
```

### Provider Extension Checklist

A developer-facing provider guide should require:

1. Add provider descriptor and aliases.
2. Declare capabilities, privacy tier, cost tier, and default model.
3. Define config keys and validation.
4. Implement text completion.
5. Implement image completion only if capability says `vision = true`.
6. Implement provider availability probe.
7. Add provider tests for basic text, strict JSON, retry behavior, and image if
   supported.
8. Add one stage-routing fixture to prove it works in a real briefing stage.
9. Document required environment variables and local setup.
10. Verify `alvum providers list`, `alvum providers test --provider <id>`, and
    an `alvum extract` run.

## Gap 3: Briefing and Decision Graph Methodology

### Current Implemented Methodology

The implemented pipeline is source-grounded and hierarchical:

| Level | Name | Current artifact | Method |
| --- | --- | --- | --- |
| L0 | Observations | `transcript.jsonl` | Connector processors produce timestamped observations. |
| L1 | Time blocks | in-memory, checkpointed through later stages | Deterministic 5-minute windows. |
| L2 | Threads | `threads.json` | LLM groups blocks into context threads and scores relevance. |
| L3 | Clusters | `tree/L3-clusters.jsonl` | LLM groups relevant threads into higher-level clusters. |
| L3 edges | Cluster edges | `tree/L3-edges.jsonl` | LLM correlates clusters and passes those relationships into L4 decision extraction. |
| L4 | Domain decisions | `tree/L4-domains.jsonl`, `decisions.jsonl` | LLM extracts decisions into enabled synthesis profile domains. |
| L4 edges | Decision edges | `tree/L4-edges.jsonl` | LLM correlates decisions with causal and alignment relations. |
| L5 | Day briefing | `tree/L5-day.json`, `briefing.md` | LLM writes gap-narrative briefing from domains, decisions, edges, and knowledge. |
| Profile | Synthesis profile | `synthesis-profile.snapshot.json`, `tree/artifacts/L5-briefing-source.json` | User-managed domains, intentions, tracked interests, writing preferences, and advanced guidance are injected as bounded context and snapshotted for traceability. |
| Knowledge | Corpus update | `~/.alvum/generated/knowledge/{entities,patterns,facts}.jsonl`, mirrored under the output dir | Best-effort extraction after briefing artifacts are written. |

The graph authority is:

```
tree/L4-edges.jsonl     # full edge metadata
decisions.jsonl         # decision nodes plus compatibility causes/effects projection
briefing.md             # narrative output citing decision IDs
```

`Decision.causes` and `Decision.effects` are not the full graph. They are a
lossy projection from `Edge` so existing consumers can render simple incoming
and outgoing links.

### Methodological Rules Already In Code

The code and prompts already encode these rules:

- Raw connector output is untrusted data. Prompts tell models not to treat
  observation text as instructions.
- Time blocking is deterministic before LLM reasoning starts.
- Every observation should be assigned to a thread unless it is filtered earlier.
- Thread relevance is scored before expensive higher-level extraction.
- Decisions are separated into spoken, revealed, and explained sources.
- Domains come from the enabled synthesis profile domains. The default profile
  is Career, Health, and Family; custom profiles may replace or extend those
  lanes.
- Decision edges use explicit relation names such as direct causation, resource
  competition, emotional influence, precedent, accumulation, constraint,
  alignment break, and alignment honor.
- Forward references and dangling edge IDs are filtered out.
- Briefings are not daily summaries. The day prompt is written around gaps
  between stated or explained intent and revealed behavior.
- The briefing must include uncertainty and cite decision IDs.
- Processor failures should degrade the briefing rather than abort the whole
  day unless no observations remain.
- Knowledge extraction is best-effort and should not prevent briefing delivery.

### Methodology Gaps

| Gap | Current impact | Target |
| --- | --- | --- |
| Methodology is hidden in prompts | Developers cannot reason about extraction changes without reading prompt strings. | Public briefing methodology doc with stage goals, schemas, invariants, and examples. |
| Older docs describe old pipeline | Specs mention `distill.rs`, `causal.rs`, and broader Align stages that no longer match current code. | Mark current tree pipeline as implemented source of truth and older Prepare/Fuse/Align as future target. |
| Cross-day graph not implemented | L3 and L4 edges are consumed within one run, but no persistent graph currently links decisions across days. | Define persistent cross-day graph storage separately under `generated/decisions/`. |
| L2 correlation not orchestrated | Thread correlation helper exists but is not in the main extraction flow. | Either wire it in or document it as unused. |
| Validation is mostly structural | Briefing validation checks presence, citations, and sections but not semantic quality. | Regression corpus with expected decisions, edges, uncertainty, and suppression behavior. |
| Knowledge lifecycle needs clearer docs | Extraction updates the global generated knowledge corpus and mirrors it into each run, but older specs still imply only per-run storage. | Document global ownership, per-run snapshots, and merge behavior. |
| Knowledge IDs are fragile | Prompts ask for knowledge IDs, while formatted context may not expose every ID clearly. | Make knowledge formatting and prompt citation rules consistent. |
| Source confidence lacks public semantics | Multi-source evidence and confidence fields exist but are not explained. | Define how confidence should be derived from source count, modality agreement, and ambiguity. |
| Future alignment engine is conflated with current briefing | Current system has single-day gap narratives, not full intention/outcome tracking. | Separate "implemented briefing graph" from "future alignment engine" in docs. |

### Target Briefing Methodology Doc

The missing methodology doc should define:

1. Inputs:
   - `DataRef` source rules.
   - `Observation` kind and metadata conventions.
   - Evidence and media reference conventions.
2. Stage responsibilities:
   - L1 time blocking is deterministic.
   - L2 threading owns context continuity and relevance.
   - L3 clustering owns higher-level topic compression.
   - L4 domain extraction owns decisions and attribution.
   - L4 edge correlation owns causal and alignment relations.
   - L5 day briefing owns the gap narrative.
   - Knowledge extraction owns persistent context, not same-day truth.
3. Required invariants:
   - No forward edges.
   - No dangling IDs.
   - Every briefing claim should cite a decision ID or be placed in uncertainty.
   - Revealed decisions need behavioral evidence, not just model inference.
   - Spoken decisions need attribution and source context.
   - Cross-domain edges need a mechanism, not just co-occurrence.
4. Output authority:
   - `tree/L4-edges.jsonl` is the rich graph.
   - `decisions.jsonl` is the node snapshot plus compatibility projection.
   - `briefing.md` is a rendered narrative, not canonical graph data.
   - The menubar Synthesis calendar reads those artifacts through
     `alvum:decision-graph-date` and renders them as a per-day graph; legacy
     days without `tree/L4-edges.jsonl` derive display edges from the
     projected `causes` / `effects` fields.
5. Debugging:
   - Use `alvum tail --follow --filter stage`.
   - Use `alvum tail --follow --filter llm_call`.
   - Use `alvum tail --follow --filter input_filter`.
   - Inspect `pipeline.events` for inventory, parse failures, retries, and
     warnings.
6. Evaluation:
   - Golden transcript fixtures.
   - Expected decision nodes.
   - Expected edge relation vocabulary.
   - Required uncertainty behavior.
   - Snapshot tests for rendered briefing sections.
   - Provider conformance tests for JSON-heavy prompts.

## Source-Of-Truth Drift To Resolve

These are documentation or architecture mismatches that should be fixed before
Alvum is presented as a contributor platform:

| Drift | Current risk | Resolution |
| --- | --- | --- |
| System design architecture lists crates that no longer match the workspace. | Contributors start from stale crate names and miss the tree pipeline. | Add a current architecture index or amend the architecture overview. |
| Progress matrix says app shell and several decision fields are absent. | It underreports current implementation state. | Mark it as historical or update it after this audit. |
| Storage spec describes global `generated/decisions` and `generated/knowledge`, while current briefing output is per-run. | Developers may write integrations to the wrong path. | Document current per-run outputs and future persistent graph separately. |
| Provider UI writes config that scheduled/app briefings can bypass with `--provider auto`. | User thinks they customized provider but production runs a different policy. | Make config authoritative or explicitly label provider selection scope. |
| `Connector::capture_sources()` and `alvum capture` source creation are split. | Connector abstraction is not actually end-to-end. | Build capture source enumeration from connector descriptors. |
| CLI exposes some legacy extraction flags that are ignored by the current command path. | Developers and users can pass flags that appear to work but do nothing. | Remove, wire, or document deprecated flags. |
| Observability constants still include older stage names in some places. | Logs and docs can disagree with actual tree stages. | Align stage constants and docs with `cluster`, `domain`, edge correlation, and `day`. |

## Recommended Documentation Set

This audit should become the bridge to three durable docs:

1. `docs/superpowers/specs/README.md`
   - Index specs, plans, current runtime docs, and stale/historical plans.
   - Tell contributors where to start for architecture, operations, connectors,
     providers, and briefing methodology.
2. `docs/superpowers/specs/2026-04-28-component-extension-runtime.md`
   - External package runtime source of truth.
   - Manifest schema, component IDs, route matrix, HTTP lifecycle, context/LLM
     broker, commands, and v1 limitations.
3. `docs/superpowers/specs/2026-04-28-briefing-decision-methodology.md`
   - Current L0-L5 tree pipeline.
   - Decision and edge authority.
   - Briefing quality rules.
   - Evaluation and debugging method.

This file can remain as the gap audit. The follow-up docs should be written as
operational references, not broad vision documents.

## Priority Order

The correct order is:

1. Document current architecture and tree methodology.
2. Add provider resolution correctness so app, CLI, and scheduled briefings use
   the same provider policy.
3. Add provider descriptors and capability checks.
4. Use the built-in descriptor catalog as the construction source for core
   connectors and capture sources.
5. Route production capture through connector descriptors for built-ins too.
6. Add connector, extension, and provider conformance tests.
7. Add richer examples, conformance tests, and helper SDKs for external packages.
8. Split future cross-day alignment-engine docs from the implemented per-day
   decision graph surface.

This keeps the platform honest. It documents and hardens the system that exists
while making third-party extension points explicit about their v1 trust and
sandbox limits.
