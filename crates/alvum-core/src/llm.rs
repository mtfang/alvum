//! LlmProvider trait — provider-agnostic LLM interface.
//!
//! Lives in alvum-core so foundational crates (alvum-episode, alvum-knowledge,
//! alvum-pipeline) can all share the trait without cyclic dependencies.
//! Concrete provider implementations (CLI, API, Ollama) live in alvum-pipeline.

use anyhow::Result;
use std::path::Path;

/// Provider-agnostic LLM interface. Implementations handle the transport
/// (HTTP API, CLI subprocess, local model) — callers just send prompts.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String>;

    /// Complete with an image attachment. Providers that support vision implement
    /// this directly; others fall back to text-only (image is ignored).
    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        let _ = image_path; // default: ignore image
        self.complete(system, user_message).await
    }

    fn name(&self) -> &str;
}
