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
    TranscriptMeta, pairs_from_connectors, read_transcript_meta, run_processors_with_retry,
    write_transcript_meta,
};

/// How many total attempts (initial + retries) each processor gets before we
/// give up and record a failure. 3 = one real try + two retries.
const MAX_PROCESSOR_ATTEMPTS: u32 = 3;

// THREADING_CHUNK_BUDGET lives on `tree::thread::THREADING_CHUNK_BUDGET`
// — the threading layer is the only place this knob matters now.

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
    /// Date printed in the L5 briefing heading. Backfill runners set this to
    /// the capture day; interactive runs default to the local observation date.
    pub briefing_date: Option<String>,
}

pub struct ExtractOutput {
    pub observations: Vec<Observation>,
    pub threading: crate::tree::thread::ThreadingResult,
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
        let mut direct_observations: Vec<Observation> = Vec::new();
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
            match c.gather_observations(&config.capture_dir) {
                Ok(observations) => {
                    for observation in &observations {
                        *per_source_counts
                            .entry((c.name().to_string(), observation.source.clone()))
                            .or_insert(0) += 1;
                    }
                    direct_observations.extend(observations);
                }
                Err(e) => {
                    warn!(connector = %c.name(), error = %e, "gather_observations failed; skipping direct observations");
                    events::emit(Event::Error {
                        source: format!("connector/{}/observations", c.name()),
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
            for s in c.expected_source_names() {
                sources_seen.insert(s);
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
                if count == 0 && c.expected_source_names().iter().any(|x| x == &source) {
                    events::emit(Event::Warning {
                        source: format!("connector/{}", c.name()),
                        message: format!(
                            "expected source `{source}` produced 0 refs (modality silent)"
                        ),
                    });
                }
            }
        }
        total_refs_seen = all_refs.len() + direct_observations.len();
        gather_timer.finish_ok(serde_json::json!({
            "ref_count": all_refs.len(),
            "direct_observation_count": direct_observations.len(),
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
            all_refs
                .into_iter()
                .filter(|dr| !processed.contains(dr))
                .collect()
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
            .map(|(_, p)| filtered_refs.iter().filter(|dr| p.accepts(dr)).count())
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
        let mut observations = direct_observations;
        observations.extend(outcome.observations);
        write_jsonl_atomic(&transcript_path, &observations)?;
        write_transcript_meta(
            &config.output_dir,
            &TranscriptMeta {
                connectors: current_connector_names.clone(),
                failed_processors: outcome.failures,
            },
        )?;
        info!(
            path = %transcript_path.display(),
            count = observations.len(),
            "saved transcript"
        );
        observations
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

    // Load user-managed synthesis profile and generated knowledge corpus for
    // context-aware threading and downstream synthesis.
    let profile = alvum_core::synthesis_profile::SynthesisProfile::load_or_default()?;
    let profile_snapshot_path = config.output_dir.join("synthesis-profile.snapshot.json");
    let profile_snapshot = profile.snapshot();
    if config.resume {
        match synthesis_profile_matches_snapshot(&config.output_dir, &profile) {
            Ok(true) => {}
            Ok(false) => {
                info!("resume: synthesis profile changed, clearing LLM checkpoints");
                clear_downstream_checkpoints(&config.output_dir);
            }
            Err(error) => {
                warn!(error = %error, "resume: synthesis profile snapshot unreadable, clearing LLM checkpoints");
                clear_downstream_checkpoints(&config.output_dir);
            }
        }
    }
    write_atomic(
        &profile_snapshot_path,
        serde_json::to_string_pretty(&profile_snapshot)?.as_bytes(),
    )?;
    let knowledge_dir = alvum_core::synthesis_profile::generated_knowledge_dir();
    let corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

    // L1 → L2: episodic alignment — chunked. The tree primitive lives
    // in `crate::tree::thread`; orchestration here owns the resume
    // checkpoint plumbing and the StageTimer that surfaces the work
    // on the live observability layer.
    let threads_path = config.output_dir.join("threads.json");
    let threading: crate::tree::thread::ThreadingResult = if config.resume && threads_path.exists()
    {
        info!(
            path = %threads_path.display(),
            "resume: loading final threads from disk (skipping threading LLM calls)"
        );
        let json = std::fs::read_to_string(&threads_path)?;
        serde_json::from_str(&json).context("failed to parse existing threads.json")?
    } else {
        let time_blocks = crate::tree::blocks::assemble_time_blocks(
            &all_observations,
            chrono::Duration::minutes(5),
        );

        let thread_timer = StageTimer::start(events::STAGE_THREAD);
        // The chunked driver inside `tree::thread` owns the per-chunk
        // call-site labelling, parse retry, defang, and progress
        // ticks; we leave the per-chunk checkpoint files behind so a
        // resume against a half-completed run can still skip work.
        crate::progress::report(crate::progress::STAGE_THREAD, 0, 1);
        let all_threads = crate::tree::thread::identify_threads_chunked(
            provider.as_ref(),
            &time_blocks,
            Some(&corpus),
            &profile,
        )
        .await?;
        crate::progress::report(crate::progress::STAGE_THREAD, 1, 1);

        let mut sources: Vec<String> = all_observations.iter().map(|o| o.source.clone()).collect();
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

        let threading = crate::tree::thread::ThreadingResult {
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
            "thread_count": threading.threads.len(),
            "observation_count": threading.observation_count,
        }));
        threading
    };

    // L2 relevance filter — survives until the tree's L3 cluster step
    // takes over. Threads that miss the threshold drop out of the
    // observation set fed upward.
    let relevant: Vec<&crate::tree::thread::Thread> = threading
        .threads
        .iter()
        .filter(|t| t.is_relevant(config.relevance_threshold))
        .collect();

    let relevant_observations: Vec<Observation> = relevant
        .iter()
        .flat_map(|t| t.observations.clone())
        .collect();

    // Tree-level checkpoint paths. Each level writes its own JSON so a
    // transient failure doesn't wipe upstream work; `--resume` reloads
    // any existing checkpoint and skips that level's LLM call.
    let tree_dir = config.output_dir.join("tree");
    std::fs::create_dir_all(&tree_dir)?;
    let l3_clusters_path = tree_dir.join("L3-clusters.jsonl");
    let l3_edges_path = tree_dir.join("L3-edges.jsonl");
    let l4_domains_path = tree_dir.join("L4-domains.jsonl");
    let l4_edges_path = tree_dir.join("L4-edges.jsonl");
    let l5_day_path = tree_dir.join("L5-day.json");
    let artifact_dir = tree_dir.join("artifacts");
    let l2_thread_dossiers_path = artifact_dir.join("L2-thread-dossiers.jsonl");
    let l3_cluster_dossiers_path = artifact_dir.join("L3-cluster-dossiers.jsonl");
    let l4_domain_dossiers_path = artifact_dir.join("L4-domain-dossiers.jsonl");
    let l4_decision_dossiers_path = artifact_dir.join("L4-decision-dossiers.jsonl");
    let l5_source_pack_path = artifact_dir.join("L5-briefing-source.json");
    let knowledge_run_marker_path = artifact_dir.join("knowledge-extracted.json");
    // Backwards-compat artifacts the website + tray panel still expect:
    let decisions_path = config.output_dir.join("decisions.jsonl");
    let briefing_path = config.output_dir.join("briefing.md");
    let extraction_path = config.output_dir.join("extraction.json");

    // L2 → L3: cluster reduction.
    crate::progress::report(crate::progress::STAGE_CLUSTER, 0, 1);
    let clusters: Vec<crate::tree::cluster::Cluster> = if config.resume && l3_clusters_path.exists()
    {
        info!(path = %l3_clusters_path.display(), "resume: loading L3 clusters");
        storage::read_jsonl(&l3_clusters_path)?
    } else {
        let timer = StageTimer::start(events::STAGE_CLUSTER);
        let owned_threads: Vec<crate::tree::thread::Thread> =
            relevant.iter().map(|t| (*t).clone()).collect();
        let result =
            crate::tree::cluster::distill_clusters(&owned_threads, &profile, provider.as_ref())
                .await?;
        timer.finish_ok(serde_json::json!({
            "thread_count": owned_threads.len(),
            "cluster_count": result.len(),
        }));
        write_jsonl_atomic(&l3_clusters_path, &result)?;
        info!(path = %l3_clusters_path.display(), count = result.len(), "checkpoint: L3 clusters");
        result
    };
    crate::progress::report(crate::progress::STAGE_CLUSTER, 1, 1);

    // L3 cross-correlate.
    crate::progress::report(crate::progress::STAGE_CLUSTER_CORRELATE, 0, 1);
    let cluster_edges: Vec<alvum_core::decision::Edge> = if config.resume && l3_edges_path.exists()
    {
        info!(path = %l3_edges_path.display(), "resume: loading L3 edges");
        storage::read_jsonl(&l3_edges_path)?
    } else {
        let timer = StageTimer::start(events::STAGE_CLUSTER_CORRELATE);
        let result = crate::tree::cluster::correlate_clusters(&clusters, provider.as_ref()).await?;
        timer.finish_ok(serde_json::json!({"edge_count": result.len()}));
        write_jsonl_atomic(&l3_edges_path, &result)?;
        info!(path = %l3_edges_path.display(), count = result.len(), "checkpoint: L3 edges");
        result
    };
    crate::progress::report(crate::progress::STAGE_CLUSTER_CORRELATE, 1, 1);

    let date_str = effective_briefing_date(&config, &all_observations);

    // L3 → L4: domain reduction (emits Decision atoms).
    crate::progress::report(crate::progress::STAGE_DOMAIN, 0, 1);
    let mut domains: Vec<crate::tree::domain::DomainNode> = if config.resume
        && l4_domains_path.exists()
    {
        info!(path = %l4_domains_path.display(), "resume: loading L4 domains");
        storage::read_jsonl(&l4_domains_path)?
    } else {
        let timer = StageTimer::start(events::STAGE_DOMAIN);
        let result = crate::tree::domain::distill_domains(
            &clusters,
            &cluster_edges,
            Some(&date_str),
            &profile,
            provider.as_ref(),
        )
        .await?;
        let total_decisions: usize = result.iter().map(|d| d.decisions.len()).sum();
        timer.finish_ok(serde_json::json!({
            "cluster_count": clusters.len(),
            "domain_count": result.len(),
            "decision_count": total_decisions,
        }));
        write_jsonl_atomic(&l4_domains_path, &result)?;
        info!(path = %l4_domains_path.display(), domains = result.len(), decisions = total_decisions, "checkpoint: L4 domains");
        result
    };
    crate::progress::report(crate::progress::STAGE_DOMAIN, 1, 1);
    let normalized_dates = normalize_domain_decision_dates(&mut domains, &date_str);
    if normalized_dates > 0 {
        events::emit(Event::InputFiltered {
            processor: "domain/date-normalizer".into(),
            file: None,
            kept: domains.iter().map(|d| d.decisions.len()).sum(),
            dropped: 0,
            reasons: serde_json::json!({
                "decision_dates_rewritten": normalized_dates,
                "briefing_date": date_str,
            }),
        });
        write_jsonl_atomic(&l4_domains_path, &domains)?;
        info!(
            path = %l4_domains_path.display(),
            corrected = normalized_dates,
            "checkpoint: normalized L4 decision dates"
        );
    }

    let enriched_refs = apply_profile_refs_to_domain_decisions(&mut domains, &profile);
    if enriched_refs > 0 {
        write_jsonl_atomic(&l4_domains_path, &domains)?;
        info!(
            path = %l4_domains_path.display(),
            refs = enriched_refs,
            "checkpoint: enriched L4 decisions with profile refs"
        );
    }

    // Flatten decisions across the five domains for the L4 cross-
    // correlation pass and the backwards-compat decisions.jsonl.
    let mut decisions: Vec<alvum_core::decision::Decision> = domains
        .iter()
        .flat_map(|d| d.decisions.iter().cloned())
        .collect();

    // L4 cross-correlate (decision → decision edges, including
    // alignment_break / alignment_honor that the L5 briefing reads).
    crate::progress::report(crate::progress::STAGE_DOMAIN_CORRELATE, 0, 1);
    let decision_edges: Vec<alvum_core::decision::Edge> = if config.resume && l4_edges_path.exists()
    {
        info!(path = %l4_edges_path.display(), "resume: loading L4 edges");
        storage::read_jsonl(&l4_edges_path)?
    } else {
        let timer = StageTimer::start(events::STAGE_DOMAIN_CORRELATE);
        let result =
            crate::tree::domain::correlate_decisions(&decisions, &profile, provider.as_ref())
                .await?;
        timer.finish_ok(serde_json::json!({"edge_count": result.len()}));
        write_jsonl_atomic(&l4_edges_path, &result)?;
        info!(path = %l4_edges_path.display(), count = result.len(), "checkpoint: L4 edges");
        result
    };
    crate::progress::report(crate::progress::STAGE_DOMAIN_CORRELATE, 1, 1);

    // Project decision edges back onto Decision.causes / .effects so
    // the website's decisions UI (which reads decision.causes directly)
    // still works on the new schema.
    populate_causes_effects(&mut decisions, &decision_edges);
    write_jsonl_atomic(&decisions_path, &decisions)?;
    info!(path = %decisions_path.display(), count = decisions.len(), "checkpoint: decisions.jsonl");

    // Deterministic lower-level evidence pack for L5. These artifacts
    // preserve the trace from final briefing claims back to threads,
    // clusters, decisions, edges, and transcript observations.
    let briefing_artifacts = crate::tree::artifacts::build_briefing_artifacts(
        &date_str,
        &all_observations,
        &threading,
        &clusters,
        &domains,
        &decisions,
        &decision_edges,
        &profile_snapshot,
    );
    write_jsonl_atomic(
        &l2_thread_dossiers_path,
        &briefing_artifacts.thread_dossiers,
    )?;
    write_jsonl_atomic(
        &l3_cluster_dossiers_path,
        &briefing_artifacts.cluster_dossiers,
    )?;
    write_jsonl_atomic(
        &l4_domain_dossiers_path,
        &briefing_artifacts.domain_dossiers,
    )?;
    write_jsonl_atomic(
        &l4_decision_dossiers_path,
        &briefing_artifacts.decision_dossiers,
    )?;
    write_atomic(
        &l5_source_pack_path,
        serde_json::to_string_pretty(&briefing_artifacts.source_pack)?.as_bytes(),
    )?;
    info!(
        path = %artifact_dir.display(),
        threads = briefing_artifacts.thread_dossiers.len(),
        clusters = briefing_artifacts.cluster_dossiers.len(),
        decisions = briefing_artifacts.decision_dossiers.len(),
        "checkpoint: briefing evidence artifacts"
    );

    // L4 → L5: source-pack-backed morning briefing.
    crate::progress::report(crate::progress::STAGE_DAY, 0, 1);
    let day: crate::tree::day::Day = if config.resume && l5_day_path.exists() {
        info!(path = %l5_day_path.display(), "resume: loading L5 day");
        let json = std::fs::read_to_string(&l5_day_path)?;
        serde_json::from_str(&json).context("failed to parse existing L5-day.json")?
    } else {
        let timer = StageTimer::start(events::STAGE_DAY);
        let result = crate::tree::day::distill_day(
            &domains,
            &decision_edges,
            Some(&briefing_artifacts.source_pack),
            Some(&corpus),
            &profile,
            &date_str,
            provider.as_ref(),
        )
        .await?;
        timer.finish_ok(serde_json::json!({
            "briefing_chars": result.briefing.len(),
            "decision_count_by_domain": result.decision_count_by_domain,
        }));
        write_atomic(
            &l5_day_path,
            serde_json::to_string_pretty(&result)?.as_bytes(),
        )?;
        result
    };
    crate::progress::report(crate::progress::STAGE_DAY, 1, 1);

    let briefing = day.briefing.clone();
    write_atomic(&briefing_path, briefing.as_bytes())?;
    info!(path = %briefing_path.display(), "checkpoint: briefing.md");

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
    crate::progress::report(crate::progress::STAGE_KNOWLEDGE, 0, 1);
    if config.resume && knowledge_run_marker_path.exists() {
        info!(
            path = %knowledge_run_marker_path.display(),
            "resume: knowledge already extracted, skipping"
        );
    } else {
        let knowledge_timer = StageTimer::start(events::STAGE_KNOWLEDGE);
        match alvum_knowledge::extract::extract_knowledge(
            provider.as_ref(),
            &relevant_observations,
            &corpus,
            &profile,
        )
        .await
        {
            Ok(new_knowledge) => {
                let entity_count = new_knowledge.entities.len();
                let pattern_count = new_knowledge.patterns.len();
                let fact_count = new_knowledge.facts.len();
                let mut updated = corpus;
                updated.merge(new_knowledge);
                let save_result =
                    alvum_knowledge::store::save(&knowledge_dir, &updated).and_then(|_| {
                        alvum_knowledge::store::save(&config.output_dir.join("knowledge"), &updated)
                    });
                match save_result {
                    Ok(()) => {
                        if let Err(e) = write_atomic(
                            &knowledge_run_marker_path,
                            serde_json::to_string_pretty(&serde_json::json!({
                                "ok": true,
                                "knowledge_dir": knowledge_dir.display().to_string(),
                                "extracted_at": chrono::Utc::now().to_rfc3339(),
                            }))?
                            .as_bytes(),
                        ) {
                            warn!(error = %e, "failed to write knowledge run marker");
                        }
                        knowledge_timer.finish_ok(serde_json::json!({
                            "new_entities": entity_count,
                            "new_patterns": pattern_count,
                            "new_facts": fact_count,
                        }))
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to save knowledge corpus");
                        events::emit(Event::Warning {
                            source: "knowledge/save".into(),
                            message: format!("{e:#}"),
                        });
                        knowledge_timer.finish_ok(serde_json::json!({
                            "skipped": true,
                            "reason": "knowledge_save_failed",
                        }));
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "knowledge extraction failed, skipping");
                events::emit(Event::Warning {
                    source: "knowledge/extract".into(),
                    message: format!("{e:#}"),
                });
                knowledge_timer.finish_ok(serde_json::json!({
                    "skipped": true,
                    "reason": "knowledge_extraction_failed",
                }));
            }
        }
    }
    crate::progress::report(crate::progress::STAGE_KNOWLEDGE, 1, 1);

    Ok(ExtractOutput {
        observations: all_observations,
        threading,
        result,
    })
}

fn effective_briefing_date(config: &ExtractConfig, observations: &[Observation]) -> String {
    effective_briefing_date_with_formatters(
        config,
        observations,
        crate::local_time::format_date,
        crate::local_time::today,
    )
}

#[cfg(test)]
fn effective_briefing_date_with_offset(
    config: &ExtractConfig,
    observations: &[Observation],
    offset: chrono::FixedOffset,
) -> String {
    effective_briefing_date_with_formatters(
        config,
        observations,
        |ts| crate::local_time::format_date_with_offset(ts, offset),
        || crate::local_time::format_date_with_offset(chrono::Utc::now(), offset),
    )
}

fn effective_briefing_date_with_formatters(
    config: &ExtractConfig,
    observations: &[Observation],
    format_date: impl Fn(chrono::DateTime<chrono::Utc>) -> String,
    today: impl Fn() -> String,
) -> String {
    if let Some(date) = &config.briefing_date {
        return date.clone();
    }
    observations
        .iter()
        .map(|observation| format_date(observation.ts))
        .min()
        .unwrap_or_else(today)
}

fn normalize_domain_decision_dates(
    domains: &mut [crate::tree::domain::DomainNode],
    briefing_date: &str,
) -> usize {
    let mut corrected = 0;
    for domain in domains {
        for decision in &mut domain.decisions {
            if decision.date != briefing_date {
                decision.date = briefing_date.to_string();
                corrected += 1;
            }
        }
    }
    corrected
}

fn apply_profile_refs_to_domain_decisions(
    domains: &mut [crate::tree::domain::DomainNode],
    profile: &alvum_core::synthesis_profile::SynthesisProfile,
) -> usize {
    let mut changed = 0;
    for domain in domains {
        for decision in &mut domain.decisions {
            let text = decision_profile_match_text(decision);
            changed += merge_profile_refs(&mut decision.interest_refs, profile.match_text(&text));
            changed += merge_profile_refs(
                &mut decision.intention_refs,
                profile.match_intentions(&text),
            );
        }
    }
    changed
}

fn decision_profile_match_text(decision: &alvum_core::decision::Decision) -> String {
    format!(
        "{} {} {} {} {} {}",
        decision.id,
        decision.summary,
        decision.reasoning.clone().unwrap_or_default(),
        decision.evidence.join(" "),
        decision.knowledge_refs.join(" "),
        decision.cross_domain.join(" ")
    )
}

fn merge_profile_refs(target: &mut Vec<String>, additions: Vec<String>) -> usize {
    use std::collections::BTreeSet;
    let before = target.len();
    let mut refs: BTreeSet<String> = target.drain(..).collect();
    refs.extend(additions);
    *target = refs.into_iter().collect();
    target.len().saturating_sub(before)
}

/// Project the L4-edges graph back onto each `Decision`'s `causes` /
/// `effects` arrays. The website prototype's decisions UI reads
/// `decision.causes` directly as a flat ID list; the richer Edge
/// metadata lives only in `tree/L4-edges.jsonl`.
fn populate_causes_effects(
    decisions: &mut [alvum_core::decision::Decision],
    edges: &[alvum_core::decision::Edge],
) {
    use std::collections::HashMap;
    let mut causes: HashMap<&str, Vec<String>> = HashMap::new();
    let mut effects: HashMap<&str, Vec<String>> = HashMap::new();
    for edge in edges {
        causes
            .entry(edge.to_id.as_str())
            .or_default()
            .push(edge.from_id.clone());
        effects
            .entry(edge.from_id.as_str())
            .or_default()
            .push(edge.to_id.clone());
    }
    for d in decisions.iter_mut() {
        d.causes = causes.get(d.id.as_str()).cloned().unwrap_or_default();
        d.effects = effects.get(d.id.as_str()).cloned().unwrap_or_default();
    }
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

fn synthesis_profile_matches_snapshot(
    out_dir: &Path,
    current_profile: &alvum_core::synthesis_profile::SynthesisProfile,
) -> anyhow::Result<bool> {
    let path = out_dir.join("synthesis-profile.snapshot.json");
    if !path.exists() {
        return Ok(false);
    }
    let json = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read synthesis profile snapshot: {}",
            path.display()
        )
    })?;
    let snapshot: alvum_core::synthesis_profile::SynthesisProfileSnapshot =
        serde_json::from_str(&json).with_context(|| {
            format!(
                "failed to parse synthesis profile snapshot: {}",
                path.display()
            )
        })?;
    Ok(snapshot.profile == *current_profile)
}

/// Remove all downstream checkpoint files so that a fresh re-gather isn't
/// inadvertently paired with threads/decisions/briefing from a prior run.
/// Called when the transcript fingerprint mismatches on --resume. Best-effort:
/// failures to remove individual files are silently ignored (the stage will
/// simply recompute, which is the correct outcome).
fn clear_downstream_checkpoints(out_dir: &Path) {
    for name in ["threads.json", "decisions.jsonl", "briefing.md"] {
        let p = out_dir.join(name);
        if p.exists() {
            let _ = std::fs::remove_file(&p);
        }
    }
    // Per-chunk threading outputs from the pre-tree-rewrite era. Sweep
    // anything matching the pattern so a stale chunk file doesn't
    // pollute a fresh run.
    if let Ok(entries) = std::fs::read_dir(out_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("threads-chunk-") && name.ends_with(".json") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // Tree-level checkpoints — every level's output must be wiped on
    // fingerprint mismatch so a fresh re-gather isn't paired with
    // stale upper-level results.
    let tree = out_dir.join("tree");
    if tree.exists() {
        let _ = std::fs::remove_dir_all(&tree);
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
    use alvum_core::decision::{
        Actor, ActorAttribution, ActorKind, Decision, DecisionSource, DecisionStatus,
    };
    use std::fs;
    use tempfile::TempDir;

    fn base_config(briefing_date: Option<&str>) -> ExtractConfig {
        ExtractConfig {
            capture_dir: PathBuf::new(),
            output_dir: PathBuf::new(),
            relevance_threshold: 0.0,
            resume: false,
            no_skip_processed: false,
            briefing_date: briefing_date.map(str::to_string),
        }
    }

    fn self_attr() -> ActorAttribution {
        ActorAttribution {
            actor: Actor {
                name: "user".into(),
                kind: ActorKind::Self_,
            },
            confidence: 0.9,
        }
    }

    fn decision_with_date(date: &str) -> Decision {
        Decision {
            id: "dec_001".into(),
            date: date.into(),
            time: "10:00".into(),
            summary: "Choose transcript-backed regeneration.".into(),
            domain: "Career".into(),
            source: DecisionSource::Spoken,
            magnitude: 0.7,
            reasoning: None,
            alternatives: Vec::new(),
            participants: vec!["user".into()],
            proposed_by: self_attr(),
            status: DecisionStatus::Accepted,
            resolved_by: Some(self_attr()),
            open: false,
            check_by: None,
            cross_domain: Vec::new(),
            evidence: vec!["let's try it".into()],
            multi_source_evidence: false,
            confidence_overall: 0.8,
            anchor_observations: Vec::new(),
            knowledge_refs: Vec::new(),
            interest_refs: Vec::new(),
            intention_refs: Vec::new(),
            causes: Vec::new(),
            effects: Vec::new(),
        }
    }

    fn domain_with_decision(date: &str) -> crate::tree::domain::DomainNode {
        crate::tree::domain::DomainNode {
            id: "Career".into(),
            summary: "Career work happened.".into(),
            cluster_ids: vec!["cluster_001".into()],
            key_actors: vec!["user".into()],
            decisions: vec![decision_with_date(date)],
        }
    }

    fn write_transcript(dir: &std::path::Path, names: &[&str]) {
        fs::write(dir.join("transcript.jsonl"), "").unwrap();
        let meta = serde_json::json!({
            "connectors": names.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        });
        fs::write(
            dir.join("transcript.meta.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn effective_briefing_date_prefers_configured_date() {
        let observations = vec![Observation::dialogue(
            "2026-04-28T10:00:00Z".parse().unwrap(),
            "codex",
            "user",
            "today is the model run date, not the briefing date",
        )];

        assert_eq!(
            effective_briefing_date(&base_config(Some("2026-04-18")), &observations),
            "2026-04-18"
        );
    }

    #[test]
    fn effective_briefing_date_falls_back_to_observation_date() {
        let observations = vec![
            Observation::dialogue(
                "2026-04-19T08:00:00Z".parse().unwrap(),
                "codex",
                "user",
                "later",
            ),
            Observation::dialogue(
                "2026-04-18T16:00:00Z".parse().unwrap(),
                "codex",
                "user",
                "earlier",
            ),
        ];

        assert_eq!(
            effective_briefing_date(&base_config(None), &observations),
            "2026-04-18"
        );
    }

    #[test]
    fn effective_briefing_date_falls_back_to_local_observation_date() {
        let observations = vec![Observation::dialogue(
            "2026-04-19T06:30:00Z".parse().unwrap(),
            "codex",
            "user",
            "late evening local work",
        )];
        let local_offset = chrono::FixedOffset::west_opt(7 * 60 * 60).unwrap();

        assert_eq!(
            effective_briefing_date_with_offset(&base_config(None), &observations, local_offset),
            "2026-04-18"
        );
    }

    #[test]
    fn normalize_domain_decision_dates_rewrites_llm_run_date() {
        let mut domains = vec![domain_with_decision("2026-04-28")];

        let corrected = normalize_domain_decision_dates(&mut domains, "2026-04-18");

        assert_eq!(corrected, 1);
        assert_eq!(domains[0].decisions[0].date, "2026-04-18");
    }

    #[test]
    fn fingerprint_matches_when_connector_sets_equal() {
        let tmp = TempDir::new().unwrap();
        write_transcript(tmp.path(), &["audio", "claude-code"]);
        let current: Vec<String> = ["claude-code", "audio"]
            .iter()
            .map(|s| s.to_string())
            .collect();
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
            .iter()
            .map(|s| s.to_string())
            .collect();
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
        for f in ["threads.json", "decisions.jsonl", "briefing.md"] {
            fs::write(tmp.path().join(f), "stale").unwrap();
        }
        // Tree-level checkpoints from the new pipeline shape.
        fs::create_dir_all(tmp.path().join("tree")).unwrap();
        for f in [
            "L3-clusters.jsonl",
            "L3-edges.jsonl",
            "L4-domains.jsonl",
            "L4-edges.jsonl",
            "L5-day.json",
        ] {
            fs::write(tmp.path().join("tree").join(f), "stale").unwrap();
        }
        fs::create_dir_all(tmp.path().join("knowledge")).unwrap();
        fs::write(tmp.path().join("knowledge").join("entities.jsonl"), "stale").unwrap();

        clear_downstream_checkpoints(tmp.path());

        for f in ["threads.json", "decisions.jsonl", "briefing.md"] {
            assert!(!tmp.path().join(f).exists(), "{f} should be removed");
        }
        assert!(
            !tmp.path().join("tree").exists(),
            "tree/ should be removed wholesale on fingerprint mismatch"
        );
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

    #[test]
    fn synthesis_profile_snapshot_detects_changed_customization() {
        let tmp = TempDir::new().unwrap();
        let current = alvum_core::synthesis_profile::SynthesisProfile::default();
        let mut stale = current.clone();
        stale.advanced_instructions = "Prefer terse bullets.".into();
        let snapshot = alvum_core::synthesis_profile::SynthesisProfileSnapshot {
            schema: "alvum.synthesis_profile.snapshot.v1".into(),
            snapshotted_at: chrono::Utc::now(),
            profile_path: tmp.path().join("synthesis-profile.toml"),
            profile: stale,
        };
        fs::write(
            tmp.path().join("synthesis-profile.snapshot.json"),
            serde_json::to_string(&snapshot).unwrap(),
        )
        .unwrap();

        assert_eq!(
            synthesis_profile_matches_snapshot(tmp.path(), &current).unwrap(),
            false,
            "resume must not reuse LLM checkpoints from an older customization profile"
        );
    }
}
