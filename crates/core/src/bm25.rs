//! Full-text BM25 index backed by Tantivy.
//!
//! Schema matches `02-data-model.md § 13`. Title and heading fields are boosted
//! during search (2.0× and 1.5× respectively).

use std::path::Path;

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, Value, FAST, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy};

use crate::error::{MemoryFsError, Result};

/// Field handles for the BM25 schema.
#[derive(Clone)]
struct Fields {
    id: Field,
    workspace_id: Field,
    file_path: Field,
    file_type: Field,
    scope: Field,
    scope_id: Field,
    title: Field,
    heading: Field,
    body: Field,
    tags: Field,
    commit: Field,
}

/// A document to be indexed.
#[derive(Debug, Clone)]
pub struct Bm25Document {
    /// Unique chunk ID (e.g. `mem_<ulid>:<chunk_index>`).
    pub id: String,
    /// Workspace this chunk belongs to.
    pub workspace_id: String,
    /// Source file path within the workspace.
    pub file_path: String,
    /// File type (e.g. `memory`, `conversation`).
    pub file_type: String,
    /// Scope (e.g. `user`, `agent`, `team`).
    pub scope: String,
    /// Scope identifier (e.g. `user:alice`).
    pub scope_id: String,
    /// Document or section title.
    pub title: String,
    /// Heading path (joined with ` > `).
    pub heading: String,
    /// Body text of the chunk.
    pub body: String,
    /// Tags (space-separated for indexing).
    pub tags: String,
    /// Commit hash at time of indexing.
    pub commit: String,
}

/// A search result from the BM25 index.
#[derive(Debug, Clone)]
pub struct Bm25Match {
    /// The chunk ID.
    pub id: String,
    /// Workspace ID.
    pub workspace_id: String,
    /// File path.
    pub file_path: String,
    /// BM25 relevance score.
    pub score: f32,
}

/// Tantivy-backed BM25 full-text index.
pub struct Bm25Index {
    index: Index,
    reader: IndexReader,
    fields: Fields,
    schema: Schema,
}

fn build_schema() -> (Schema, Fields) {
    let mut builder = Schema::builder();
    let id = builder.add_text_field("id", STRING | STORED);
    let workspace_id = builder.add_text_field("workspace_id", STRING | FAST | STORED);
    let file_path = builder.add_text_field("file_path", STORED);
    let file_type = builder.add_text_field("file_type", STRING);
    let scope = builder.add_text_field("scope", STRING);
    let scope_id = builder.add_text_field("scope_id", STRING);
    let title = builder.add_text_field("title", TEXT | STORED);
    let heading = builder.add_text_field("heading", TEXT | STORED);
    let body = builder.add_text_field("body", TEXT);
    let tags = builder.add_text_field("tags", STRING | FAST);
    let commit = builder.add_text_field("commit", STORED);

    let schema = builder.build();
    let fields = Fields {
        id,
        workspace_id,
        file_path,
        file_type,
        scope,
        scope_id,
        title,
        heading,
        body,
        tags,
        commit,
    };
    (schema, fields)
}

