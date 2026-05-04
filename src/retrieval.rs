//! Multi-signal retrieval engine with Reciprocal Rank Fusion.
//!
//! Queries vector store (semantic) and BM25 (keyword) in parallel, fuses results
//! via RRF, applies scope/recency boosts and entity graph signal, then enforces
//! ACL on the API layer. See `04-tasks-dod.md` Phase 4–5 (tasks 4.1–4.4, 5.3).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::acl;
use crate::bm25::{Bm25Index, Bm25Match};
use crate::embedder::Embedder;
use crate::error::{MemoryFsError, Result};
use crate::graph::EntityGraph;
use crate::policy::Policy;
use crate::storage::{InodeIndex, ObjectStore};
use crate::vector_store::{HybridSearch, VectorFilter, VectorMatch, VectorStore};

/// RRF constant (k). Standard value from the original paper.
const RRF_K: f32 = 60.0;

/// Retrieval parameters for a single context query.
#[derive(Debug, Clone)]
pub struct RetrievalQuery {
    /// Natural language query text.
    pub query: String,
    /// Maximum results to return after fusion.
    pub top_k: usize,
    /// Workspace ID for scoping.
    pub workspace_id: String,
    /// Optional scope filter (e.g. `agent`, `team`, `user`).
    pub scope: Option<String>,
    /// Optional tag filter (AND semantics).
    pub tags: Option<Vec<String>>,
    /// Recency boost half-life in days. Memories older than this get half the boost.
    /// `None` disables recency boost.
    pub recency_half_life_days: Option<f64>,
    /// Use server-side hybrid search when backend supports it.
    /// `None` = use local RRF (default). `Some((vector_w, bm25_w))` = server-side fusion.
    pub hybrid_weights: Option<(f32, f32)>,
    /// Time-travel/audit toggle. When `false` (default) only `status: active`
    /// chunks are eligible — superseded history stays out of normal recall.
    /// When `true` the status filter is dropped so callers see the full
    /// append-only chain (including older revisions and tombstones).
    pub include_superseded: bool,
}

/// A single retrieval result with provenance.
#[derive(Debug, Clone, Serialize)]
pub struct RetrievalResult {
    /// Memory ID of the matched memory.
    pub memory_id: String,
    /// File path in the workspace.
    pub file_path: String,
    /// Final fused score.
    pub score: f32,
    /// Per-source scores for transparency.
    pub source_scores: SourceScores,
    /// Content snippet (read from current FS state).
    pub snippet: String,
    /// Chunk index within the memory.
    pub chunk_index: u32,
    /// ISO 8601 timestamp from frontmatter (for recency boost).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// Per-source score breakdown for provenance.
#[derive(Debug, Clone, Serialize)]
pub struct SourceScores {
    /// Cosine similarity from the vector store.
    pub vector: Option<f32>,
    /// BM25 relevance score from the full-text index.
    pub bm25: Option<f32>,
    /// Entity graph boost (set when query matches an entity and memory mentions it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<f32>,
}

/// Context response returned by the `/v1/context` endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct ContextResponse {
    /// Original query text.
    pub query: String,
    /// Ranked retrieval results with provenance.
    pub results: Vec<RetrievalResult>,
    /// Total candidates before ACL filtering.
    pub total_candidates: usize,
    /// Number of candidates removed by ACL check.
    pub filtered_by_acl: usize,
}

/// Internal candidate used during fusion.
struct Candidate {
    memory_id: String,
    chunk_index: u32,
    file_path: String,
    vector_rank: Option<usize>,
    bm25_rank: Option<usize>,
    vector_score: Option<f32>,
    bm25_score: Option<f32>,
    entity_score: Option<f32>,
    metadata: serde_json::Value,
}

/// Entity graph boost weight in the fused score.
const ENTITY_BOOST_WEIGHT: f32 = 0.5;

