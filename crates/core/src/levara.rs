//! Levara gRPC integration — vector store + embedder backend.
//!
//! Replaces Qdrant + separate TEI server with a single Levara instance.
//! Uses `SearchByText` for combined embed+search and `BatchEmbedAndIndex`
//! for combined embed+index operations.

use crate::embedder::Embedder;
use crate::error::{MemoryFsError, Result};
use crate::ids::MemoryId;
use crate::vector_store::{HybridSearch, VectorFilter, VectorMatch, VectorStore};

/// Generated Levara gRPC bindings from `proto/levara.proto`.
#[allow(missing_docs)]
pub mod pb {
    tonic::include_proto!("levara.v1");
}

use pb::levara_service_client::LevaraServiceClient;
use tonic::transport::Channel;

/// Shared gRPC connection to a Levara instance.
///
/// Also carries an optional HTTP base URL — used by `LevaraVectorStore::upsert`
/// to reach Levara's collection-aware HTTP write path. Since Levara HTTP now
/// routes to the per-collection HNSW when `collection` is set in the payload,
/// HTTP and gRPC writes converge on the same backend. HTTP is preferred here
/// because it shares one connection pool with future REST endpoints; gRPC stays
/// as a fallback when `http_base` is unset.
#[derive(Clone)]
pub struct LevaraClient {
    inner: LevaraServiceClient<Channel>,
    http: reqwest::Client,
    http_base: Option<String>,
    embed_endpoint: String,
    embed_model: String,
    dimension: usize,
}

impl LevaraClient {
    /// Connect to a Levara gRPC server.
    ///
    /// `grpc_url`: e.g. `http://localhost:50051`
    /// `embed_endpoint`: embedding API URL that Levara proxies to (e.g. `http://localhost:8080/v1/embeddings`)
    /// `embed_model`: model name for embedding requests (e.g. `google/embedding-gemma`)
    pub async fn connect(
        grpc_url: &str,
        embed_endpoint: &str,
        embed_model: &str,
        dimension: usize,
    ) -> Result<Self> {
        let channel = Channel::from_shared(grpc_url.to_string())
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("invalid Levara URL: {e}")))?
            .connect()
            .await
            .map_err(|e| MemoryFsError::Unavailable(format!("failed to connect to Levara: {e}")))?;

        Ok(Self {
            inner: LevaraServiceClient::new(channel),
            http: reqwest::Client::new(),
            http_base: None,
            embed_endpoint: embed_endpoint.to_string(),
            embed_model: embed_model.to_string(),
            dimension,
        })
    }

    /// Attach Levara's HTTP base URL (e.g. `http://levara:8080`). When set,
    /// `LevaraVectorStore::upsert` writes via HTTP `/api/v1/batch_insert` with
    /// the `collection` field, so records land in the per-collection HNSW that
    /// mem0's HTTP `/api/v1/search` reads from when it carries the same
    /// `collection`.
    pub fn with_http_base(mut self, http_base: impl Into<String>) -> Self {
        self.http_base = Some(http_base.into().trim_end_matches('/').to_string());
        self
    }

    /// Create from an existing tonic channel (useful for testing).
    pub fn from_channel(
        channel: Channel,
        embed_endpoint: &str,
        embed_model: &str,
        dimension: usize,
    ) -> Self {
        Self {
            inner: LevaraServiceClient::new(channel),
            http: reqwest::Client::new(),
            http_base: None,
            embed_endpoint: embed_endpoint.to_string(),
            embed_model: embed_model.to_string(),
            dimension,
        }
    }
}

// ── LevaraVectorStore ─────────────────────────────────────────────────────────

/// Vector store backed by Levara's HNSW index via gRPC.
pub struct LevaraVectorStore {
    client: LevaraClient,
    collection: String,
}

impl LevaraVectorStore {
    /// Create a new vector store backed by the given Levara collection.
    pub fn new(client: LevaraClient, collection: String) -> Self {
        Self { client, collection }
    }

