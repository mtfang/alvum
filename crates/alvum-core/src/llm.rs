//! LlmProvider trait — provider-agnostic LLM interface.
//!
//! Lives in alvum-core so foundational crates (alvum-episode, alvum-knowledge,
//! alvum-pipeline) can all share the trait without cyclic dependencies.
//! Concrete provider implementations (CLI, API, Ollama) live in alvum-pipeline.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub tokens_per_sec: Option<f64>,
    pub source: Option<String>,
    pub stop_reason: Option<String>,
    pub content_block_kinds: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub text: String,
    pub usage: Option<LlmUsage>,
}

impl LlmResponse {
    pub fn text(text: String) -> Self {
        Self { text, usage: None }
    }

    pub fn with_usage(text: String, usage: Option<LlmUsage>) -> Self {
        Self { text, usage }
    }
}

pub fn estimate_tokens(chars: usize) -> u64 {
    // The providers do not all expose token accounting. A 4-char token
    // estimate keeps CLI and local-model observability comparable without
    // pretending it is a billing-grade metric.
    ((chars as u64) + 3) / 4
}

fn tokens_per_second(tokens: u64, latency_ms: u64) -> Option<f64> {
    if tokens == 0 || latency_ms == 0 {
        return None;
    }
    Some(tokens as f64 / (latency_ms as f64 / 1000.0))
}

pub fn emit_llm_call_start(provider: &str, call_site: &str, prompt_chars: usize) {
    crate::pipeline_events::emit(crate::pipeline_events::Event::LlmCallStart {
        call_site: call_site.to_string(),
        provider: provider.to_string(),
        prompt_chars,
        prompt_tokens_estimate: estimate_tokens(prompt_chars),
    });
}

pub fn emit_llm_call_end(
    provider: &str,
    call_site: &str,
    prompt_chars: usize,
    latency_ms: u64,
    outcome: &Result<LlmResponse>,
) {
    let (response_chars, usage, ok) = match outcome {
        Ok(response) => (response.text.len(), response.usage.clone(), true),
        Err(_) => (0, None, false),
    };
    let prompt_tokens_estimate = estimate_tokens(prompt_chars);
    let response_tokens_estimate = estimate_tokens(response_chars);
    crate::pipeline_events::emit(crate::pipeline_events::Event::LlmCallEnd {
        call_site: call_site.to_string(),
        provider: provider.to_string(),
        prompt_chars,
        latency_ms,
        response_chars,
        input_tokens: usage.as_ref().and_then(|u| u.input_tokens),
        output_tokens: usage.as_ref().and_then(|u| u.output_tokens),
        total_tokens: usage.as_ref().and_then(|u| {
            u.total_tokens.or_else(|| {
                Some(u.input_tokens.unwrap_or(0) + u.output_tokens.unwrap_or(0))
                    .filter(|total| *total > 0)
            })
        }),
        tokens_per_sec: usage.as_ref().and_then(|u| u.tokens_per_sec),
        token_source: usage.as_ref().and_then(|u| u.source.clone()),
        prompt_tokens_estimate,
        response_tokens_estimate,
        total_tokens_estimate: prompt_tokens_estimate + response_tokens_estimate,
        tokens_per_sec_estimate: tokens_per_second(response_tokens_estimate, latency_ms),
        stop_reason: usage.as_ref().and_then(|u| u.stop_reason.clone()),
        content_block_kinds: usage.as_ref().and_then(|u| u.content_block_kinds.clone()),
        attempts: 1,
        ok,
    });
    if let Err(error) = outcome {
        crate::pipeline_events::emit(crate::pipeline_events::Event::Error {
            source: format!("llm/{provider}/{call_site}"),
            message: format!("{error:#}"),
        });
    }
}

