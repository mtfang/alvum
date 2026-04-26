//! Parallel processor execution with bounded retry, and the transcript
//! metadata sidecar that records which processors failed.
//!
//! The briefing pipeline fans each connector's processors out as independent
//! tokio tasks. A processor that `Err`s gets up to N total attempts with
//! short linear backoff. Across-connector, observations that succeed are
//! collected even when other processors fail — a partial briefing is better
//! than no briefing.

use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{info, warn};

// ──────────────────────────── types ────────────────────────────

/// Records a processor that failed all its retry attempts during a run.
/// Persisted inside `transcript.meta.json` so a later `--resume` can warn
/// the user that the briefing is missing data from this source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessorFailure {
    pub connector: String,
    pub processor: String,
    pub attempts: u32,
    pub last_error: String,
}

/// On-disk shape of `briefings/<date>/transcript.meta.json`. `connectors`
/// records the enabled-connector set that produced the sibling
/// `transcript.jsonl`. `failed_processors` is empty on a clean run.
///
/// Backwards-compat: old files from the previous sidecar generation carry
/// only `{"connectors": [...]}`. `#[serde(default)]` on `failed_processors`
/// makes those parse cleanly into a meta with no recorded failures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TranscriptMeta {
    pub connectors: Vec<String>,
    #[serde(default)]
    pub failed_processors: Vec<ProcessorFailure>,
}

// ──────────────────────────── sidecar I/O ────────────────────────────

/// Read the sidecar next to `transcript.jsonl`. Returns `Ok(None)` when the
/// sidecar doesn't exist. Returns `Err` on parse failure — callers treat
/// that as "don't trust, re-gather" consistent with the prior guard.
pub fn read_transcript_meta(out_dir: &Path) -> Result<Option<TranscriptMeta>> {
    let path = out_dir.join("transcript.meta.json");
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let meta: TranscriptMeta = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(meta))
}

/// Write the sidecar atomically. Connector names are sorted so the sidecar
/// is deterministic / diff-friendly and order-insensitive on compare.
pub fn write_transcript_meta(out_dir: &Path, meta: &TranscriptMeta) -> Result<()> {
    let mut sorted = meta.clone();
    sorted.connectors.sort();
    let bytes = serde_json::to_vec_pretty(&sorted)?;
    crate::extract::write_atomic(&out_dir.join("transcript.meta.json"), &bytes)
}

// ──────────────────────────── runner ────────────────────────────

/// Per-run outputs from the parallel processor runner.
pub struct RunOutcome {
    pub observations: Vec<Observation>,
    pub failures: Vec<ProcessorFailure>,
}

/// Flatten a `Vec<Box<dyn Connector>>` into the flat list of
/// (connector_name, processor) pairs that the runner operates on.
pub fn pairs_from_connectors(
    connectors: &[Box<dyn Connector>],
) -> Vec<(String, Box<dyn Processor>)> {
    connectors
        .iter()
        .flat_map(|c| {
            let name = c.name().to_string();
            c.processors().into_iter().map(move |p| (name.clone(), p))
        })
        .collect()
}

