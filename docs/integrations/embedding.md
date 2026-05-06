# Embedding Integration

MemoryFS uses the `Embedder` trait for pluggable embedding backends.

## EmbeddingGemma (default)

Local inference via HuggingFace Text Embeddings Inference (TEI).

```bash
# Start TEI with EmbeddingGemma
docker run -p 8080:80 \
  ghcr.io/huggingface/text-embeddings-inference:latest \
  --model-id google/embedding-gemma
```

Configure `HttpEmbedder`:

```rust
use memoryfs_core::embedder::{HttpEmbedder, HttpEmbedderConfig};

let embedder = HttpEmbedder::new(HttpEmbedderConfig {
    endpoint: "http://localhost:8080".into(),
    model: "google/embedding-gemma".into(),
    dimensions: 768,
    max_batch_size: 32,
    timeout_secs: 30,
});
```

## Custom embedders

Implement the `Embedder` trait:

```rust
#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}
```

Any OpenAI-compatible embedding API works with `HttpEmbedder` by
changing the endpoint and model name.

## Vector store

Embeddings are stored in Qdrant (via gRPC). The `VectorStore` trait
supports pluggable backends:

```rust
#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn upsert(&self, collection: &str, points: Vec<VectorPoint>) -> Result<()>;
    async fn search(&self, collection: &str, vector: Vec<f32>,
                    limit: usize, filter: Option<VectorFilter>) -> Result<Vec<VectorMatch>>;
    async fn delete(&self, collection: &str, ids: &[String]) -> Result<()>;
}
```

Start Qdrant locally:

```bash
just dev-up  # starts Qdrant on port 6334 (gRPC)
```
