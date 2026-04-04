use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::debug;

pub struct LlmClient {
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

impl LlmClient {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            http: reqwest::Client::new(),
        }
    }

    pub async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        let request = ApiRequest {
            model: self.model.clone(),
            max_tokens: 16000,
            system: system.to_string(),
            messages: vec![ApiMessage {
                role: "user".into(),
                content: user_message.to_string(),
            }],
        };

        debug!(model = %self.model, system_len = system.len(), user_len = user_message.len(), "sending LLM request");

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

        debug!(response_len = text.len(), "received LLM response");
        Ok(text)
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
}
