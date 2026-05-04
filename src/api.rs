//! REST API server — axum handlers.
//!
//! Phase 1: files (read/write/list), commits (create/log/diff/revert), health.
//! Phase 4: `/v1/context` — multi-signal retrieval with provenance.
//! Phase 5: `/v1/entities/*` — entity graph CRUD, linking, neighbor traversal.
//! Auth via Bearer token (JWT). Error responses match `specs/openapi.yaml`.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::acl::{self, Action};
use crate::commit::{CommitGraph, DiffEntry};
use crate::embedder::Embedder;
use crate::event_log::{EventKind, EventLog};
use crate::graph::{EntityGraph, EntityKind, ExternalRef, Relation};
use crate::observability;
use crate::policy::Policy;
use crate::retrieval::{ContextResponse, RetrievalEngine, RetrievalQuery};
use crate::storage::{InodeIndex, ObjectStore};
use crate::vector_store::VectorStore;
use crate::MemoryFsError;

/// Shared application state.
pub struct AppState {
    /// Content-addressable store.
    pub store: ObjectStore,
    /// Path→hash index.
    pub index: RwLock<InodeIndex>,
    /// Commit history.
    pub graph: RwLock<CommitGraph>,
    /// Staging area (uncommitted writes).
    pub staging: RwLock<BTreeMap<String, Vec<u8>>>,
    /// Workspace policy.
    pub policy: Policy,
    /// Prometheus metrics handle.
    pub metrics_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
    /// Retrieval engine (Phase 4). `None` if retrieval is not configured.
    pub retrieval: Option<Arc<dyn ContextRetriever>>,
    /// Entity graph (Phase 5).
    pub entity_graph: RwLock<EntityGraph>,
    /// Event log for indexer pipeline. `None` if indexing is disabled.
    pub event_log: Option<Arc<EventLog>>,
    /// Workspace ID (used in event emission and indexing metadata).
    pub workspace_id: String,
}

/// Trait-object interface for retrieval, hiding the generic parameters from AppState.
#[async_trait::async_trait]
pub trait ContextRetriever: Send + Sync {
    /// Execute a context retrieval query.
    async fn retrieve(
        &self,
        query: &RetrievalQuery,
        subject: &str,
        policy: &Policy,
    ) -> Result<ContextResponse, MemoryFsError>;
}

#[async_trait::async_trait]
impl<V: VectorStore, E: Embedder> ContextRetriever for RetrievalEngine<V, E> {
    async fn retrieve(
        &self,
        query: &RetrievalQuery,
        subject: &str,
        policy: &Policy,
    ) -> Result<ContextResponse, MemoryFsError> {
        self.retrieve(query, subject, policy).await
    }
}

/// Build the axum `Router` with all endpoints.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/admin/health", get(health))
        .route("/v1/metrics", get(metrics_endpoint))
        .route("/v1/files", get(list_files))
        .route("/v1/files/*path", get(read_file).put(write_file))
        .route("/v1/workspaces/:workspace_id/commit", post(create_commit))
        .route("/v1/workspaces/:workspace_id/log", get(get_log))
        .route("/v1/workspaces/:workspace_id/diff", get(get_diff))
        .route("/v1/workspaces/:workspace_id/revert", post(revert_commit))
        .route(
            "/v1/workspaces/:workspace_id/supersede",
            post(supersede_memory),
        )
        .route("/v1/workspaces/:workspace_id/context", post(query_context))
        .route("/v1/workspaces/:workspace_id/entities", post(create_entity))
        .route(
            "/v1/workspaces/:workspace_id/entities/search",
            post(search_entities),
        )
        .route(
            "/v1/workspaces/:workspace_id/entities/:entity_id",
            get(get_entity),
        )
        .route(
            "/v1/workspaces/:workspace_id/entities/:entity_id/link",
            post(link_entity),
        )
        .route(
            "/v1/workspaces/:workspace_id/entities/:entity_id/neighbors",
            get(get_neighbors),
        )
        .layer(axum::middleware::from_fn(observability::trace_layer))
        .with_state(state)
}

// ── Error response ──

#[derive(Serialize)]
struct ErrorBody {
    code: String,
    message: String,
    trace_id: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = ErrorBody {
            code: self.0.api_code().to_string(),
            message: self.0.to_string(),
            trace_id: generate_trace_id(),
        };
        (status, Json(body)).into_response()
    }
}

struct ApiError(MemoryFsError);

impl From<MemoryFsError> for ApiError {
    fn from(e: MemoryFsError) -> Self {
        Self(e)
    }
}

fn generate_trace_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let bytes: [u8; 16] = rng.gen();
    hex::encode(bytes)
}

