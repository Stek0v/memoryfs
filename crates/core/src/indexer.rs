//! Event-driven indexer worker.
//!
//! Polls commit events from the [`EventLog`], chunks affected files,
//! embeds them, and upserts into the vector and BM25 indexes.
//! Uses [`ConsumerOffset`] for at-least-once delivery.

use std::sync::Arc;

use crate::bm25::{Bm25Document, Bm25Index};
use crate::chunker::{chunk_markdown, ChunkConfig};
use crate::embedder::Embedder;
use crate::error::{MemoryFsError, Result};
use crate::event_log::{ConsumerOffset, Event, EventKind, EventLog};
use crate::ids::MemoryId;
use crate::schema::parse_frontmatter;
use crate::vector_store::VectorStore;

/// Configuration for the indexer worker.
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    /// Chunk configuration for the markdown splitter.
    pub chunk_config: ChunkConfig,
    /// Maximum events to process in a single batch before committing offset.
    pub batch_size: usize,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            chunk_config: ChunkConfig::default(),
            batch_size: 100,
        }
    }
}

/// Describes a file change to be indexed.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// The memory ID this file belongs to.
    pub memory_id: MemoryId,
    /// Workspace-relative path (e.g. `memory/user/prefs.md`).
    pub file_path: String,
    /// Workspace ID.
    pub workspace_id: String,
    /// The raw markdown content (None if the file was deleted).
    pub content: Option<String>,
    /// Commit hash at time of indexing.
    pub commit: String,
}

/// Result of processing a single file through the indexing pipeline.
#[derive(Debug)]
pub struct IndexResult {
    /// The memory ID.
    pub memory_id: MemoryId,
    /// Number of chunks produced.
    pub chunk_count: usize,
    /// Whether the file was deleted (vs. upserted).
    pub deleted: bool,
}

/// The indexer worker — coordinates chunking, embedding, and storage.
pub struct Indexer<E: Embedder, V: VectorStore> {
    embedder: Arc<E>,
    vector_store: Arc<V>,
    bm25_index: Arc<Bm25Index>,
    config: IndexerConfig,
}

impl<E: Embedder, V: VectorStore> Indexer<E, V> {
    /// Create a new indexer.
    pub fn new(
        embedder: Arc<E>,
        vector_store: Arc<V>,
        bm25_index: Arc<Bm25Index>,
        config: IndexerConfig,
    ) -> Self {
        Self {
            embedder,
            vector_store,
            bm25_index,
            config,
        }
    }

    /// Process a single file change: chunk → embed → upsert vector + BM25.
    pub async fn index_file(&self, change: &FileChange) -> Result<IndexResult> {
        match &change.content {
            None => {
                self.vector_store.delete(&change.memory_id).await?;
                self.delete_bm25_chunks(&change.memory_id)?;
                Ok(IndexResult {
                    memory_id: change.memory_id.clone(),
                    chunk_count: 0,
                    deleted: true,
                })
            }
            Some(content) => {
                let mut chunk_config = self.config.chunk_config.clone();

                let frontmatter = parse_frontmatter(content).ok().map(|d| d.frontmatter);

                if chunk_config.document_title.is_none() {
                    chunk_config.document_title = frontmatter
                        .as_ref()
                        .and_then(|f| f.get("title").and_then(|v| v.as_str()))
                        .map(|s| s.to_string());
                }

                let chunks = chunk_markdown(content, &chunk_config);

                if chunks.is_empty() {
                    self.vector_store.delete(&change.memory_id).await?;
                    self.delete_bm25_chunks(&change.memory_id)?;
                    return Ok(IndexResult {
                        memory_id: change.memory_id.clone(),
                        chunk_count: 0,
                        deleted: false,
                    });
                }

                let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
                let embeddings = self.embedder.embed(&texts).await?;

                let created_at = frontmatter
                    .as_ref()
                    .and_then(|f| f.get("created_at").and_then(|v| v.as_str()))
                    .unwrap_or("");

                // Surface owner/scope fields from frontmatter into chunk metadata so
                // downstream consumers (e.g. mem0) can post-filter by user_id, agent_id,
                // or run_id without reading the .md file. mem0 stores these in the
                // memory frontmatter when writing through the MemoryFSVectorStore.
                let owner = |key: &str| -> Option<String> {
                    frontmatter
                        .as_ref()
                        .and_then(|f| f.get(key))
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                };
                let user_id = owner("user_id");
                let agent_id = owner("agent_id");
                let run_id = owner("run_id");

                // status defaults to "active" so files written before the supersede
                // schema bump still index and remain searchable.
                let status = owner("status").unwrap_or_else(|| "active".to_string());

                let array_of_strings = |key: &str| -> Vec<String> {
                    frontmatter
                        .as_ref()
                        .and_then(|f| f.get(key))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let supersedes = array_of_strings("supersedes");
                let superseded_by = array_of_strings("superseded_by");

                let mut vectors = Vec::with_capacity(chunks.len());
                for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                    let mut metadata = serde_json::json!({
                        "workspace_id": change.workspace_id,
                        "file_path": change.file_path,
                        "memory_id": change.memory_id.to_string(),
                        "heading_path": chunk.heading_path,
                        "heading_level": chunk.heading_level,
                        "char_start": chunk.char_start,
                        "char_end": chunk.char_end,
                        "model": self.embedder.model_id(),
                        "created_at": created_at,
                        "status": status,
                        "data": chunk.text,
                    });
                    if let serde_json::Value::Object(ref mut map) = metadata {
                        if let Some(v) = &user_id {
                            map.insert("user_id".into(), serde_json::Value::String(v.clone()));
                        }
                        if let Some(v) = &agent_id {
                            map.insert("agent_id".into(), serde_json::Value::String(v.clone()));
                        }
                        if let Some(v) = &run_id {
                            map.insert("run_id".into(), serde_json::Value::String(v.clone()));
                        }
                        if !supersedes.is_empty() {
                            map.insert(
                                "supersedes".into(),
                                serde_json::Value::Array(
                                    supersedes
                                        .iter()
                                        .map(|s| serde_json::Value::String(s.clone()))
                                        .collect(),
                                ),
                            );
                        }
                        if !superseded_by.is_empty() {
                            map.insert(
                                "superseded_by".into(),
                                serde_json::Value::Array(
                                    superseded_by
                                        .iter()
                                        .map(|s| serde_json::Value::String(s.clone()))
                                        .collect(),
                                ),
                            );
                        }
                    }
                    vectors.push((i as u32, embedding.clone(), metadata));
                }

                self.vector_store
                    .upsert(&change.memory_id, &vectors)
                    .await?;

                self.upsert_bm25_chunks(change, &chunks, &frontmatter)?;

                Ok(IndexResult {
                    memory_id: change.memory_id.clone(),
                    chunk_count: chunks.len(),
                    deleted: false,
                })
            }
        }
    }

