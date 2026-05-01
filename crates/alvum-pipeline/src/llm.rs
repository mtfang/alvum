use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

// Re-export the LlmProvider trait from alvum-core so callers using
// `alvum_pipeline::llm::LlmProvider` continue to work transparently.
pub use alvum_core::llm::LlmProvider;
use alvum_core::llm::{LlmResponse, LlmUsage, emit_llm_call_end, emit_llm_call_start};

const CLI_PROVIDER_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const OLLAMA_MODEL_LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);

// ---------------------------------------------------------------------------
// Claude CLI provider — shells out to `claude -p` and lets the CLI use
// whichever backend/auth mode it is configured for.
// ---------------------------------------------------------------------------

pub struct ClaudeCliProvider {
    model: String,
}

impl ClaudeCliProvider {
    pub fn new(model: String) -> Self {
        Self { model }
    }
}

fn claude_cli_args<'a>(model: &'a str, sys_prompt_file: &'a str) -> Vec<&'a str> {
    let mut args = vec!["-p", "--no-session-persistence"];
    if !model.trim().is_empty() {
        args.push("--model");
        args.push(model);
    }
    args.extend([
        "--output-format",
        "text",
        "--system-prompt-file",
        sys_prompt_file,
    ]);
    args
}

#[async_trait::async_trait]
impl LlmProvider for ClaudeCliProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        use tokio::io::AsyncWriteExt;

        let max_retries = 3;

        for attempt in 0..max_retries {
            if attempt > 0 {
                let delay = 10 * attempt as u64;
                tracing::warn!(
                    attempt,
                    delay_secs = delay,
                    "retrying after transient error"
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }

            debug!(model = %self.model, attempt, system_len = system.len(), user_len = user_message.len(), "sending to claude CLI");

            let sys_prompt_file =
                std::env::temp_dir().join(format!("alvum-sys-prompt-{}.txt", std::process::id()));
            tokio::fs::write(&sys_prompt_file, system)
                .await
                .context("failed to write system prompt temp file")?;

            let sys_prompt_path = sys_prompt_file.to_string_lossy().to_string();
            let claude_args = claude_cli_args(&self.model, &sys_prompt_path);
            let mut child = tokio::process::Command::new("claude")
                .args(&claude_args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn `claude` — is Claude Code installed?")?;

            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(user_message.as_bytes()).await?;
            }

            let output =
                match tokio::time::timeout(CLI_PROVIDER_TIMEOUT, child.wait_with_output()).await {
                    Ok(result) => result.context("claude process failed")?,
                    Err(_) => {
                        let _ = tokio::fs::remove_file(&sys_prompt_file).await;
                        bail!(
                            "claude CLI timed out after {}s",
                            CLI_PROVIDER_TIMEOUT.as_secs()
                        );
                    }
                };

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

            bail!(
                "claude CLI exited with {}:\nstderr: {stderr}\nstdout (first 500): {}",
                output.status,
                &stdout[..stdout.len().min(500)]
            );
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
                tracing::warn!(
                    attempt,
                    delay_secs = delay,
                    "retrying after transient codex error"
                );
                tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            }

            // Codex doesn't accept a separate system prompt file like Claude;
            // its model takes a single combined message. Use a clear delimiter
            // between system instructions and user content so the model can
            // distinguish the two halves itself.
            let combined = format!(
                "<system_instructions>\n{system}\n</system_instructions>\n\n<user_message>\n{user_message}\n</user_message>"
            );

            let last_msg_file =
                std::env::temp_dir().join(format!("alvum-codex-out-{}.txt", std::process::id()));
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
                "--output-last-message",
                &last_msg_path,
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

            let output =
                match tokio::time::timeout(CLI_PROVIDER_TIMEOUT, child.wait_with_output()).await {
                    Ok(result) => result.context("codex process failed")?,
                    Err(_) => {
                        let _ = tokio::fs::remove_file(&last_msg_file).await;
                        bail!(
                            "codex CLI timed out after {}s",
                            CLI_PROVIDER_TIMEOUT.as_secs()
                        );
                    }
                };

            if output.status.success() {
                let text = tokio::fs::read_to_string(&last_msg_file)
                    .await
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
            bail!(
                "codex CLI exited with {}:\nstderr (first 500): {}\nstdout (first 500): {}",
                output.status,
                &stderr[..stderr.len().min(500)],
                &stdout[..stdout.len().min(500)]
            );
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
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ApiUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
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

fn anthropic_response_text(api_response: &ApiResponse) -> String {
    api_response
        .content
        .iter()
        .filter(|b| b.block_type == "text")
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

fn anthropic_usage(usage: Option<ApiUsage>) -> Option<LlmUsage> {
    let usage = usage?;
    let input = usage.input_tokens;
    let output = usage.output_tokens;
    Some(LlmUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: Some(input.unwrap_or(0) + output.unwrap_or(0)).filter(|total| *total > 0),
        tokens_per_sec: None,
        source: Some("anthropic-api".into()),
    })
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicApiProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        self.complete_with_usage(system, user_message)
            .await
            .map(|response| response.text)
    }

    async fn complete_with_usage(&self, system: &str, user_message: &str) -> Result<LlmResponse> {
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

        let text = anthropic_response_text(&api_response);
        let usage = anthropic_usage(api_response.usage);

        debug!(response_len = text.len(), "received API response");
        Ok(LlmResponse::with_usage(text, usage))
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        self.complete_with_image_with_usage(system, user_message, image_path)
            .await
            .map(|response| response.text)
    }

    async fn complete_with_image_with_usage(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<LlmResponse> {
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

        let text = anthropic_response_text(&api_response);
        let usage = anthropic_usage(api_response.usage);

        debug!(response_len = text.len(), "received API vision response");
        Ok(LlmResponse::with_usage(text, usage))
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
        Self::with_options(model_id, None, None, None).await
    }

    pub async fn with_options(
        model_id: String,
        profile: Option<String>,
        region: Option<String>,
        extra_path: Option<String>,
    ) -> Result<Self> {
        let cfg = crate::bedrock::sdk_config(profile, region, extra_path).await;
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
        let output = resp.output.context("Bedrock returned no output")?;
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
        Self::with_base_url(model, "http://localhost:11434".into())
    }

    pub fn with_base_url(model: String, base_url: String) -> Self {
        Self {
            model,
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[derive(Deserialize)]
struct OllamaGenerateResponse {
    response: Option<String>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
}

fn ollama_usage(resp: &OllamaGenerateResponse) -> Option<LlmUsage> {
    let input = resp.prompt_eval_count;
    let output = resp.eval_count;
    let tokens_per_sec = match (output, resp.eval_duration) {
        (Some(tokens), Some(duration_ns)) if tokens > 0 && duration_ns > 0 => {
            Some(tokens as f64 / (duration_ns as f64 / 1_000_000_000.0))
        }
        _ => None,
    };
    Some(LlmUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: Some(input.unwrap_or(0) + output.unwrap_or(0)).filter(|total| *total > 0),
        tokens_per_sec,
        source: Some("ollama".into()),
    })
    .filter(|usage| {
        usage.input_tokens.is_some()
            || usage.output_tokens.is_some()
            || usage.total_tokens.is_some()
            || usage.tokens_per_sec.is_some()
    })
}

#[async_trait::async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        self.complete_with_usage(system, user_message)
            .await
            .map(|response| response.text)
    }

    async fn complete_with_usage(&self, system: &str, user_message: &str) -> Result<LlmResponse> {
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

        let resp: OllamaGenerateResponse = response.json().await?;
        let text = resp.response.clone().unwrap_or_default();
        let usage = ollama_usage(&resp);

        debug!(response_len = text.len(), "received Ollama response");
        Ok(LlmResponse::with_usage(text, usage))
    }

    async fn complete_with_image(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<String> {
        self.complete_with_image_with_usage(system, user_message, image_path)
            .await
            .map(|response| response.text)
    }

    async fn complete_with_image_with_usage(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
    ) -> Result<LlmResponse> {
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

        let resp: OllamaGenerateResponse = response.json().await?;
        let text = resp.response.clone().unwrap_or_default();
        let usage = ollama_usage(&resp);

        debug!(response_len = text.len(), "received Ollama vision response");
        Ok(LlmResponse::with_usage(text, usage))
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

    async fn complete_observed_response(
        &self,
        system: &str,
        user_message: &str,
        call_site: &str,
    ) -> Result<LlmResponse> {
        let mut errors = Vec::new();
        let prompt_chars = system.len() + user_message.len();

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

                emit_llm_call_start(provider.name(), call_site, prompt_chars);
                let started = std::time::Instant::now();
                let outcome = provider.complete_with_usage(system, user_message).await;
                emit_llm_call_end(
                    provider.name(),
                    call_site,
                    prompt_chars,
                    started.elapsed().as_millis() as u64,
                    &outcome,
                );

                match outcome {
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

    async fn complete_with_image_observed_response(
        &self,
        system: &str,
        user_message: &str,
        image_path: &Path,
        call_site: &str,
    ) -> Result<LlmResponse> {
        let mut errors = Vec::new();
        let prompt_chars = system.len() + user_message.len();

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

                emit_llm_call_start(provider.name(), call_site, prompt_chars);
                let started = std::time::Instant::now();
                let outcome = provider
                    .complete_with_image_with_usage(system, user_message, image_path)
                    .await;
                emit_llm_call_end(
                    provider.name(),
                    call_site,
                    prompt_chars,
                    started.elapsed().as_millis() as u64,
                    &outcome,
                );

                match outcome {
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
            ProviderFailureKind::NotInstalled | ProviderFailureKind::AuthUnavailable => {
                if let Ok(mut unavailable) = self.unavailable.lock() {
                    unavailable.insert(name.to_string());
                }
                None
            }
            ProviderFailureKind::RequiresInferenceProfile => {
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
    NotInstalled,
    AuthUnavailable,
    RequiresInferenceProfile,
    UsageLimited,
    Retryable,
}

impl ProviderFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::AuthUnavailable => "auth_unavailable",
            Self::RequiresInferenceProfile => "requires_inference_profile",
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
    if (text.contains("on-demand throughput") && text.contains("inference profile"))
        || text.contains("retry your request with the id or arn of an inference profile")
    {
        return ProviderFailureKind::RequiresInferenceProfile;
    }
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
    let install_patterns = [
        "failed to spawn",
        "is claude code installed",
        "is codex cli installed",
        "no such file or directory",
    ];
    let credential_process_patterns = [
        "credential_process",
        "profilefile provider",
        "credentials provider",
        "isengardcli",
        "siengarcli",
    ];

    if credential_process_patterns
        .iter()
        .any(|pattern| text.contains(pattern))
    {
        ProviderFailureKind::AuthUnavailable
    } else if install_patterns
        .iter()
        .any(|pattern| text.contains(pattern))
    {
        ProviderFailureKind::NotInstalled
    } else if usage_patterns.iter().any(|pattern| text.contains(pattern)) {
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
        "claude" | "claude-cli"
            if requested_model.is_empty() || requested_model == "claude-sonnet-4-6" =>
        {
            String::new()
        }
        "codex" | "codex-cli"
            if requested_model.is_empty() || requested_model.starts_with("claude-") =>
        {
            String::new()
        }
        _ => requested_model.to_string(),
    }
}

fn provider_setting_string(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    key: &str,
) -> Option<String> {
    config
        .provider(provider)
        .and_then(|provider| provider.settings.get(key))
        .and_then(|value| match value {
            toml::Value::String(s) => Some(s.clone()),
            toml::Value::Integer(n) => Some(n.to_string()),
            toml::Value::Float(n) => Some(n.to_string()),
            toml::Value::Boolean(v) => Some(v.to_string()),
            _ => None,
        })
        .filter(|value| !value.trim().is_empty())
}

fn default_model_for_provider(provider: &str, requested_model: &str) -> String {
    match provider {
        "ollama" if requested_model.is_empty() || requested_model.starts_with("claude-") => {
            String::new()
        }
        "bedrock" if requested_model.is_empty() || requested_model.starts_with("claude-") => {
            String::new()
        }
        _ => model_for_provider(provider, requested_model),
    }
}

fn default_image_model_for_provider(provider: &str) -> String {
    match provider {
        "ollama" => String::new(),
        "bedrock" => String::new(),
        "claude" | "cli" | "claude-cli" => String::new(),
        "codex" | "codex-cli" => String::new(),
        _ => "claude-sonnet-4-6".into(),
    }
}

#[derive(Clone)]
struct OllamaInstalledModel {
    name: String,
    text: bool,
    image: bool,
    audio: bool,
}

impl OllamaInstalledModel {
    fn supports(&self, modality: &str) -> bool {
        match modality {
            "text" => self.text,
            "image" => self.image,
            "audio" => self.audio,
            _ => false,
        }
    }
}

fn ollama_modalities_from_json(json: &serde_json::Value) -> (bool, bool, bool) {
    let mut text = false;
    let mut image = false;
    let mut audio = false;
    if let Some(values) = json.get("capabilities").and_then(|value| value.as_array()) {
        for value in values {
            if let Some(item) = value.as_str().map(|item| item.to_ascii_lowercase()) {
                match item.as_str() {
                    "text" | "completion" | "chat" => text = true,
                    "image" | "vision" => image = true,
                    "audio" | "speech" => audio = true,
                    _ => {}
                }
            }
        }
    }
    (text, image, audio)
}

async fn ollama_installed_models(base_url: &str) -> Result<Vec<OllamaInstalledModel>> {
    let base_url = base_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(OLLAMA_MODEL_LOOKUP_TIMEOUT)
        .build()?;
    let tags_json: serde_json::Value = client
        .get(format!("{base_url}/api/tags"))
        .send()
        .await
        .context("failed to query installed Ollama models")?
        .error_for_status()
        .context("Ollama installed model list request failed")?
        .json()
        .await
        .context("Ollama returned malformed installed model list JSON")?;
    let names = tags_json
        .get("models")
        .and_then(|models| models.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| {
            model
                .get("model")
                .or_else(|| model.get("name"))
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();

    let mut models = Vec::new();
    for name in names {
        let show_json: serde_json::Value = client
            .post(format!("{base_url}/api/show"))
            .json(&serde_json::json!({ "model": name }))
            .send()
            .await
            .with_context(|| format!("failed to query Ollama model details for {name}"))?
            .error_for_status()
            .with_context(|| format!("Ollama model details request failed for {name}"))?
            .json()
            .await
            .with_context(|| format!("Ollama returned malformed model details JSON for {name}"))?;
        let (text, image, audio) = ollama_modalities_from_json(&show_json);
        models.push(OllamaInstalledModel {
            name,
            text,
            image,
            audio,
        });
    }
    Ok(models)
}

fn ollama_configured_model_for_modality(
    config: &alvum_core::config::AlvumConfig,
    requested_model: &str,
    modality: &str,
) -> Option<String> {
    match modality {
        "text" => provider_setting_string(config, "ollama", "text_model")
            .or_else(|| provider_setting_string(config, "ollama", "model"))
            .or_else(|| {
                (!requested_model.trim().is_empty() && !requested_model.starts_with("claude-"))
                    .then(|| requested_model.to_string())
            }),
        "image" => provider_setting_string(config, "ollama", "image_model"),
        "audio" => provider_setting_string(config, "ollama", "audio_model"),
        _ => None,
    }
}

async fn resolve_ollama_model_for_modality(
    config: &alvum_core::config::AlvumConfig,
    requested_model: &str,
    modality: &str,
    base_url: &str,
) -> Result<String> {
    let models = ollama_installed_models(base_url).await?;
    let installed_names = models
        .iter()
        .map(|model| model.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    if let Some(configured) =
        ollama_configured_model_for_modality(config, requested_model, modality)
    {
        let Some(model) = models.iter().find(|model| model.name == configured) else {
            bail!(
                "Ollama {modality} model {configured:?} is not installed. Installed models: {}",
                if installed_names.is_empty() {
                    "none"
                } else {
                    installed_names.as_str()
                }
            );
        };
        if !model.supports(modality) {
            bail!("Ollama model {configured:?} is installed but does not support {modality} input");
        }
        return Ok(configured);
    }

    models
        .iter()
        .find(|model| model.supports(modality))
        .map(|model| model.name.clone())
        .with_context(|| {
            format!(
                "No installed Ollama model supports {modality} input. Installed models: {}",
                if installed_names.is_empty() {
                    "none"
                } else {
                    installed_names.as_str()
                }
            )
        })
}

fn canonical_provider_name(provider: &str) -> &str {
    match provider {
        "cli" => "claude-cli",
        "codex" => "codex-cli",
        "api" => "anthropic-api",
        other => other,
    }
}

fn adapter_supports_modality(provider: &str, modality: &str) -> bool {
    match modality {
        "text" => true,
        "image" => matches!(
            canonical_provider_name(provider),
            "anthropic-api" | "ollama"
        ),
        "audio" => false,
        _ => false,
    }
}

fn configured_model_for_modality(
    config: &alvum_core::config::AlvumConfig,
    provider: &str,
    requested_model: &str,
    modality: &str,
) -> String {
    match modality {
        "text" => {
            if let Some(model) = provider_setting_string(config, provider, "text_model")
                .or_else(|| provider_setting_string(config, provider, "model"))
            {
                return model_for_provider(provider, &model);
            }
            default_model_for_provider(provider, requested_model)
        }
        "image" => provider_setting_string(config, provider, "image_model")
            .map(|model| model_for_provider(provider, &model))
            .unwrap_or_else(|| default_image_model_for_provider(provider)),
        "audio" => provider_setting_string(config, provider, "audio_model")
            .map(|model| model_for_provider(provider, &model))
            .unwrap_or_default(),
        _ => default_model_for_provider(provider, requested_model),
    }
}

fn bedrock_configured_model_for_modality(
    config: &alvum_core::config::AlvumConfig,
    requested_model: &str,
    modality: &str,
) -> Option<String> {
    match modality {
        "text" => provider_setting_string(config, "bedrock", "text_model")
            .or_else(|| provider_setting_string(config, "bedrock", "model")),
        "image" => provider_setting_string(config, "bedrock", "image_model"),
        "audio" => provider_setting_string(config, "bedrock", "audio_model"),
        _ => None,
    }
    .or_else(|| {
        let requested = requested_model.trim();
        (!requested.is_empty() && !requested.starts_with("claude-")).then(|| requested.to_string())
    })
}

async fn resolve_bedrock_invoke_target(
    config: &alvum_core::config::AlvumConfig,
    requested_model: &str,
    modality: &str,
) -> Result<crate::bedrock::BedrockInvokeTarget> {
    let configured = bedrock_configured_model_for_modality(config, requested_model, modality);
    let profile = provider_setting_string(config, "bedrock", "aws_profile");
    let region = provider_setting_string(config, "bedrock", "aws_region");
    let extra_path = provider_setting_string(config, "bedrock", "extra_path");
    match crate::bedrock::BedrockCatalog::load(profile, region, extra_path).await {
        Ok(catalog) => catalog.resolve_invoke_target(configured.as_deref(), modality),
        Err(error) => {
            if let Some(configured) = configured {
                warn!(
                    error = %error,
                    configured,
                    "Bedrock catalog lookup failed; using explicit configured target"
                );
                return Ok(crate::bedrock::unverified_configured_target(&configured));
            }
            Err(error).context(
                "Bedrock live catalog is required when no Bedrock model or inference profile is configured",
            )
        }
    }
}

fn anthropic_api_key() -> Result<String> {
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(key);
        }
    }
    alvum_core::keychain::read_provider_secret("anthropic-api", "api_key")?
        .filter(|key| !key.trim().is_empty())
        .context(
            "Anthropic API key required. Add it in Alvum Providers setup or set ANTHROPIC_API_KEY",
        )
}

/// Create an LLM provider by name. Sync variant — used by callers that
/// can't await. Bedrock and `auto` need async (Bedrock for SDK config
/// loading; auto because it may select Bedrock); use `create_provider_async`
/// for those.
pub fn create_provider(provider: &str, model: &str) -> Result<Box<dyn LlmProvider>> {
    create_provider_for_modality(provider, model, "text")
}

fn create_provider_for_modality(
    provider: &str,
    model: &str,
    modality: &str,
) -> Result<Box<dyn LlmProvider>> {
    if !adapter_supports_modality(provider, modality) {
        bail!("provider {provider:?} does not support {modality} input through Alvum's adapter");
    }
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    match provider {
        "cli" | "claude-cli" => {
            let model = configured_model_for_modality(&config, "claude-cli", model, modality);
            info!(model, "using Claude CLI provider");
            Ok(Box::new(ClaudeCliProvider::new(model)))
        }
        "codex" | "codex-cli" => {
            let model = configured_model_for_modality(&config, "codex-cli", model, modality);
            info!(model, "using Codex CLI provider");
            Ok(Box::new(CodexCliProvider::new(model)))
        }
        "api" | "anthropic-api" => {
            let api_key = anthropic_api_key()?;
            let model = configured_model_for_modality(&config, "anthropic-api", model, modality);
            info!(model, "using Anthropic API provider");
            Ok(Box::new(AnthropicApiProvider::new(api_key, model)))
        }
        "ollama" => {
            let model = configured_model_for_modality(&config, "ollama", model, modality);
            let base_url = provider_setting_string(&config, "ollama", "base_url")
                .unwrap_or_else(|| "http://localhost:11434".into());
            info!(model, "using Ollama provider (local)");
            Ok(Box::new(OllamaProvider::with_base_url(model, base_url)))
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
pub async fn create_provider_async(provider: &str, model: &str) -> Result<Box<dyn LlmProvider>> {
    create_provider_for_modality_async(provider, model, "text").await
}

pub async fn create_provider_for_modality_async(
    provider: &str,
    model: &str,
    modality: &str,
) -> Result<Box<dyn LlmProvider>> {
    match provider {
        "bedrock" => {
            if !adapter_supports_modality(provider, modality) {
                bail!(
                    "provider {provider:?} does not support {modality} input through Alvum's adapter"
                );
            }
            let config = alvum_core::config::AlvumConfig::load()
                .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
            let profile = provider_setting_string(&config, "bedrock", "aws_profile");
            let region = provider_setting_string(&config, "bedrock", "aws_region");
            let extra_path = provider_setting_string(&config, "bedrock", "extra_path");
            let target = resolve_bedrock_invoke_target(&config, model, modality).await?;
            info!(
                model = %target.invoke_id,
                configured = ?bedrock_configured_model_for_modality(&config, model, modality),
                source = %target.source,
                "using Bedrock provider"
            );
            Ok(Box::new(
                BedrockProvider::with_options(target.invoke_id, profile, region, extra_path)
                    .await?,
            ))
        }
        "ollama" => {
            if !adapter_supports_modality(provider, modality) {
                bail!(
                    "provider {provider:?} does not support {modality} input through Alvum's adapter"
                );
            }
            let config = alvum_core::config::AlvumConfig::load()
                .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
            let base_url = provider_setting_string(&config, "ollama", "base_url")
                .unwrap_or_else(|| "http://localhost:11434".into());
            let model =
                resolve_ollama_model_for_modality(&config, model, modality, &base_url).await?;
            info!(model, "using Ollama provider (local)");
            Ok(Box::new(OllamaProvider::with_base_url(model, base_url)))
        }
        "auto" => select_first_authenticated_for_modality(model, modality).await,
        other => create_provider_for_modality(other, model, modality),
    }
}

/// Walk a fixed preference list and return every provider whose cheap
/// availability check succeeds. The returned `FallbackProvider` tries
/// them in order on each call, so a runtime auth failure on Claude can
/// fall through to Codex without aborting the briefing. Order:
///
///   1. claude-cli  — Claude CLI configured backend
///   2. codex-cli   — local subscription, zero config
///   3. anthropic-api — explicit API key in env
///   4. bedrock     — explicit AWS credentials in env
///   5. ollama      — last-resort local fallback
///
/// The availability checks are intentionally cheap: binary-on-PATH for
/// the CLI providers, env-var-set for the cloud providers. We don't
/// probe the network here; real auth failures are handled by the
/// fallback wrapper at completion time.
async fn select_first_authenticated_for_modality(
    model: &str,
    modality: &str,
) -> Result<Box<dyn LlmProvider>> {
    let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());

    if adapter_supports_modality("claude-cli", modality)
        && config.provider_enabled("claude-cli")
        && cli_binary_available("claude")
    {
        let provider_model = configured_model_for_modality(&config, "claude-cli", model, modality);
        info!(model = %provider_model, "auto: adding claude-cli");
        providers.push(Box::new(ClaudeCliProvider::new(provider_model)));
    }
    if adapter_supports_modality("codex-cli", modality)
        && config.provider_enabled("codex-cli")
        && cli_binary_available("codex")
    {
        let codex_model = configured_model_for_modality(&config, "codex-cli", model, modality);
        info!(model = %codex_model, "auto: adding codex-cli");
        providers.push(Box::new(CodexCliProvider::new(codex_model)));
    }
    if adapter_supports_modality("anthropic-api", modality)
        && config.provider_enabled("anthropic-api")
    {
        if let Ok(key) = anthropic_api_key() {
            let provider_model =
                configured_model_for_modality(&config, "anthropic-api", model, modality);
            info!(model = %provider_model, "auto: adding anthropic-api");
            providers.push(Box::new(AnthropicApiProvider::new(key, provider_model)));
        }
    }
    if adapter_supports_modality("bedrock", modality)
        && config.provider_enabled("bedrock")
        && aws_credentials_available()
    {
        match resolve_bedrock_invoke_target(&config, model, modality).await {
            Ok(target) => {
                let profile = provider_setting_string(&config, "bedrock", "aws_profile");
                let region = provider_setting_string(&config, "bedrock", "aws_region");
                let extra_path = provider_setting_string(&config, "bedrock", "extra_path");
                info!(
                    model = %target.invoke_id,
                    source = %target.source,
                    "auto: adding bedrock"
                );
                providers.push(Box::new(
                    BedrockProvider::with_options(target.invoke_id, profile, region, extra_path)
                        .await?,
                ));
            }
            Err(error) => {
                warn!(error = %error, "auto: skipping bedrock");
            }
        }
    }
    if adapter_supports_modality("ollama", modality)
        && config.provider_enabled("ollama")
        && cli_binary_available("ollama")
    {
        let base_url = provider_setting_string(&config, "ollama", "base_url")
            .unwrap_or_else(|| "http://localhost:11434".into());
        match resolve_ollama_model_for_modality(&config, model, modality, &base_url).await {
            Ok(provider_model) => {
                info!(model = %provider_model, "auto: adding ollama (last resort)");
                providers.push(Box::new(OllamaProvider::with_base_url(
                    provider_model,
                    base_url,
                )));
            }
            Err(error) => {
                warn!(error = %error, "auto: skipping ollama");
            }
        }
    }

    if !providers.is_empty() {
        return Ok(Box::new(FallbackProvider::new(providers)));
    }

    bail!(
        "no LLM provider available. Authenticate one of:\n  \
         • Claude CLI:                configure Claude CLI auth/backend\n  \
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

fn env_var_available(name: &str) -> bool {
    std::env::var(name)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn aws_credentials_available() -> bool {
    // The AWS SDK accepts any of these as valid auth indicators; if none
    // is set we don't even try to construct the client (which would slow
    // startup with IMDS probes).
    let config = alvum_core::config::AlvumConfig::load()
        .unwrap_or_else(|_| alvum_core::config::AlvumConfig::default());
    env_var_available("AWS_PROFILE")
        || env_var_available("AWS_ACCESS_KEY_ID")
        || env_var_available("AWS_SESSION_TOKEN")
        || provider_setting_string(&config, "bedrock", "aws_profile").is_some()
        || dirs::home_dir()
            .map(|h| h.join(".aws/credentials").exists() || h.join(".aws/config").exists())
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn claude_cli_omits_model_for_cli_default() {
        let args = claude_cli_args("", "/tmp/system.txt");
        assert!(!args.contains(&"--model"));
        assert!(args.contains(&"--system-prompt-file"));
    }

    #[test]
    fn claude_cli_includes_explicit_model_override() {
        let args = claude_cli_args("sonnet", "/tmp/system.txt");
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "--model" && pair[1] == "sonnet")
        );
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
        let bedrock_credential_helper = anyhow::anyhow!(
            "Bedrock Converse API call failed: credentials provider was not properly configured: \
             ProfileFile provider failed to run credential_process: isengardcli not found: \
             No such file or directory"
        );
        let bedrock_requires_profile = anyhow::anyhow!(
            "Bedrock Converse API call failed: service error: ValidationException: \
             Invocation of model ID anthropic.claude-opus-4-20250514-v1:0 with on-demand \
             throughput isn't supported. Retry your request with the ID or ARN of an inference \
             profile that contains this model."
        );

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
        assert_eq!(
            classify_provider_error(&bedrock_credential_helper),
            ProviderFailureKind::AuthUnavailable
        );
        assert_eq!(
            classify_provider_error_status(&bedrock_requires_profile),
            "requires_inference_profile"
        );
    }

    #[test]
    fn bedrock_resolver_prefers_global_inference_profile_for_configured_base_model() {
        let catalog = crate::bedrock::BedrockCatalog::from_test_records(
            vec![crate::bedrock::BedrockFoundationModel::test(
                "anthropic.claude-opus-4-20250514-v1:0",
                "Claude Opus 4",
                true,
                &["TEXT", "IMAGE"],
                &["TEXT"],
                &[],
            )],
            vec![
                crate::bedrock::BedrockInferenceProfile::test_system(
                    "us.anthropic.claude-opus-4-20250514-v1:0",
                    "US Claude Opus 4",
                    &["anthropic.claude-opus-4-20250514-v1:0"],
                ),
                crate::bedrock::BedrockInferenceProfile::test_system(
                    "global.anthropic.claude-opus-4-20250514-v1:0",
                    "Global Claude Opus 4",
                    &["anthropic.claude-opus-4-20250514-v1:0"],
                ),
            ],
        );

        let target = catalog
            .resolve_invoke_target(Some("anthropic.claude-opus-4-20250514-v1:0"), "text")
            .unwrap();

        assert_eq!(
            target.invoke_id,
            "global.anthropic.claude-opus-4-20250514-v1:0"
        );
        assert_eq!(target.source, "inference_profile");
        assert_eq!(
            target.source_model_id.as_deref(),
            Some("anthropic.claude-opus-4-20250514-v1:0")
        );
        assert!(target.input_support.text);
        assert!(target.input_support.image);
    }

    #[test]
    fn bedrock_resolver_preserves_explicit_profile_arn() {
        let catalog = crate::bedrock::BedrockCatalog::from_test_records(
            vec![crate::bedrock::BedrockFoundationModel::test(
                "anthropic.claude-opus-4-20250514-v1:0",
                "Claude Opus 4",
                true,
                &["TEXT"],
                &["TEXT"],
                &[],
            )],
            vec![crate::bedrock::BedrockInferenceProfile::test_system(
                "global.anthropic.claude-opus-4-20250514-v1:0",
                "Global Claude Opus 4",
                &["anthropic.claude-opus-4-20250514-v1:0"],
            )],
        );
        let profile_arn = "arn:aws:bedrock:us-east-1::inference-profile/global.anthropic.claude-opus-4-20250514-v1:0";

        let target = catalog
            .resolve_invoke_target(Some(profile_arn), "text")
            .unwrap();

        assert_eq!(target.invoke_id, profile_arn);
        assert_eq!(target.source, "configured");
    }

    #[test]
    fn bedrock_resolver_uses_on_demand_base_only_without_matching_profile() {
        let catalog = crate::bedrock::BedrockCatalog::from_test_records(
            vec![crate::bedrock::BedrockFoundationModel::test(
                "anthropic.claude-sonnet-4-20250514-v1:0",
                "Claude Sonnet 4",
                true,
                &["TEXT"],
                &["TEXT"],
                &["ON_DEMAND"],
            )],
            vec![],
        );

        let target = catalog
            .resolve_invoke_target(Some("anthropic.claude-sonnet-4-20250514-v1:0"), "text")
            .unwrap();

        assert_eq!(target.invoke_id, "anthropic.claude-sonnet-4-20250514-v1:0");
        assert_eq!(target.source, "base_model");
    }

    #[test]
    fn bedrock_resolver_rejects_default_when_catalog_has_no_usable_text_target() {
        let catalog = crate::bedrock::BedrockCatalog::from_test_records(
            vec![crate::bedrock::BedrockFoundationModel::test(
                "anthropic.claude-sonnet-4-20250514-v1:0",
                "Claude Sonnet 4",
                false,
                &["TEXT"],
                &["TEXT"],
                &["ON_DEMAND"],
            )],
            vec![],
        );

        let error = catalog.resolve_invoke_target(None, "text").unwrap_err();

        assert!(format!("{error:#}").contains("No usable Bedrock text model"));
    }

    #[test]
    fn bedrock_extra_path_is_prepended_for_credential_process_helpers() {
        let merged = crate::bedrock::path_with_extra_path(
            Some(std::ffi::OsString::from("/usr/bin:/bin")),
            Some("/opt/isengard/bin:/usr/local/bin"),
        )
        .unwrap();

        assert_eq!(
            merged.to_string_lossy(),
            "/opt/isengard/bin:/usr/local/bin:/usr/bin:/bin"
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
    fn claude_cli_uses_cli_default_for_legacy_default_model() {
        assert_eq!(model_for_provider("claude-cli", "claude-sonnet-4-6"), "");
        assert_eq!(model_for_provider("claude-cli", "sonnet"), "sonnet");
    }

    #[test]
    fn configured_cli_legacy_claude_model_uses_cli_default() {
        let mut config = alvum_core::config::AlvumConfig::default();
        config.providers.insert(
            "claude-cli".into(),
            alvum_core::config::ProviderConfig {
                enabled: true,
                settings: HashMap::from([
                    (
                        "text_model".into(),
                        toml::Value::String("claude-sonnet-4-6".into()),
                    ),
                    (
                        "image_model".into(),
                        toml::Value::String("claude-sonnet-4-6".into()),
                    ),
                    (
                        "audio_model".into(),
                        toml::Value::String("claude-sonnet-4-6".into()),
                    ),
                ]),
            },
        );
        config.providers.insert(
            "codex-cli".into(),
            alvum_core::config::ProviderConfig {
                enabled: true,
                settings: HashMap::from([
                    (
                        "text_model".into(),
                        toml::Value::String("claude-sonnet-4-6".into()),
                    ),
                    (
                        "image_model".into(),
                        toml::Value::String("claude-sonnet-4-6".into()),
                    ),
                    (
                        "audio_model".into(),
                        toml::Value::String("claude-sonnet-4-6".into()),
                    ),
                ]),
            },
        );

        for provider in ["claude-cli", "codex-cli"] {
            for modality in ["text", "image", "audio"] {
                assert_eq!(
                    configured_model_for_modality(&config, provider, "", modality),
                    ""
                );
            }
        }
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
        assert_eq!(
            json["messages"][0]["content"][0]["source"]["type"],
            "base64"
        );
        assert_eq!(
            json["messages"][0]["content"][0]["source"]["media_type"],
            "image/png"
        );
        assert_eq!(json["messages"][0]["content"][1]["type"], "text");
        assert_eq!(
            json["messages"][0]["content"][1]["text"],
            "What is on this screen?"
        );
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
            response.map(str::to_string).map_err(|e| anyhow::anyhow!(e))
        }

        fn name(&self) -> &str {
            self.name
        }
    }
}