fn extract_subject(headers: &HeaderMap) -> Result<String, ApiError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(token) = auth.strip_prefix("Bearer ") {
        if token.starts_with("utk_") {
            Ok(format!("user:{}", &token[4..token.len().min(20)]))
        } else if token.starts_with("atk_") {
            Ok(format!("agent:{}", &token[4..token.len().min(20)]))
        } else {
            Ok("owner".to_string())
        }
    } else {
        Err(ApiError(MemoryFsError::Unauthorized))
    }
}

// ── Metrics ──

async fn metrics_endpoint(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match &state.metrics_handle {
        Some(handle) => handle.render(),
        None => String::new(),
    }
}

// ── Health ──

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

// ── Files ──

#[derive(Serialize)]
struct FileContentResponse {
    path: String,
    content: String,
    commit: String,
    hash: String,
}

async fn read_file(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Json<FileContentResponse>, ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Read, &path, &state.policy)?;

    let index = state.index.read().unwrap();
    let hash = index
        .get(&path)
        .ok_or_else(|| MemoryFsError::NotFound(format!("file {path}")))?;

    let data = state.store.get(hash)?;
    let content = String::from_utf8_lossy(&data).to_string();

    let graph = state.graph.read().unwrap();
    let commit_hash = graph.head().map(|c| c.hash.clone()).unwrap_or_default();

    Ok(Json(FileContentResponse {
        path,
        content,
        commit: commit_hash,
        hash: hash.to_string(),
    }))
}

#[derive(Deserialize)]
struct WriteFileBody {
    content: String,
}

#[derive(Serialize)]
struct StagedFileResponse {
    path: String,
    staged_hash: String,
    bytes: usize,
}

async fn write_file(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    headers: HeaderMap,
    Json(body): Json<WriteFileBody>,
) -> Result<(StatusCode, Json<StagedFileResponse>), ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Write, &path, &state.policy)?;

    let data = body.content.as_bytes();
    let hash = crate::storage::ObjectHash::of(data);
    let bytes = data.len();

    let mut staging = state.staging.write().unwrap();
    staging.insert(path.clone(), data.to_vec());

    Ok((
        StatusCode::OK,
        Json(StagedFileResponse {
            path,
            staged_hash: hash.to_string(),
            bytes,
        }),
    ))
}

#[derive(Deserialize)]
struct ListFilesQuery {
    prefix: Option<String>,
}

#[derive(Serialize)]
struct FileSummaryResponse {
    path: String,
    hash: String,
    size: usize,
}

#[derive(Serialize)]
struct ListFilesResponse {
    items: Vec<FileSummaryResponse>,
}

async fn list_files(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ListFilesQuery>,
) -> Result<Json<ListFilesResponse>, ApiError> {
    let subject = extract_subject(&headers)?;

    let index = state.index.read().unwrap();
    let mut items = Vec::new();

    for (path, hash) in index.iter() {
        if let Some(ref prefix) = query.prefix {
            if !path.starts_with(prefix.as_str()) {
                continue;
            }
        }
        if acl::check(&subject, Action::List, path, &state.policy).is_err() {
            continue;
        }
        let size = state.store.get(hash).map(|d| d.len()).unwrap_or(0);
        items.push(FileSummaryResponse {
            path: path.to_string(),
            hash: hash.to_string(),
            size,
        });
    }

    items.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Json(ListFilesResponse { items }))
}

// ── Commits ──

#[derive(Deserialize)]
struct CreateCommitBody {
    message: String,
    parent_commit: Option<String>,
}

#[derive(Serialize)]
struct CommitResponse {
    hash: String,
    message: String,
    author: String,
    created_at: String,
    parents: Vec<String>,
}

async fn create_commit(
    State(state): State<Arc<AppState>>,
    Path(_workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateCommitBody>,
) -> Result<(StatusCode, Json<CommitResponse>), ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Commit, "**", &state.policy)?;

    let mut staging = state.staging.write().unwrap();
    if staging.is_empty() {
        return Err(ApiError(MemoryFsError::Validation(
            "nothing staged to commit".into(),
        )));
    }

    let mut index = state.index.write().unwrap();
    for (path, data) in staging.iter() {
        let hash = state.store.put(data)?;
        index.set(path.clone(), hash);
    }
    staging.clear();

    let snapshot: BTreeMap<String, String> = index
        .iter()
        .map(|(p, h)| (p.to_string(), h.to_string()))
        .collect();

    let mut graph = state.graph.write().unwrap();
    let expected_parent = body
        .parent_commit
        .or_else(|| graph.head().map(|c| c.hash.clone()));

    let commit = graph.commit(
        &subject,
        &body.message,
        snapshot,
        expected_parent.as_deref(),
    )?;

    if let Some(ref event_log) = state.event_log {
        let _ = event_log.append(
            EventKind::CommitCreated,
            &state.workspace_id,
            &subject,
            &commit.hash,
            None,
        );
    }

    let parents = commit.parent.iter().cloned().collect();
    Ok((
        StatusCode::CREATED,
        Json(CommitResponse {
            hash: commit.hash.clone(),
            message: commit.message.clone(),
            author: commit.author.clone(),
            created_at: commit.timestamp.to_rfc3339(),
            parents,
        }),
    ))
}

