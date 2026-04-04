use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Provider-agnostic LLM interface. Implementations handle the transport
/// (HTTP API, CLI subprocess, local model) — callers just send prompts.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, system: &str, user_message: &str) -> Result<String>;
    fn name(&self) -> &str;
}

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

        debug!(model = %self.model, system_len = system.len(), user_len = user_message.len(), "sending to claude CLI");

        let mut child = tokio::process::Command::new("claude")
            .args([
                "-p",
                "--model", &self.model,
                "--output-format", "text",
                "--system-prompt", system,
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

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("claude CLI exited with {}: {stderr}", output.status);
        }

        let text = String::from_utf8(output.stdout)
            .context("claude CLI output is not valid UTF-8")?;

        debug!(response_len = text.len(), "received claude CLI response");
        Ok(text)
    }

    fn name(&self) -> &str {
        "claude-cli"
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

    fn name(&self) -> &str {
        "anthropic-api"
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

    fn name(&self) -> &str {
        "ollama"
    }
}

// ---------------------------------------------------------------------------
// Provider construction helper
// ---------------------------------------------------------------------------

/// Create the appropriate provider based on the provider name.
pub fn create_provider(provider: &str, model: &str) -> Result<Box<dyn LlmProvider>> {
    match provider {
        "cli" => {
            info!(model, "using Claude CLI provider (no API key needed)");
            Ok(Box::new(ClaudeCliProvider::new(model.to_string())))
        }
        "api" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY required for 'api' provider")?;
            info!(model, "using Anthropic API provider");
            Ok(Box::new(AnthropicApiProvider::new(api_key, model.to_string())))
        }
        "ollama" => {
            info!(model, "using Ollama provider (local)");
            Ok(Box::new(OllamaProvider::new(model.to_string())))
        }
        other => bail!("unknown provider: {other}. Options: cli, api, ollama"),
    }
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
}
