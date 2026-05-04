//! Pluggable embedding interface.
//!
//! Default implementation: `HttpEmbedder` — generic OpenAI-compatible HTTP client.
//! Default model: EmbeddingGemma (local inference via HuggingFace TEI).
//! See ADR-013.

use crate::error::{MemoryFsError, Result};
use serde::{Deserialize, Serialize};

/// Pluggable text embedding backend.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts, returning one vector per input.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Dimensionality of the output vectors.
    fn dimension(&self) -> usize;

    /// Model identifier (stored in embedding metadata for reindex tracking).
    fn model_id(&self) -> &str;
}

/// Configuration for [`HttpEmbedder`].
#[derive(Debug, Clone)]
pub struct HttpEmbedderConfig {
    /// Base URL of the OpenAI-compatible embedding endpoint (e.g. `http://localhost:8080`).
    pub endpoint: String,
    /// Model identifier sent in the request body.
    pub model: String,
    /// Expected output dimension (used for validation).
    pub dimension: usize,
    /// Maximum texts per single HTTP request.
    pub batch_size: usize,
}

impl Default for HttpEmbedderConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:8080".into(),
            model: "embedding-gemma".into(),
            dimension: 768,
            batch_size: 64,
        }
    }
}

/// Generic HTTP embedder for any OpenAI-compatible `/v1/embeddings` endpoint.
pub struct HttpEmbedder {
    client: reqwest::Client,
    config: HttpEmbedderConfig,
}

impl HttpEmbedder {
    /// Create a new embedder with the given configuration.
    pub fn new(config: HttpEmbedderConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        Self { client, config }
    }

    /// Create from an existing `reqwest::Client` (useful for testing or custom TLS).
    pub fn with_client(client: reqwest::Client, config: HttpEmbedderConfig) -> Self {
        Self { client, config }
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!(
            "{}/v1/embeddings",
            self.config.endpoint.trim_end_matches('/')
        );
        let body = EmbeddingRequest {
            input: texts.iter().map(|t| (*t).to_string()).collect(),
            model: self.config.model.clone(),
        };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| MemoryFsError::Unavailable(format!("embedding request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(MemoryFsError::Unavailable(format!(
                "embedding endpoint returned {status}: {body_text}"
            )));
        }

        let response: EmbeddingResponse = resp
            .json()
            .await
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("bad embedding response: {e}")))?;

        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        let mut sorted = response.data;
        sorted.sort_by_key(|d| d.index);

        for datum in sorted {
            if datum.embedding.len() != self.config.dimension {
                return Err(MemoryFsError::Internal(anyhow::anyhow!(
                    "expected dimension {}, got {}",
                    self.config.dimension,
                    datum.embedding.len()
                )));
            }
            vectors.push(datum.embedding);
        }

        if vectors.len() != texts.len() {
            return Err(MemoryFsError::Internal(anyhow::anyhow!(
                "expected {} embeddings, got {}",
                texts.len(),
                vectors.len()
            )));
        }

        Ok(vectors)
    }
}

#[async_trait::async_trait]
impl Embedder for HttpEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());

        for batch in texts.chunks(self.config.batch_size) {
            let batch_result = self.embed_batch(batch).await?;
            all_vectors.extend(batch_result);
        }

        Ok(all_vectors)
    }

    fn dimension(&self) -> usize {
        self.config.dimension
    }

    fn model_id(&self) -> &str {
        &self.config.model
    }
}

// ── Wire types (OpenAI-compatible) ─────────────────────────────────────────

#[derive(Serialize)]
struct EmbeddingRequest {
    input: Vec<String>,
    model: String,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
    index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let cfg = HttpEmbedderConfig::default();
        assert_eq!(cfg.endpoint, "http://localhost:8080");
        assert_eq!(cfg.model, "embedding-gemma");
        assert_eq!(cfg.dimension, 768);
        assert_eq!(cfg.batch_size, 64);
    }

    #[test]
    fn embedder_accessors() {
        let embedder = HttpEmbedder::new(HttpEmbedderConfig {
            endpoint: "http://test:9999".into(),
            model: "test-model".into(),
            dimension: 384,
            batch_size: 32,
        });
        assert_eq!(embedder.dimension(), 384);
        assert_eq!(embedder.model_id(), "test-model");
    }

    #[tokio::test]
    async fn empty_input_returns_empty() {
        let embedder = HttpEmbedder::new(HttpEmbedderConfig::default());
        let result = embedder.embed(&[]).await.unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn request_serialization() {
        let req = EmbeddingRequest {
            input: vec!["hello".into(), "world".into()],
            model: "test".into(),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["input"], serde_json::json!(["hello", "world"]));
        assert_eq!(json["model"], "test");
    }

    #[test]
    fn response_deserialization() {
        let json = serde_json::json!({
            "object": "list",
            "data": [
                {"object": "embedding", "embedding": [0.1, 0.2, 0.3], "index": 0},
                {"object": "embedding", "embedding": [0.4, 0.5, 0.6], "index": 1}
            ],
            "model": "test",
            "usage": {"prompt_tokens": 10, "total_tokens": 10}
        });
        let resp: EmbeddingResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 2);
        assert_eq!(resp.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.data[1].index, 1);
    }

    #[test]
    fn response_out_of_order() {
        let json = serde_json::json!({
            "data": [
                {"embedding": [0.4, 0.5, 0.6], "index": 1},
                {"embedding": [0.1, 0.2, 0.3], "index": 0}
            ]
        });
        let mut resp: EmbeddingResponse = serde_json::from_value(json).unwrap();
        resp.data.sort_by_key(|d| d.index);
        assert_eq!(resp.data[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(resp.data[1].embedding, vec![0.4, 0.5, 0.6]);
    }
}