    /// Ensure the collection exists on the Levara server.
    pub async fn ensure_collection(&self) -> Result<()> {
        let mut client = self.client.inner.clone();

        let resp = client
            .has_collection(pb::HasCollectionReq {
                name: self.collection.clone(),
            })
            .await
            .map_err(grpc_err)?;

        if !resp.into_inner().exists {
            let resp = client
                .create_collection(pb::CreateCollectionReq {
                    name: self.collection.clone(),
                })
                .await
                .map_err(grpc_err)?;

            let status = resp.into_inner();
            if !status.ok {
                return Err(MemoryFsError::Internal(anyhow::anyhow!(
                    "failed to create Levara collection: {}",
                    status.error
                )));
            }
        }

        Ok(())
    }

    /// Combined embed + index: sends raw text to Levara which embeds and indexes
    /// in a single gRPC call (no separate embedding roundtrip).
    pub async fn embed_and_index(
        &self,
        items: Vec<(String, String, serde_json::Value)>,
    ) -> Result<u32> {
        let mut client = self.client.inner.clone();

        let index_items: Vec<pb::IndexItem> = items
            .into_iter()
            .map(|(id, text, metadata)| pb::IndexItem {
                id,
                text,
                metadata_json: serde_json::to_string(&metadata).unwrap_or_default(),
            })
            .collect();

        let resp = client
            .batch_embed_and_index(pb::BatchEmbedAndIndexReq {
                groups: vec![pb::IndexGroup {
                    collection: self.collection.clone(),
                    items: index_items,
                }],
                embed_endpoint: self.client.embed_endpoint.clone(),
                embed_model: self.client.embed_model.clone(),
                batch_size: 64,
                concurrency: 3,
            })
            .await
            .map_err(grpc_err)?;

        let inner = resp.into_inner();
        if !inner.errors.is_empty() {
            tracing::warn!(
                errors = ?inner.errors,
                "Levara BatchEmbedAndIndex had partial failures"
            );
        }

        Ok(inner.total_indexed as u32)
    }

    /// Combined embed + search: Levara embeds the query text and searches
    /// the vector index in a single gRPC call.
    pub async fn search_by_text(&self, query: &str, limit: usize) -> Result<Vec<VectorMatch>> {
        let mut client = self.client.inner.clone();

        let resp = client
            .search_by_text(pb::SearchByTextReq {
                collection: self.collection.clone(),
                query_text: query.to_string(),
                top_k: limit as i32,
                embed_endpoint: self.client.embed_endpoint.clone(),
                embed_model: self.client.embed_model.clone(),
            })
            .await
            .map_err(grpc_err)?;

        parse_search_results(&resp.into_inner().results)
    }

    /// Hybrid search: vector + BM25 via Reciprocal Rank Fusion, all in one call.
    pub async fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
        vector_weight: f32,
        bm25_weight: f32,
    ) -> Result<Vec<VectorMatch>> {
        let mut client = self.client.inner.clone();

        let resp = client
            .hybrid_search(pb::HybridSearchReq {
                collection: self.collection.clone(),
                query_text: query.to_string(),
                top_k: limit as i32,
                embed_endpoint: self.client.embed_endpoint.clone(),
                embed_model: self.client.embed_model.clone(),
                vector_weight,
                bm25_weight,
            })
            .await
            .map_err(grpc_err)?;

        let inner = resp.into_inner();
        let mut results = Vec::with_capacity(inner.results.len());
        for r in &inner.results {
            let (memory_id, chunk_index) = parse_point_id(&r.id)?;
            let metadata = parse_metadata_json(&r.metadata_json);
            results.push(VectorMatch {
                memory_id,
                chunk_index,
                score: r.fused_score as f32,
                metadata,
            });
        }
        Ok(results)
    }
}

