//! extract_and_pipeline — the full observation → decision pipeline as a library function.
//!
//! Takes a set of connectors, runs their processors, does episodic alignment,
//! extracts decisions, links causally, generates briefing, and updates the
//! knowledge corpus. Returns the complete extraction result.

use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::decision::ExtractionResult;
use alvum_core::observation::Observation;
use alvum_core::pipeline_events::{self as events, Event, StageTimer};
use alvum_core::storage;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use crate::llm::LlmProvider;
use crate::processor_runner::{
    pairs_from_connectors, read_transcript_meta, run_processors_with_retry,
    write_transcript_meta, TranscriptMeta,
};

/// How many total attempts (initial + retries) each processor gets before we
/// give up and record a failure. 3 = one real try + two retries.
const MAX_PROCESSOR_ATTEMPTS: u32 = 3;

/// Max formatted-prompt bytes per threading LLM chunk. Claude's context is
/// ~800KB chars (~200K tokens) but we leave generous headroom for the
/// system prompt, knowledge corpus, and response. A single chunk ≤ 100KB
/// keeps every call well within the window.
const THREADING_CHUNK_BUDGET: usize = 100_000;

pub struct ExtractConfig {
    pub capture_dir: PathBuf,
    pub output_dir: PathBuf,
    pub relevance_threshold: f32,
    /// Resume from any per-stage checkpoint files that already exist in
    /// output_dir. A previously-successful stage's file is loaded from
    /// disk and the LLM call skipped. Idempotent on a fresh output_dir.
    pub resume: bool,
    /// Re-process every DataRef even if it appears in `output_dir/processed.jsonl`.
    /// Default `false` — re-runs over the same capture dir skip work.
    pub no_skip_processed: bool,
}

pub struct ExtractOutput {
    pub observations: Vec<Observation>,
    pub threading: alvum_episode::types::ThreadingResult,
    pub result: ExtractionResult,
}