    /// Process a batch of file changes.
    pub async fn index_batch(&self, changes: &[FileChange]) -> Result<Vec<IndexResult>> {
        let mut results = Vec::with_capacity(changes.len());
        for change in changes {
            results.push(self.index_file(change).await?);
        }
        Ok(results)
    }

    /// Poll the event log for new events and process them.
    /// Returns the number of events processed.
    pub async fn poll_events(
        &self,
        event_log: &EventLog,
        consumer: &ConsumerOffset,
        resolve_content: &dyn Fn(&Event) -> Option<Vec<FileChange>>,
    ) -> Result<usize> {
        let from_offset = consumer.get()?;
        let events = event_log.read_from(from_offset)?;

        if events.is_empty() {
            return Ok(0);
        }

        let mut processed = 0;
        for batch in events.chunks(self.config.batch_size) {
            for event in batch {
                if !is_indexable_event(&event.kind) {
                    continue;
                }
                if let Some(changes) = resolve_content(event) {
                    self.index_batch(&changes).await?;
                }
                processed += 1;
            }

            if let Some(last) = batch.last() {
                consumer.commit(last.offset + 1)?;
            }
        }

        Ok(processed)
    }

    fn delete_bm25_chunks(&self, memory_id: &MemoryId) -> Result<()> {
        let prefix = format!("{memory_id}:");
        let mut writer = self.bm25_index.writer(15_000_000)?;
        // Delete all chunks with this memory ID prefix.
        // Tantivy doesn't support prefix delete, so we delete the memory_id term
        // (which we store in workspace_id field as a convention for per-memory deletion).
        // Actually, we index id as "mem_xxx:0", "mem_xxx:1", etc.
        // For bulk delete, we need to search and delete. But since we know chunk IDs
        // are memory_id:N, we delete up to a reasonable max.
        for i in 0..1000 {
            let chunk_id = format!("{prefix}{i}");
            self.bm25_index.delete_by_id(&writer, &chunk_id);
        }
        writer
            .commit()
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 commit: {e}")))?;
        self.bm25_index.reload()?;
        Ok(())
    }

