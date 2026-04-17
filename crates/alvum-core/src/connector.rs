//! Connector trait: the user-facing plugin concept.
//!
//! A Connector is what the user adds and manages. Internally, it bundles one or
//! more capture sources (daemons or importers) with one or more processors
//! (which interpret raw data into Observations).

use crate::capture::CaptureSource;
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
}