#[async_trait::async_trait]
impl VectorStore for LevaraVectorStore {
    async fn upsert(
        &self,
        memory_id: &MemoryId,
        vectors: &[(u32, Vec<f32>, serde_json::Value)],
    ) -> Result<()> {
        let mid = memory_id.to_string();

        // Build records once — same shape works for both the HTTP cluster path
        // and the gRPC per-collection path. The collection field is dropped on
        // the HTTP side because that endpoint is collection-blind.
        let mut records_meta: Vec<(String, &Vec<f32>, serde_json::Value)> = vectors
            .iter()
            .map(|(chunk_index, embedding, metadata)| {
                let point_id = format!("{mid}:{chunk_index}");
                let mut meta = metadata.clone();
                if let serde_json::Value::Object(ref mut map) = meta {
                    map.insert("memory_id".into(), serde_json::json!(mid));
                    map.insert("chunk_index".into(), serde_json::json!(chunk_index));
                    map.insert("collection".into(), serde_json::json!(self.collection));
                }
                (point_id, embedding, meta)
            })
            .collect();

        if let Some(http_base) = &self.client.http_base {
            // Collection-aware HTTP write — Levara routes to per-tenant HNSW
            // when `collection` is set, so mem0's HTTP `/api/v1/search` (which
            // also passes `collection`) finds the same records.
            let payload = serde_json::json!({
                "collection": self.collection,
                "records": records_meta
                    .iter_mut()
                    .map(|(id, vec, meta)| serde_json::json!({
                        "id": id,
                        "vector": vec,
                        "metadata": meta,
                    }))
                    .collect::<Vec<_>>()
            });

            let url = format!("{http_base}/api/v1/batch_insert");
            let resp = self
                .client
                .http
                .post(&url)
                .json(&payload)
                .send()
                .await
                .map_err(|e| MemoryFsError::Unavailable(format!("Levara HTTP: {e}")))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(MemoryFsError::Internal(anyhow::anyhow!(
                    "Levara HTTP batch_insert {status}: {body}"
                )));
            }

            #[derive(serde::Deserialize)]
            struct BatchResp {
                inserted: u32,
                failed: u32,
                #[serde(default)]
                errors: Vec<String>,
            }
            let parsed: BatchResp = resp.json().await.map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("bad batch_insert resp: {e}"))
            })?;
            if parsed.failed > 0 {
                return Err(MemoryFsError::Internal(anyhow::anyhow!(
                    "Levara HTTP batch_insert: {} ok, {} failed (errors: {:?})",
                    parsed.inserted,
                    parsed.failed,
                    parsed.errors
                )));
            }
            return Ok(());
        }

        // Fallback: gRPC per-collection store (only useful when nothing reads
        // through `/api/v1/search`). Kept so existing tests and callers still work.
        let mut client = self.client.inner.clone();
        let records: Vec<pb::InsertRecord> = records_meta
            .into_iter()
            .map(|(id, vec, meta)| pb::InsertRecord {
                id,
                vector: vec.clone(),
                metadata_json: serde_json::to_string(&meta).unwrap_or_default(),
            })
            .collect();

        let resp = client
            .batch_insert(pb::BatchInsertReq {
                collection: self.collection.clone(),
                records,
            })
            .await
            .map_err(grpc_err)?;

        let inner = resp.into_inner();
        if inner.failed > 0 {
            return Err(MemoryFsError::Internal(anyhow::anyhow!(
                "Levara batch insert: {} failed (errors: {:?})",
                inner.failed,
                inner.errors
            )));
        }

        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        limit: usize,
        filter: Option<&VectorFilter>,
    ) -> Result<Vec<VectorMatch>> {
        let mut client = self.client.inner.clone();

        // Levara's gRPC SearchReq has no payload-filter equivalent of Qdrant
        // conditions, so we over-fetch and post-filter on metadata. Caller
        // (RetrievalEngine) already requests top_k*3, so dropping a few
        // superseded chunks still leaves enough candidates for RRF.
        let resp = client
            .search(pb::SearchReq {
                collection: self.collection.clone(),
                vector: query.to_vec(),
                top_k: limit as i32,
            })
            .await
            .map_err(grpc_err)?;

        let raw = parse_search_results(&resp.into_inner().results)?;
        Ok(apply_metadata_filter(raw, filter))
    }

    async fn delete(&self, memory_id: &MemoryId) -> Result<()> {
        let mut client = self.client.inner.clone();
        let mid = memory_id.to_string();

        let resp = client
            .get_by_id(pb::GetByIdReq {
                collection: self.collection.clone(),
                ids: vec![],
            })
            .await;

        // Levara Delete takes explicit IDs — collect all point IDs for this memory.
        // Since we format IDs as "mem_xxx:chunk_idx", we list and filter.
        // For efficiency, we delete by known chunk range. If the exact chunks
        // are unknown, we attempt deletion of indices 0..1000.
        let ids: Vec<String> = (0..1000u32).map(|i| format!("{mid}:{i}")).collect();

        // Levara silently ignores non-existent IDs in Delete.
        let _ = client
            .delete(pb::DeleteReq {
                collection: self.collection.clone(),
                ids,
            })
            .await
            .map_err(grpc_err)?;

        // Suppress unused variable warning from the GetByID attempt above.
        drop(resp);

        Ok(())
    }

    async fn reset(&self) -> Result<()> {
        let mut client = self.client.inner.clone();

        // Drop and recreate.
        let has = client
            .has_collection(pb::HasCollectionReq {
                name: self.collection.clone(),
            })
            .await
            .map_err(grpc_err)?;

        if has.into_inner().exists {
            client
                .drop_collection(pb::DropCollectionReq {
                    name: self.collection.clone(),
                })
                .await
                .map_err(grpc_err)?;
        }

        client
            .create_collection(pb::CreateCollectionReq {
                name: self.collection.clone(),
            })
            .await
            .map_err(grpc_err)?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl HybridSearch for LevaraVectorStore {
    async fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
        vector_weight: f32,
        bm25_weight: f32,
    ) -> Result<Vec<VectorMatch>> {
        // The HybridSearch trait has no filter parameter, but superseded chunks
        // must never appear in default retrieval — drop them client-side.
        // Time-travel queries that need superseded results should use the
        // raw vector search path with an explicit VectorFilter.
        let raw = LevaraVectorStore::hybrid_search(self, query, limit, vector_weight, bm25_weight)
            .await?;
        let active_only = VectorFilter {
            status: Some("active".into()),
            ..VectorFilter::default()
        };
        Ok(apply_metadata_filter(raw, Some(&active_only)))
    }
}

