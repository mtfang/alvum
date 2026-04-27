use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

// Re-export the LlmProvider trait from alvum-core so callers using
// `alvum_pipeline::llm::LlmProvider` continue to work transparently.
pub use alvum_core::llm::LlmProvider;

// ---------------------------------------------------------------------------
// Claude CLI provider — shells out to `claude -p` (no API key needed)
// ---------------------------------------------------------------------------

pub struct ClaudeCliProvider {
    model: String,
}

impl ClaudeCliProvider {
    pub fn new(model: String) -> Self {
        Self { model }
    }
}

#[async_trait::async_trait]
impl LlmProvider for ClaudeCliProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        use tokio::io::AsyncWriteExt;

        let max_retries = 3;

        for attempt in 0..max_retries {
            if attempt > 0 {
                let delay = 10 * attempt as u64;
                tracing::warn!(attempt, delay_secs = delay, "retrying after transient error");
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }

            debug!(model = %self.model, attempt, system_len = system.len(), user_len = user_message.len(), "sending to claude CLI");

            let sys_prompt_file = std::env::temp_dir().join(format!("alvum-sys-prompt-{}.txt", std::process::id()));
            tokio::fs::write(&sys_prompt_file, system).await
                .context("failed to write system prompt temp file")?;

            let mut child = tokio::process::Command::new("claude")
                .args([
                    "-p",
                    "--no-session-persistence",
                    "--model", &self.model,
                    "--output-format", "text",
                    "--system-prompt-file", &sys_prompt_file.to_string_lossy(),
                ])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn `claude` — is Claude Code installed?")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(user_message.as_bytes()).await?;
            }

            let output = child.wait_with_output().await
                .context("claude process failed")?;

            let _ = tokio::fs::remove_file(&sys_prompt_file).await;

            if output.status.success() {
                let text = String::from_utf8(output.stdout)
                    .context("claude CLI output is not valid UTF-8")?;
                debug!(response_len = text.len(), "received claude CLI response");
                return Ok(text);
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let is_transient = stdout.contains("ConnectionRefused")
                || stdout.contains("rate")
                || stdout.contains("overloaded")
                || stderr.contains("ConnectionRefused");

            if is_transient && attempt < max_retries - 1 {
                tracing::warn!(stdout = %&stdout[..stdout.len().min(200)], "transient error, will retry");
                continue;
            }

            bail!("claude CLI exited with {}:\nstderr: {stderr}\nstdout (first 500): {}",
                output.status, &stdout[..stdout.len().min(500)]);
        }

        bail!("all {max_retries} attempts failed")
    }

    fn name(&self) -> &str {
        "claude-cli"
    }
}

// ---------------------------------------------------------------------------
// Codex CLI provider — shells out to `codex exec` (subscription auth via
// `codex login`, no API key needed). Codex is an *agent* — it normally
// runs tools and sandboxed shell commands — but for our briefing prompts
// (well-formed JSON requests, no file mutations expected) we want plain
// text responses only. We pass `--dangerously-bypass-approvals-and-sandbox`
// so it never blocks on approval prompts, and pipe the response through
// `--output-last-message FILE` so we get the model's final answer free of
// agent-loop log noise.
// ---------------------------------------------------------------------------

pub struct CodexCliProvider {
    model: String,
}

impl CodexCliProvider {
    pub fn new(model: String) -> Self {
        Self { model }
    }
}

