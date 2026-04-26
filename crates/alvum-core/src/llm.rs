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

/// Wrap a provider call with `LlmCallStart` / `LlmCallEnd` events. The
/// helper is the canonical entry point for every pipeline LLM call so
/// the event channel sees a consistent before/after pair regardless of
/// provider transport. Callers pass a stable `call_site` label
/// (e.g. `"thread/chunk_0"`, `"thread/chunk_0/retry"`, `"distill"`) so
/// the popover and `alvum tail` can correlate events back to the stage
/// that issued them.
///
/// Retries are surfaced by the caller invoking this helper a second
/// time with a distinct `call_site` (e.g. suffix `/retry`); the event
/// stream then carries two start/end pairs that downstream tooling can
/// count without needing access to provider-internal retry state.
pub async fn complete_observed(
    provider: &dyn LlmProvider,
    system: &str,
    user_message: &str,
    call_site: &str,
) -> Result<String> {
    let prompt_chars = system.len() + user_message.len();
    crate::pipeline_events::emit(crate::pipeline_events::Event::LlmCallStart {
        call_site: call_site.to_string(),
        prompt_chars,
    });
    let started = std::time::Instant::now();
    let outcome = provider.complete(system, user_message).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let (response_chars, ok) = match &outcome {
        Ok(r) => (r.len(), true),
        Err(_) => (0, false),
    };
    crate::pipeline_events::emit(crate::pipeline_events::Event::LlmCallEnd {
        call_site: call_site.to_string(),
        latency_ms,
        response_chars,
        // Provider-internal transport retries (e.g. ClaudeCliProvider's
        // 3-attempt loop) are not exposed through the trait, so this
        // helper can only count from its own POV — one observed call,
        // one attempt. Pipeline-level retries surface as multiple
        // calls with distinct call_sites.
        attempts: 1,
        ok,
    });
    outcome
}

/// Image-attachment counterpart of [`complete_observed`]. Same event
/// vocabulary; image bytes don't enter the `prompt_chars` count (the
/// path string is included in the user_message length the caller
/// constructs, if they want it counted).
pub async fn complete_with_image_observed(
    provider: &dyn LlmProvider,
    system: &str,
    user_message: &str,
    image_path: &Path,
    call_site: &str,
) -> Result<String> {
    let prompt_chars = system.len() + user_message.len();
    crate::pipeline_events::emit(crate::pipeline_events::Event::LlmCallStart {
        call_site: call_site.to_string(),
        prompt_chars,
    });
    let started = std::time::Instant::now();
    let outcome = provider
        .complete_with_image(system, user_message, image_path)
        .await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let (response_chars, ok) = match &outcome {
        Ok(r) => (r.len(), true),
        Err(_) => (0, false),
    };
    crate::pipeline_events::emit(crate::pipeline_events::Event::LlmCallEnd {
        call_site: call_site.to_string(),
        latency_ms,
        response_chars,
        attempts: 1,
        ok,
    });
    outcome
}