/// Run the given (connector, processor) pairs concurrently via `tokio::spawn`.
/// Each processor gets up to `max_attempts` total attempts with a linear
/// backoff schedule of `backoffs[i]` between attempt `i+1` and `i+2`.
/// Successful observations are accumulated; exhausted retries become
/// `ProcessorFailure` entries. Task panics are also captured as failures.
///
/// `all_refs` is the merged list of DataRefs from every connector. Each
/// processor receives the subset whose `source` matches its `handles()`.
///
/// **Failure policy**: every processor follows the implicit "skip with
/// warning" policy — exhausted retries do NOT abort the pipeline. The
/// failure is appended to `RunOutcome::failures`, which the caller in
/// `extract.rs` records into `transcript.meta.json` and emits as a
/// `pipeline_events::Event::Error`. Surviving processors' observations
/// still flow through. A future processor that genuinely cannot tolerate
/// being skipped (e.g. a hypothetical "pipeline_health_check") will need
/// an explicit `FailurePolicy` enum on the trait; we deliberately
/// haven't added the machinery yet because no current processor needs
/// it — see `docs/superpowers/plans/…` if/when that changes.
pub async fn run_processors_with_retry(
    pairs: Vec<(String, Box<dyn Processor>)>,
    all_refs: Vec<DataRef>,
    capture_dir: &Path,
    max_attempts: u32,
    backoffs: Vec<Duration>,
) -> RunOutcome {
    let cap: PathBuf = capture_dir.to_path_buf();
    let all_refs = std::sync::Arc::new(all_refs);
    let mut set = tokio::task::JoinSet::new();

    for (connector_name, processor) in pairs {
        let cap = cap.clone();
        let backoffs = backoffs.clone();
        let processor_name = processor.name().to_string();
        let identity = (connector_name.clone(), processor_name);
        let refs = all_refs.clone();
        set.spawn(async move {
            let result = run_one(
                identity.0.clone(),
                processor,
                refs,
                cap,
                max_attempts,
                backoffs,
            )
            .await;
            (identity, result)
        });
    }

    let mut observations = Vec::new();
    let mut failures = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(((connector, processor), Ok(obs))) => {
                let _ = processor;
                let _ = connector;
                observations.extend(obs);
            }
            Ok(((connector, processor), Err((attempts, last_error)))) => {
                failures.push(ProcessorFailure {
                    connector,
                    processor,
                    attempts,
                    last_error,
                });
            }
            Err(join_err) => {
                failures.push(ProcessorFailure {
                    connector: "unknown".into(),
                    processor: "unknown".into(),
                    attempts: 0,
                    last_error: format!("task panicked: {join_err}"),
                });
            }
        }
    }

    RunOutcome {
        observations,
        failures,
    }
}