    fn upsert_bm25_chunks(
        &self,
        change: &FileChange,
        chunks: &[crate::chunker::Chunk],
        frontmatter: &Option<serde_json::Value>,
    ) -> Result<()> {
        self.delete_bm25_chunks(&change.memory_id)?;

        let scope = frontmatter
            .as_ref()
            .and_then(|f| f.get("scope").and_then(|v| v.as_str()))
            .unwrap_or("");
        let scope_id = frontmatter
            .as_ref()
            .and_then(|f| f.get("scope_id").and_then(|v| v.as_str()))
            .unwrap_or("");
        let file_type = frontmatter
            .as_ref()
            .and_then(|f| f.get("type").and_then(|v| v.as_str()))
            .unwrap_or("unknown");
        let tags = frontmatter
            .as_ref()
            .and_then(|f| {
                f.get("tags").and_then(|v| {
                    v.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|t| t.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                })
            })
            .unwrap_or_default();

        let docs: Vec<Bm25Document> = chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let title = if chunk.heading_path.is_empty() {
                    String::new()
                } else {
                    chunk.heading_path.last().unwrap().clone()
                };
                let heading = chunk.heading_path.join(" > ");

                Bm25Document {
                    id: format!("{}:{}", change.memory_id, i),
                    workspace_id: change.workspace_id.clone(),
                    file_path: change.file_path.clone(),
                    file_type: file_type.to_string(),
                    scope: scope.to_string(),
                    scope_id: scope_id.to_string(),
                    title,
                    heading,
                    body: chunk.text.clone(),
                    tags: tags.clone(),
                    commit: change.commit.clone(),
                }
            })
            .collect();

        let mut writer = self.bm25_index.writer(15_000_000)?;
        self.bm25_index.add_documents(&writer, &docs)?;
        writer
            .commit()
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 commit: {e}")))?;
        self.bm25_index.reload()?;

        Ok(())
    }
}

/// Type-erased indexer handle — lets a non-generic state struct (e.g. McpState)
/// hold an indexer without leaking its `<E, V>` parameters into every consumer.
#[async_trait::async_trait]
pub trait IndexBatch: Send + Sync {
    /// Index a batch of file changes; returns total chunks written across the batch.
    async fn run(&self, changes: Vec<FileChange>) -> Result<usize>;
}

#[async_trait::async_trait]
impl<E: Embedder + 'static, V: VectorStore + 'static> IndexBatch for Indexer<E, V> {
    async fn run(&self, changes: Vec<FileChange>) -> Result<usize> {
        let results = self.index_batch(&changes).await?;
        Ok(results.iter().map(|r| r.chunk_count).sum())
    }
}

