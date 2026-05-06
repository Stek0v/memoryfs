# LLM Integration

MemoryFS uses the `LlmClient` trait for pluggable LLM access. The default
implementation talks to DeepSeek's cloud API (ADR-014).

## DeepSeek (default)

```rust
use memoryfs_core::llm::{LlmConfig, OpenAiCompatibleClient};

let client = OpenAiCompatibleClient::new(LlmConfig {
    endpoint: "https://api.deepseek.com/v1".into(),
    api_key: "sk-...".into(),
    model: "deepseek-chat".into(),
    max_tokens: 4096,
    temperature: 0.0,
    timeout_secs: 60,
});
```

## Custom LLM backends

Implement the `LlmClient` trait:

```rust
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn chat(&self, messages: &[Message]) -> Result<String>;
    async fn chat_json<T: DeserializeOwned>(&self, messages: &[Message]) -> Result<T>;
}
```

Any OpenAI-compatible API works with `OpenAiCompatibleClient`.

## Use cases

- **Memory extraction** (`extraction.rs`): Extracts structured memories
  from conversation text using LLM
- **Entity extraction** (`entity_extraction.rs`): NER via LLM with
  entity linking and fuzzy matching (Levenshtein, threshold 0.8)
- **Content classification**: Determines sensitivity level for review
  routing
