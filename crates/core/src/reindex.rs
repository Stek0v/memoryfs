//! Full rebuild of vector and BM25 indexes from workspace contents.
//!
//! Walks all files in an [`InodeIndex`], re-chunks, re-embeds, and re-indexes
//! everything. Supports progress reporting and checkpointing for resumability.

use crate::embedder::Embedder;
use crate::error::Result;
use crate::ids::MemoryId;
use crate::indexer::{FileChange, IndexResult, Indexer};
use crate::storage::{InodeIndex, ObjectStore};
use crate::vector_store::VectorStore;

/// Progress report for a reindex operation.
#[derive(Debug, Clone)]
pub struct ReindexProgress {
    /// Total files to process.
    pub total: usize,
    /// Files processed so far.
    pub processed: usize,
    /// Files that failed processing.
    pub failed: usize,
    /// Last successfully processed file path (for checkpointing).
    pub last_path: Option<String>,
}

impl ReindexProgress {
    fn new(total: usize) -> Self {
        Self {
            total,
            processed: 0,
            failed: 0,
            last_path: None,
        }
    }

    /// Fraction complete (0.0 to 1.0).
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.processed as f64 / self.total as f64
        }
    }
}

/// Result of a full reindex operation.
#[derive(Debug)]
pub struct ReindexResult {
    /// Final progress state.
    pub progress: ReindexProgress,
    /// Per-file index results (only successful ones).
    pub results: Vec<IndexResult>,
    /// Paths that failed with their error messages.
    pub errors: Vec<(String, String)>,
}

/// Perform a full reindex of all files in the workspace.
///
/// Steps:
/// 1. Reset the vector store (truncate).
/// 2. Walk all files in the inode index, sorted by path.
/// 3. For each file: read content from object store → chunk → embed → upsert.
/// 4. Report progress via the callback.
///
/// If `resume_from` is provided, files with paths <= that value are skipped
/// (for checkpoint-based resumability).
pub async fn full_reindex<E, V, F>(
    indexer: &Indexer<E, V>,
    object_store: &ObjectStore,
    inode_index: &InodeIndex,
    workspace_id: &str,
    commit: &str,
    resume_from: Option<&str>,
    on_progress: F,
) -> Result<ReindexResult>
where
    E: Embedder,
    V: VectorStore,
    F: Fn(&ReindexProgress),
{
    let paths = inode_index.paths();
    let total = paths.len();
    let mut progress = ReindexProgress::new(total);
    let mut results = Vec::new();
    let mut errors = Vec::new();

    on_progress(&progress);

    for path in &paths {
        if let Some(resume) = resume_from {
            if *path <= resume {
                progress.processed += 1;
                continue;
            }
        }

        let hash = match inode_index.get(path) {
            Some(h) => h,
            None => {
                progress.processed += 1;
                continue;
            }
        };

        let content = match object_store.get(hash) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(e) => {
                    errors.push((path.to_string(), format!("non-UTF8 content: {e}")));
                    progress.processed += 1;
                    progress.failed += 1;
                    on_progress(&progress);
                    continue;
                }
            },
            Err(e) => {
                errors.push((path.to_string(), format!("failed to read object: {e}")));
                progress.processed += 1;
                progress.failed += 1;
                on_progress(&progress);
                continue;
            }
        };

        let memory_id = extract_memory_id_from_path(path);

        let change = FileChange {
            memory_id,
            file_path: path.to_string(),
            workspace_id: workspace_id.to_string(),
            content: Some(content),
            commit: commit.to_string(),
        };

        match indexer.index_file(&change).await {
            Ok(result) => {
                results.push(result);
            }
            Err(e) => {
                errors.push((path.to_string(), e.to_string()));
                progress.failed += 1;
            }
        }

        progress.processed += 1;
        progress.last_path = Some(path.to_string());
        on_progress(&progress);
    }

    Ok(ReindexResult {
        progress,
        results,
        errors,
    })
}

