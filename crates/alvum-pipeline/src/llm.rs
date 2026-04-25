use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

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
// Provider construction helper
// ---------------------------------------------------------------------------

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
            info!(model, "using Codex CLI provider");
            Ok(Box::new(CodexCliProvider::new(model.to_string())))
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

/// Walk a fixed preference list and return the first provider whose
/// minimal smoke test succeeds. Order:
///
///   1. claude-cli  — local subscription, zero config
///   2. codex-cli   — local subscription, zero config
///   3. anthropic-api — explicit API key in env
///   4. bedrock     — explicit AWS credentials in env
///   5. ollama      — last-resort local fallback
///
/// The smoke tests are intentionally cheap: binary-on-PATH for the CLI
/// providers, env-var-set for the cloud providers. We don't probe the
/// network here — a real auth-failed call surfaces via the normal
/// retry/error path on first complete().
async fn select_first_authenticated(model: &str) -> Result<Box<dyn LlmProvider>> {
    if cli_binary_available("claude") {
        info!(model, "auto: selected claude-cli");
        return Ok(Box::new(ClaudeCliProvider::new(model.to_string())));
    }
    if cli_binary_available("codex") {
        info!(model, "auto: selected codex-cli");
        return Ok(Box::new(CodexCliProvider::new(model.to_string())));
    }
    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        info!(model, "auto: selected anthropic-api");
        return Ok(Box::new(AnthropicApiProvider::new(key, model.to_string())));
    }
    if aws_credentials_available() {
        info!(model, "auto: selected bedrock");
        return Ok(Box::new(BedrockProvider::new(model.to_string()).await?));
    }
    if cli_binary_available("ollama") {
        info!(model, "auto: selected ollama (last resort)");
        return Ok(Box::new(OllamaProvider::new(model.to_string())));
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
}