/// Run the full extraction pipeline for a set of connectors.
pub async fn extract_and_pipeline(
    connectors: Vec<Box<dyn Connector>>,
    provider: Arc<dyn LlmProvider>,
    config: ExtractConfig,
) -> Result<ExtractOutput> {
    // Reset the progress IPC file at the very top of the run so the tray
    // popover never displays stale stage/percent from the previous run.
    // The richer event channel resets on the same heartbeat so any open
    // tail/popover never confuses the prior tail with the new run.
    crate::progress::init();
    events::init();
    std::fs::create_dir_all(&config.output_dir)?;

    let transcript_path = config.output_dir.join("transcript.jsonl");

    let current_connector_names: Vec<String> =
        connectors.iter().map(|c| c.name().to_string()).collect();

    let resume_ok = config.resume
        && transcript_path.exists()
        && transcript_fingerprint_matches(&config.output_dir, &current_connector_names)
            .unwrap_or(false);

    // Distinguishes "the pipeline genuinely had nothing to work with"
    // from "input arrived but processors filtered it down to zero
    // observations." The first should abort the run, the second
    // should warn-and-proceed — empty downstream stages handle empty
    // input gracefully and produce a brief "nothing to report"
    // briefing. Tracked outside the branch because the resume path
    // skips the gather entirely.
    let mut total_refs_seen: usize = 0;

    // Stage 1-2: gather observations (from connectors or from prior transcript)
    let all_observations: Vec<Observation> = if resume_ok {
        // If the prior run recorded processor failures, warn so the user
        // knows the reused briefing is partial. We reuse the transcript
        // as-is rather than retrying — the user is explicitly opting
        // into the cached run by passing --resume.
        if let Ok(Some(meta)) = read_transcript_meta(&config.output_dir) {
            if !meta.failed_processors.is_empty() {
                let summary: Vec<String> = meta
                    .failed_processors
                    .iter()
                    .map(|f| format!("{}/{}", f.connector, f.processor))
                    .collect();
                warn!(
                    failed = ?summary,
                    "resume: transcript records prior processor failures; briefing will be missing that data"
                );
            }
        }
        info!(
            path = %transcript_path.display(),
            "resume: transcript fingerprint matches, reusing"
        );
        storage::read_jsonl(&transcript_path)?
    } else {
        if config.resume && transcript_path.exists() {
            warn!(path = %transcript_path.display(), "resume: transcript fingerprint mismatch, re-gathering observations");
            // A fingerprint mismatch means the transcript is stale, so every
            // downstream checkpoint derived from it is stale too. Remove them so
            // the later resume guards don't reload yesterday's threads/decisions/
            // briefing against today's observations.
            clear_downstream_checkpoints(&config.output_dir);
        }

        // Each connector enumerates its own DataRefs (filesystem walk, JSONL
        // index, session-file enumeration). The pipeline merges them and
        // dispatches by `Processor::handles()`.
        let gather_timer = StageTimer::start(events::STAGE_GATHER);
        crate::progress::report(crate::progress::STAGE_GATHER, 0, connectors.len());
        let mut all_refs: Vec<DataRef> = Vec::new();
        // Per-source ref counts for the inventory event. A connector
        // can emit refs spanning several sources (e.g. audio → mic +
        // system + wearable); we tally each source as observed so the
        // popover surfaces silent modalities individually.
        let mut per_source_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (i, c) in connectors.iter().enumerate() {
            match c.gather_data_refs(&config.capture_dir) {
                Ok(refs) => {
                    for r in &refs {
                        *per_source_counts
                            .entry((c.name().to_string(), r.source.clone()))
                            .or_insert(0) += 1;
                    }
                    all_refs.extend(refs);
                }
                Err(e) => {
                    warn!(connector = %c.name(), error = %e, "gather_data_refs failed; skipping connector");
                    events::emit(Event::Error {
                        source: format!("connector/{}", c.name()),
                        message: format!("{e:#}"),
                    });
                }
            }
            crate::progress::report(crate::progress::STAGE_GATHER, i + 1, connectors.len());
        }
        // Emit one inventory event per (connector, source). The set is
        // the union of (a) sources the connector actually produced refs
        // for and (b) sources it declared via `expected_sources()`. The
        // declaration list is what makes silent modalities visible —
        // without it, a connector returning an empty Vec would simply
        // not appear, and the operator wouldn't know whether the
        // modality was disabled, broken, or just had nothing to scan.
        for c in &connectors {
            let mut sources_seen: std::collections::BTreeSet<String> = per_source_counts
                .keys()
                .filter_map(|(conn, source)| (conn == c.name()).then(|| source.clone()))
                .collect();
            for s in c.expected_sources() {
                sources_seen.insert(s.to_string());
            }
            if sources_seen.is_empty() {
                // Opportunistic connector with no expected sources and
                // no observed refs. Emit a single zero-count tuple under
                // the connector name so the operator at least sees it
                // ran and produced nothing.
                events::emit(Event::InputInventory {
                    connector: c.name().to_string(),
                    source: c.name().to_string(),
                    ref_count: 0,
                });
                continue;
            }
            for source in sources_seen {
                let count = per_source_counts
                    .get(&(c.name().to_string(), source.clone()))
                    .copied()
                    .unwrap_or(0);
                events::emit(Event::InputInventory {
                    connector: c.name().to_string(),
                    source: source.clone(),
                    ref_count: count,
                });
                if count == 0 && c.expected_sources().iter().any(|x| *x == source) {
                    events::emit(Event::Warning {
                        source: format!("connector/{}", c.name()),
                        message: format!(
                            "expected source `{source}` produced 0 refs (modality silent)"
                        ),
                    });
                }
            }
        }
        total_refs_seen = all_refs.len();
        gather_timer.finish_ok(serde_json::json!({
            "ref_count": all_refs.len(),
            "connector_count": connectors.len(),
        }));

        // Filter against the idempotency sidecar so re-runs over the same
        // capture dir skip already-processed refs. The sidecar lives next
        // to transcript.jsonl in the output dir.
        let processed_path = config.output_dir.join("processed.jsonl");
        let mut processed = crate::processed_index::ProcessedIndex::load(processed_path.clone())
            .with_context(|| format!("failed to load {}", processed_path.display()))?;
        let total_refs = all_refs.len();
        let filtered_refs: Vec<DataRef> = if config.no_skip_processed {
            all_refs
        } else {
            all_refs.into_iter().filter(|dr| !processed.contains(dr)).collect()
        };
        let skipped = total_refs.saturating_sub(filtered_refs.len());
        if skipped > 0 {
            info!(skipped, "skipping refs already recorded in processed.jsonl");
        }

        // Snapshot the to-be-processed refs so we can record them after the
        // run completes. We record only the refs that were actually fed to
        // processors; partial runs (some processors fail) still record the
        // refs because the failure is per-processor, not per-ref.
        let refs_to_record = filtered_refs.clone();

        // Parallel fan-out of (connector, processor) pairs. Each processor
        // gets up to MAX_PROCESSOR_ATTEMPTS tries with 500ms / 1s linear
        // backoff. Exhausted failures are collected into the sidecar so
        // they're visible on the next --resume run.
        let pairs = pairs_from_connectors(&connectors);

        // Pre-compute total work units = sum of file counts each
        // processor will see. Each processor calls tick_stage after
        // every file it processes; the shared atomic counter in
        // alvum_core::progress aggregates parallel ticks into a single
        // monotonic stream so the bar reflects real per-file progress
        // even with Whisper + vision running concurrently.
        let total_process_units: usize = pairs
            .iter()
            .map(|(_, p)| {
                let h = p.handles();
                filtered_refs
                    .iter()
                    .filter(|dr| h.iter().any(|x| x == &dr.source))
                    .count()
            })
            .sum();
        alvum_core::progress::set_stage_total(
            alvum_core::progress::STAGE_PROCESS,
            total_process_units.max(1),
        );

        let process_timer = StageTimer::start(events::STAGE_PROCESS);
        let outcome = run_processors_with_retry(
            pairs,
            filtered_refs,
            &config.capture_dir,
            MAX_PROCESSOR_ATTEMPTS,
            vec![Duration::from_millis(500), Duration::from_secs(1)],
        )
        .await;

        // Force the bar to 100 % at stage end. tick_stage emits one
        // event per file, but a few may be claude-code/codex one-shots
        // that don't tick (their processors return without per-file
        // iteration), or transient failures that skipped the tick — so
        // we top up here to avoid a permanently-stuck 90-something %.
        alvum_core::progress::report(
            alvum_core::progress::STAGE_PROCESS,
            total_process_units.max(1),
            total_process_units.max(1),
        );

        for dr in &refs_to_record {
            if let Err(e) = processed.record(dr) {
                warn!(path = %dr.path, error = %e, "failed to record processed ref");
            }
        }

        for f in &outcome.failures {
            warn!(
                connector = %f.connector,
                processor = %f.processor,
                attempts = f.attempts,
                error = %f.last_error,
                "processor failed all retries"
            );
            events::emit(Event::Error {
                source: format!("processor/{}/{}", f.connector, f.processor),
                message: format!("failed all retries: {}", f.last_error),
            });
        }
        process_timer.finish_ok(serde_json::json!({
            "observation_count": outcome.observations.len(),
            "processor_failures": outcome.failures.len(),
            "ref_count": total_process_units,
        }));

        // Atomic write — survives crash mid-write.
        write_jsonl_atomic(&transcript_path, &outcome.observations)?;
        write_transcript_meta(
            &config.output_dir,
            &TranscriptMeta {
                connectors: current_connector_names.clone(),
                failed_processors: outcome.failures,
            },
        )?;
        info!(
            path = %transcript_path.display(),
            count = outcome.observations.len(),
            "saved transcript"
        );
        outcome.observations
    };

    if all_observations.is_empty() {
        // Distinguish two cases:
        //   1. No connector produced any refs at all (`total_refs_seen
        //      == 0` AND we ran a fresh gather). Abort — there's nothing
        //      meaningful for downstream stages to chew on.
        //   2. Refs existed but processors filtered everything (e.g.
        //      Whisper rejected every segment as non-speech). Warn and
        //      proceed; downstream stages handle empty input and the
        //      briefing reflects "no decisions found".
        // On a resume path `total_refs_seen` is 0 because we skipped
        // gather; an empty observations vector THERE means the cached
        // transcript itself is empty, also case 1.
        if total_refs_seen == 0 {
            events::emit(Event::Error {
                source: "pipeline".into(),
                message: "no observations and no input refs — modality is fully silent".into(),
            });
            anyhow::bail!("no observations produced by any connector");
        } else {
            warn!(
                total_refs_seen,
                "all observations were filtered out; proceeding with empty pipeline"
            );
            events::emit(Event::Warning {
                source: "pipeline".into(),
                message: format!(
                    "{total_refs_seen} input refs produced 0 observations after filtering — proceeding with empty pipeline"
                ),
            });
        }
    }

    // Load knowledge corpus for context-aware threading (and for later merge).
    let knowledge_dir = config.output_dir.join("knowledge");
    let corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

    // Stage 4: episodic alignment — chunked. A full day of observations
    // formatted for threading can easily exceed Claude's context window,
    // so we split the time blocks into byte-budgeted chunks, run one LLM
    // call per chunk, and persist each chunk's threads to its own
    // `threads-chunk-{N}.json`. A mid-run crash therefore only loses the
    // in-flight chunk; prior chunks resume from disk on the next run.
    let threads_path = config.output_dir.join("threads.json");
    let threading: alvum_episode::types::ThreadingResult = if config.resume && threads_path.exists() {
        info!(
            path = %threads_path.display(),
            "resume: loading final threads from disk (skipping threading LLM calls)"
        );
        let json = std::fs::read_to_string(&threads_path)?;
        serde_json::from_str(&json).context("failed to parse existing threads.json")?
    } else {
        let time_blocks = alvum_episode::time_block::assemble_time_blocks(
            &all_observations,
            chrono::Duration::minutes(5),
        );
        let chunks = alvum_episode::time_block::chunk_time_blocks_by_budget(
            &time_blocks,
            THREADING_CHUNK_BUDGET,
        );
        info!(
            blocks = time_blocks.len(),
            chunks = chunks.len(),
            budget_bytes = THREADING_CHUNK_BUDGET,
            "running episodic alignment (chunked)"
        );

        let thread_timer = StageTimer::start(events::STAGE_THREAD);
        let mut all_threads: Vec<alvum_episode::types::ContextThread> = Vec::new();
        crate::progress::report(crate::progress::STAGE_THREAD, 0, chunks.len());
        for (i, chunk_blocks) in chunks.iter().enumerate() {
            let chunk_path = config
                .output_dir
                .join(format!("threads-chunk-{i}.json"));
            let chunk_threads: Vec<alvum_episode::types::ContextThread> =
                if config.resume && chunk_path.exists() {
                    info!(chunk = i, path = %chunk_path.display(), "resume: loading chunk threads from disk");
                    let s = std::fs::read_to_string(&chunk_path)?;
                    serde_json::from_str(&s).with_context(|| {
                        format!("failed to parse existing {}", chunk_path.display())
                    })?
                } else {
                    info!(
                        chunk = i,
                        of = chunks.len(),
                        blocks = chunk_blocks.len(),
                        "threading chunk"
                    );
                    let chunk_call_site = format!("thread/chunk_{i}");
                    let mut threads = alvum_episode::threading::identify_threads(
                        provider.as_ref(),
                        chunk_blocks,
                        Some(&corpus),
                        &chunk_call_site,
                    )
                    .await
                    .with_context(|| format!("threading chunk {i} failed"))?;
                    // Namespace per-chunk thread IDs so merging across chunks
                    // never collides. Chunk index prefix also makes it obvious
                    // in downstream debugging which chunk produced which thread.
                    for t in &mut threads {
                        t.id = format!("c{i}_{}", t.id);
                    }
                    write_atomic(&chunk_path, serde_json::to_string_pretty(&threads)?.as_bytes())?;
                    threads
                };
            all_threads.extend(chunk_threads);
            crate::progress::report(crate::progress::STAGE_THREAD, i + 1, chunks.len());
        }

        // Assemble the final ThreadingResult from the merged chunks.
        // Source + timestamp aggregation is straightforward — threads are
        // already sorted by chunk order, which respects time order (the
        // chunker preserves it).
        let mut sources: Vec<String> =
            all_observations.iter().map(|o| o.source.clone()).collect();
        sources.sort();
        sources.dedup();
        let start = time_blocks
            .first()
            .map(|b| b.start)
            .unwrap_or_else(chrono::Utc::now);
        let end = time_blocks
            .last()
            .map(|b| b.end)
            .unwrap_or_else(chrono::Utc::now);

        let threading = alvum_episode::types::ThreadingResult {
            start,
            end,
            time_blocks,
            threads: all_threads,
            observation_count: all_observations.len(),
            source_count: sources.len(),
        };

        write_atomic(
            &threads_path,
            serde_json::to_string_pretty(&threading)?.as_bytes(),
        )?;
        thread_timer.finish_ok(serde_json::json!({
            "chunk_count": chunks.len(),
            "thread_count": threading.threads.len(),
            "observation_count": threading.observation_count,
        }));
        threading
    };

    // 5. Filter relevant threads
    let relevant: Vec<&alvum_episode::types::ContextThread> = threading
        .threads
        .iter()
        .filter(|t| t.is_relevant(config.relevance_threshold))
        .collect();

    let relevant_observations: Vec<Observation> = relevant
        .iter()
        .flat_map(|t| t.observations.clone())
        .collect();

    // Intermediate checkpoint paths. Each LLM stage writes its own output so
    // a transient failure doesn't wipe upstream work. With `resume`, existing
    // outputs short-circuit the LLM call and the result loads from disk.
    let decisions_raw_path = config.output_dir.join("decisions.raw.jsonl");
    let decisions_path = config.output_dir.join("decisions.jsonl");
    let briefing_path = config.output_dir.join("briefing.md");
    let extraction_path = config.output_dir.join("extraction.json");

    // Stages 6-7: distill + causal (resumable at two granularities)
    //
    //   state on disk                      | action
    //   -----------------------------------+----------------------------------
    //   decisions.jsonl exists             | skip distill + causal entirely
    //   decisions.raw.jsonl exists         | skip distill, re-run causal
    //   (neither)                          | full distill → causal pipeline
    //
    // A successful causal always leaves decisions.jsonl and removes the raw.
    let decisions: Vec<alvum_core::decision::Decision>
        = if config.resume && decisions_path.exists() {
            info!(
                path = %decisions_path.display(),
                "resume: loading decisions.jsonl (post-causal) from disk"
            );
            storage::read_jsonl(&decisions_path)?
        } else if config.resume && decisions_raw_path.exists() {
            info!(
                path = %decisions_raw_path.display(),
                "resume: loading decisions.raw.jsonl; will re-run causal"
            );
            let mut d: Vec<alvum_core::decision::Decision> =
                storage::read_jsonl(&decisions_raw_path)?;
            if !d.is_empty() {
                let causal_timer = StageTimer::start(events::STAGE_CAUSAL);
                crate::progress::report(crate::progress::STAGE_CAUSAL, 0, 1);
                crate::causal::link_decisions(provider.as_ref(), &mut d).await?;
                crate::progress::report(crate::progress::STAGE_CAUSAL, 1, 1);
                causal_timer.finish_ok(serde_json::json!({
                    "decisions": d.len(),
                }));
            }
            write_jsonl_atomic(&decisions_path, &d)?;
            let _ = std::fs::remove_file(&decisions_raw_path);
            info!(
                path = %decisions_path.display(),
                count = d.len(),
                "checkpoint: decisions post-causal"
            );
            d
        } else {
            info!(count = relevant_observations.len(), "extracting decisions");
            let distill_timer = StageTimer::start(events::STAGE_DISTILL);
            crate::progress::report(crate::progress::STAGE_DISTILL, 0, 1);
            let mut d =
                crate::distill::extract_decisions(provider.as_ref(), &relevant_observations).await?;
            crate::progress::report(crate::progress::STAGE_DISTILL, 1, 1);
            distill_timer.finish_ok(serde_json::json!({
                "observations_in": relevant_observations.len(),
                "decisions_out": d.len(),
            }));
            write_jsonl_atomic(&decisions_raw_path, &d)?;
            info!(
                path = %decisions_raw_path.display(),
                count = d.len(),
                "checkpoint: decisions post-distill"
            );
            if !d.is_empty() {
                let causal_timer = StageTimer::start(events::STAGE_CAUSAL);
                crate::progress::report(crate::progress::STAGE_CAUSAL, 0, 1);
                crate::causal::link_decisions(provider.as_ref(), &mut d).await?;
                crate::progress::report(crate::progress::STAGE_CAUSAL, 1, 1);
                causal_timer.finish_ok(serde_json::json!({
                    "decisions": d.len(),
                }));
            }
            write_jsonl_atomic(&decisions_path, &d)?;
            let _ = std::fs::remove_file(&decisions_raw_path);
            info!(
                path = %decisions_path.display(),
                count = d.len(),
                "checkpoint: decisions post-causal"
            );
            d
        };

    // Stage 8: briefing (resumable)
    let briefing: String = if config.resume && briefing_path.exists() {
        info!(
            path = %briefing_path.display(),
            "resume: loading briefing.md from disk"
        );
        std::fs::read_to_string(&briefing_path)?
    } else {
        let brief_timer = StageTimer::start(events::STAGE_BRIEF);
        crate::progress::report(crate::progress::STAGE_BRIEF, 0, 1);
        let b = if !decisions.is_empty() {
            crate::briefing::generate_briefing(provider.as_ref(), &decisions).await?
        } else {
            String::from("No decisions found.")
        };
        crate::progress::report(crate::progress::STAGE_BRIEF, 1, 1);
        write_atomic(&briefing_path, b.as_bytes())?;
        info!(path = %briefing_path.display(), "checkpoint: briefing.md");
        brief_timer.finish_ok(serde_json::json!({
            "decision_count": decisions.len(),
            "briefing_chars": b.len(),
        }));
        b
    };

    // 9. Aggregate extraction result
    let result = ExtractionResult {
        session_id: "cross-source".into(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        decisions: decisions.clone(),
        briefing: briefing.clone(),
    };
    write_atomic(
        &extraction_path,
        serde_json::to_string_pretty(&result)?.as_bytes(),
    )?;

    // Stage 10: knowledge extraction — best-effort + resumable.
    // If resume is on and knowledge already has been extracted for this run
    // (entities.jsonl exists), skip the LLM call. Otherwise run it; pipeline
    // doesn't fail if this stage errors.
    let knowledge_entities_path = knowledge_dir.join("entities.jsonl");
    if config.resume && knowledge_entities_path.exists() {
        info!(
            path = %knowledge_entities_path.display(),
            "resume: knowledge already extracted, skipping"
        );
    } else {
        let knowledge_timer = StageTimer::start(events::STAGE_KNOWLEDGE);
        match alvum_knowledge::extract::extract_knowledge(
            provider.as_ref(),
            &relevant_observations,
            &corpus,
        )
        .await
        {
            Ok(new_knowledge) => {
                let entity_count = new_knowledge.entities.len();
                let pattern_count = new_knowledge.patterns.len();
                let fact_count = new_knowledge.facts.len();
                let mut updated = corpus;
                updated.merge(new_knowledge);
                match alvum_knowledge::store::save(&knowledge_dir, &updated) {
                    Ok(()) => knowledge_timer.finish_ok(serde_json::json!({
                        "new_entities": entity_count,
                        "new_patterns": pattern_count,
                        "new_facts": fact_count,
                    })),
                    Err(e) => {
                        warn!(error = %e, "failed to save knowledge corpus");
                        events::emit(Event::Error {
                            source: "knowledge/save".into(),
                            message: format!("{e:#}"),
                        });
                        knowledge_timer.finish_err(serde_json::Value::Null);
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "knowledge extraction failed, skipping");
                events::emit(Event::Error {
                    source: "knowledge/extract".into(),
                    message: format!("{e:#}"),
                });
                knowledge_timer.finish_err(serde_json::Value::Null);
            }
        }
    }

    Ok(ExtractOutput {
        observations: all_observations,
        threading,
        result,
    })
}

/// Write bytes to `path` atomically: write to `path.tmp`, fsync, rename.
/// A crash during write never leaves `path` with partial content — readers
/// either see the prior version or the new one, never a torn write.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => format!("{ext}.tmp"),
        None => "tmp".into(),
    });
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Atomically write a JSONL file: serialize each item onto its own line, then
/// rename into place. Replaces any prior file at `path`.
fn write_jsonl_atomic<T: serde::Serialize>(path: &Path, items: &[T]) -> Result<()> {
    let mut body = String::new();
    for item in items {
        body.push_str(&serde_json::to_string(item)?);
        body.push('\n');
    }
    write_atomic(path, body.as_bytes())
}