#[async_trait::async_trait]
impl LlmProvider for CodexCliProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        use tokio::io::AsyncWriteExt;

        let max_retries = 3;

        for attempt in 0..max_retries {
            if attempt > 0 {
                let delay = 10 * attempt as u64;
                tracing::warn!(attempt, delay_secs = delay, "retrying after transient codex error");
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }

            // Codex doesn't accept a separate system prompt file like Claude;
            // its model takes a single combined message. Use a clear delimiter
            // between system instructions and user content so the model can
            // distinguish the two halves itself.
            let combined = format!(
                "<system_instructions>\n{system}\n</system_instructions>\n\n<user_message>\n{user_message}\n</user_message>"
            );

            let last_msg_file = std::env::temp_dir()
                .join(format!("alvum-codex-out-{}.txt", std::process::id()));
            let _ = tokio::fs::remove_file(&last_msg_file).await;

            debug!(
                model = %self.model, attempt,
                combined_len = combined.len(),
                "sending to codex CLI"
            );

            // Build args dynamically — `--model` only when the caller
            // gave us a concrete model. Codex rejects an unknown model
            // with a 400 invalid_request_error (e.g. an Anthropic model
            // name passed to OpenAI). Letting Codex pick from
            // ~/.codex/config.toml is the safe default.
            let last_msg_path = last_msg_file.to_string_lossy().to_string();
            let mut codex_args: Vec<&str> = vec![
                "exec",
                "--skip-git-repo-check",
                "--dangerously-bypass-approvals-and-sandbox",
                "--output-last-message", &last_msg_path,
            ];
            if !self.model.is_empty() {
                codex_args.push("--model");
                codex_args.push(&self.model);
            }
            codex_args.push("-"); // read prompt from stdin

            let mut child = tokio::process::Command::new("codex")
                .args(&codex_args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn `codex` — is Codex CLI installed?")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(combined.as_bytes()).await?;
            }

            let output = child.wait_with_output().await
                .context("codex process failed")?;

            if output.status.success() {
                let text = tokio::fs::read_to_string(&last_msg_file).await
                    .context("codex CLI produced no last-message file")?;
                let _ = tokio::fs::remove_file(&last_msg_file).await;
                debug!(response_len = text.len(), "received codex CLI response");
                return Ok(text);
            }

            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let is_transient = stderr.contains("rate")
                || stderr.contains("overloaded")
                || stderr.contains("timeout")
                || stderr.contains("ConnectionRefused");

            if is_transient && attempt < max_retries - 1 {
                tracing::warn!(stderr = %&stderr[..stderr.len().min(200)], "transient codex error, will retry");
                continue;
            }

            let _ = tokio::fs::remove_file(&last_msg_file).await;
            bail!("codex CLI exited with {}:\nstderr (first 500): {}\nstdout (first 500): {}",
                output.status,
                &stderr[..stderr.len().min(500)],
                &stdout[..stdout.len().min(500)]);
        }

        bail!("all {max_retries} codex attempts failed")
    }

    fn name(&self) -> &str {
        "codex-cli"
    }
}

// ---------------------------------------------------------------------------
// Anthropic API provider — direct HTTP calls (needs ANTHROPIC_API_KEY)
// ---------------------------------------------------------------------------

pub struct AnthropicApiProvider {
    api_key: String,
    model: String,
    http: reqwest::Client,
}

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessage>,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

/// A content block in a multimodal API message (text or image).
#[derive(Serialize)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ImageSource },
}

#[derive(Serialize)]
struct ImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

/// API message with multimodal content blocks.
#[derive(Serialize)]
struct ApiMessageMultimodal {
    role: String,
    content: Vec<ApiContentBlock>,
}

/// API request that accepts multimodal content.
#[derive(Serialize)]
struct ApiRequestMultimodal {
    model: String,
    max_tokens: u32,
    system: String,
    messages: Vec<ApiMessageMultimodal>,
}

