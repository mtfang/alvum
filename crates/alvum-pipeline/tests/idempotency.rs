//! Integration test for the ProcessedIndex sidecar.
//!
//! Builds a stub connector + counter-tracking processor, runs the pipeline
//! twice, and asserts that the second run skips refs already recorded in
//! `processed.jsonl`. Also verifies the `no_skip_processed` opt-out.

use alvum_core::capture::CaptureSource;
use alvum_core::connector::Connector;
use alvum_core::data_ref::DataRef;
use alvum_core::observation::Observation;
use alvum_core::processor::Processor;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts how many DataRefs have been fed into a processor across all runs.
struct CountingProcessor {
    received_total: Arc<AtomicUsize>,
}

#[async_trait]
impl Processor for CountingProcessor {
    fn name(&self) -> &str {
        "counter"
    }
    fn handles(&self) -> Vec<String> {
        vec!["test".into()]
    }
    async fn process(
        &self,
        data_refs: &[DataRef],
        _capture_dir: &Path,
    ) -> anyhow::Result<Vec<Observation>> {
        self.received_total
            .fetch_add(data_refs.len(), Ordering::SeqCst);
        Ok(data_refs
            .iter()
            .map(|dr| Observation {
                ts: dr.ts,
                source: dr.source.clone(),
                kind: "test".into(),
                content: dr.path.clone(),
                metadata: None,
                media_ref: None,
            })
            .collect())
    }
}

/// Connector that emits a fixed pair of DataRefs pointing at on-disk files
/// in `data_dir`. Used to drive the pipeline without real capture sources.
struct StubConnector {
    data_dir: std::path::PathBuf,
    received_total: Arc<AtomicUsize>,
}

impl Connector for StubConnector {
    fn name(&self) -> &str {
        "stub"
    }
    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>> {
        vec![]
    }
    fn processors(&self) -> Vec<Box<dyn Processor>> {
        vec![Box::new(CountingProcessor {
            received_total: self.received_total.clone(),
        })]
    }
    fn gather_data_refs(&self, _capture_dir: &Path) -> anyhow::Result<Vec<DataRef>> {
        let mut refs = Vec::new();
        for name in &["a.bin", "b.bin"] {
            let path = self.data_dir.join(name);
            refs.push(DataRef {
                ts: chrono::Utc::now(),
                source: "test".into(),
                path: path.to_string_lossy().into_owned(),
                mime: "application/octet-stream".into(),
                metadata: None,
            });
        }
        Ok(refs)
    }
}

/// Drive `extract_and_pipeline` once and report how many refs the processor
/// observed in this run alone (resets the counter before the call).
async fn run_once(
    output_dir: &Path,
    capture_dir: &Path,
    data_dir: &Path,
    no_skip: bool,
) -> usize {
    let received = Arc::new(AtomicUsize::new(0));
    let connector: Box<dyn Connector> = Box::new(StubConnector {
        data_dir: data_dir.to_path_buf(),
        received_total: received.clone(),
    });

    // Real LLM provider would be heavy; this test only cares about the
    // refs-into-processor count, so use the cli provider with a no-op
    // model name. The pipeline will fail downstream at threading/decision
    // extraction since no LLM is available, but that's *after* the
    // processor invocation we care about. We swallow the result.
    let provider = alvum_pipeline::llm::create_provider("cli", "claude-sonnet-4-6").unwrap();
    let provider: Arc<dyn alvum_core::llm::LlmProvider> = provider.into();

    let cfg = alvum_pipeline::extract::ExtractConfig {
        capture_dir: capture_dir.to_path_buf(),
        output_dir: output_dir.to_path_buf(),
        relevance_threshold: 0.5,
        resume: false,
        no_skip_processed: no_skip,
        briefing_date: None,
    };
    // We expect this to fail at the LLM-driven stage (no model available),
    // but the processor will have run by then. Discard the error.
    let _ = alvum_pipeline::extract::extract_and_pipeline(vec![connector], provider, cfg).await;
    received.load(Ordering::SeqCst)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_run_skips_processed_refs() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("a.bin"), b"alpha").unwrap();
    std::fs::write(data_dir.join("b.bin"), b"beta").unwrap();

    let capture_dir = tmp.path().join("capture");
    std::fs::create_dir_all(&capture_dir).unwrap();

    // First run on a clean output dir: both refs go through.
    let out1 = tmp.path().join("out1");
    let first = run_once(&out1, &capture_dir, &data_dir, false).await;
    assert_eq!(first, 2, "first run should process both refs");

    // Sidecar must exist now.
    let sidecar = out1.join("processed.jsonl");
    assert!(sidecar.exists(), "processed.jsonl should be created");
    let body = std::fs::read_to_string(&sidecar).unwrap();
    let lines: Vec<_> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "sidecar should record both refs");

    // Second run against the same output dir: refs are filtered out
    // before reaching the processor.
    let second = run_once(&out1, &capture_dir, &data_dir, false).await;
    assert_eq!(
        second, 0,
        "second run should skip both refs (already in processed.jsonl)"
    );

    // Third run with --no-skip-processed: every ref runs again.
    let third = run_once(&out1, &capture_dir, &data_dir, true).await;
    assert_eq!(third, 2, "no_skip_processed re-runs every ref");
}