/// Compare the enabled-connector set used to build an existing transcript
/// against the set that's active NOW. `Ok(true)` means the transcript is
/// reusable, `Ok(false)` means the sidecar is missing or the sets differ.
/// Returns `Err` only on IO/parse failure; callers should treat errors as
/// "don't trust, re-gather".
///
/// Thin wrapper around `processor_runner::read_transcript_meta` that keeps
/// the long-standing resume-guard tests in `resume_tests` happy.
fn transcript_fingerprint_matches(
    out_dir: &std::path::Path,
    current_connectors: &[String],
) -> anyhow::Result<bool> {
    match read_transcript_meta(out_dir)? {
        Some(meta) => {
            let mut stored = meta.connectors;
            let mut current: Vec<String> = current_connectors.to_vec();
            stored.sort();
            current.sort();
            Ok(stored == current)
        }
        None => Ok(false),
    }
}

/// Remove all downstream checkpoint files so that a fresh re-gather isn't
/// inadvertently paired with threads/decisions/briefing from a prior run.
/// Called when the transcript fingerprint mismatches on --resume. Best-effort:
/// failures to remove individual files are silently ignored (the stage will
/// simply recompute, which is the correct outcome).
fn clear_downstream_checkpoints(out_dir: &Path) {
    for name in ["threads.json", "decisions.jsonl", "decisions.raw.jsonl", "briefing.md"] {
        let p = out_dir.join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    // Per-chunk threading outputs. Sweep anything matching the pattern so
    // future chunk indices (e.g., if a day has 20 chunks one run, 8 the next)
    // don't leave orphans.
    if let Ok(entries) = std::fs::read_dir(out_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("threads-chunk-") && name.ends_with(".json") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // Knowledge entities are guarded by the same existence+resume check, so
    // they must also be cleared to prevent them being skipped on re-run.
    let knowledge_entities = out_dir.join("knowledge").join("entities.jsonl");
    if knowledge_entities.exists() {
        let _ = std::fs::remove_file(&knowledge_entities);
    }
}

#[cfg(test)]
mod resume_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_transcript(dir: &std::path::Path, names: &[&str]) {
        fs::write(dir.join("transcript.jsonl"), "").unwrap();
        let meta = serde_json::json!({
            "connectors": names.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        });
        fs::write(
            dir.join("transcript.meta.json"),
            serde_json::to_string(&meta).unwrap(),
        ).unwrap();
    }

    #[test]
    fn fingerprint_matches_when_connector_sets_equal() {
        let tmp = TempDir::new().unwrap();
        write_transcript(tmp.path(), &["audio", "claude-code"]);
        let current: Vec<String> = ["claude-code", "audio"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            transcript_fingerprint_matches(tmp.path(), &current).unwrap(),
            true,
            "transcript should be reused when connector set matches (order-insensitive)"
        );
    }

    #[test]
    fn fingerprint_mismatches_when_connector_set_grew() {
        let tmp = TempDir::new().unwrap();
        write_transcript(tmp.path(), &["claude-code", "codex"]);
        let current: Vec<String> = ["audio", "claude-code", "codex", "screen"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(
            transcript_fingerprint_matches(tmp.path(), &current).unwrap(),
            false,
            "transcript should be invalidated when connector set differs"
        );
    }

    #[test]
    fn fingerprint_missing_sidecar_returns_false() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("transcript.jsonl"), "").unwrap();
        // No transcript.meta.json written.
        let current: Vec<String> = ["claude-code"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            transcript_fingerprint_matches(tmp.path(), &current).unwrap(),
            false,
            "missing sidecar should be conservatively treated as mismatch"
        );
    }

    #[test]
    fn fingerprint_malformed_sidecar_returns_err() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("transcript.jsonl"), "").unwrap();
        fs::write(tmp.path().join("transcript.meta.json"), b"not json").unwrap();
        let current: Vec<String> = ["claude-code"].iter().map(|s| s.to_string()).collect();
        assert!(
            transcript_fingerprint_matches(tmp.path(), &current).is_err(),
            "malformed JSON sidecar must surface as Err (caller collapses Err to false)"
        );
    }

    #[test]
    fn mismatch_clears_downstream_checkpoints() {
        let tmp = TempDir::new().unwrap();
        // Pretend a prior run left every stage's checkpoint on disk.
        fs::write(tmp.path().join("transcript.jsonl"), "").unwrap();
        fs::write(
            tmp.path().join("transcript.meta.json"),
            r#"{"connectors":["old"]}"#,
        )
        .unwrap();
        for f in ["threads.json", "decisions.jsonl", "decisions.raw.jsonl", "briefing.md"] {
            fs::write(tmp.path().join(f), "stale").unwrap();
        }
        fs::create_dir_all(tmp.path().join("knowledge")).unwrap();
        fs::write(tmp.path().join("knowledge").join("entities.jsonl"), "stale").unwrap();

        clear_downstream_checkpoints(tmp.path());

        for f in ["threads.json", "decisions.jsonl", "decisions.raw.jsonl", "briefing.md"] {
            assert!(!tmp.path().join(f).exists(), "{f} should be removed");
        }
        assert!(
            !tmp.path().join("knowledge").join("entities.jsonl").exists(),
            "knowledge/entities.jsonl should be removed"
        );
        // The transcript itself is left alone — the re-gather loop overwrites it.
        assert!(
            tmp.path().join("transcript.jsonl").exists(),
            "transcript.jsonl cleanup is out of scope for this helper"
        );
    }
}