fn is_indexable_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::CommitCreated
            | EventKind::CommitReverted
            | EventKind::MemoryAutoCommitted
            | EventKind::MemoryApproved
            | EventKind::MemorySuperseded
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::Embedder;

    struct MockEmbedder {
        dim: usize,
    }

    #[async_trait::async_trait]
    impl Embedder for MockEmbedder {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0; self.dim]).collect())
        }
        fn dimension(&self) -> usize {
            self.dim
        }
        fn model_id(&self) -> &str {
            "mock-embed"
        }
    }

    struct MockVectorStore {
        upserted: std::sync::Mutex<Vec<(String, usize)>>,
        deleted: std::sync::Mutex<Vec<String>>,
    }

    impl MockVectorStore {
        fn new() -> Self {
            Self {
                upserted: std::sync::Mutex::new(Vec::new()),
                deleted: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl VectorStore for MockVectorStore {
        async fn upsert(
            &self,
            memory_id: &MemoryId,
            vectors: &[(u32, Vec<f32>, serde_json::Value)],
        ) -> Result<()> {
            self.upserted
                .lock()
                .unwrap()
                .push((memory_id.to_string(), vectors.len()));
            Ok(())
        }
        async fn search(
            &self,
            _query: &[f32],
            _limit: usize,
            _filter: Option<&crate::vector_store::VectorFilter>,
        ) -> Result<Vec<crate::vector_store::VectorMatch>> {
            Ok(Vec::new())
        }
        async fn delete(&self, memory_id: &MemoryId) -> Result<()> {
            self.deleted.lock().unwrap().push(memory_id.to_string());
            Ok(())
        }
        async fn reset(&self) -> Result<()> {
            Ok(())
        }
    }

    fn make_indexer() -> (Indexer<MockEmbedder, MockVectorStore>, Arc<MockVectorStore>) {
        let embedder = Arc::new(MockEmbedder { dim: 3 });
        let vs = Arc::new(MockVectorStore::new());
        let bm25 = Arc::new(Bm25Index::in_memory().unwrap());
        let indexer = Indexer::new(embedder, vs.clone(), bm25, IndexerConfig::default());
        (indexer, vs)
    }

    #[tokio::test]
    async fn index_file_creates_chunks() {
        let (indexer, vs) = make_indexer();
        let change = FileChange {
            memory_id: MemoryId::new(),
            file_path: "memory/test.md".into(),
            workspace_id: "ws_test".into(),
            content: Some("# Title\n\nSome body text.\n".into()),
            commit: "abc".into(),
        };

        let result = indexer.index_file(&change).await.unwrap();
        assert!(!result.deleted);
        assert!(result.chunk_count > 0);

        let upserted = vs.upserted.lock().unwrap();
        assert_eq!(upserted.len(), 1);
        assert_eq!(upserted[0].1, result.chunk_count);
    }

    #[tokio::test]
    async fn index_file_delete() {
        let (indexer, vs) = make_indexer();
        let mid = MemoryId::new();
        let change = FileChange {
            memory_id: mid.clone(),
            file_path: "memory/test.md".into(),
            workspace_id: "ws_test".into(),
            content: None,
            commit: "abc".into(),
        };

        let result = indexer.index_file(&change).await.unwrap();
        assert!(result.deleted);
        assert_eq!(result.chunk_count, 0);

        let deleted = vs.deleted.lock().unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0], mid.to_string());
    }

    #[tokio::test]
    async fn index_empty_content() {
        let (indexer, vs) = make_indexer();
        let change = FileChange {
            memory_id: MemoryId::new(),
            file_path: "memory/empty.md".into(),
            workspace_id: "ws_test".into(),
            content: Some("---\ntitle: test\n---\n".into()),
            commit: "abc".into(),
        };

        let result = indexer.index_file(&change).await.unwrap();
        assert_eq!(result.chunk_count, 0);
        assert!(!result.deleted);

        // Vector store should have been called with delete (to clear old chunks)
        let deleted = vs.deleted.lock().unwrap();
        assert_eq!(deleted.len(), 1);
    }

    #[tokio::test]
    async fn index_batch_processes_all() {
        let (indexer, vs) = make_indexer();
        let changes = vec![
            FileChange {
                memory_id: MemoryId::new(),
                file_path: "a.md".into(),
                workspace_id: "ws_test".into(),
                content: Some("# A\n\nContent A.\n".into()),
                commit: "c1".into(),
            },
            FileChange {
                memory_id: MemoryId::new(),
                file_path: "b.md".into(),
                workspace_id: "ws_test".into(),
                content: Some("# B\n\nContent B.\n".into()),
                commit: "c1".into(),
            },
        ];

        let results = indexer.index_batch(&changes).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.chunk_count > 0));

        let upserted = vs.upserted.lock().unwrap();
        assert_eq!(upserted.len(), 2);
    }

    #[tokio::test]
    async fn index_file_with_frontmatter() {
        let (indexer, _vs) = make_indexer();
        let content = "\
---
type: memory
scope: user
scope_id: user:alice
tags: [preference, devops]
---
# Architecture

User prefers local-first infrastructure.
";
        let change = FileChange {
            memory_id: MemoryId::new(),
            file_path: "memory/pref.md".into(),
            workspace_id: "ws_test".into(),
            content: Some(content.into()),
            commit: "abc".into(),
        };

        let result = indexer.index_file(&change).await.unwrap();
        assert!(result.chunk_count > 0);
    }

    #[test]
    fn is_indexable_events() {
        assert!(is_indexable_event(&EventKind::CommitCreated));
        assert!(is_indexable_event(&EventKind::CommitReverted));
        assert!(is_indexable_event(&EventKind::MemoryAutoCommitted));
        assert!(is_indexable_event(&EventKind::MemoryApproved));
        assert!(is_indexable_event(&EventKind::MemorySuperseded));

        assert!(!is_indexable_event(&EventKind::RunStarted));
        assert!(!is_indexable_event(&EventKind::RunFinished));
        assert!(!is_indexable_event(&EventKind::PolicyChanged));
        assert!(!is_indexable_event(&EventKind::RedactionApplied));
    }

    #[tokio::test]
    async fn poll_events_empty_log() {
        let (indexer, _vs) = make_indexer();
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path().join("events.ndjson")).unwrap();
        let consumer = ConsumerOffset::open(dir.path().join("offset")).unwrap();

        let count = indexer
            .poll_events(&log, &consumer, &|_| None)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn poll_events_advances_offset() {
        let (indexer, _vs) = make_indexer();
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::open(dir.path().join("events.ndjson")).unwrap();
        let consumer = ConsumerOffset::open(dir.path().join("offset")).unwrap();

        log.append(
            EventKind::CommitCreated,
            "ws_test",
            "user:alice",
            "commit_1",
            None,
        )
        .unwrap();
        log.append(
            EventKind::RunStarted,
            "ws_test",
            "user:alice",
            "run_1",
            None,
        )
        .unwrap();

        let count = indexer
            .poll_events(&log, &consumer, &|_event| {
                Some(vec![FileChange {
                    memory_id: MemoryId::new(),
                    file_path: "test.md".into(),
                    workspace_id: "ws_test".into(),
                    content: Some("# Test\n\nBody.\n".into()),
                    commit: "abc".into(),
                }])
            })
            .await
            .unwrap();

        // Only CommitCreated is indexable
        assert_eq!(count, 1);
        assert_eq!(consumer.get().unwrap(), 2);
    }
}