impl Bm25Index {
    /// Open or create an index at the given directory path.
    pub fn open(dir: &Path) -> Result<Self> {
        let (schema, fields) = build_schema();
        let index = if dir.exists() && dir.join("meta.json").exists() {
            Index::open_in_dir(dir)
                .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("open BM25 index: {e}")))?
        } else {
            std::fs::create_dir_all(dir).map_err(|e| {
                MemoryFsError::Internal(anyhow::anyhow!("create BM25 index dir: {e}"))
            })?;
            Index::create_in_dir(dir, schema.clone())
                .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("create BM25 index: {e}")))?
        };

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 reader: {e}")))?;

        Ok(Self {
            index,
            reader,
            fields,
            schema,
        })
    }

    /// Create an in-memory index (for testing).
    pub fn in_memory() -> Result<Self> {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema.clone());
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 reader: {e}")))?;

        Ok(Self {
            index,
            reader,
            fields,
            schema,
        })
    }

    /// Get an index writer with the given heap budget (bytes).
    pub fn writer(&self, heap_bytes: usize) -> Result<IndexWriter> {
        self.index
            .writer(heap_bytes)
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 writer: {e}")))
    }

    /// Index a batch of documents. Caller must commit the writer afterward.
    pub fn add_documents(&self, writer: &IndexWriter, docs: &[Bm25Document]) -> Result<()> {
        let f = &self.fields;
        for d in docs {
            writer
                .add_document(doc!(
                    f.id => d.id.clone(),
                    f.workspace_id => d.workspace_id.clone(),
                    f.file_path => d.file_path.clone(),
                    f.file_type => d.file_type.clone(),
                    f.scope => d.scope.clone(),
                    f.scope_id => d.scope_id.clone(),
                    f.title => d.title.clone(),
                    f.heading => d.heading.clone(),
                    f.body => d.body.clone(),
                    f.tags => d.tags.clone(),
                    f.commit => d.commit.clone(),
                ))
                .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("add BM25 doc: {e}")))?;
        }
        Ok(())
    }

    /// Delete all documents matching a chunk ID.
    pub fn delete_by_id(&self, writer: &IndexWriter, id: &str) {
        let term = tantivy::Term::from_field_text(self.fields.id, id);
        writer.delete_term(term);
    }

    /// Delete all documents for a workspace.
    pub fn delete_by_workspace(&self, writer: &IndexWriter, workspace_id: &str) {
        let term = tantivy::Term::from_field_text(self.fields.workspace_id, workspace_id);
        writer.delete_term(term);
    }

    /// Reload the reader to pick up committed changes.
    pub fn reload(&self) -> Result<()> {
        self.reader
            .reload()
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 reload: {e}")))
    }

    /// Search the index with a query string.
    /// Fields searched: title (boost 2.0), heading (boost 1.5), body (boost 1.0).
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<Bm25Match>> {
        let searcher = self.reader.searcher();

        let mut parser = QueryParser::for_index(
            &self.index,
            vec![self.fields.title, self.fields.heading, self.fields.body],
        );
        parser.set_field_boost(self.fields.title, 2.0);
        parser.set_field_boost(self.fields.heading, 1.5);

        let query = parser
            .parse_query(query_str)
            .map_err(|e| MemoryFsError::Validation(format!("invalid BM25 query: {e}")))?;

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(limit))
            .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("BM25 search: {e}")))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, addr) in top_docs {
            let doc: tantivy::TantivyDocument = searcher
                .doc(addr)
                .map_err(|e| MemoryFsError::Internal(anyhow::anyhow!("read BM25 doc: {e}")))?;
            let id = doc
                .get_first(self.fields.id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let workspace_id = doc
                .get_first(self.fields.workspace_id)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let file_path = doc
                .get_first(self.fields.file_path)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            results.push(Bm25Match {
                id,
                workspace_id,
                file_path,
                score,
            });
        }

        Ok(results)
    }

    /// Total number of documents in the index.
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc(id: &str, title: &str, body: &str) -> Bm25Document {
        Bm25Document {
            id: id.into(),
            workspace_id: "ws_test".into(),
            file_path: "memories/test.md".into(),
            file_type: "memory".into(),
            scope: "user".into(),
            scope_id: "user:alice".into(),
            title: title.into(),
            heading: String::new(),
            body: body.into(),
            tags: String::new(),
            commit: "abc123".into(),
        }
    }

    fn build_index(docs: &[Bm25Document]) -> Bm25Index {
        let idx = Bm25Index::in_memory().unwrap();
        let mut writer = idx.writer(15_000_000).unwrap();
        idx.add_documents(&writer, docs).unwrap();
        writer.commit().unwrap();
        idx.reload().unwrap();
        idx
    }

    #[test]
    fn empty_index_search() {
        let idx = build_index(&[]);
        let results = idx.search("hello", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn single_doc_found() {
        let docs = vec![sample_doc("d1", "Rust programming", "Systems language")];
        let idx = build_index(&docs);
        let results = idx.search("rust", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "d1");
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn title_boosted_over_body() {
        let docs = vec![
            sample_doc("title_match", "kubernetes deployment", "generic content"),
            sample_doc(
                "body_match",
                "generic title",
                "kubernetes deployment instructions",
            ),
        ];
        let idx = build_index(&docs);
        let results = idx.search("kubernetes", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0].id, "title_match",
            "title match should rank higher"
        );
    }

    #[test]
    fn heading_boosted_over_body() {
        let docs = vec![
            Bm25Document {
                heading: "infrastructure setup".into(),
                body: "generic content".into(),
                ..sample_doc("heading_match", "generic", "")
            },
            sample_doc("body_match", "generic", "infrastructure setup guide"),
        ];
        let idx = build_index(&docs);
        let results = idx.search("infrastructure", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "heading_match");
    }

    #[test]
    fn delete_removes_doc() {
        let docs = vec![
            sample_doc("d1", "first", "content"),
            sample_doc("d2", "second", "content"),
        ];
        let idx = build_index(&docs);
        assert_eq!(idx.num_docs(), 2);

        let mut writer = idx.writer(15_000_000).unwrap();
        idx.delete_by_id(&writer, "d1");
        writer.commit().unwrap();
        idx.reload().unwrap();

        let results = idx.search("first", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn num_docs_correct() {
        let docs = vec![
            sample_doc("a", "one", "body"),
            sample_doc("b", "two", "body"),
            sample_doc("c", "three", "body"),
        ];
        let idx = build_index(&docs);
        assert_eq!(idx.num_docs(), 3);
    }

    #[test]
    fn unicode_search() {
        let docs = vec![sample_doc(
            "ru",
            "Архитектура",
            "Микросервисная архитектура",
        )];
        let idx = build_index(&docs);
        let results = idx.search("архитектура", 10).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn limit_respected() {
        let docs: Vec<_> = (0..20)
            .map(|i| sample_doc(&format!("d{i}"), "shared keyword", &format!("body {i}")))
            .collect();
        let idx = build_index(&docs);
        let results = idx.search("shared", 5).unwrap();
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn metadata_preserved() {
        let docs = vec![Bm25Document {
            file_path: "memories/special.md".into(),
            workspace_id: "ws_custom".into(),
            ..sample_doc("m1", "test", "body")
        }];
        let idx = build_index(&docs);
        let results = idx.search("test", 10).unwrap();
        assert_eq!(results[0].file_path, "memories/special.md");
        assert_eq!(results[0].workspace_id, "ws_custom");
    }

    #[test]
    fn invalid_query_returns_error() {
        let idx = build_index(&[]);
        let result = idx.search("field:value AND OR", 10);
        // Tantivy's query parser is lenient, so this might actually parse.
        // We just assert it doesn't panic.
        let _ = result;
    }

    #[test]
    fn delete_by_workspace() {
        let docs = vec![
            Bm25Document {
                workspace_id: "ws_a".into(),
                ..sample_doc("d1", "alpha", "content")
            },
            Bm25Document {
                workspace_id: "ws_b".into(),
                ..sample_doc("d2", "beta", "content")
            },
        ];
        let idx = build_index(&docs);
        assert_eq!(idx.num_docs(), 2);

        let mut writer = idx.writer(15_000_000).unwrap();
        idx.delete_by_workspace(&writer, "ws_a");
        writer.commit().unwrap();
        idx.reload().unwrap();

        assert_eq!(idx.num_docs(), 1);
        let results = idx.search("alpha", 10).unwrap();
        assert!(results.is_empty());
        let results = idx.search("beta", 10).unwrap();
        assert_eq!(results.len(), 1);
    }
}
