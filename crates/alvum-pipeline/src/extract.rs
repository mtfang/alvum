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
use tracing::{info, warn};

use crate::llm::LlmProvider;

pub struct ExtractConfig {
    pub capture_dir: PathBuf,
    pub output_dir: PathBuf,
    pub relevance_threshold: f32,
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

    let mut all_observations: Vec<Observation> = Vec::new();

    // 1. Each connector's processors produce Observations from its DataRefs
    for connector in &connectors {
        let connector_name = connector.name();
        let processors = connector.processors();

        for processor in processors {
            let handles = processor.handles();
            info!(
                connector = connector_name,
                processor = processor.name(),
                handles = ?handles,
                "running processor"
            );

            // Gather DataRefs for this processor's handles from capture directory
            let data_refs = gather_data_refs_for_handles(&config.capture_dir, &handles)?;

            if data_refs.is_empty() {
                info!(processor = processor.name(), "no data refs found, skipping");
                continue;
            }

            match processor.process(&data_refs, &config.capture_dir).await {
                Ok(obs) => {
                    info!(
                        processor = processor.name(),
                        count = obs.len(),
                        "processor produced observations"
                    );
                    all_observations.extend(obs);
                }
                Err(e) => {
                    warn!(processor = processor.name(), error = %e, "processor failed, continuing");
                }
            }
        }
    }

    // 2. Save unified transcript (atomic write — survives crash mid-write)
    let transcript_path = config.output_dir.join("transcript.jsonl");
    write_jsonl_atomic(&transcript_path, &all_observations)?;
    info!(
        path = %transcript_path.display(),
        count = all_observations.len(),
        "saved transcript"
    );

    if all_observations.is_empty() {
        anyhow::bail!("no observations produced by any connector");
    }

    // 3. Load knowledge corpus for context-aware threading
    let knowledge_dir = config.output_dir.join("knowledge");
    let corpus = alvum_knowledge::store::load(&knowledge_dir).unwrap_or_default();

    // 4. Episodic alignment
    info!("running episodic alignment");
    let threading = alvum_episode::threading::align_episodes(
        provider.as_ref(),
        &all_observations,
        chrono::Duration::minutes(5),
        Some(&corpus),
    )
    .await?;

    let threads_path = config.output_dir.join("threads.json");
    write_atomic(
        &threads_path,
        serde_json::to_string_pretty(&threading)?.as_bytes(),
    )?;

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

    // Intermediate checkpoint paths. See docs/superpowers/plans/... for the
    // resumable-pipeline design. Each LLM stage writes its own output so a
    // transient failure doesn't wipe upstream work.
    let decisions_raw_path = config.output_dir.join("decisions.raw.jsonl");
    let decisions_path = config.output_dir.join("decisions.jsonl");
    let briefing_path = config.output_dir.join("briefing.md");
    let extraction_path = config.output_dir.join("extraction.json");

    // 6. Extract decisions (distill stage)
    info!(count = relevant_observations.len(), "extracting decisions");
    let mut decisions =
        crate::distill::extract_decisions(provider.as_ref(), &relevant_observations).await?;
    // Checkpoint: write post-distill decisions before attempting causal linking.
    // If causal or briefing flakes, this survives so a re-run can resume.
    write_jsonl_atomic(&decisions_raw_path, &decisions)?;
    info!(
        path = %decisions_raw_path.display(),
        count = decisions.len(),
        "checkpoint: decisions post-distill"
    );

    // 7. Link causally
    if !decisions.is_empty() {
        crate::causal::link_decisions(provider.as_ref(), &mut decisions).await?;
    }
    // Checkpoint: write decisions with causal links. This is the authoritative
    // decisions.jsonl; the .raw.jsonl is now redundant.
    write_jsonl_atomic(&decisions_path, &decisions)?;
    let _ = std::fs::remove_file(&decisions_raw_path);
    info!(
        path = %decisions_path.display(),
        count = decisions.len(),
        "checkpoint: decisions post-causal"
    );

    // 8. Briefing
    let briefing = if !decisions.is_empty() {
        crate::briefing::generate_briefing(provider.as_ref(), &decisions).await?
    } else {
        String::from("No decisions found.")
    };
    // Checkpoint: write briefing immediately. Downstream steps (knowledge
    // extraction) are best-effort; briefing must survive their failure.
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

    // 10. Knowledge extraction (best-effort — don't fail pipeline on this)
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

    Ok(ExtractOutput {
        observations: all_observations,
        threading,
        result,
    })
}

/// Gather DataRefs from the capture directory for the given handles.
/// Handles are source names (e.g., "audio-mic", "screen") or MIME types.
fn gather_data_refs_for_handles(
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
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
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