impl AnthropicApiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicApiProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        let request = ApiRequest {
            model: self.model.clone(),
            max_tokens: 16000,
            system: system.to_string(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: user_message.to_string(),
            }],
        };

        debug!(model = %self.model, system_len = system.len(), user_len = user_message.len(), "sending to Anthropic API");

        let response = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send request to Claude API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Claude API error {status}: {body}");
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .context("failed to parse Claude API response")?;

        let text = api_response
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        debug!(response_len = text.len(), "received API response");
        Ok(text)
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        use base64::Engine;

        let image_bytes = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("failed to read image: {}", image_path.display()))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        let media_type = match image_path.extension().and_then(|e| e.to_str()) {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            Some("gif") => "image/gif",
            _ => "image/png",
        };

        let request = ApiRequestMultimodal {
            model: self.model.clone(),
            max_tokens: 16000,
            system: system.to_string(),
            messages: vec![ApiMessageMultimodal {
                role: "user".into(),
                content: vec![
                    ApiContentBlock::Image {
                        source: ImageSource {
                            source_type: "base64".into(),
                            media_type: media_type.into(),
                            data: b64,
                        },
                    },
                    ApiContentBlock::Text {
                        text: user_message.to_string(),
                    },
                ],
            }],
        };

        debug!(model = %self.model, image = %image_path.display(), "sending image to Anthropic API");

        let response = self
            .http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("failed to send image request to Claude API")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Claude API vision error {status}: {body}");
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .context("failed to parse Claude API vision response")?;

        let text = api_response
            .content
            .iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");

        debug!(response_len = text.len(), "received API vision response");
        Ok(text)
    }

    fn name(&self) -> &str {
        "anthropic-api"
    }
}

// ---------------------------------------------------------------------------
// Bedrock provider — Anthropic-on-Bedrock via AWS SDK Converse API
//
// Auth via the standard AWS credential chain (env vars, ~/.aws/credentials,
// IAM instance profile, SSO). Model IDs are AWS-namespaced — e.g.
// `anthropic.claude-sonnet-4-20250514-v1:0` for direct invocation, or
// `us.anthropic.claude-sonnet-4-20250514-v1:0` for cross-region inference
// profiles. The Converse API normalises Anthropic-style "system + messages"
// requests across model families, so the call shape is similar to the
// direct Anthropic HTTP API but auth is signed with AWS SigV4.
// ---------------------------------------------------------------------------

pub struct BedrockProvider {
    client: aws_sdk_bedrockruntime::Client,
    model_id: String,
}

impl BedrockProvider {
    /// Construct from the user's AWS credential chain. Async because the
    /// SDK config loader may need to fetch IMDS / SSO tokens.
    pub async fn new(model_id: String) -> Result<Self> {
        let cfg = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        let client = aws_sdk_bedrockruntime::Client::new(&cfg);
        Ok(Self { client, model_id })
    }
}

#[async_trait::async_trait]
impl LlmProvider for BedrockProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        use aws_sdk_bedrockruntime::types::{
            ContentBlock, ConversationRole, Message, SystemContentBlock,
        };

        debug!(model = %self.model_id, "sending to Bedrock Converse API");

        let user_msg = Message::builder()
            .role(ConversationRole::User)
            .content(ContentBlock::Text(user_message.to_string()))
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build Bedrock user message: {e}"))?;

        let resp = self
            .client
            .converse()
            .model_id(&self.model_id)
            .messages(user_msg)
            .system(SystemContentBlock::Text(system.to_string()))
            .send()
            .await
            .context("Bedrock Converse API call failed")?;

        // The Converse response has output -> Message -> Vec<ContentBlock>.
        // We expect a single Text block; concatenate any extras defensively.
        let output = resp
            .output
            .context("Bedrock returned no output")?;
        let message = output
            .as_message()
            .map_err(|_| anyhow::anyhow!("Bedrock returned non-message output"))?;
        let text: String = message
            .content
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        if text.is_empty() {
            bail!("Bedrock response contained no text blocks");
        }
        debug!(response_len = text.len(), "received Bedrock response");
        Ok(text)
    }

    fn name(&self) -> &str {
        "bedrock"
    }
}

// ---------------------------------------------------------------------------
// Ollama provider — local models via HTTP (localhost:11434)
// ---------------------------------------------------------------------------

pub struct OllamaProvider {
    model: String,
    http: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    pub fn new(model: String) -> Self {
        Self {
            model,
            http: reqwest::Client::new(),
            base_url: "http://localhost:11434".into(),
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        debug!(model = %self.model, "sending to Ollama");

        let body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "system": system,
            "prompt": user_message,
        });

