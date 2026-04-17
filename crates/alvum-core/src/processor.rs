//! Processor trait: reads DataRefs, produces Observations.
//!
//! Processors interpret raw captured data (audio files, screenshots, etc.) into
//! LLM-readable Observation objects. They are paired with capture sources inside
//! a Connector.

use anyhow::Result;
use async_trait::async_trait;
use std::path::Path;

use crate::data_ref::DataRef;
use crate::observation::Observation;

/// A processor reads DataRefs and produces Observations.
#[async_trait]
pub trait Processor: Send + Sync {
    /// Unique name (e.g., "whisper", "vision-local", "ocr").
    fn name(&self) -> &str;

    /// Which sources or MIME types this processor handles.
    /// Examples: ["audio-mic", "audio-system"] or ["image/png"].
    fn handles(&self) -> Vec<String>;

    /// Process the given DataRefs into Observations.
    /// `capture_dir` is the root of the capture directory for resolving relative paths.
    async fn process(
        &self,
        data_refs: &[DataRef],
        capture_dir: &Path,
    ) -> Result<Vec<Observation>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_ref::DataRef;

    struct DummyProcessor;

    #[async_trait]
    impl Processor for DummyProcessor {
        fn name(&self) -> &str { "dummy" }
        fn handles(&self) -> Vec<String> { vec!["test".into()] }
        async fn process(&self, _refs: &[DataRef], _dir: &Path) -> Result<Vec<Observation>> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn processor_trait_is_implementable() {
        let p = DummyProcessor;
        assert_eq!(p.name(), "dummy");
        assert_eq!(p.handles(), vec!["test".to_string()]);
        let result = p.process(&[], std::path::Path::new("/tmp")).await.unwrap();
        assert!(result.is_empty());
    }
}
