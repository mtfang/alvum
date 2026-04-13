//! Trait contract for capture sources — always-on daemons that write raw data
//! to the capture directory. Each source runs until the shutdown signal fires.

use anyhow::Result;
use std::path::Path;
use tokio::sync::watch;

/// A capture source that runs continuously and writes files to the capture directory.
/// Sources own a subdirectory under `capture_dir` (e.g., `audio/mic/`, `screen/`).
/// They must exit cleanly when the shutdown receiver transitions to `true`.
#[async_trait::async_trait]
pub trait CaptureSource: Send + Sync {
    /// Unique name matching the config key (e.g., "audio-mic", "screen").
    fn name(&self) -> &str;

    /// Run the capture loop. Blocks until shutdown signal fires or an error occurs.
    /// Implementations must flush any buffered data before returning.
    async fn run(&self, capture_dir: &Path, shutdown: watch::Receiver<bool>) -> Result<()>;
}