        let response = self
            .http
            .post(format!("{}/api/generate", self.base_url))
            .json(&body)
            .send()
            .await
            .context("failed to connect to Ollama — is it running?")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Ollama error: {body}");
        }

        let resp: serde_json::Value = response.json().await?;
        let text = resp["response"]
            .as_str()
            .unwrap_or("")
            .to_string();

        debug!(response_len = text.len(), "received Ollama response");
        Ok(text)
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        use base64::Engine;

        let image_bytes = tokio::fs::read(image_path)
            .await
            .with_context(|| format!("failed to read image: {}", image_path.display()))?;

        let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        debug!(model = %self.model, image = %image_path.display(), "sending image to Ollama");

        let body = serde_json::json!({
            "model": self.model,
            "stream": false,
            "system": system,
            "prompt": user_message,
            "images": [b64],
        });

        let response = self
            .http
            .post(format!("{}/api/generate", self.base_url))
            .json(&body)
            .send()
            .await
            .context("failed to connect to Ollama — is it running?")?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("Ollama vision error: {body}");
        }

        let resp: serde_json::Value = response.json().await?;
        let text = resp["response"]
            .as_str()
            .unwrap_or("")
            .to_string();

        debug!(response_len = text.len(), "received Ollama vision response");
        Ok(text)
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

// ---------------------------------------------------------------------------
// Fallback provider — used by `auto` so runtime auth failures on an earlier
// provider can fall through to the next authenticated backend.
// ---------------------------------------------------------------------------

struct FallbackProvider {
    providers: Vec<Box<dyn LlmProvider>>,
    unavailable: Mutex<HashSet<String>>,
    cooling_down: Mutex<HashMap<String, tokio::time::Instant>>,
    usage_cooldown: Duration,
}

impl FallbackProvider {
    fn new(providers: Vec<Box<dyn LlmProvider>>) -> Self {
        Self::with_usage_cooldown(providers, Duration::from_secs(5 * 60 * 60))
    }

    fn with_usage_cooldown(providers: Vec<Box<dyn LlmProvider>>, usage_cooldown: Duration) -> Self {
        Self {
            providers,
            unavailable: Mutex::new(HashSet::new()),
            cooling_down: Mutex::new(HashMap::new()),
            usage_cooldown,
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for FallbackProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        let mut errors = Vec::new();

        loop {
            let mut next_retry_at = None;

            for provider in &self.providers {
                if self.provider_unavailable(provider.name()) {
                    continue;
                }
                if let Some(retry_at) = self.provider_cooling_down(provider.name()) {
                    next_retry_at = min_instant(next_retry_at, retry_at);
                    continue;
                }
                match provider.complete(system, user_message).await {
                    Ok(response) => return Ok(response),
                    Err(err) => {
                        let failure_kind = classify_provider_error(&err);
                        warn!(
                            provider = provider.name(),
                            failure_kind = failure_kind.as_str(),
                            error = %err,
                            "auto provider failed; trying next provider"
                        );
                        if let Some(retry_at) = self.record_failure(provider.name(), failure_kind) {
                            next_retry_at = min_instant(next_retry_at, retry_at);
                        }
                        errors.push(format!("{}: {err:#}", provider.name()));
                    }
                }
            }

            if let Some(retry_at) = next_retry_at {
                let wait = retry_at.saturating_duration_since(tokio::time::Instant::now());
                warn!(
                    wait_secs = wait.as_secs(),
                    "all usable auto providers are cooling down; waiting before retry"
                );
                tokio::time::sleep_until(retry_at).await;
                continue;
            }

            bail!("all auto providers failed:\n{}", errors.join("\n"))
        }
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        let mut errors = Vec::new();

        loop {
            let mut next_retry_at = None;

            for provider in &self.providers {
                if self.provider_unavailable(provider.name()) {
                    continue;
                }
                if let Some(retry_at) = self.provider_cooling_down(provider.name()) {
                    next_retry_at = min_instant(next_retry_at, retry_at);
                    continue;
                }
                match provider
                    .complete_with_image(system, user_message, image_path)
                    .await
                {
                    Ok(response) => return Ok(response),
                    Err(err) => {
                        let failure_kind = classify_provider_error(&err);
                        warn!(
                            provider = provider.name(),
                            failure_kind = failure_kind.as_str(),
                            error = %err,
                            "auto provider image call failed; trying next provider"
                        );
                        if let Some(retry_at) = self.record_failure(provider.name(), failure_kind) {
                            next_retry_at = min_instant(next_retry_at, retry_at);
                        }
                        errors.push(format!("{}: {err:#}", provider.name()));
                    }
                }
            }

            if let Some(retry_at) = next_retry_at {
                let wait = retry_at.saturating_duration_since(tokio::time::Instant::now());
                warn!(
                    wait_secs = wait.as_secs(),
                    "all usable auto providers are cooling down; waiting before retry"
                );
                tokio::time::sleep_until(retry_at).await;
                continue;
            }

            bail!("all auto providers failed:\n{}", errors.join("\n"))
        }
    }

    fn name(&self) -> &str {
        "auto"
    }
}

impl FallbackProvider {
    fn provider_unavailable(&self, name: &str) -> bool {
        self.unavailable
            .lock()
            .map(|unavailable| unavailable.contains(name))
            .unwrap_or(false)
    }

