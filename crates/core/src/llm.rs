//! Pluggable LLM client interface.
//!
//! Default implementation: `OpenAiCompatibleClient` — works with any
//! OpenAI-compatible API (DeepSeek, OpenAI, Ollama, vLLM).
//! Default provider: DeepSeek cloud (`api.deepseek.com`).
//! See ADR-014.

use crate::error::{MemoryFsError, Result};
use serde::{Deserialize, Serialize};

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// Role of the message sender.
    pub role: Role,
    /// Message text.
    pub content: String,
}

/// Chat message role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System prompt.
    System,
    /// User input.
    User,
    /// Model response.
    Assistant,
}

/// Pluggable LLM chat completion backend.
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a chat completion request, optionally constraining output to a JSON schema.
    async fn chat(
        &self,
        messages: &[Message],
        json_schema: Option<&serde_json::Value>,
    ) -> Result<String>;

    /// Model identifier (stored in provenance metadata).
    fn model_id(&self) -> &str;
}

/// Configuration for [`OpenAiCompatibleClient`].
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Base URL (e.g. `https://api.deepseek.com`).
    pub endpoint: String,
    /// Model name sent in the request body.
    pub model: String,
    /// Optional API key (sent as Bearer token).
    pub api_key: Option<String>,
    /// Maximum tokens for the response.
    pub max_tokens: u32,
    /// Sampling temperature (0.0 = deterministic).
    pub temperature: f32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.deepseek.com".into(),
            model: "deepseek-chat".into(),
            api_key: None,
            max_tokens: 4096,
            temperature: 0.0,
        }
    }
}

/// Generic client for any OpenAI-compatible chat API.
pub struct OpenAiCompatibleClient {
    client: reqwest::Client,
    config: LlmConfig,
}

impl OpenAiCompatibleClient {
    /// Create a new client with the given configuration.
    pub fn new(config: LlmConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        Self { client, config }
    }

    /// Create from an existing `reqwest::Client`.
    pub fn with_client(client: reqwest::Client, config: LlmConfig) -> Self {
        Self { client, config }
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiCompatibleClient {
    async fn chat(
        &self,
        messages: &[Message],
        json_schema: Option<&serde_json::Value>,
    ) -> Result<String> {
        let url = format!(
            "{}/v1/chat/completions",
            self.config.endpoint.trim_end_matches('/')
        );

        let wire_messages: Vec<WireMessage> = messages
            .iter()
            .map(|m| WireMessage {
                role: m.role.clone(),
                content: m.content.clone(),
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": wire_messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
        });

        if let Some(schema) = json_schema {
            body["response_format"] = serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "response",
                    "schema": schema,
                    "strict": true,
                }
            });
        }

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.config.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| MemoryFsError::Unavailable(format!("LLM request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(MemoryFsError::Unavailable(format!(
                "LLM endpoint returned {status}: {body_text}"
            )));
        }

        let response: ChatCompletionResponse = resp
            .json()
            .await
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("bad LLM response: {e}")))?;

        response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| MemoryFsError::Internal(anyhow::anyhow!("LLM returned no choices")))
    }

    fn model_id(&self) -> &str {
        &self.config.model
    }
}

// ── Wire types (OpenAI-compatible) ─────────────────────────────────────────

#[derive(Serialize)]
struct WireMessage {
    role: Role,
    content: String,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = LlmConfig::default();
        assert_eq!(cfg.endpoint, "https://api.deepseek.com");
        assert_eq!(cfg.model, "deepseek-chat");
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.max_tokens, 4096);
        assert_eq!(cfg.temperature, 0.0);
    }

    #[test]
    fn client_accessors() {
        let client = OpenAiCompatibleClient::new(LlmConfig {
            model: "test-model".into(),
            ..Default::default()
        });
        assert_eq!(client.model_id(), "test-model");
    }

    #[test]
    fn role_serialization() {
        assert_eq!(serde_json::to_string(&Role::System).unwrap(), r#""system""#);
        assert_eq!(serde_json::to_string(&Role::User).unwrap(), r#""user""#);
        assert_eq!(
            serde_json::to_string(&Role::Assistant).unwrap(),
            r#""assistant""#
        );
    }

    #[test]
    fn message_serialization() {
        let msg = Message {
            role: Role::User,
            content: "hello".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hello");
    }

    #[test]
    fn response_deserialization() {
        let json = serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        });
        let resp: ChatCompletionResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content, "Hello!");
    }

    #[test]
    fn response_empty_choices() {
        let json = serde_json::json!({"choices": []});
        let resp: ChatCompletionResponse = serde_json::from_value(json).unwrap();
        assert!(resp.choices.is_empty());
    }
}