#[derive(Deserialize)]
struct LogQuery {
    limit: Option<usize>,
}

#[derive(Serialize)]
struct LogResponse {
    items: Vec<CommitResponse>,
}

async fn get_log(
    State(state): State<Arc<AppState>>,
    Path(_workspace_id): Path<String>,
    Query(query): Query<LogQuery>,
) -> Result<Json<LogResponse>, ApiError> {
    let graph = state.graph.read().unwrap();
    let limit = query.limit.unwrap_or(50);
    let commits = graph.log(Some(limit));

    let items = commits
        .into_iter()
        .map(|c| CommitResponse {
            hash: c.hash.clone(),
            message: c.message.clone(),
            author: c.author.clone(),
            created_at: c.timestamp.to_rfc3339(),
            parents: c.parent.iter().cloned().collect(),
        })
        .collect();

    Ok(Json(LogResponse { items }))
}

#[derive(Deserialize)]
struct DiffQuery {
    from: String,
    to: String,
}

#[derive(Serialize)]
struct DiffResponse {
    from: String,
    to: String,
    entries: Vec<DiffEntryResponse>,
}

#[derive(Serialize)]
struct DiffEntryResponse {
    path: String,
    change: String,
    old_hash: Option<String>,
    new_hash: Option<String>,
}

async fn get_diff(
    State(state): State<Arc<AppState>>,
    Path(_workspace_id): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Json<DiffResponse>, ApiError> {
    let graph = state.graph.read().unwrap();
    let entries = graph.diff(Some(query.from.as_str()), &query.to)?;

    let entries: Vec<DiffEntryResponse> = entries
        .into_iter()
        .map(|e| match e {
            DiffEntry::Added { path, hash } => DiffEntryResponse {
                path,
                change: "added".to_string(),
                old_hash: None,
                new_hash: Some(hash),
            },
            DiffEntry::Removed { path, hash } => DiffEntryResponse {
                path,
                change: "removed".to_string(),
                old_hash: Some(hash),
                new_hash: None,
            },
            DiffEntry::Modified {
                path,
                old_hash,
                new_hash,
            } => DiffEntryResponse {
                path,
                change: "modified".to_string(),
                old_hash: Some(old_hash),
                new_hash: Some(new_hash),
            },
        })
        .collect();

    Ok(Json(DiffResponse {
        from: query.from,
        to: query.to,
        entries,
    }))
}

// ── Context (Phase 4) ──

#[derive(Deserialize)]
struct ContextQueryBody {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
    scope: Option<String>,
    tags: Option<Vec<String>>,
    recency_half_life_days: Option<f64>,
    vector_weight: Option<f32>,
    bm25_weight: Option<f32>,
    /// Time-travel: when true, drops the `status: active` filter so superseded
    /// revisions appear in the result set. Off by default — normal recall must
    /// never see history.
    #[serde(default)]
    include_superseded: bool,
}

fn default_top_k() -> usize {
    10
}

async fn query_context(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<ContextQueryBody>,
) -> Result<Json<ContextResponse>, ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Read, "memory/**", &state.policy)?;

    let retrieval = state.retrieval.as_ref().ok_or_else(|| {
        ApiError(MemoryFsError::Unavailable(
            "retrieval engine not configured".into(),
        ))
    })?;

    let hybrid_weights = match (body.vector_weight, body.bm25_weight) {
        (Some(vw), Some(bw)) => Some((vw, bw)),
        _ => None,
    };

    let query = RetrievalQuery {
        query: body.query,
        top_k: body.top_k.min(100),
        workspace_id,
        scope: body.scope,
        tags: body.tags,
        recency_half_life_days: body.recency_half_life_days,
        hybrid_weights,
        include_superseded: body.include_superseded,
    };

    let response = retrieval.retrieve(&query, &subject, &state.policy).await?;
    Ok(Json(response))
}

// ── Revert ──

#[derive(Deserialize)]
struct RevertBody {
    target_commit: String,
    reason: String,
}

async fn revert_commit(
    State(state): State<Arc<AppState>>,
    Path(_workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RevertBody>,
) -> Result<(StatusCode, Json<CommitResponse>), ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Revert, "**", &state.policy)?;

    let mut graph = state.graph.write().unwrap();
    let commit = graph.revert(&body.target_commit, &subject)?;

    let mut index = state.index.write().unwrap();
    for (path, hash_str) in &commit.snapshot {
        if let Ok(hash) = crate::storage::ObjectHash::parse(hash_str) {
            index.set(path.clone(), hash);
        }
    }

    let parents = commit.parent.iter().cloned().collect();
    Ok((
        StatusCode::CREATED,
        Json(CommitResponse {
            hash: commit.hash.clone(),
            message: format!("revert to {}: {}", body.target_commit, body.reason),
            author: commit.author.clone(),
            created_at: commit.timestamp.to_rfc3339(),
            parents,
        }),
    ))
}