    fn provider_cooling_down(&self, name: &str) -> Option<tokio::time::Instant> {
        let now = tokio::time::Instant::now();
        let mut cooling_down = self.cooling_down.lock().ok()?;
        match cooling_down.get(name).copied() {
            Some(retry_at) if retry_at > now => Some(retry_at),
            Some(_) => {
                cooling_down.remove(name);
                None
            }
            None => None,
        }
    }

    fn record_failure(
        &self,
        name: &str,
        failure_kind: ProviderFailureKind,
    ) -> Option<tokio::time::Instant> {
        match failure_kind {
            ProviderFailureKind::AuthUnavailable => {
                if let Ok(mut unavailable) = self.unavailable.lock() {
                    unavailable.insert(name.to_string());
                }
                None
            }
            ProviderFailureKind::UsageLimited => {
                let retry_at = tokio::time::Instant::now() + self.usage_cooldown;
                if let Ok(mut cooling_down) = self.cooling_down.lock() {
                    cooling_down.insert(name.to_string(), retry_at);
                }
                Some(retry_at)
            }
            ProviderFailureKind::Retryable => None,
        }
    }
}

fn min_instant(
    current: Option<tokio::time::Instant>,
    candidate: tokio::time::Instant,
) -> Option<tokio::time::Instant> {
    Some(match current {
        Some(current) if current <= candidate => current,
        _ => candidate,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderFailureKind {
    AuthUnavailable,
    UsageLimited,
    Retryable,
}

impl ProviderFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::AuthUnavailable => "auth_unavailable",
            Self::UsageLimited => "usage_limited",
            Self::Retryable => "retryable",
        }
    }
}

pub fn classify_provider_error_status(error: &anyhow::Error) -> &'static str {
    classify_provider_error(error).as_str()
}

fn classify_provider_error(error: &anyhow::Error) -> ProviderFailureKind {
    let text = format!("{error:#}").to_ascii_lowercase();
    let usage_patterns = [
        "usage limit",
        "rate limit",
        "quota",
        "too many requests",
        "429",
        "purchase more credits",
        "upgrade to plus",
        "request to your admin",
    ];
    let auth_patterns = [
        "does not have access",
        "please login again",
        "not logged in",
        "unauthorized",
        "authentication",
        "invalid api key",
        "forbidden",
        "accessdenied",
        "expiredtoken",
        "unrecognizedclient",
    ];

    if usage_patterns.iter().any(|pattern| text.contains(pattern)) {
        ProviderFailureKind::UsageLimited
    } else if auth_patterns.iter().any(|pattern| text.contains(pattern)) {
        ProviderFailureKind::AuthUnavailable
    } else {
        ProviderFailureKind::Retryable
    }
}