/// Multi-signal retrieval engine.
pub struct RetrievalEngine<V: VectorStore, E: Embedder> {
    vector_store: Arc<V>,
    embedder: Arc<E>,
    bm25: Arc<Bm25Index>,
    object_store: Arc<ObjectStore>,
    index: Arc<std::sync::RwLock<InodeIndex>>,
    entity_graph: Option<Arc<std::sync::RwLock<EntityGraph>>>,
    hybrid_backend: Option<Arc<dyn HybridSearch>>,
}

impl<V: VectorStore, E: Embedder> RetrievalEngine<V, E> {
    /// Create a new retrieval engine from its component backends.
    pub fn new(
        vector_store: Arc<V>,
        embedder: Arc<E>,
        bm25: Arc<Bm25Index>,
        object_store: Arc<ObjectStore>,
        index: Arc<std::sync::RwLock<InodeIndex>>,
    ) -> Self {
        Self {
            vector_store,
            embedder,
            bm25,
            object_store,
            index,
            entity_graph: None,
            hybrid_backend: None,
        }
    }

    /// Attach an entity graph for entity-aware retrieval.
    pub fn with_entity_graph(mut self, graph: Arc<std::sync::RwLock<EntityGraph>>) -> Self {
        self.entity_graph = Some(graph);
        self
    }

    /// Attach a server-side hybrid search backend (e.g. Levara).
    pub fn with_hybrid_backend(mut self, backend: Arc<dyn HybridSearch>) -> Self {
        self.hybrid_backend = Some(backend);
        self
    }

    /// Execute a retrieval query: embed → parallel search → RRF → ACL filter → file read.
    ///
    /// When `query.hybrid_weights` is set and a `HybridSearch` backend is attached,
    /// the engine delegates fusion to the server (single gRPC call). Otherwise it does
    /// parallel vector + local BM25 with client-side RRF.
    pub async fn retrieve(
        &self,
        query: &RetrievalQuery,
        subject: &str,
        policy: &Policy,
    ) -> Result<ContextResponse> {
        let fetch_limit = query.top_k * 3;

        let mut candidates =
            if let (Some((vw, bw)), Some(backend)) = (query.hybrid_weights, &self.hybrid_backend) {
                let hybrid_results = backend
                    .hybrid_search(&query.query, fetch_limit, vw, bw)
                    .await?;

                hybrid_results
                    .into_iter()
                    .enumerate()
                    .map(|(rank, vm)| {
                        let file_path = vm
                            .metadata
                            .get("file_path")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Candidate {
                            memory_id: vm.memory_id.to_string(),
                            chunk_index: vm.chunk_index,
                            file_path,
                            vector_rank: Some(rank),
                            bm25_rank: None,
                            vector_score: Some(vm.score),
                            bm25_score: None,
                            entity_score: None,
                            metadata: vm.metadata,
                        }
                    })
                    .collect::<Vec<_>>()
            } else {
                let query_embedding = self
                    .embedder
                    .embed(&[query.query.as_str()])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        MemoryFsError::Internal(anyhow::anyhow!("embedder returned no vectors"))
                    })?;

                let filter = VectorFilter {
                    workspace_id: Some(query.workspace_id.clone()),
                    scope: query.scope.clone(),
                    tags: query.tags.clone(),
                    status: if query.include_superseded {
                        None
                    } else {
                        Some("active".into())
                    },
                };

                let (vector_results, bm25_results) = tokio::join!(
                    self.vector_store
                        .search(&query_embedding, fetch_limit, Some(&filter)),
                    tokio::task::spawn_blocking({
                        let bm25 = self.bm25.clone();
                        let q = query.query.clone();
                        let limit = fetch_limit;
                        move || bm25.search(&q, limit)
                    })
                );

                let vector_results = vector_results?;
                let bm25_results = bm25_results.map_err(|e| {
                    MemoryFsError::Internal(anyhow::anyhow!("BM25 task panicked: {e}"))
                })??;