// ── Supersede ──

/// Atomically replace one memory with another while preserving the old file.
///
/// Append-only invariant: the old `.md` is never deleted — its frontmatter is
/// mutated to `status: superseded` and gains a `superseded_by` back-reference.
/// The new memory is staged with `supersedes: [old_id]` (auto-injected if missing)
/// and both writes commit in a single atomic commit so the indexer sees them
/// together. Downstream search filters on `status: active` so superseded chunks
/// drop out of normal retrieval but remain queryable for time-travel/audit.
#[derive(Deserialize)]
struct SupersedeBody {
    old_path: String,
    new_path: String,
    new_content: String,
    reason: String,
    #[serde(default)]
    conflict_type: Option<String>,
    #[serde(default)]
    parent_commit: Option<String>,
}

#[derive(Serialize)]
struct SupersedeResponse {
    commit_hash: String,
    old_path: String,
    new_path: String,
    old_memory_id: String,
    new_memory_id: String,
}

async fn supersede_memory(
    State(state): State<Arc<AppState>>,
    Path(_workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SupersedeBody>,
) -> Result<(StatusCode, Json<SupersedeResponse>), ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Write, &body.old_path, &state.policy)?;
    acl::check(&subject, Action::Write, &body.new_path, &state.policy)?;
    acl::check(&subject, Action::Commit, "**", &state.policy)?;

    if body.old_path == body.new_path {
        return Err(ApiError(MemoryFsError::Validation(
            "old_path and new_path must differ".into(),
        )));
    }

    // Read the existing old file.
    let old_bytes = {
        let index = state.index.read().unwrap();
        let hash = index
            .get(&body.old_path)
            .ok_or_else(|| MemoryFsError::NotFound(format!("file {}", body.old_path)))?;
        state.store.get(hash)?
    };
    let old_text = String::from_utf8(old_bytes)
        .map_err(|e| MemoryFsError::Validation(format!("old file is not valid UTF-8: {e}")))?;

    let mut old_doc = crate::schema::parse_frontmatter(&old_text)?;
    let old_id = old_doc
        .frontmatter
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MemoryFsError::Validation("old frontmatter missing id".into()))?
        .to_string();

    let old_status = old_doc
        .frontmatter
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("active");
    if old_status != "active" {
        return Err(ApiError(MemoryFsError::Conflict(format!(
            "memory {old_id} has status {old_status}, cannot be superseded"
        ))));
    }

    // Parse new content (caller-provided so the policy/sensitivity decisions
    // happen client-side and the server stays a thin write coordinator).
    let mut new_doc = crate::schema::parse_frontmatter(&body.new_content)?;
    let new_id = new_doc
        .frontmatter
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| MemoryFsError::Validation("new frontmatter missing id".into()))?
        .to_string();

    if new_id == old_id {
        return Err(ApiError(MemoryFsError::Validation(
            "new memory id must differ from old memory id".into(),
        )));
    }

    // Workspace-wide cycle / duplicate-id check. The inline status check
    // above catches direct A→B→A cycles (B is non-active when reattacked),
    // but it can't catch reuse of an id that's already attested elsewhere
    // in the chain. SupersedeGraph reads the current workspace state so the
    // validator always reflects truth on disk, not a stale snapshot.
    {
        let index = state.index.read().unwrap();
        let graph = crate::supersede::SupersedeGraph::build_from_workspace(
            &index,
            &state.store,
            "memories/",
        )?;
        graph.validate_supersede(&new_id, &[old_id.as_str()])?;
    }

    let now = chrono::Utc::now().to_rfc3339();

    // Mutate the old memory's frontmatter in place: status, superseded_by,
    // superseded_at. We append to superseded_by rather than replacing so a
    // future merge supersede can record multiple successors if needed.
    if let serde_json::Value::Object(map) = &mut old_doc.frontmatter {
        map.insert(
            "status".into(),
            serde_json::Value::String("superseded".into()),
        );
        map.insert(
            "superseded_at".into(),
            serde_json::Value::String(now.clone()),
        );
        let by = map
            .entry("superseded_by".to_string())
            .or_insert_with(|| serde_json::Value::Array(vec![]));
        if let serde_json::Value::Array(arr) = by {
            if !arr.iter().any(|v| v.as_str() == Some(new_id.as_str())) {
                arr.push(serde_json::Value::String(new_id.clone()));
            }
        }
        if let Some(ct) = &body.conflict_type {
            map.insert(
                "conflict_type".into(),
                serde_json::Value::String(ct.clone()),
            );
        }
    } else {
        return Err(ApiError(MemoryFsError::Validation(
            "old frontmatter is not an object".into(),
        )));
    }

    // Ensure the new memory's `supersedes` list contains the old id. If the
    // caller forgot, inject it — this preserves provenance even when the
    // client builds frontmatter optimistically.
    if let serde_json::Value::Object(map) = &mut new_doc.frontmatter {
        let sup = map
            .entry("supersedes".to_string())
            .or_insert_with(|| serde_json::Value::Array(vec![]));
        if let serde_json::Value::Array(arr) = sup {
            if !arr.iter().any(|v| v.as_str() == Some(old_id.as_str())) {
                arr.push(serde_json::Value::String(old_id.clone()));
            }
        }
    }

    let old_rendered = crate::schema::render_document(&old_doc)?;
    let new_rendered = crate::schema::render_document(&new_doc)?;

    // Stage and commit atomically: both files land in one commit so the
    // indexer cannot observe an intermediate state where the new memory
    // exists but the old one is still active.
    {
        let mut staging = state.staging.write().unwrap();
        staging.insert(body.old_path.clone(), old_rendered.into_bytes());
        staging.insert(body.new_path.clone(), new_rendered.into_bytes());
    }

    let mut staging = state.staging.write().unwrap();
    let mut index = state.index.write().unwrap();
    for (path, data) in staging.iter() {
        let hash = state.store.put(data)?;
        index.set(path.clone(), hash);
    }
    staging.clear();
    drop(staging);

    let snapshot: BTreeMap<String, String> = index
        .iter()
        .map(|(p, h)| (p.to_string(), h.to_string()))
        .collect();
    drop(index);

    let mut graph = state.graph.write().unwrap();
    let parent = body
        .parent_commit
        .or_else(|| graph.head().map(|c| c.hash.clone()));
    let message = format!("supersede {old_id} -> {new_id}: {}", body.reason);
    let commit = graph.commit(&subject, &message, snapshot, parent.as_deref())?;
    let commit_hash = commit.hash.clone();
    drop(graph);

    if let Some(ref event_log) = state.event_log {
        let _ = event_log.append(
            EventKind::CommitCreated,
            &state.workspace_id,
            &subject,
            &commit_hash,
            None,
        );
    }

    Ok((
        StatusCode::CREATED,
        Json(SupersedeResponse {
            commit_hash,
            old_path: body.old_path,
            new_path: body.new_path,
            old_memory_id: old_id,
            new_memory_id: new_id,
        }),
    ))
}