// ---------------------------------------------------------------------------
// Provider construction helper
// ---------------------------------------------------------------------------

fn model_for_provider(provider: &str, requested_model: &str) -> String {
    match provider {
        "codex" | "codex-cli"
            if requested_model.is_empty() || requested_model.starts_with("claude-") =>
        {
            String::new()
        }
        _ => requested_model.to_string(),
    }
}

/// Create an LLM provider by name. Sync variant — used by callers that
/// can't await. Bedrock and `auto` need async (Bedrock for SDK config
/// loading; auto because it may select Bedrock); use `create_provider_async`
/// for those.
pub fn create_provider(provider: &str, model: &str) -> Result<Box<dyn LlmProvider>> {
    match provider {
        "cli" | "claude-cli" => {
            info!(model, "using Claude CLI provider");
            Ok(Box::new(ClaudeCliProvider::new(model.to_string())))
        }
        "codex" | "codex-cli" => {
            let model = model_for_provider(provider, model);
            info!(model, "using Codex CLI provider");
            Ok(Box::new(CodexCliProvider::new(model)))
        }
        "api" | "anthropic-api" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY required for the Anthropic API provider")?;
            info!(model, "using Anthropic API provider");
            Ok(Box::new(AnthropicApiProvider::new(api_key, model.to_string())))
        }
        "ollama" => {
            info!(model, "using Ollama provider (local)");
            Ok(Box::new(OllamaProvider::new(model.to_string())))
        }
        "bedrock" | "auto" => bail!(
            "the {provider:?} provider requires async construction; \
             call create_provider_async instead"
        ),
        other => bail!(
            "unknown provider: {other}. \
             Options: claude-cli, codex-cli, anthropic-api, bedrock, ollama, auto"
        ),
    }
}

/// Async variant. Use when the chosen provider is `bedrock` (SDK config
/// load) or `auto` (which may select bedrock). Falls through to the sync
/// builder for non-async providers so callers can use this uniformly.
pub async fn create_provider_async(
    provider: &str,
    model: &str,
) -> Result<Box<dyn LlmProvider>> {
    match provider {
        "bedrock" => {
            info!(model, "using Bedrock provider");
            Ok(Box::new(BedrockProvider::new(model.to_string()).await?))
        }
        "auto" => select_first_authenticated(model).await,
        other => create_provider(other, model),
    }
}

/// Walk a fixed preference list and return every provider whose cheap
/// availability check succeeds. The returned `FallbackProvider` tries
/// them in order on each call, so a runtime auth failure on Claude can
/// fall through to Codex without aborting the briefing. Order:
///
///   1. claude-cli  — local subscription, zero config
///   2. codex-cli   — local subscription, zero config
///   3. anthropic-api — explicit API key in env
///   4. bedrock     — explicit AWS credentials in env
///   5. ollama      — last-resort local fallback
///
/// The availability checks are intentionally cheap: binary-on-PATH for
/// the CLI providers, env-var-set for the cloud providers. We don't
/// probe the network here; real auth failures are handled by the
/// fallback wrapper at completion time.
async fn select_first_authenticated(model: &str) -> Result<Box<dyn LlmProvider>> {
    let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();

    if cli_binary_available("claude") {
        info!(model, "auto: adding claude-cli");
        providers.push(Box::new(ClaudeCliProvider::new(model.to_string())));
    }
    if cli_binary_available("codex") {
        let codex_model = model_for_provider("codex-cli", model);
        info!(model = %codex_model, "auto: adding codex-cli");
        providers.push(Box::new(CodexCliProvider::new(codex_model)));
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        info!(model, "auto: adding anthropic-api");
        providers.push(Box::new(AnthropicApiProvider::new(key, model.to_string())));
    }
    if aws_credentials_available() {
        info!(model, "auto: adding bedrock");
        providers.push(Box::new(BedrockProvider::new(model.to_string()).await?));
    }
    if cli_binary_available("ollama") {
        info!(model, "auto: adding ollama (last resort)");
        providers.push(Box::new(OllamaProvider::new(model.to_string())));
    }

    if !providers.is_empty() {
        return Ok(Box::new(FallbackProvider::new(providers)));
    }

    bail!(
        "no LLM provider available. Authenticate one of:\n  \
         • Claude Code subscription:  run `claude login`\n  \
         • Codex / ChatGPT Plus:      run `codex login`\n  \
         • Anthropic API:             export ANTHROPIC_API_KEY=...\n  \
         • AWS Bedrock:               export AWS_PROFILE=... or AWS_ACCESS_KEY_ID=...\n  \
         • Local Ollama:              install from ollama.ai"
    )
}

