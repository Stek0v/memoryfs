//! Pluggable vector store interface.
//!
//! Default implementation: `QdrantVectorStore` — Qdrant vector database via gRPC.
//! Trait designed for future Levara backend (https://github.com/levara).
//! See ADR-004.

use crate::error::{MemoryFsError, Result};
use crate::ids::MemoryId;

/// Metadata filters for vector search.
#[derive(Debug, Default, Clone)]
pub struct VectorFilter {
    /// Restrict to a specific workspace.
    pub workspace_id: Option<String>,
    /// Restrict by scope (e.g. `agent`, `team`).
    pub scope: Option<String>,
    /// Filter by tags (AND semantics).
    pub tags: Option<Vec<String>>,
    /// Filter by memory status.
    pub status: Option<String>,
}

/// A single search result with score.
#[derive(Debug, Clone)]
pub struct VectorMatch {
    /// ID of the matched memory.
    pub memory_id: MemoryId,
    /// Chunk index within the memory.
    pub chunk_index: u32,
    /// Similarity score (higher = more similar).
    pub score: f32,
    /// Stored metadata for the chunk.
    pub metadata: serde_json::Value,
}

/// Optional server-side hybrid search (vector + BM25 fused on the backend).
///
/// Implementations like Levara support this natively via their HybridSearch gRPC.
/// When available, the retrieval engine can delegate fusion to the server instead of
/// doing parallel vector + local BM25 with client-side RRF.
#[async_trait::async_trait]
pub trait HybridSearch: Send + Sync {
    /// Execute a hybrid vector + BM25 search with the given fusion weights.
    async fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
        vector_weight: f32,
        bm25_weight: f32,
    ) -> Result<Vec<VectorMatch>>;
}

/// Pluggable vector storage backend.
#[async_trait::async_trait]
pub trait VectorStore: Send + Sync {
    /// Upsert vectors with metadata. Replaces existing vectors for the same memory+chunk.
    async fn upsert(
        &self,
        memory_id: &MemoryId,
        vectors: &[(u32, Vec<f32>, serde_json::Value)],
    ) -> Result<()>;

    /// Search for nearest neighbors with optional metadata filters.
    async fn search(
        &self,
        query: &[f32],
        limit: usize,
        filter: Option<&VectorFilter>,
    ) -> Result<Vec<VectorMatch>>;

    /// Delete all vectors for a given memory.
    async fn delete(&self, memory_id: &MemoryId) -> Result<()>;

    /// Delete all vectors and rebuild (used by reindex).
    async fn reset(&self) -> Result<()>;
}

/// Qdrant vector database implementation via gRPC.
pub struct QdrantVectorStore {
    client: qdrant_client::Qdrant,
    collection: String,
    dimension: usize,
}

impl QdrantVectorStore {
    /// Connect to Qdrant and wrap it as a vector store.
    ///
    /// `url` is the gRPC endpoint (e.g. `http://localhost:6334`).
    /// `collection` is the Qdrant collection name.
    pub fn new(client: qdrant_client::Qdrant, collection: String, dimension: usize) -> Self {
        Self {
            client,
            collection,
            dimension,
        }
    }

    /// Ensure the collection exists with the correct vector config.
    pub async fn ensure_collection(&self) -> Result<()> {
        use qdrant_client::qdrant::{CreateCollectionBuilder, Distance, VectorParamsBuilder};

        let exists = self
            .client
            .collection_exists(&self.collection)
            .await
            .map_err(|e| MemoryFsError::Unavailable(format!("qdrant check collection: {e}")))?;

        if !exists {
            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.collection).vectors_config(
                        VectorParamsBuilder::new(self.dimension as u64, Distance::Cosine),
                    ),
                )
                .await
                .map_err(|e| {
                    MemoryFsError::Unavailable(format!("qdrant create collection: {e}"))
                })?;
        }

        Ok(())
    }

    /// Build a point ID from memory_id + chunk_index.
    /// Uses a deterministic UUID v5 so the same (memory, chunk) always maps to the same point.
    fn point_id(memory_id: &MemoryId, chunk_index: u32) -> String {
        format!("{memory_id}:{chunk_index}")
    }

    fn build_filter(filter: &VectorFilter) -> qdrant_client::qdrant::Filter {
        use qdrant_client::qdrant::Condition;

        let mut conditions = Vec::new();

        if let Some(ref ws) = filter.workspace_id {
            conditions.push(Condition::matches("workspace_id", ws.clone()));
        }
        if let Some(ref scope) = filter.scope {
            conditions.push(Condition::matches("scope", scope.clone()));
        }
        if let Some(ref status) = filter.status {
            conditions.push(Condition::matches("status", status.clone()));
        }
        if let Some(ref tags) = filter.tags {
            for tag in tags {
                conditions.push(Condition::matches("tags", tag.clone()));
            }
        }

        qdrant_client::qdrant::Filter::all(conditions)
    }
}