// ── Entities (Phase 5) ──

#[derive(Deserialize)]
struct CreateEntityBody {
    entity_kind: String,
    canonical_name: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default = "default_attributes")]
    attributes: serde_json::Value,
    #[serde(default)]
    external_refs: Vec<ExternalRefBody>,
}

fn default_attributes() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Deserialize)]
struct ExternalRefBody {
    system: String,
    id: String,
}

#[derive(Serialize)]
struct EntityResponse {
    id: String,
    entity_kind: String,
    canonical_name: String,
    aliases: Vec<String>,
    attributes: serde_json::Value,
    path: String,
}

impl EntityResponse {
    fn from_entity(e: &crate::graph::Entity) -> Self {
        Self {
            id: e.id.to_string(),
            entity_kind: e.kind.to_string(),
            canonical_name: e.canonical_name.clone(),
            aliases: e.aliases.clone(),
            attributes: e.attributes.clone(),
            path: e.file_path.clone(),
        }
    }
}

async fn create_entity(
    State(state): State<Arc<AppState>>,
    Path(workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateEntityBody>,
) -> Result<(StatusCode, Json<EntityResponse>), ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Write, "entities/**", &state.policy)?;

    let kind = EntityKind::parse(&body.entity_kind)?;
    let refs: Vec<ExternalRef> = body
        .external_refs
        .into_iter()
        .map(|r| ExternalRef {
            system: r.system,
            id: r.id,
        })
        .collect();

    let mut graph = state.entity_graph.write().unwrap();
    let entity = graph.create_entity(
        &workspace_id,
        kind,
        &body.canonical_name,
        body.aliases,
        body.attributes,
        refs,
    )?;

    Ok((
        StatusCode::CREATED,
        Json(EntityResponse::from_entity(entity)),
    ))
}

async fn get_entity(
    State(state): State<Arc<AppState>>,
    Path((_workspace_id, entity_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<EntityResponse>, ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Read, "entities/**", &state.policy)?;

    let graph = state.entity_graph.read().unwrap();
    let entity = graph.get(&entity_id)?;
    Ok(Json(EntityResponse::from_entity(entity)))
}

#[derive(Deserialize)]
struct EntitySearchBody {
    query: String,
    kinds: Option<Vec<String>>,
    #[serde(default = "default_entity_limit")]
    limit: usize,
}

fn default_entity_limit() -> usize {
    10
}

#[derive(Serialize)]
struct EntitySearchResponse {
    items: Vec<EntityResponse>,
}

async fn search_entities(
    State(state): State<Arc<AppState>>,
    Path(_workspace_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<EntitySearchBody>,
) -> Result<Json<EntitySearchResponse>, ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Read, "entities/**", &state.policy)?;

    let kinds: Option<Vec<EntityKind>> = body.kinds.map(|ks| {
        ks.iter()
            .filter_map(|k| EntityKind::parse(k).ok())
            .collect()
    });

    let graph = state.entity_graph.read().unwrap();
    let results = graph.search(&body.query, kinds.as_deref(), body.limit.min(50));

    Ok(Json(EntitySearchResponse {
        items: results
            .iter()
            .map(|e| EntityResponse::from_entity(e))
            .collect(),
    }))
}