fn cli_binary_available(name: &str) -> bool {
    // `which` is universal on macOS/Linux. We check the exit status only;
    // ignoring stdout means broken locales etc. don't trip the detection.
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn aws_credentials_available() -> bool {
    // The AWS SDK accepts any of these as valid auth indicators; if none
    // is set we don't even try to construct the client (which would slow
    // startup with IMDS probes).
    std::env::var("AWS_PROFILE").is_ok()
        || std::env::var("AWS_ACCESS_KEY_ID").is_ok()
        || std::env::var("AWS_SESSION_TOKEN").is_ok()
        || dirs::home_dir()
            .map(|h| h.join(".aws/credentials").exists() || h.join(".aws/config").exists())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn api_request_serializes_correctly() {
        let req = ApiRequest {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 8000,
            system: "You are helpful.".into(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: "Hello".into(),
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["messages"][0]["role"], "user");
    }

    #[test]
    fn create_cli_provider() {
        let provider = create_provider("cli", "sonnet").unwrap();
        assert_eq!(provider.name(), "claude-cli");
    }

    #[test]
    fn create_ollama_provider() {
        let provider = create_provider("ollama", "llama3.2").unwrap();
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn unknown_provider_errors() {
        assert!(create_provider("unknown", "model").is_err());
    }

    #[test]
    fn fallback_provider_uses_next_provider_after_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let provider = FallbackProvider::new(vec![
            Box::new(ScriptedProvider::err("claude-cli", "subscription expired")),
            Box::new(ScriptedProvider::ok("codex-cli", "OK from codex")),
        ]);

        let response = rt
            .block_on(async { provider.complete("system", "user").await })
            .unwrap();

        assert_eq!(response, "OK from codex");
    }

    #[test]
    fn provider_error_classifier_marks_auth_and_usage_limits_unavailable() {
        let claude_no_access = anyhow::anyhow!(
            "claude CLI exited with exit status: 1:\nstdout (first 500): \
             Your organization does not have access to Claude. Please login again"
        );
        let codex_usage_limit = anyhow::anyhow!(
            "codex CLI exited with exit status: 1:\nstderr (first 500): \
             You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage \
             to purchase more credits"
        );
        let transient = anyhow::anyhow!("failed to connect to Ollama — is it running?");

        assert_eq!(
            classify_provider_error(&claude_no_access),
            ProviderFailureKind::AuthUnavailable
        );
        assert_eq!(
            classify_provider_error(&codex_usage_limit),
            ProviderFailureKind::UsageLimited
        );
        assert_eq!(
            classify_provider_error_status(&codex_usage_limit),
            "usage_limited"
        );
        assert_eq!(
            classify_provider_error(&transient),
            ProviderFailureKind::Retryable
        );
    }

    #[test]
    fn fallback_provider_skips_usage_limited_provider_after_first_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let limited_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::with_usage_cooldown(
            vec![
                Box::new(ScriptedProvider::sequence(
                    "codex-cli",
                    vec![
                        Err("You've hit your usage limit for the 5-hour block"),
                        Ok("should not be used"),
                    ],
                    limited_calls.clone(),
                )),
                Box::new(ScriptedProvider::sequence(
                    "ollama",
                    vec![Ok("first fallback"), Ok("second fallback")],
                    fallback_calls.clone(),
                )),
            ],
            Duration::from_secs(60),
        );

        let first = rt
            .block_on(async { provider.complete("system", "first").await })
            .unwrap();
        let second = rt
            .block_on(async { provider.complete("system", "second").await })
            .unwrap();

        assert_eq!(first, "first fallback");
        assert_eq!(second, "second fallback");
        assert_eq!(limited_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn fallback_provider_waits_for_usage_limit_cooldown_when_every_provider_is_blocked() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let limited_calls = Arc::new(AtomicUsize::new(0));
        let provider = FallbackProvider::with_usage_cooldown(
            vec![Box::new(ScriptedProvider::sequence(
                "codex-cli",
                vec![
                    Err("You've hit your usage limit for the 5-hour block"),
                    Ok("after cooldown"),
                ],
                limited_calls.clone(),
            ))],
            Duration::from_millis(10),
        );

        let response = rt
            .block_on(async { provider.complete("system", "user").await })
            .unwrap();

        assert_eq!(response, "after cooldown");
        assert_eq!(limited_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn codex_uses_configured_default_when_given_claude_model() {
        assert_eq!(model_for_provider("codex-cli", "claude-sonnet-4-6"), "");
        assert_eq!(model_for_provider("codex-cli", "gpt-5.5"), "gpt-5.5");
    }

    #[test]
    fn multimodal_request_serializes_correctly() {
        let req = ApiRequestMultimodal {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 4096,
            system: "Describe this image.".into(),
            messages: vec![ApiMessageMultimodal {
                role: "user".into(),
                content: vec![
                    ApiContentBlock::Image {
                        source: ImageSource {
                            source_type: "base64".into(),
                            media_type: "image/png".into(),
                            data: "iVBORw0KGgo=".into(),
                        },
                    },
                    ApiContentBlock::Text {
                        text: "What is on this screen?".into(),
                    },
                ],
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "claude-sonnet-4-6");
        assert_eq!(json["messages"][0]["content"][0]["type"], "image");
        assert_eq!(json["messages"][0]["content"][0]["source"]["type"], "base64");
        assert_eq!(json["messages"][0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(json["messages"][0]["content"][1]["type"], "text");
        assert_eq!(json["messages"][0]["content"][1]["text"], "What is on this screen?");
    }

    #[test]
    fn image_source_serializes_type_field() {
        let src = ImageSource {
            source_type: "base64".into(),
            media_type: "image/png".into(),
            data: "abc123".into(),
        };
        let json = serde_json::to_value(&src).unwrap();
        assert_eq!(json["type"], "base64");
        assert_eq!(json["media_type"], "image/png");
    }

    struct ScriptedProvider {
        name: &'static str,
        responses: Mutex<VecDeque<std::result::Result<&'static str, &'static str>>>,
        calls: Arc<AtomicUsize>,
    }

    impl ScriptedProvider {
        fn ok(name: &'static str, response: &'static str) -> Self {
            Self::sequence(name, vec![Ok(response)], Arc::new(AtomicUsize::new(0)))
        }

        fn err(name: &'static str, error: &'static str) -> Self {
            Self::sequence(name, vec![Err(error)], Arc::new(AtomicUsize::new(0)))
        }

        fn sequence(
            name: &'static str,
            responses: Vec<std::result::Result<&'static str, &'static str>>,
            calls: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                name,
                responses: Mutex::new(VecDeque::from(responses)),
                calls,
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for ScriptedProvider {
        async fn complete(&self, _system: &str, _user_message: &str) -> Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let response = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err("scripted provider exhausted"));
            response
                .map(str::to_string)
                .map_err(|e| anyhow::anyhow!(e))
        }

        fn name(&self) -> &str {
            self.name
        }
    }
}