#[async_trait::async_trait]
impl VectorStore for QdrantVectorStore {
    async fn upsert(
        &self,
        memory_id: &MemoryId,
        vectors: &[(u32, Vec<f32>, serde_json::Value)],
    ) -> Result<()> {
        use qdrant_client::qdrant::{PointStruct, UpsertPointsBuilder};
        use qdrant_client::Payload;

        let mid = memory_id.to_string();
        let mut points = Vec::with_capacity(vectors.len());

        for (chunk_index, embedding, metadata) in vectors {
            let point_id = Self::point_id(memory_id, *chunk_index);

            let mut json_payload = serde_json::json!({
                "memory_id": &mid,
                "chunk_index": *chunk_index,
            });

            if let serde_json::Value::Object(map) = metadata {
                if let serde_json::Value::Object(ref mut base) = json_payload {
                    for (k, v) in map {
                        base.insert(k.clone(), v.clone());
                    }
                }
            }

            let payload: Payload = json_payload
                .try_into()
                .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("payload conversion: {e}")))?;

            points.push(PointStruct::new(point_id, embedding.clone(), payload));
        }

        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.collection, points).wait(true))
            .await
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("qdrant upsert: {e}")))?;

        Ok(())
    }

    async fn search(
        &self,
        query: &[f32],
        limit: usize,
        filter: Option<&VectorFilter>,
    ) -> Result<Vec<VectorMatch>> {
        use qdrant_client::qdrant::SearchPointsBuilder;

        let mut builder = SearchPointsBuilder::new(&self.collection, query.to_vec(), limit as u64)
            .with_payload(true);

        if let Some(f) = filter {
            builder = builder.filter(Self::build_filter(f));
        }

        let response = self
            .client
            .search_points(builder)
            .await
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("qdrant search: {e}")))?;

        let mut results = Vec::with_capacity(response.result.len());
        for point in response.result {
            let mid_str = point
                .payload
                .get("memory_id")
                .and_then(|v| match &v.kind {
                    Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("");

            let chunk_index = point
                .payload
                .get("chunk_index")
                .and_then(|v| match &v.kind {
                    Some(qdrant_client::qdrant::value::Kind::IntegerValue(i)) => Some(*i as u32),
                    _ => None,
                })
                .unwrap_or(0);

            let metadata = payload_to_json(&point.payload);
            let memory_id = MemoryId::parse(mid_str)?;

            results.push(VectorMatch {
                memory_id,
                chunk_index,
                score: point.score,
                metadata,
            });
        }

        Ok(results)
    }

    async fn delete(&self, memory_id: &MemoryId) -> Result<()> {
        use qdrant_client::qdrant::{Condition, DeletePointsBuilder, Filter};

        let mid = memory_id.to_string();
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection)
                    .points(Filter::must([Condition::matches("memory_id", mid)]))
                    .wait(true),
            )
            .await
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("qdrant delete: {e}")))?;

        Ok(())
    }

    async fn reset(&self) -> Result<()> {
        use qdrant_client::qdrant::{CreateCollectionBuilder, Distance, VectorParamsBuilder};

        let exists = self
            .client
            .collection_exists(&self.collection)
            .await
            .map_err(|e| MemoryFsError::Unavailable(format!("qdrant check collection: {e}")))?;

        if exists {
            self.client
                .delete_collection(&self.collection)
                .await
                .map_err(|e| {
                    MemoryFsError::Internal(anyhow::anyhow!("qdrant delete collection: {e}"))
                })?;
        }

        self.client
            .create_collection(
                CreateCollectionBuilder::new(&self.collection).vectors_config(
                    VectorParamsBuilder::new(self.dimension as u64, Distance::Cosine),
                ),
            )
            .await
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("qdrant recreate: {e}")))?;

        Ok(())
    }
}