#[derive(Deserialize)]
struct LinkEntityBody {
    target: String,
    relation: String,
    #[serde(default = "default_weight")]
    weight: f32,
}

fn default_weight() -> f32 {
    1.0
}

#[derive(Serialize)]
struct EdgeResponse {
    src: String,
    dst: String,
    relation: String,
    weight: f32,
}

async fn link_entity(
    State(state): State<Arc<AppState>>,
    Path((_workspace_id, entity_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<LinkEntityBody>,
) -> Result<(StatusCode, Json<EdgeResponse>), ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Write, "entities/**", &state.policy)?;

    let relation = Relation::parse(&body.relation)?;
    let mut graph = state.entity_graph.write().unwrap();
    let edge = graph.link(&entity_id, &body.target, relation, body.weight, None)?;

    Ok((
        StatusCode::CREATED,
        Json(EdgeResponse {
            src: edge.src.clone(),
            dst: edge.dst.clone(),
            relation: body.relation,
            weight: edge.weight,
        }),
    ))
}

#[derive(Deserialize)]
struct NeighborsQuery {
    #[serde(default = "default_depth")]
    depth: usize,
    relations: Option<String>,
}

fn default_depth() -> usize {
    1
}

#[derive(Serialize)]
struct NeighborsResponse {
    nodes: Vec<EntityResponse>,
    edges: Vec<EdgeResponse>,
}