/// Retry loop for a single processor. Returns observations on success or
/// `(attempts_used, last_error_string)` on exhausted retries.
async fn run_one(
    connector_name: String,
    processor: Box<dyn Processor>,
    all_refs: std::sync::Arc<Vec<DataRef>>,
    capture_dir: PathBuf,
    max_attempts: u32,
    backoffs: Vec<Duration>,
) -> std::result::Result<Vec<Observation>, (u32, String)> {
    let processor_name = processor.name().to_string();
    let handles = processor.handles();
    info!(
        connector = %connector_name,
        processor = %processor_name,
        handles = ?handles,
        "running processor"
    );

    let data_refs: Vec<DataRef> = all_refs
        .iter()
        .filter(|dr| handles.iter().any(|h| h == &dr.source))
        .cloned()
        .collect();

    if data_refs.is_empty() {
        info!(processor = %processor_name, "no data refs found, skipping");
        return Ok(vec![]);
    }

    let mut last_error = String::new();
    for attempt in 1..=max_attempts {
        match processor.process(&data_refs, &capture_dir).await {
            Ok(obs) => {
                info!(
                    processor = %processor_name,
                    count = obs.len(),
                    attempt,
                    "processor produced observations"
                );
                return Ok(obs);
            }
            Err(e) => {
                last_error = format!("{e}");
                warn!(
                    processor = %processor_name,
                    attempt,
                    max_attempts,
                    error = %e,
                    "processor attempt failed"
                );
                if attempt < max_attempts {
                    let idx = (attempt as usize - 1).min(backoffs.len().saturating_sub(1));
                    let delay = backoffs.get(idx).copied().unwrap_or_default();
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
    }
    Err((max_attempts, last_error))
}

// ──────────────────────────── tests ────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alvum_core::data_ref::DataRef;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn sample_obs() -> Observation {
        Observation {
            ts: chrono::Utc::now(),
            source: "test".into(),
            kind: "test".into(),
            content: "ok".into(),
            metadata: None,
            media_ref: None,
        }
    }

    // A fake processor that fails the first `fail_times` attempts and then
    // succeeds with one synthetic observation. Used to exercise the retry
    // loop end-to-end without touching Whisper/OCR.
    struct FlakyProcessor {
        name: &'static str,
        handles: Vec<String>,
        attempts_so_far: Arc<AtomicU32>,
        fail_times: u32,
    }

    #[async_trait]
    impl Processor for FlakyProcessor {
        fn name(&self) -> &str {
            self.name
        }
        fn handles(&self) -> Vec<String> {
            self.handles.clone()
        }
        async fn process(
            &self,
            _refs: &[DataRef],
            _capture_dir: &Path,
        ) -> anyhow::Result<Vec<Observation>> {
            let n = self.attempts_so_far.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= self.fail_times {
                anyhow::bail!("simulated failure on attempt {n}");
            }
            Ok(vec![sample_obs()])
        }
    }

    // A processor whose handles() returns the marker source the test feeds in.
    fn flaky(name: &'static str, fail_times: u32) -> Box<dyn Processor> {
        Box::new(FlakyProcessor {
            name,
            handles: vec!["test-source".into()],
            attempts_so_far: Arc::new(AtomicU32::new(0)),
            fail_times,
        })
    }

    /// One synthetic DataRef matching the flaky processor's handle.
    fn one_ref() -> Vec<DataRef> {
        vec![DataRef {
            ts: chrono::Utc::now(),
            source: "test-source".into(),
            path: "test.bin".into(),
            mime: "application/octet-stream".into(),
            metadata: None,
        }]
    }

    #[tokio::test]
    async fn retry_succeeds_on_second_attempt() {
        let tmp = TempDir::new().unwrap();
        let outcome = run_processors_with_retry(
            vec![("conn".into(), flaky("p", 1))],
            one_ref(),
            tmp.path(),
            3,
            vec![Duration::from_millis(1), Duration::from_millis(1)],
        )
        .await;
        assert_eq!(outcome.observations.len(), 1);
        assert!(outcome.failures.is_empty());
    }

    #[tokio::test]
    async fn retry_exhausted_records_failure() {
        let tmp = TempDir::new().unwrap();
        let outcome = run_processors_with_retry(
            vec![("conn".into(), flaky("p", u32::MAX))],
            one_ref(),
            tmp.path(),
            3,
            vec![Duration::from_millis(1), Duration::from_millis(1)],
        )
        .await;
        assert!(outcome.observations.is_empty());
        assert_eq!(outcome.failures.len(), 1);
        let f = &outcome.failures[0];
        assert_eq!(f.connector, "conn");
        assert_eq!(f.processor, "p");
        assert_eq!(f.attempts, 3);
        assert!(f.last_error.contains("simulated failure"));
    }

    #[tokio::test]
    async fn mixed_success_and_failure_coexist() {
        let tmp = TempDir::new().unwrap();
        let outcome = run_processors_with_retry(
            vec![
                ("good-conn".into(), flaky("good", 0)),
                ("bad-conn".into(), flaky("bad", u32::MAX)),
            ],
            one_ref(),
            tmp.path(),
            3,
            vec![Duration::from_millis(1), Duration::from_millis(1)],
        )
        .await;
        assert_eq!(outcome.observations.len(), 1, "good processor contributes");
        assert_eq!(outcome.failures.len(), 1, "bad processor fails out");
        assert_eq!(outcome.failures[0].connector, "bad-conn");
    }

    #[test]
    fn transcript_meta_serde_roundtrip_new_shape() {
        let meta = TranscriptMeta {
            connectors: vec!["audio".into(), "codex".into()],
            failed_processors: vec![ProcessorFailure {
                connector: "audio".into(),
                processor: "audio".into(),
                attempts: 3,
                last_error: "model not found".into(),
            }],
        };
        let json = serde_json::to_string(&meta).unwrap();
        let parsed: TranscriptMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, meta);
    }

    #[test]
    fn transcript_meta_serde_backwards_compat_old_shape() {
        // A sidecar written by the previous generation of this code — only
        // "connectors" present, no "failed_processors" key. Must still parse
        // into a TranscriptMeta with an empty failure list.
        let old = r#"{"connectors": ["audio", "codex"]}"#;
        let parsed: TranscriptMeta = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.connectors, vec!["audio".to_string(), "codex".to_string()]);
        assert!(parsed.failed_processors.is_empty());
    }

    #[test]
    fn read_transcript_meta_returns_none_when_absent() {
        let tmp = TempDir::new().unwrap();
        assert!(read_transcript_meta(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn read_transcript_meta_errors_on_corrupt_file() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("transcript.meta.json"), "not json").unwrap();
        assert!(read_transcript_meta(tmp.path()).is_err());
    }

    #[test]
    fn write_then_read_transcript_meta_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let meta = TranscriptMeta {
            connectors: vec!["z-first-before-sort".into(), "a-second-before-sort".into()],
            failed_processors: vec![],
        };
        write_transcript_meta(tmp.path(), &meta).unwrap();
        let loaded = read_transcript_meta(tmp.path()).unwrap().unwrap();
        // The writer sorts before persisting, so the reader sees sorted order.
        assert_eq!(
            loaded.connectors,
            vec!["a-second-before-sort".to_string(), "z-first-before-sort".to_string()]
        );
    }
}