/// Convert Qdrant payload map to serde_json::Value.
fn payload_to_json(
    payload: &std::collections::HashMap<String, qdrant_client::qdrant::Value>,
) -> serde_json::Value {
    use qdrant_client::qdrant::value::Kind;

    let mut map = serde_json::Map::new();
    for (k, v) in payload {
        if let Some(ref kind) = v.kind {
            let json_val = match kind {
                Kind::NullValue(_) => serde_json::Value::Null,
                Kind::BoolValue(b) => serde_json::Value::Bool(*b),
                Kind::IntegerValue(i) => serde_json::json!(*i),
                Kind::DoubleValue(d) => serde_json::json!(*d),
                Kind::StringValue(s) => serde_json::Value::String(s.clone()),
                Kind::ListValue(list) => {
                    let items: Vec<serde_json::Value> = list
                        .values
                        .iter()
                        .filter_map(|v| v.kind.as_ref())
                        .map(|k| match k {
                            Kind::StringValue(s) => serde_json::Value::String(s.clone()),
                            Kind::IntegerValue(i) => serde_json::json!(*i),
                            Kind::DoubleValue(d) => serde_json::json!(*d),
                            Kind::BoolValue(b) => serde_json::Value::Bool(*b),
                            _ => serde_json::Value::Null,
                        })
                        .collect();
                    serde_json::Value::Array(items)
                }
                Kind::StructValue(s) => payload_to_json(&s.fields),
            };
            map.insert(k.clone(), json_val);
        }
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_filter_default() {
        let f = VectorFilter::default();
        assert!(f.workspace_id.is_none());
        assert!(f.scope.is_none());
        assert!(f.tags.is_none());
        assert!(f.status.is_none());
    }

    #[test]
    fn build_filter_empty() {
        let f = VectorFilter::default();
        let filter = QdrantVectorStore::build_filter(&f);
        assert!(filter.must.is_empty());
    }

    #[test]
    fn build_filter_workspace_only() {
        let f = VectorFilter {
            workspace_id: Some("ws_test".into()),
            ..Default::default()
        };
        let filter = QdrantVectorStore::build_filter(&f);
        assert_eq!(filter.must.len(), 1);
    }

    #[test]
    fn build_filter_all_fields() {
        let f = VectorFilter {
            workspace_id: Some("ws_1".into()),
            scope: Some("agent".into()),
            status: Some("active".into()),
            tags: Some(vec!["tag1".into(), "tag2".into()]),
        };
        let filter = QdrantVectorStore::build_filter(&f);
        assert_eq!(filter.must.len(), 5); // ws + scope + status + 2 tags
    }

    #[test]
    fn build_filter_tags_and_semantics() {
        let f = VectorFilter {
            tags: Some(vec!["a".into(), "b".into()]),
            ..Default::default()
        };
        let filter = QdrantVectorStore::build_filter(&f);
        assert_eq!(filter.must.len(), 2);
    }

    #[test]
    fn point_id_deterministic() {
        let mid = MemoryId::parse("mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7").unwrap();
        let a = QdrantVectorStore::point_id(&mid, 0);
        let b = QdrantVectorStore::point_id(&mid, 0);
        assert_eq!(a, b);
        let c = QdrantVectorStore::point_id(&mid, 1);
        assert_ne!(a, c);
    }

    #[test]
    fn payload_to_json_basic() {
        use qdrant_client::qdrant::{value::Kind, Value};

        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "name".to_string(),
            Value {
                kind: Some(Kind::StringValue("test".to_string())),
            },
        );
        payload.insert(
            "count".to_string(),
            Value {
                kind: Some(Kind::IntegerValue(42)),
            },
        );

        let json = payload_to_json(&payload);
        assert_eq!(json["name"], "test");
        assert_eq!(json["count"], 42);
    }
}