                self.fuse_rrf(vector_results, bm25_results)
            };

        self.apply_entity_boost(&query.query, &mut candidates);

        let (filtered, acl_denied) = self.apply_acl_filter(candidates, subject, policy);

        let mut results = self.read_snippets(filtered, query.top_k);

        if let Some(half_life) = query.recency_half_life_days {
            apply_recency_boost(&mut results, half_life);
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(query.top_k);

        Ok(ContextResponse {
            query: query.query.clone(),
            total_candidates: results.len() + acl_denied,
            filtered_by_acl: acl_denied,
            results,
        })
    }

    /// Apply entity graph boost: find entities matching the query, then boost
    /// candidates whose file_path or metadata mentions those entities.
    fn apply_entity_boost(&self, query: &str, candidates: &mut [Candidate]) {
        let graph = match &self.entity_graph {
            Some(g) => g,
            None => return,
        };
        let graph = graph.read().unwrap();

        let matched_entities = graph.search(query, None, 10);
        if matched_entities.is_empty() {
            return;
        }

        let mut entity_file_paths: HashSet<String> = HashSet::new();
        let mut entity_ids: HashSet<String> = HashSet::new();

        for entity in &matched_entities {
            entity_file_paths.insert(entity.file_path.clone());
            entity_ids.insert(entity.id.to_string());

            if let Ok((neighbors, _)) = graph.neighbors(&entity.id.to_string(), 1, None) {
                for neighbor in neighbors {
                    entity_file_paths.insert(neighbor.file_path.clone());
                    entity_ids.insert(neighbor.id.to_string());
                }
            }
        }

        for c in candidates.iter_mut() {
            let mut boost = 0.0f32;

            if entity_file_paths.contains(&c.file_path) {
                boost = 1.0;
            } else if let Some(entities) = c.metadata.get("entities") {
                if let Some(arr) = entities.as_array() {
                    for ent in arr {
                        if let Some(eid) = ent.as_str() {
                            if entity_ids.contains(eid) {
                                boost = 0.8;
                                break;
                            }
                        }
                    }
                }
            }

            if boost > 0.0 {
                c.entity_score = Some(boost);
            }
        }
    }

    /// Reciprocal Rank Fusion across vector and BM25 results.
    fn fuse_rrf(
        &self,
        vector_results: Vec<VectorMatch>,
        bm25_results: Vec<Bm25Match>,
    ) -> Vec<Candidate> {
        let mut by_key: HashMap<String, Candidate> = HashMap::new();

        for (rank, vm) in vector_results.iter().enumerate() {
            let mid = vm.memory_id.to_string();
            let key = format!("{}:{}", mid, vm.chunk_index);
            let file_path = vm
                .metadata
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let entry = by_key.entry(key).or_insert_with(|| Candidate {
                memory_id: mid,
                chunk_index: vm.chunk_index,
                file_path: file_path.clone(),
                vector_rank: None,
                bm25_rank: None,
                vector_score: None,
                bm25_score: None,
                entity_score: None,
                metadata: vm.metadata.clone(),
            });
            entry.vector_rank = Some(rank);
            entry.vector_score = Some(vm.score);
            if entry.file_path.is_empty() && !file_path.is_empty() {
                entry.file_path = file_path;
            }
        }

        for (rank, bm) in bm25_results.iter().enumerate() {
            let key = bm.id.clone();
            let parts: Vec<&str> = key.splitn(2, ':').collect();
            let (mid, chunk_idx) = if parts.len() == 2 {
                (parts[0].to_string(), parts[1].parse::<u32>().unwrap_or(0))
            } else {
                (key.clone(), 0)
            };

            let entry = by_key.entry(key).or_insert_with(|| Candidate {
                memory_id: mid,
                chunk_index: chunk_idx,
                file_path: bm.file_path.clone(),
                vector_rank: None,
                bm25_rank: None,
                vector_score: None,
                bm25_score: None,
                entity_score: None,
                metadata: serde_json::Value::Null,
            });
            entry.bm25_rank = Some(rank);
            entry.bm25_score = Some(bm.score);
            if entry.file_path.is_empty() {
                entry.file_path = bm.file_path.clone();
            }
        }

        by_key.into_values().collect()
    }

    /// ACL check on each candidate. Returns (allowed, denied_count).
    fn apply_acl_filter(
        &self,
        candidates: Vec<Candidate>,
        subject: &str,
        policy: &Policy,
    ) -> (Vec<Candidate>, usize) {
        let mut allowed = Vec::new();
        let mut denied = 0usize;

        for c in candidates {
            match acl::check(subject, acl::Action::Read, &c.file_path, policy) {
                Ok(()) => allowed.push(c),
                Err(_) => {
                    tracing::info!(
                        subject = subject,
                        path = c.file_path.as_str(),
                        memory_id = c.memory_id.as_str(),
                        "retrieval ACL denied"
                    );
                    denied += 1;
                }
            }
        }

        (allowed, denied)
    }

    /// Read actual file content from the object store (deterministic file read).
    fn read_snippets(&self, candidates: Vec<Candidate>, limit: usize) -> Vec<RetrievalResult> {
        let index = self.index.read().unwrap();
        let mut results = Vec::with_capacity(candidates.len().min(limit));

        for c in candidates.into_iter().take(limit * 2) {
            let snippet = if !c.file_path.is_empty() {
                index
                    .get(&c.file_path)
                    .and_then(|hash| self.object_store.get(hash).ok())
                    .map(|data| {
                        let content = String::from_utf8_lossy(&data);
                        truncate_snippet(&content, 500)
                    })
                    .unwrap_or_default()
            } else {
                String::new()
            };

            let mut fused = rrf_score(c.vector_rank, c.bm25_rank);
            if let Some(entity_boost) = c.entity_score {
                fused += entity_boost * ENTITY_BOOST_WEIGHT;
            }

            let created_at = c
                .metadata
                .get("created_at")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            results.push(RetrievalResult {
                memory_id: c.memory_id,
                file_path: c.file_path,
                score: fused,
                source_scores: SourceScores {
                    vector: c.vector_score,
                    bm25: c.bm25_score,
                    entity: c.entity_score,
                },
                snippet,
                chunk_index: c.chunk_index,
                created_at,
            });
        }

        results
    }
}