/// Provider-agnostic LLM interface. Implementations handle the transport
/// (HTTP API, CLI subprocess, local model) — callers just send prompts.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String>;

    async fn complete_with_usage(&self, system: &str, user_message: &str) -> Result<LlmResponse> {
        self.complete(system, user_message)
            .await
            .map(LlmResponse::text)
    }

    /// Complete with an image attachment. Providers that support vision implement
    /// this directly; others fall back to text-only (image is ignored).
    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        let _ = (system, user_message, image_path);
        anyhow::bail!(
            "provider {} does not support image input in Alvum",
            self.name()
        )
    }

    async fn complete_with_image_with_usage(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<LlmResponse> {
        self.complete_with_image(system, user_message, image_path)
            .await
            .map(LlmResponse::text)
    }

    async fn complete_observed_response(
        &self,
        system: &str,
        user_message: &str,
        call_site: &str,
    ) -> Result<LlmResponse> {
        let prompt_chars = system.len() + user_message.len();
        emit_llm_call_start(self.name(), call_site, prompt_chars);
        let started = std::time::Instant::now();
        let outcome = self.complete_with_usage(system, user_message).await;
        emit_llm_call_end(
            self.name(),
            call_site,
            prompt_chars,
            started.elapsed().as_millis() as u64,
            &outcome,
        );
        outcome
    }

    async fn complete_with_image_observed_response(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
        call_site: &str,
    ) -> Result<LlmResponse> {
        let prompt_chars = system.len() + user_message.len();
        emit_llm_call_start(self.name(), call_site, prompt_chars);
        let started = std::time::Instant::now();
        let outcome = self
            .complete_with_image_with_usage(system, user_message, image_path)
            .await;
        emit_llm_call_end(
            self.name(),
            call_site,
            prompt_chars,
            started.elapsed().as_millis() as u64,
            &outcome,
        );
        outcome
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
    provider
        .complete_observed_response(system, user_message, call_site)
        .await
        .map(|response| response.text)
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
    provider
        .complete_with_image_observed_response(system, user_message, image_path, call_site)
        .await
        .map(|response| response.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::io::BufRead;

    struct FailingProvider;

    #[async_trait::async_trait]
    impl LlmProvider for FailingProvider {
        async fn complete(&self, _system: &str, _user_message: &str) -> Result<String> {
            Err(anyhow!(
                "Bedrock Converse API call failed: throttled by provider"
            ))
        }

        fn name(&self) -> &str {
            "bedrock"
        }
    }

    fn read_events(path: &std::path::Path) -> Vec<serde_json::Value> {
        let file = std::fs::File::open(path).unwrap();
        std::io::BufReader::new(file)
            .lines()
            .filter_map(|line| line.ok())
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(&line).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn observed_provider_failure_emits_traceable_error_event() {
        let _g = crate::pipeline_events::test_env_lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var("ALVUM_PIPELINE_EVENTS_FILE", tmp.path()) };
        crate::pipeline_events::init();

        let result = FailingProvider
            .complete_observed_response("system", "user", "thread/chunk_0/retry")
            .await;

        assert!(result.is_err());
        let events = read_events(tmp.path());
        assert_eq!(events[0]["kind"], "llm_call_start");
        assert_eq!(events[1]["kind"], "llm_call_end");
        assert_eq!(events[1]["ok"], false);
        assert_eq!(events[2]["kind"], "error");
        assert_eq!(events[2]["source"], "llm/bedrock/thread/chunk_0/retry");
        assert!(
            events[2]["message"]
                .as_str()
                .unwrap()
                .contains("Bedrock Converse API call failed")
        );

        unsafe { std::env::remove_var("ALVUM_PIPELINE_EVENTS_FILE") };
    }

    #[test]
    fn llm_call_end_includes_provider_stop_reason_and_block_kinds() {
        let _g = crate::pipeline_events::test_env_lock();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        unsafe { std::env::set_var("ALVUM_PIPELINE_EVENTS_FILE", tmp.path()) };
        crate::pipeline_events::init();

        let outcome = Ok(LlmResponse::with_usage(
            "[]".into(),
            Some(LlmUsage {
                input_tokens: Some(120),
                output_tokens: Some(34),
                total_tokens: Some(154),
                tokens_per_sec: None,
                source: Some("bedrock".into()),
                stop_reason: Some("max_tokens".into()),
                content_block_kinds: Some(vec!["reasoning_content".into(), "text".into()]),
            }),
        ));
        emit_llm_call_end("bedrock", "thread/chunk_0", 512, 1000, &outcome);

        let events = read_events(tmp.path());
        assert_eq!(events[0]["kind"], "llm_call_end");
        assert_eq!(events[0]["stop_reason"], "max_tokens");
        assert_eq!(events[0]["content_block_kinds"][0], "reasoning_content");
        assert_eq!(events[0]["content_block_kinds"][1], "text");

        unsafe { std::env::remove_var("ALVUM_PIPELINE_EVENTS_FILE") };
    }
}