/// Drop matches whose metadata fails the filter. Levara's gRPC API has no
/// payload condition equivalent to Qdrant's, so the indexer-written metadata
/// is the only place to enforce status/scope/workspace constraints.
fn apply_metadata_filter(
    matches: Vec<VectorMatch>,
    filter: Option<&VectorFilter>,
) -> Vec<VectorMatch> {
    let Some(filter) = filter else {
        return matches;
    };

    matches
        .into_iter()
        .filter(|m| {
            if let Some(expected) = filter.status.as_deref() {
                let actual = m
                    .metadata
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("active");
                if actual != expected {
                    return false;
                }
            }
            if let Some(expected) = filter.workspace_id.as_deref() {
                if m.metadata.get("workspace_id").and_then(|v| v.as_str()) != Some(expected) {
                    return false;
                }
            }
            if let Some(expected) = filter.scope.as_deref() {
                if let Some(actual) = m.metadata.get("scope").and_then(|v| v.as_str()) {
                    if actual != expected {
                        return false;
                    }
                }
            }
            if let Some(required_tags) = filter.tags.as_ref() {
                let actual_tags = m.metadata.get("tags").and_then(|v| v.as_array());
                for tag in required_tags {
                    let has = actual_tags
                        .map(|arr| arr.iter().any(|t| t.as_str() == Some(tag.as_str())))
                        .unwrap_or(false);
                    if !has {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

// ── LevaraEmbedder ────────────────────────────────────────────────────────────

/// Embedder that uses Levara's `BatchEmbedAndIndex` to compute embeddings.
///
/// Since Levara's proto doesn't have a standalone "embed only" RPC, this
/// uses a scratch collection to embed texts and then reads back the vectors.
/// For production hot paths, prefer `LevaraVectorStore::embed_and_index()`
/// or `search_by_text()` which avoid the extra roundtrip.
pub struct LevaraEmbedder {
    client: LevaraClient,
}

impl LevaraEmbedder {
    /// Create an embedder that proxies embedding through Levara.
    pub fn new(client: LevaraClient) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Embedder for LevaraEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut client = self.client.inner.clone();
        let scratch_collection = "_memoryfs_embed_scratch".to_string();

        // Ensure scratch collection exists.
        let has = client
            .has_collection(pb::HasCollectionReq {
                name: scratch_collection.clone(),
            })
            .await
            .map_err(grpc_err)?;

        if !has.into_inner().exists {
            client
                .create_collection(pb::CreateCollectionReq {
                    name: scratch_collection.clone(),
                })
                .await
                .map_err(grpc_err)?;
        }

        // Generate deterministic IDs for this batch.
        let items: Vec<pb::IndexItem> = texts
            .iter()
            .enumerate()
            .map(|(i, text)| pb::IndexItem {
                id: format!("_embed_batch_{i}"),
                text: text.to_string(),
                metadata_json: String::new(),
            })
            .collect();

        let ids: Vec<String> = items.iter().map(|it| it.id.clone()).collect();

        // Embed + index into scratch collection.
        client
            .batch_embed_and_index(pb::BatchEmbedAndIndexReq {
                groups: vec![pb::IndexGroup {
                    collection: scratch_collection.clone(),
                    items,
                }],
                embed_endpoint: self.client.embed_endpoint.clone(),
                embed_model: self.client.embed_model.clone(),
                batch_size: 64,
                concurrency: 3,
            })
            .await
            .map_err(grpc_err)?;

        // Read back vectors.
        let resp = client
            .get_by_id(pb::GetByIdReq {
                collection: scratch_collection.clone(),
                ids: ids.clone(),
            })
            .await
            .map_err(grpc_err)?;

        let records = resp.into_inner().records;
        let mut vectors = Vec::with_capacity(texts.len());

        // Records may come back unordered — sort by ID suffix.
        let mut sorted: Vec<_> = records.into_iter().filter(|r| r.found).collect();
        sorted.sort_by_key(|r| {
            r.id.strip_prefix("_embed_batch_")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
        });

        for record in &sorted {
            if record.vector.len() != self.client.dimension {
                return Err(MemoryFsError::Internal(anyhow::anyhow!(
                    "expected dimension {}, got {}",
                    self.client.dimension,
                    record.vector.len()
                )));
            }
            vectors.push(record.vector.clone());
        }

        if vectors.len() != texts.len() {
            return Err(MemoryFsError::Internal(anyhow::anyhow!(
                "expected {} vectors, got {}",
                texts.len(),
                vectors.len()
            )));
        }

        // Clean up scratch vectors.
        let _ = client
            .delete(pb::DeleteReq {
                collection: scratch_collection,
                ids,
            })
            .await;

        Ok(vectors)
    }

    fn dimension(&self) -> usize {
        self.client.dimension
    }

    fn model_id(&self) -> &str {
        &self.client.embed_model
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn grpc_err(e: tonic::Status) -> MemoryFsError {
    match e.code() {
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => {
            MemoryFsError::Unavailable(format!("Levara gRPC: {e}"))
        }
        tonic::Code::NotFound => MemoryFsError::NotFound(format!("Levara: {e}")),
        tonic::Code::InvalidArgument => MemoryFsError::Validation(format!("Levara: {e}")),
        _ => MemoryFsError::Internal(anyhow::anyhow!("Levara gRPC: {e}")),
    }
}

/// Parse a point ID formatted as "mem_xxxx:chunk_index".
fn parse_point_id(id: &str) -> Result<(MemoryId, u32)> {
    let (mid_str, chunk_str) = id
        .rsplit_once(':')
        .ok_or_else(|| MemoryFsError::Internal(anyhow::anyhow!("bad point ID: {id}")))?;

    let memory_id = MemoryId::parse(mid_str)?;
    let chunk_index: u32 = chunk_str
        .parse()
        .map_err(|_| MemoryFsError::Internal(anyhow::anyhow!("bad chunk index in: {id}")))?;

    Ok((memory_id, chunk_index))
}

fn parse_metadata_json(json_str: &str) -> serde_json::Value {
    if json_str.is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    serde_json::from_str(json_str).unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
}

fn parse_search_results(results: &[pb::SearchResult]) -> Result<Vec<VectorMatch>> {
    let mut matches = Vec::with_capacity(results.len());
    for r in results {
        let (memory_id, chunk_index) = parse_point_id(&r.id)?;
        let metadata = parse_metadata_json(&r.metadata_json);
        matches.push(VectorMatch {
            memory_id,
            chunk_index,
            score: r.score,
            metadata,
        });
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_point_id_valid() {
        let (mid, chunk) = parse_point_id("mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7:3").unwrap();
        assert_eq!(mid.to_string(), "mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7");
        assert_eq!(chunk, 3);
    }

    #[test]
    fn parse_point_id_zero() {
        let (_, chunk) = parse_point_id("mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7:0").unwrap();
        assert_eq!(chunk, 0);
    }

    #[test]
    fn parse_point_id_invalid_no_colon() {
        assert!(parse_point_id("mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7").is_err());
    }

    #[test]
    fn parse_point_id_invalid_chunk() {
        assert!(parse_point_id("mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7:abc").is_err());
    }

    #[test]
    fn parse_metadata_json_empty() {
        let v = parse_metadata_json("");
        assert!(v.is_object());
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn parse_metadata_json_valid() {
        let v = parse_metadata_json(r#"{"key":"val","n":42}"#);
        assert_eq!(v["key"], "val");
        assert_eq!(v["n"], 42);
    }

    #[test]
    fn parse_metadata_json_malformed() {
        let v = parse_metadata_json("{not json}");
        assert!(v.is_object());
    }

    #[test]
    fn grpc_err_mapping_unavailable() {
        let status = tonic::Status::unavailable("conn refused");
        let err = grpc_err(status);
        assert!(matches!(err, MemoryFsError::Unavailable(_)));
    }

    #[test]
    fn grpc_err_mapping_not_found() {
        let status = tonic::Status::not_found("no such collection");
        let err = grpc_err(status);
        assert!(matches!(err, MemoryFsError::NotFound(_)));
    }

    #[test]
    fn grpc_err_mapping_invalid() {
        let status = tonic::Status::invalid_argument("bad vector");
        let err = grpc_err(status);
        assert!(matches!(err, MemoryFsError::Validation(_)));
    }

    #[test]
    fn grpc_err_mapping_internal() {
        let status = tonic::Status::internal("oom");
        let err = grpc_err(status);
        assert!(matches!(err, MemoryFsError::Internal(_)));
    }

    #[test]
    fn parse_search_results_empty() {
        let results = parse_search_results(&[]).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn parse_search_results_valid() {
        let results = parse_search_results(&[
            pb::SearchResult {
                id: "mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7:0".into(),
                score: 0.95,
                metadata_json: r#"{"workspace_id":"ws_test"}"#.into(),
            },
            pb::SearchResult {
                id: "mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7:1".into(),
                score: 0.87,
                metadata_json: String::new(),
            },
        ])
        .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].score, 0.95);
        assert_eq!(results[0].chunk_index, 0);
        assert_eq!(results[1].chunk_index, 1);
        assert_eq!(results[0].metadata["workspace_id"], "ws_test");
    }
}