fn extract_memory_id_from_path(path: &str) -> MemoryId {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");

    if stem.starts_with("mem_") {
        MemoryId::parse(stem).unwrap_or_else(|_| MemoryId::new())
    } else {
        MemoryId::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bm25::Bm25Index;
    use crate::embedder::Embedder;
    use crate::indexer::IndexerConfig;
    use crate::storage::ObjectHash;
    use std::sync::{Arc, Mutex};

    struct MockEmbedder;

    #[async_trait::async_trait]
    impl Embedder for MockEmbedder {
        async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![0.0; 3]).collect())
        }
        fn dimension(&self) -> usize {
            3
        }
        fn model_id(&self) -> &str {
            "mock"
        }
    }

    struct MockVectorStore {
        upserted: Mutex<Vec<String>>,
    }

    impl MockVectorStore {
        fn new() -> Self {
            Self {
                upserted: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl VectorStore for MockVectorStore {
        async fn upsert(
            &self,
            memory_id: &MemoryId,
            _vectors: &[(u32, Vec<f32>, serde_json::Value)],
        ) -> Result<()> {
            self.upserted.lock().unwrap().push(memory_id.to_string());
            Ok(())
        }
        async fn search(
            &self,
            _q: &[f32],
            _l: usize,
            _f: Option<&crate::vector_store::VectorFilter>,
        ) -> Result<Vec<crate::vector_store::VectorMatch>> {
            Ok(Vec::new())
        }
        async fn delete(&self, _mid: &MemoryId) -> Result<()> {
            Ok(())
        }
        async fn reset(&self) -> Result<()> {
            Ok(())
        }
    }

    fn setup() -> (
        Indexer<MockEmbedder, MockVectorStore>,
        Arc<MockVectorStore>,
        tempfile::TempDir,
        ObjectStore,
    ) {
        let embedder = Arc::new(MockEmbedder);
        let vs = Arc::new(MockVectorStore::new());
        let bm25 = Arc::new(Bm25Index::in_memory().unwrap());
        let indexer = Indexer::new(embedder, vs.clone(), bm25, IndexerConfig::default());
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();
        (indexer, vs, dir, store)
    }

    #[tokio::test]
    async fn reindex_empty_workspace() {
        let (indexer, _vs, _dir, store) = setup();
        let inode = InodeIndex::new();

        let result = full_reindex(&indexer, &store, &inode, "ws_test", "abc", None, |_| {})
            .await
            .unwrap();

        assert_eq!(result.progress.total, 0);
        assert_eq!(result.progress.processed, 0);
        assert!(result.results.is_empty());
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn reindex_single_file() {
        let (indexer, vs, _dir, store) = setup();
        let mut inode = InodeIndex::new();

        let content = b"# Hello\n\nWorld.\n";
        let hash = store.put(content).unwrap();
        inode.set("memory/hello.md", hash);

        let result = full_reindex(&indexer, &store, &inode, "ws_test", "abc", None, |_| {})
            .await
            .unwrap();

        assert_eq!(result.progress.total, 1);
        assert_eq!(result.progress.processed, 1);
        assert_eq!(result.results.len(), 1);
        assert!(result.results[0].chunk_count > 0);
        assert!(result.errors.is_empty());

        let upserted = vs.upserted.lock().unwrap();
        assert_eq!(upserted.len(), 1);
    }

    #[tokio::test]
    async fn reindex_multiple_files() {
        let (indexer, vs, _dir, store) = setup();
        let mut inode = InodeIndex::new();

        for i in 0..5 {
            let content = format!("# File {i}\n\nContent for file {i}.\n");
            let hash = store.put(content.as_bytes()).unwrap();
            inode.set(format!("memory/file_{i}.md"), hash);
        }

        let result = full_reindex(&indexer, &store, &inode, "ws_test", "abc", None, |_| {})
            .await
            .unwrap();

        assert_eq!(result.progress.total, 5);
        assert_eq!(result.progress.processed, 5);
        assert_eq!(result.results.len(), 5);

        let upserted = vs.upserted.lock().unwrap();
        assert_eq!(upserted.len(), 5);
    }

    #[tokio::test]
    async fn reindex_resume_from_checkpoint() {
        let (indexer, vs, _dir, store) = setup();
        let mut inode = InodeIndex::new();

        for name in &["a.md", "b.md", "c.md", "d.md"] {
            let content = format!("# {name}\n\nBody.\n");
            let hash = store.put(content.as_bytes()).unwrap();
            inode.set(format!("memory/{name}"), hash);
        }

        let result = full_reindex(
            &indexer,
            &store,
            &inode,
            "ws_test",
            "abc",
            Some("memory/b.md"),
            |_| {},
        )
        .await
        .unwrap();

        // a.md and b.md should be skipped
        assert_eq!(result.progress.total, 4);
        assert_eq!(result.results.len(), 2); // c.md, d.md

        let upserted = vs.upserted.lock().unwrap();
        assert_eq!(upserted.len(), 2);
    }

    #[tokio::test]
    async fn reindex_progress_callback() {
        let (indexer, _vs, _dir, store) = setup();
        let mut inode = InodeIndex::new();

        for i in 0..3 {
            let content = format!("# F{i}\n\nBody.\n");
            let hash = store.put(content.as_bytes()).unwrap();
            inode.set(format!("memory/f{i}.md"), hash);
        }

        let progress_reports = Arc::new(Mutex::new(Vec::new()));
        let reports_clone = progress_reports.clone();

        full_reindex(&indexer, &store, &inode, "ws_test", "abc", None, move |p| {
            reports_clone.lock().unwrap().push(p.clone());
        })
        .await
        .unwrap();

        let reports = progress_reports.lock().unwrap();
        // Initial report + one per file = 4
        assert_eq!(reports.len(), 4);
        assert_eq!(reports[0].processed, 0);
        assert_eq!(reports.last().unwrap().processed, 3);
        assert!((reports.last().unwrap().fraction() - 1.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn reindex_handles_missing_object() {
        let (indexer, _vs, _dir, store) = setup();
        let mut inode = InodeIndex::new();

        let fake_hash = ObjectHash::parse(&"a".repeat(64)).unwrap();
        inode.set("memory/missing.md", fake_hash);

        let result = full_reindex(&indexer, &store, &inode, "ws_test", "abc", None, |_| {})
            .await
            .unwrap();

        assert_eq!(result.progress.total, 1);
        assert_eq!(result.progress.failed, 1);
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].0.contains("missing"));
    }

    #[test]
    fn progress_fraction() {
        let p = ReindexProgress::new(0);
        assert!((p.fraction() - 1.0).abs() < f64::EPSILON);

        let mut p = ReindexProgress::new(10);
        p.processed = 5;
        assert!((p.fraction() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn extract_memory_id_with_prefix() {
        let mid = extract_memory_id_from_path("memory/mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7.md");
        assert!(mid.to_string().starts_with("mem_"));
    }

    #[test]
    fn extract_memory_id_without_prefix() {
        let mid = extract_memory_id_from_path("memory/prefs.md");
        assert!(mid.to_string().starts_with("mem_"));
    }
}
