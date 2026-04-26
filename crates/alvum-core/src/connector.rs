//! Connector trait: the user-facing plugin concept.
//!
//! A Connector is what the user adds and manages. Internally, it bundles one or
//! more capture sources (daemons or importers) with one or more processors
//! (which interpret raw data into Observations).

use crate::capture::CaptureSource;
use crate::data_ref::DataRef;
use crate::processor::Processor;

/// A Connector bundles capture sources and processors into a complete plugin.
pub trait Connector: Send + Sync {
    /// Unique name (e.g., "audio", "screen", "claude-code").
    fn name(&self) -> &str;

    /// Capture sources owned by this connector. May be empty for one-shot
    /// importers that don't run as daemons (e.g., claude-code).
    fn capture_sources(&self) -> Vec<Box<dyn CaptureSource>>;

    /// Processors owned by this connector. Each handles specific sources
    /// or MIME types produced by this connector's capture sources.
    fn processors(&self) -> Vec<Box<dyn Processor>>;

    /// Enumerate DataRefs available for processing within `capture_dir`.
    /// Each connector decides how to scan: filesystem walk, JSONL index,
    /// session-file enumeration, etc. The pipeline merges all connectors'
    /// results, then dispatches them to processors via `Processor::handles()`.
    fn gather_data_refs(&self, capture_dir: &std::path::Path) -> anyhow::Result<Vec<DataRef>>;

    /// Sources this connector expects to produce in normal operation.
    /// Used by the pipeline's pre-processing inventory pass to surface
    /// silent modalities — if `expected_sources` lists a source that
    /// `gather_data_refs` returned zero refs for, the pipeline emits a
    /// `Warning` event rather than letting the modality vanish silently.
    ///
    /// Returned as an owned `Vec` so connectors can compute the list
    /// from runtime config (e.g. only include `audio-mic` when the mic
    /// is enabled). Default is empty: a connector that doesn't override
    /// is treated as opportunistic and won't trigger silent-modality
    /// warnings.
    fn expected_sources(&self) -> Vec<&'static str> {
        Vec::new()
    }
}
