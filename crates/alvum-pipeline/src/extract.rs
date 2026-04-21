//! extract_and_pipeline — the full observation → decision pipeline as a library function.
//!
//! Takes a set of connectors, runs their processors, does episodic alignment,
//! extracts decisions, links causally, generates briefing, and updates the
//! knowledge corpus. Returns the complete extraction result.

use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::decision::ExtractionResult;
use alvum_core::observation::Observation;
use alvum_core::storage;
use anyhow::{Context, Result};
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

pub struct ExtractConfig {
    pub capture_dir: PathBuf,
    pub output_dir: PathBuf,
    pub relevance_threshold: f32,
    /// Resume from any per-stage checkpoint files that already exist in
    /// output_dir. A previously-successful stage's file is loaded from
    /// disk and the LLM call skipped. Idempotent on a fresh output_dir.
    pub resume: bool,
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
    std::fs::create_dir_all(&config.output_dir)?;

    let transcript_path = config.output_dir.join("transcript.jsonl");

    let current_connector_names: Vec<String> =
        connectors.iter().map(|c| c.name().to_string()).collect();

    let resume_ok = config.resume
        && transcript_path.exists()
        && transcript_fingerprint_matches(&config.output_dir, &current_connector_names)
            .unwrap_or(false);

    // Stage 1-2: gather observations (from connectors or from prior transcript)
    let all_observations: Vec<Observation> = if resume_ok {
        // If the prior run recorded processor failures, warn so the user
        // knows the reused briefing is partial. Option (a) in the design:
        // reuse transcript as-is, don't retry — the user is explicitly
        // opting into the cached run by passing --resume.
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

        // Parallel fan-out of (connector, processor) pairs. Each processor
        // gets up to MAX_PROCESSOR_ATTEMPTS tries with 500ms / 1s linear
        // backoff. Exhausted failures are collected into the sidecar so
        // they're visible on the next --resume run.
        let pairs = pairs_from_connectors(&connectors);
        let outcome = run_processors_with_retry(
            pairs,
            &config.capture_dir,
            MAX_PROCESSOR_ATTEMPTS,
            vec![Duration::from_millis(500), Duration::from_secs(1)],
        )
        .await;

        for f in &outcome.failures {
            warn!(
                connector = %f.connector,
                processor = %f.processor,
                attempts = f.attempts,
                error = %f.last_error,
                "processor failed all retries"
            );
        }

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
        anyhow::bail!("no observations produced by any connector");
    }

    // Load knowledge corpus for context-aware threading (and for later merge).
    let knowledge_dir = config.output_dir.join("knowledge");
    let corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