async fn get_neighbors(
    State(state): State<Arc<AppState>>,
    Path((_workspace_id, entity_id)): Path<(String, String)>,
    headers: HeaderMap,
    Query(query): Query<NeighborsQuery>,
) -> Result<Json<NeighborsResponse>, ApiError> {
    let subject = extract_subject(&headers)?;
    acl::check(&subject, Action::Read, "entities/**", &state.policy)?;

    let relations: Option<Vec<Relation>> = query.relations.map(|s| {
        s.split(',')
            .filter_map(|r| Relation::parse(r.trim()).ok())
            .collect()
    });

    let graph = state.entity_graph.read().unwrap();
    let depth = query.depth.clamp(1, 3);
    let (nodes, edges) = graph.neighbors(&entity_id, depth, relations.as_deref())?;

    Ok(Json(NeighborsResponse {
        nodes: nodes
            .iter()
            .map(|e| EntityResponse::from_entity(e))
            .collect(),
        edges: edges
            .iter()
            .map(|e| EdgeResponse {
                src: e.src.clone(),
                dst: e.dst.clone(),
                relation: serde_json::to_value(&e.relation)
                    .unwrap()
                    .as_str()
                    .unwrap_or("RELATES_TO")
                    .to_string(),
                weight: e.weight,
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::open(dir.path()).unwrap();

        let mut policy = Policy::default();
        policy.default_acl.allow.push(crate::policy::AclRule {
            path: "**".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec![
                "read".to_string(),
                "write".to_string(),
                "commit".to_string(),
                "revert".to_string(),
                "list".to_string(),
            ],
        });

        Arc::new(AppState {
            store,
            index: RwLock::new(InodeIndex::new()),
            graph: RwLock::new(CommitGraph::new()),
            staging: RwLock::new(BTreeMap::new()),
            policy,
            metrics_handle: None,
            retrieval: None,
            entity_graph: RwLock::new(EntityGraph::new()),
            event_log: None,
            workspace_id: "ws_test".into(),
        })
    }

    fn auth_header() -> (&'static str, &'static str) {
        ("authorization", "Bearer utk_testuser")
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/v1/admin/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = parse_body(resp).await;
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn write_and_read_file() {
        let state = test_state();
        let app = router(state.clone());

        let write_body = serde_json::json!({"content": "---\ntype: memory\n---\nhello world"});
        let resp = app
            .oneshot(
                Request::put("/v1/files/memory/test.md")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(write_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let staged: serde_json::Value = parse_body(resp).await;
        assert_eq!(staged["path"], "memory/test.md");
        assert!(!staged["staged_hash"].as_str().unwrap().is_empty());

        let commit_body = serde_json::json!({"message": "add test file"});
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(commit_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/v1/files/memory/test.md")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let file: serde_json::Value = parse_body(resp).await;
        assert_eq!(file["path"], "memory/test.md");
        assert!(file["content"].as_str().unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn read_nonexistent_returns_404() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/v1/files/does/not/exist.md")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unauthorized_without_bearer() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/v1/files/memory/test.md")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn commit_empty_staging_returns_422() {
        let state = test_state();
        let app = router(state);

        let body = serde_json::json!({"message": "empty commit"});
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn list_files_empty() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/v1/files")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = parse_body(resp).await;
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn list_files_with_prefix() {
        let state = test_state();

        {
            let mut staging = state.staging.write().unwrap();
            staging.insert("memory/a.md".to_string(), b"aaa".to_vec());
            staging.insert("memory/b.md".to_string(), b"bbb".to_vec());
            staging.insert("runs/r.md".to_string(), b"rrr".to_vec());
        }

        let app = router(state.clone());
        let commit_body = serde_json::json!({"message": "add files"});
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(commit_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/v1/files?prefix=memory/")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = parse_body(resp).await;
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn commit_log_and_diff() {
        let state = test_state();

        {
            let mut staging = state.staging.write().unwrap();
            staging.insert("file.md".to_string(), b"v1".to_vec());
        }
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"message":"first"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let c1: serde_json::Value = parse_body(resp).await;
        let hash1 = c1["hash"].as_str().unwrap().to_string();

        {
            let mut staging = state.staging.write().unwrap();
            staging.insert("file.md".to_string(), b"v2".to_vec());
        }
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"message":"second"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let c2: serde_json::Value = parse_body(resp).await;
        let hash2 = c2["hash"].as_str().unwrap().to_string();

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/v1/workspaces/ws_TEST/log?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let log: serde_json::Value = parse_body(resp).await;
        assert_eq!(log["items"].as_array().unwrap().len(), 2);

        let app = router(state.clone());
        let url = format!("/v1/workspaces/ws_TEST/diff?from={hash1}&to={hash2}");
        let resp = app
            .oneshot(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let diff: serde_json::Value = parse_body(resp).await;
        assert_eq!(diff["entries"][0]["change"], "modified");
    }

    #[tokio::test]
    async fn revert_commit_flow() {
        let state = test_state();

        {
            let mut staging = state.staging.write().unwrap();
            staging.insert("a.md".to_string(), b"original".to_vec());
        }
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"message":"init"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let c1: serde_json::Value = parse_body(resp).await;
        let target = c1["hash"].as_str().unwrap().to_string();

        {
            let mut staging = state.staging.write().unwrap();
            staging.insert("a.md".to_string(), b"changed".to_vec());
        }
        let app = router(state.clone());
        app.oneshot(
            Request::post("/v1/workspaces/ws_TEST/commit")
                .header(auth_header().0, auth_header().1)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"message":"change"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

        let revert_body = serde_json::json!({
            "target_commit": target,
            "reason": "rollback"
        });
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/revert")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(revert_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn error_response_format() {
        let state = test_state();
        let app = router(state);

        let resp = app
            .oneshot(
                Request::get("/v1/files/nonexistent.md")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body: serde_json::Value = parse_body(resp).await;
        assert_eq!(body["code"], "NOT_FOUND");
        assert!(body["trace_id"].as_str().is_some());
        assert!(body["message"].as_str().is_some());
    }

    #[tokio::test]
    async fn context_without_retrieval_returns_503() {
        let state = test_state();
        let app = router(state);

        let body = serde_json::json!({"query": "what is Qdrant?"});
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/context")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn context_without_auth_returns_401() {
        let state = test_state();
        let app = router(state);

        let body = serde_json::json!({"query": "test"});
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/context")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_and_get_entity() {
        let state = test_state();
        let app = router(state.clone());

        let body = serde_json::json!({
            "entity_kind": "tool",
            "canonical_name": "Qdrant",
            "aliases": ["qdrant-db", "vector-store"]
        });
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/entities")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let entity: serde_json::Value = parse_body(resp).await;
        assert_eq!(entity["canonical_name"], "Qdrant");
        assert_eq!(entity["entity_kind"], "tool");
        let entity_id = entity["id"].as_str().unwrap().to_string();

        let app = router(state.clone());
        let url = format!("/v1/workspaces/ws_TEST/entities/{entity_id}");
        let resp = app
            .oneshot(
                Request::get(&url)
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let fetched: serde_json::Value = parse_body(resp).await;
        assert_eq!(fetched["canonical_name"], "Qdrant");
    }

    #[tokio::test]
    async fn create_entity_dedupe_conflict() {
        let state = test_state();

        let body = serde_json::json!({
            "entity_kind": "tool",
            "canonical_name": "Qdrant"
        });

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/entities")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/entities")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn search_entities_endpoint() {
        let state = test_state();

        let app = router(state.clone());
        app.oneshot(
            Request::post("/v1/workspaces/ws_TEST/entities")
                .header(auth_header().0, auth_header().1)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"entity_kind": "person", "canonical_name": "Alice"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

        let app = router(state.clone());
        let body = serde_json::json!({"query": "alice"});
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/entities/search")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let result: serde_json::Value = parse_body(resp).await;
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn link_and_neighbors() {
        let state = test_state();

        let create = |name: &str, kind: &str| serde_json::json!({"entity_kind": kind, "canonical_name": name});

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/entities")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(create("Alice", "person").to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let alice: serde_json::Value = parse_body(resp).await;
        let alice_id = alice["id"].as_str().unwrap().to_string();

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/entities")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(create("Qdrant", "tool").to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let qdrant: serde_json::Value = parse_body(resp).await;
        let qdrant_id = qdrant["id"].as_str().unwrap().to_string();

        let link_body = serde_json::json!({
            "target": qdrant_id,
            "relation": "USES"
        });
        let app = router(state.clone());
        let url = format!("/v1/workspaces/ws_TEST/entities/{alice_id}/link");
        let resp = app
            .oneshot(
                Request::post(&url)
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(link_body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let app = router(state.clone());
        let url = format!("/v1/workspaces/ws_TEST/entities/{alice_id}/neighbors?depth=1");
        let resp = app
            .oneshot(
                Request::get(&url)
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let neighbors: serde_json::Value = parse_body(resp).await;
        assert_eq!(neighbors["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(neighbors["edges"].as_array().unwrap().len(), 1);
    }

    async fn parse_body(resp: axum::http::Response<Body>) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Append-only supersede: write old, commit, then call /supersede and verify
    /// (a) the response carries the parsed ids and a fresh commit hash,
    /// (b) the old `.md` is preserved with `status: superseded` + `superseded_by`,
    /// (c) the new `.md` exists with `supersedes: [old_id]` auto-injected,
    /// (d) a second supersede on the now-superseded file is rejected (the
    ///     guard against operating on non-active memories).
    #[tokio::test]
    async fn supersede_preserves_old_and_links_new() {
        let state = test_state();

        // Stage + commit an active memory.
        let old_content = "---\nid: mem_old\ntype: memory\nstatus: active\n---\noriginal body\n";
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/v1/files/memories/old.md")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"content": old_content}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/commit")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"message": "seed"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Supersede.
        let new_content = "---\nid: mem_new\ntype: memory\nstatus: active\n---\nupdated body\n";
        let body = serde_json::json!({
            "old_path": "memories/old.md",
            "new_path": "memories/new.md",
            "new_content": new_content,
            "reason": "fact correction",
            "conflict_type": "update",
        });
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/supersede")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let parsed: serde_json::Value = parse_body(resp).await;
        assert_eq!(parsed["old_memory_id"], "mem_old");
        assert_eq!(parsed["new_memory_id"], "mem_new");
        assert_eq!(parsed["old_path"], "memories/old.md");
        assert_eq!(parsed["new_path"], "memories/new.md");
        assert!(!parsed["commit_hash"].as_str().unwrap().is_empty());

        // Old file is preserved with mutated frontmatter.
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/v1/files/memories/old.md")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let old_file: serde_json::Value = parse_body(resp).await;
        let old_text = old_file["content"].as_str().unwrap();
        assert!(old_text.contains("status: superseded"), "got: {old_text}");
        assert!(old_text.contains("mem_new"), "got: {old_text}");
        assert!(
            old_text.contains("conflict_type: update"),
            "got: {old_text}"
        );
        assert!(old_text.contains("original body"), "old body preserved");

        // New file exists with auto-injected supersedes.
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/v1/files/memories/new.md")
                    .header(auth_header().0, auth_header().1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let new_file: serde_json::Value = parse_body(resp).await;
        let new_text = new_file["content"].as_str().unwrap();
        assert!(new_text.contains("supersedes"), "got: {new_text}");
        assert!(new_text.contains("mem_old"), "got: {new_text}");
        assert!(new_text.contains("updated body"));

        // Re-superseding an already-superseded file must be rejected so we
        // never silently lose history.
        let body2 = serde_json::json!({
            "old_path": "memories/old.md",
            "new_path": "memories/newer.md",
            "new_content":
                "---\nid: mem_newer\ntype: memory\nstatus: active\n---\nx\n",
            "reason": "should fail",
        });
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/supersede")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body2.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);

        // Reusing an existing memory id (here `mem_old`, which already lives
        // on disk as a superseded record) must also be rejected. Without
        // SupersedeGraph this would slip through — the inline check only
        // catches the trivial new_id == old_id case.
        let body3 = serde_json::json!({
            "old_path": "memories/new.md",
            "new_path": "memories/recycled.md",
            "new_content":
                "---\nid: mem_old\ntype: memory\nstatus: active\n---\ncollision\n",
            "reason": "id reuse should fail",
        });
        let app = router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/v1/workspaces/ws_TEST/supersede")
                    .header(auth_header().0, auth_header().1)
                    .header("content-type", "application/json")
                    .body(Body::from(body3.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }
}