/// Compute RRF score for a candidate present in one or both ranked lists.
fn rrf_score(vector_rank: Option<usize>, bm25_rank: Option<usize>) -> f32 {
    let mut score = 0.0f32;
    if let Some(r) = vector_rank {
        score += 1.0 / (RRF_K + r as f32 + 1.0);
    }
    if let Some(r) = bm25_rank {
        score += 1.0 / (RRF_K + r as f32 + 1.0);
    }
    score
}

/// Apply exponential decay boost based on memory age.
/// score *= 2^(-age_days / half_life_days)
fn apply_recency_boost(results: &mut [RetrievalResult], half_life_days: f64) {
    let now = Utc::now();
    for r in results.iter_mut() {
        let ts_str = match &r.created_at {
            Some(s) => s.as_str(),
            None => continue,
        };
        if let Ok(created) = ts_str.parse::<DateTime<Utc>>() {
            let age_days = (now - created).num_hours() as f64 / 24.0;
            let decay = 2.0_f64.powf(-age_days / half_life_days);
            r.score *= decay.max(0.1) as f32;
        }
    }
}

/// Truncate content to approximately `max_chars`, breaking at word boundaries.
fn truncate_snippet(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let truncated = &content[..max_chars];
    if let Some(pos) = truncated.rfind(char::is_whitespace) {
        format!("{}…", &truncated[..pos])
    } else {
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_score_both_sources() {
        let score = rrf_score(Some(0), Some(0));
        let expected = 2.0 / (RRF_K + 1.0);
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_score_vector_only() {
        let score = rrf_score(Some(0), None);
        let expected = 1.0 / (RRF_K + 1.0);
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_score_bm25_only() {
        let score = rrf_score(None, Some(2));
        let expected = 1.0 / (RRF_K + 3.0);
        assert!((score - expected).abs() < 1e-6);
    }

    #[test]
    fn rrf_score_neither() {
        let score = rrf_score(None, None);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn rrf_rank_0_beats_rank_5() {
        let top = rrf_score(Some(0), Some(0));
        let low = rrf_score(Some(5), Some(5));
        assert!(top > low);
    }

    #[test]
    fn truncate_short_content() {
        let s = "hello world";
        assert_eq!(truncate_snippet(s, 100), "hello world");
    }

    #[test]
    fn truncate_long_content() {
        let s = "the quick brown fox jumps over the lazy dog";
        let result = truncate_snippet(s, 20);
        assert!(result.len() <= 25); // 20 + word break + ellipsis
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_no_whitespace() {
        let s = "abcdefghijklmnopqrstuvwxyz";
        let result = truncate_snippet(s, 10);
        assert_eq!(result, "abcdefghij…");
    }

    #[test]
    fn source_scores_serialize() {
        let scores = SourceScores {
            vector: Some(0.95),
            bm25: None,
            entity: None,
        };
        let json = serde_json::to_value(&scores).unwrap();
        let v = json["vector"].as_f64().unwrap();
        assert!((v - 0.95).abs() < 0.001);
        assert!(json["bm25"].is_null());
        assert!(json.get("entity").is_none());
    }

    #[test]
    fn retrieval_result_serialize() {
        let r = RetrievalResult {
            memory_id: "mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7".into(),
            file_path: "memories/test.md".into(),
            score: 0.032,
            source_scores: SourceScores {
                vector: Some(0.92),
                bm25: Some(15.3),
                entity: None,
            },
            snippet: "test snippet".into(),
            chunk_index: 0,
            created_at: None,
        };
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["memory_id"], "mem_01HZK4M8N1P3Q5R7S9T1V3X5Z7");
        assert_eq!(json["file_path"], "memories/test.md");
        assert!(json["score"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn context_response_serialize() {
        let resp = ContextResponse {
            query: "test".into(),
            results: vec![],
            total_candidates: 10,
            filtered_by_acl: 2,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total_candidates"], 10);
        assert_eq!(json["filtered_by_acl"], 2);
    }

    #[test]
    fn recency_boost_decays_old_results() {
        let mut results = vec![
            RetrievalResult {
                memory_id: "mem_recent".into(),
                file_path: "a.md".into(),
                score: 1.0,
                source_scores: SourceScores {
                    vector: Some(0.9),
                    bm25: None,
                    entity: None,
                },
                snippet: String::new(),
                chunk_index: 0,
                created_at: Some(Utc::now().to_rfc3339()),
            },
            RetrievalResult {
                memory_id: "mem_old".into(),
                file_path: "b.md".into(),
                score: 1.0,
                source_scores: SourceScores {
                    vector: Some(0.9),
                    bm25: None,
                    entity: None,
                },
                snippet: String::new(),
                chunk_index: 0,
                created_at: Some((Utc::now() - chrono::Duration::days(30)).to_rfc3339()),
            },
            RetrievalResult {
                memory_id: "mem_no_ts".into(),
                file_path: "c.md".into(),
                score: 1.0,
                source_scores: SourceScores {
                    vector: Some(0.9),
                    bm25: None,
                    entity: None,
                },
                snippet: String::new(),
                chunk_index: 0,
                created_at: None,
            },
        ];

        apply_recency_boost(&mut results, 7.0);

        assert!(results[0].score > 0.9, "recent should barely decay");
        assert!(
            results[1].score < 0.2,
            "30-day-old with 7-day half-life should decay heavily"
        );
        assert_eq!(results[2].score, 1.0, "no timestamp means no decay");
    }
}