    // Stage 4: episodic alignment (resumable)
    let threads_path = config.output_dir.join("threads.json");
    let threading: alvum_episode::types::ThreadingResult = if config.resume && threads_path.exists() {
        info!(
            path = %threads_path.display(),
            "resume: loading threads from disk (skipping threading LLM call)"
        );
        let json = std::fs::read_to_string(&threads_path)?;
        serde_json::from_str(&json).context("failed to parse existing threads.json")?
    } else {
        info!("running episodic alignment");
        let t = alvum_episode::threading::align_episodes(
            provider.as_ref(),
            &all_observations,
            chrono::Duration::minutes(5),
            Some(&corpus),
        )
        .await?;
        write_atomic(
            &threads_path,
            serde_json::to_string_pretty(&t)?.as_bytes(),
        )?;
        t
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
                crate::causal::link_decisions(provider.as_ref(), &mut d).await?;
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
            let mut d =
                crate::distill::extract_decisions(provider.as_ref(), &relevant_observations).await?;
            write_jsonl_atomic(&decisions_raw_path, &d)?;
            info!(
                path = %decisions_raw_path.display(),
                count = d.len(),
                "checkpoint: decisions post-distill"
            );
            if !d.is_empty() {
                crate::causal::link_decisions(provider.as_ref(), &mut d).await?;
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
        let b = if !decisions.is_empty() {
            crate::briefing::generate_briefing(provider.as_ref(), &decisions).await?
        } else {
            String::from("No decisions found.")
        };
        write_atomic(&briefing_path, b.as_bytes())?;
        info!(path = %briefing_path.display(), "checkpoint: briefing.md");
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
        match alvum_knowledge::extract::extract_knowledge(
            provider.as_ref(),
            &relevant_observations,
            &corpus,
        )
        .await
        {
            Ok(new_knowledge) => {
                let mut updated = corpus;
                updated.merge(new_knowledge);
                if let Err(e) = alvum_knowledge::store::save(&knowledge_dir, &updated) {
                    warn!(error = %e, "failed to save knowledge corpus");
                }
            }
            Err(e) => warn!(error = %e, "knowledge extraction failed, skipping"),
        }
    }

    Ok(ExtractOutput {
        observations: all_observations,
        threading,
        result,
    })
}

/// Gather DataRefs from the capture directory for the given handles.
/// Handles are source names (e.g., "audio-mic", "screen") or MIME types.
pub(crate) fn gather_data_refs_for_handles(
    capture_dir: &Path,
    handles: &[String],
) -> Result<Vec<DataRef>> {
    let mut data_refs = Vec::new();

    for handle in handles {
        match handle.as_str() {
            "audio-mic" => {
                let dir = capture_dir.join("audio").join("mic");
                data_refs.extend(scan_audio_dir(&dir, "audio-mic")?);
            }
            "audio-system" => {
                let dir = capture_dir.join("audio").join("system");
                data_refs.extend(scan_audio_dir(&dir, "audio-system")?);
            }
            "audio-wearable" => {
                let dir = capture_dir.join("audio").join("wearable");
                data_refs.extend(scan_audio_dir(&dir, "audio-wearable")?);
            }
            "screen" => {
                let captures_path = capture_dir.join("screen").join("captures.jsonl");
                if captures_path.exists() {
                    let refs: Vec<DataRef> = storage::read_jsonl(&captures_path)
                        .context("failed to read screen captures.jsonl")?;
                    data_refs.extend(refs);
                }
            }
            "claude-code" => {
                // ClaudeCodeProcessor handles this directly, ignoring data_refs
                // Emit a single dummy ref so the processor runs
                data_refs.push(DataRef {
                    ts: chrono::Utc::now(),
                    source: "claude-code".into(),
                    path: "".into(),
                    mime: "application/x-jsonl".into(),
                    metadata: None,
                });
            }
            "codex" => {
                // CodexProcessor reads ~/.codex/sessions/ directly; dummy ref
                // forces the processor to run without any capture-dir data.
                data_refs.push(DataRef {
                    ts: chrono::Utc::now(),
                    source: "codex".into(),
                    path: "".into(),
                    mime: "application/x-jsonl".into(),
                    metadata: None,
                });
            }
            other => {
                warn!(handle = other, "unknown handle, no DataRefs gathered");
            }
        }
    }

    Ok(data_refs)
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
    // Knowledge entities are guarded by the same existence+resume check, so
    // they must also be cleared to prevent them being skipped on re-run.
    let knowledge_entities = out_dir.join("knowledge").join("entities.jsonl");
    if knowledge_entities.exists() {
        let _ = std::fs::remove_file(&knowledge_entities);
    }
}

fn scan_audio_dir(dir: &Path, source: &str) -> Result<Vec<DataRef>> {
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut refs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext == "wav" || ext == "opus" {
            let mime = if ext == "wav" { "audio/wav" } else { "audio/opus" };
            refs.push(DataRef {
                ts: chrono::Utc::now(),
                source: source.into(),
                path: path.to_string_lossy().into_owned(),
                mime: mime.into(),
                metadata: None,
            });
        }
    }
    Ok(refs)
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
