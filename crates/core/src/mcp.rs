//! MCP (Model Context Protocol) server — JSON-RPC 2.0 over stdio/SSE.
//!
//! Implements the full tool manifest from `specs/mcp.tools.json`:
//! 17 tools covering file CRUD, search, recall, memory management,
//! entity graph, commit history, and agent run lifecycle.
//!
//! See `04-tasks-dod.md` Phase 6 (tasks 6.1–6.3).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::acl;
use crate::commit::CommitGraph;
use crate::error::{MemoryFsError, Result};
use crate::graph::{EntityGraph, Relation};
use crate::policy::Policy;
use crate::runs::{FinishRunParams, RunStatus, RunStore, StartRunParams, Trigger, TriggerKind};
use crate::storage::{InodeIndex, ObjectStore};

/// JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// JSON-RPC version (must be "2.0").
    pub jsonrpc: String,
    /// Request ID.
    pub id: serde_json::Value,
    /// Method name.
    pub method: String,
    /// Parameters.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Request ID (echoed back).
    pub id: serde_json::Value,
    /// Result (present on success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error (present on failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: serde_json::Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }

    fn method_not_found(id: serde_json::Value, method: &str) -> Self {
        Self::error(id, -32601, format!("method not found: {method}"))
    }

    fn invalid_params(id: serde_json::Value, msg: String) -> Self {
        Self::error(id, -32602, msg)
    }

    fn internal_error(id: serde_json::Value, msg: String) -> Self {
        Self::error(id, -32603, msg)
    }
}

/// Tool metadata for the MCP manifest.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    /// Tool name (e.g. `memoryfs_read_file`).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Input JSON Schema.
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// Auth context extracted from environment or headers.
#[derive(Debug, Clone)]
pub struct McpAuth {
    /// Subject identity (e.g. `agent:extractor`, `user:alice`).
    pub subject: String,
    /// Token type prefix (`utk_` for user, `atk_` for agent).
    pub token_type: String,
    /// Workspace ID.
    pub workspace_id: String,
}

impl McpAuth {
    /// Parse auth from environment variables (stdio transport).
    pub fn from_env() -> Result<Self> {
        let token = std::env::var("MEMORYFS_TOKEN").map_err(|_| MemoryFsError::Unauthorized)?;
        let workspace_id =
            std::env::var("MEMORYFS_WORKSPACE_ID").map_err(|_| MemoryFsError::Unauthorized)?;

        let (token_type, subject) = parse_token(&token)?;

        Ok(Self {
            subject,
            token_type,
            workspace_id,
        })
    }

    /// Create auth for testing.
    pub fn test(subject: &str, workspace_id: &str) -> Self {
        Self {
            subject: subject.into(),
            token_type: "utk_".into(),
            workspace_id: workspace_id.into(),
        }
    }
}

fn parse_token(token: &str) -> Result<(String, String)> {
    if let Some(rest) = token.strip_prefix("utk_") {
        Ok(("utk_".into(), format!("user:{rest}")))
    } else if let Some(rest) = token.strip_prefix("atk_") {
        Ok(("atk_".into(), format!("agent:{rest}")))
    } else {
        Err(MemoryFsError::Unauthorized)
    }
}

/// Shared state for the MCP server.
pub struct McpState {
    /// Object store for content-addressable storage.
    pub object_store: Arc<ObjectStore>,
    /// Inode index mapping paths to hashes.
    pub index: Arc<std::sync::RwLock<InodeIndex>>,
    /// Commit log.
    pub commit_log: Arc<std::sync::RwLock<CommitGraph>>,
    /// Entity graph.
    pub entity_graph: Arc<std::sync::RwLock<EntityGraph>>,
    /// Agent run store.
    pub run_store: Arc<std::sync::RwLock<RunStore>>,
    /// Workspace policy.
    pub policy: Policy,
    /// Auth context.
    pub auth: McpAuth,
    /// Retrieval engine for semantic recall. `None` falls back to substring search.
    pub retrieval: Option<Arc<dyn crate::api::ContextRetriever>>,
    /// Background indexer wired in when a vector backend is configured.
    /// Each successful commit fires a fire-and-forget index batch so Levara
    /// stays in sync without the agent having to wait on it.
    pub indexer: Option<Arc<dyn crate::indexer::IndexBatch>>,
}

/// MCP server that dispatches JSON-RPC calls to tool handlers.
pub struct McpServer {
    state: Arc<McpState>,
    tools: Vec<ToolDef>,
}

impl McpServer {
    /// Create a new MCP server with the given state.
    pub fn new(state: McpState) -> Self {
        let tools = build_tool_manifest();
        Self {
            state: Arc::new(state),
            tools,
        }
    }

    /// Run the stdio transport loop (reads JSON-RPC from stdin, writes to stdout).
    pub async fn run_stdio(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let mut stdout = tokio::io::stdout();
        let reader = BufReader::new(stdin);
        let mut lines = reader.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }

            let response = self.handle_message(&line).await;
            let json = serde_json::to_string(&response).unwrap_or_else(|e| {
                serde_json::to_string(&JsonRpcResponse::internal_error(
                    serde_json::Value::Null,
                    format!("serialization error: {e}"),
                ))
                .unwrap()
            });

            let _ = stdout.write_all(json.as_bytes()).await;
            let _ = stdout.write_all(b"\n").await;
            let _ = stdout.flush().await;
        }

        Ok(())
    }

    /// Handle a single JSON-RPC message.
    pub async fn handle_message(&self, msg: &str) -> JsonRpcResponse {
        let req: JsonRpcRequest = match serde_json::from_str(msg) {
            Ok(r) => r,
            Err(e) => {
                return JsonRpcResponse::error(
                    serde_json::Value::Null,
                    -32700,
                    format!("parse error: {e}"),
                );
            }
        };

        self.dispatch(req).await
    }

    /// Dispatch a parsed request to the appropriate handler.
    async fn dispatch(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        match req.method.as_str() {
            "initialize" => self.handle_initialize(req.id),
            "tools/list" => self.handle_tools_list(req.id),
            "tools/call" => self.handle_tools_call(req.id, req.params).await,
            _ => JsonRpcResponse::method_not_found(req.id, &req.method),
        }
    }

    fn handle_initialize(&self, id: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "memoryfs",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": Self::INSTRUCTIONS,
            }),
        )
    }

    /// Behavioral contract surfaced via MCP `initialize.instructions`. Clients
    /// inject this into the model's context, so it ships with the server
    /// instead of relying on per-machine CLAUDE.md edits. Keep it tight —
    /// every conversation pays for these tokens.
    const INSTRUCTIONS: &'static str = include_str!("mcp_instructions.md");

    fn handle_tools_list(&self, id: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse::success(id, serde_json::json!({ "tools": self.tools }))
    }

    async fn handle_tools_call(
        &self,
        id: serde_json::Value,
        params: serde_json::Value,
    ) -> JsonRpcResponse {
        let tool_name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => return JsonRpcResponse::invalid_params(id, "missing 'name'".into()),
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        let result = self.call_tool(&tool_name, args).await;

        match result {
            Ok(content) => JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "content": [{ "type": "text", "text": content.to_string() }],
                    "isError": false
                }),
            ),
            Err(e) => {
                let (code, msg) = error_to_rpc(&e);
                JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{ "type": "text", "text": msg }],
                        "isError": true,
                        "_errorCode": code
                    }),
                )
            }
        }
    }

    async fn call_tool(&self, name: &str, args: serde_json::Value) -> Result<serde_json::Value> {
        match name {
            "memoryfs_read_file" => self.tool_read_file(args),
            "memoryfs_write_file" => self.tool_write_file(args),
            "memoryfs_list_files" => self.tool_list_files(args),
            "memoryfs_commit" => self.tool_commit(args),
            "memoryfs_revert" => self.tool_revert(args),
            "memoryfs_log" => self.tool_log(args),
            "memoryfs_diff" => self.tool_diff(args),
            "memoryfs_search" => self.tool_search(args),
            "memoryfs_recall" => self.tool_recall(args),
            "memoryfs_remember" => self.tool_remember(args),
            "memoryfs_propose_memory_patch" => self.tool_propose(args),
            "memoryfs_review_memory" => self.tool_review(args),
            "memoryfs_supersede_memory" => self.tool_supersede(args),
            "memoryfs_link_entity" => self.tool_link_entity(args),
            "memoryfs_get_provenance" => self.tool_get_provenance(args),
            "memoryfs_create_run" => self.tool_create_run(args),
            "memoryfs_finish_run" => self.tool_finish_run(args),
            _ => Err(MemoryFsError::Validation(format!("unknown tool: {name}"))),
        }
    }

    // ── Tool implementations ─────────────────────────────────────────────

    fn tool_read_file(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let path = require_str(&args, "path")?;
        acl::check(
            &self.state.auth.subject,
            acl::Action::Read,
            &path,
            &self.state.policy,
        )?;

        let index = self.state.index.read().unwrap();
        let hash = index
            .get(&path)
            .ok_or_else(|| MemoryFsError::NotFound(format!("file {path}")))?;
        let data = self.state.object_store.get(hash)?;
        let content = String::from_utf8_lossy(&data);

        Ok(serde_json::json!({
            "path": path,
            "content": content,
        }))
    }

    fn tool_write_file(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let path = require_str(&args, "path")?;
        let content = require_str(&args, "content")?;
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            &path,
            &self.state.policy,
        )?;

        if !force && is_append_only_path(&path) {
            let index = self.state.index.read().unwrap();
            if let Some(prior_hash) = index.get(&path) {
                let prior_bytes = self.state.object_store.get(prior_hash)?;
                if prior_bytes.as_slice() != content.as_bytes() {
                    return Err(crate::MemoryFsError::Validation(format!(
                        "{path} already exists and is append-only. \
                         Decisions and discoveries preserve their audit trail — \
                         use memoryfs_supersede_memory to record the new version. \
                         For a typo-only fix, retry with force=true."
                    )));
                }
            }
        }

        let hash = self.state.object_store.put(content.as_bytes())?;
        let mut index = self.state.index.write().unwrap();
        index.set(&path, hash.clone());

        Ok(serde_json::json!({
            "path": path,
            "hash": hash.as_str(),
            "staged": true,
        }))
    }

    fn tool_list_files(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let prefix = args.get("prefix").and_then(|v| v.as_str()).unwrap_or("");
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        acl::check(
            &self.state.auth.subject,
            acl::Action::List,
            &format!("{prefix}**"),
            &self.state.policy,
        )?;

        let index = self.state.index.read().unwrap();
        let paths: Vec<&str> = index
            .paths()
            .into_iter()
            .filter(|p| p.starts_with(prefix))
            .take(limit)
            .collect();

        Ok(serde_json::json!({
            "files": paths,
            "count": paths.len(),
        }))
    }

    fn tool_commit(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let message = require_str(&args, "message")?;
        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            "**",
            &self.state.policy,
        )?;

        let index = self.state.index.read().unwrap();
        let snapshot = index
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
            .collect();
        let mut log = self.state.commit_log.write().unwrap();
        let parent = args
            .get("parent_commit")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| log.head().map(|c| c.hash.clone()));
        let commit = log.commit(
            &self.state.auth.subject,
            &message,
            snapshot,
            parent.as_deref(),
        )?;

        // Fire-and-forget indexing: when a vector backend is configured, push
        // the post-commit snapshot through it so semantic recall sees the new
        // state immediately. Errors only land in tracing; we never block or
        // fail the commit on indexer hiccups.
        if let Some(indexer) = &self.state.indexer {
            let snapshot: Vec<(String, crate::storage::ObjectHash)> = self
                .state
                .index
                .read()
                .unwrap()
                .iter()
                .map(|(p, h)| (p.to_string(), h.clone()))
                .collect();
            let workspace_id = self.state.auth.workspace_id.clone();
            let commit_hash = commit.hash.to_string();
            let store = self.state.object_store.clone();
            let indexer = indexer.clone();

            let changes: Vec<crate::indexer::FileChange> = snapshot
                .iter()
                .map(|(path, hash)| {
                    let content = store
                        .get(hash)
                        .ok()
                        .map(|data| String::from_utf8_lossy(&data).to_string());
                    crate::indexer::FileChange {
                        memory_id: crate::ids::MemoryId::from_path(path),
                        file_path: path.clone(),
                        workspace_id: workspace_id.clone(),
                        content,
                        commit: commit_hash.clone(),
                    }
                })
                .collect();

            tokio::spawn(async move {
                match indexer.run(changes).await {
                    Ok(chunks) => tracing::info!(
                        "mcp indexer: indexed commit {commit_hash} ({chunks} chunks)"
                    ),
                    Err(e) => tracing::error!("mcp indexer error: {e}"),
                }
            });
        }

        Ok(serde_json::json!({
            "commit_hash": commit.hash.as_str(),
            "message": commit.message,
            "author": commit.author,
        }))
    }

    fn tool_revert(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let target = require_str(&args, "target_commit")?;
        let _reason = require_str(&args, "reason")?;
        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            "**",
            &self.state.policy,
        )?;

        let mut log = self.state.commit_log.write().unwrap();
        let revert_commit = log.revert(&target, &self.state.auth.subject)?;

        Ok(serde_json::json!({
            "commit_hash": revert_commit.hash.as_str(),
            "reverted_to": target,
        }))
    }

    fn tool_log(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

        let log = self.state.commit_log.read().unwrap();
        let commits = log.log(Some(limit));

        let entries: Vec<serde_json::Value> = commits
            .iter()
            .map(|c| {
                serde_json::json!({
                    "hash": c.hash.as_str(),
                    "message": c.message,
                    "author": c.author,
                    "timestamp": c.timestamp,
                })
            })
            .collect();

        Ok(serde_json::json!({ "commits": entries }))
    }

    fn tool_diff(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let from = require_str(&args, "from")?;
        let to = require_str(&args, "to")?;

        let log = self.state.commit_log.read().unwrap();
        let diff = log.diff(Some(&from), &to)?;

        let entries: Vec<serde_json::Value> = diff
            .iter()
            .map(|d| match d {
                crate::commit::DiffEntry::Added { path, hash } => {
                    serde_json::json!({"type": "added", "path": path, "hash": hash})
                }
                crate::commit::DiffEntry::Removed { path, hash } => {
                    serde_json::json!({"type": "removed", "path": path, "hash": hash})
                }
                crate::commit::DiffEntry::Modified {
                    path,
                    old_hash,
                    new_hash,
                } => {
                    serde_json::json!({"type": "modified", "path": path, "old_hash": old_hash, "new_hash": new_hash})
                }
            })
            .collect();

        Ok(serde_json::json!({ "diff": entries }))
    }

    fn tool_search(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let query = require_str(&args, "query")?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let index = self.state.index.read().unwrap();
        let q = query.to_lowercase();
        let results: Vec<&str> = index
            .paths()
            .into_iter()
            .filter(|p| p.to_lowercase().contains(&q))
            .take(limit)
            .collect();

        Ok(serde_json::json!({
            "query": query,
            "results": results,
            "count": results.len(),
        }))
    }

    fn tool_recall(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let query_text = require_str(&args, "query")?;
        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8) as usize;

        acl::check(
            &self.state.auth.subject,
            acl::Action::Read,
            "**",
            &self.state.policy,
        )?;

        if let Some(retrieval) = &self.state.retrieval {
            let query = crate::retrieval::RetrievalQuery {
                query: query_text.to_string(),
                top_k: limit,
                workspace_id: self.state.auth.workspace_id.clone(),
                scope: None,
                tags: None,
                recency_half_life_days: None,
                hybrid_weights: None,
                include_superseded: args
                    .get("include_superseded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            };
            let retrieval = retrieval.clone();
            let subject = self.state.auth.subject.clone();
            let policy = self.state.policy.clone();

            let resp = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(retrieval.retrieve(&query, &subject, &policy))
            })?;

            let memories: Vec<serde_json::Value> = resp
                .results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "path": r.file_path,
                        "snippet": r.snippet,
                        "score": r.score,
                        "source_scores": r.source_scores,
                    })
                })
                .collect();

            return Ok(serde_json::json!({
                "query": query_text,
                "memories": memories,
                "count": memories.len(),
                "mode": "semantic",
            }));
        }

        let index = self.state.index.read().unwrap();
        let q = query_text.to_lowercase();
        let mut memories = Vec::new();

        for path in index
            .paths()
            .into_iter()
            .filter(|p| p.starts_with("memories/"))
        {
            if let Some(hash) = index.get(path) {
                if let Ok(data) = self.state.object_store.get(hash) {
                    let content = String::from_utf8_lossy(&data);
                    if content.to_lowercase().contains(&q) {
                        memories.push(serde_json::json!({
                            "path": path,
                            "snippet": truncate(&content, 500),
                        }));
                        if memories.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }

        Ok(serde_json::json!({
            "query": query_text,
            "memories": memories,
            "count": memories.len(),
            "mode": "substring",
        }))
    }

    fn tool_remember(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let memory_type = require_str(&args, "memory_type")?;
        let scope = require_str(&args, "scope")?;
        let scope_id = require_str(&args, "scope_id")?;
        let content = require_str(&args, "content")?;
        let confidence: f64 = args
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.9);

        let path = format!(
            "memories/{}/{}_{}.md",
            scope,
            memory_type,
            crate::MemoryId::new()
        );

        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            &path,
            &self.state.policy,
        )?;

        let frontmatter = format!(
            "---\nmemory_type: {memory_type}\nscope: {scope}\nscope_id: {scope_id}\nconfidence: {confidence}\nstatus: active\n---\n\n{content}"
        );

        let hash = self.state.object_store.put(frontmatter.as_bytes())?;
        let mut index = self.state.index.write().unwrap();
        index.set(&path, hash.clone());

        Ok(serde_json::json!({
            "path": path,
            "hash": hash.as_str(),
            "memory_type": memory_type,
        }))
    }

    fn tool_propose(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let memory = args
            .get("memory")
            .ok_or_else(|| MemoryFsError::Validation("missing 'memory'".into()))?;

        let proposal_id = crate::ids::ProposalId::new().to_string();

        Ok(serde_json::json!({
            "proposal_id": proposal_id,
            "status": "pending",
            "memory": memory,
        }))
    }

    fn tool_review(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let proposal_id = require_str(&args, "proposal_id")?;
        let decision = require_str(&args, "decision")?;
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        Ok(serde_json::json!({
            "proposal_id": proposal_id,
            "decision": decision,
            "reason": reason,
        }))
    }

    fn tool_supersede(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        // The MCP tool used to return a stub — it now runs the same atomic
        // append-only swap as `POST /v1/workspaces/.../supersede`: read the
        // old file, mark it `status: superseded` with a back-reference to the
        // new memory, write the new memory with `supersedes: [old_id]`, and
        // commit both in one atomic step. Markdown remains the source of truth
        // and nothing is destructively deleted.
        let old_path = require_str(&args, "old_path")?;
        let new_path = require_str(&args, "new_path")?;
        let new_content = require_str(&args, "new_content")?;
        let reason = require_str(&args, "reason")?;
        let conflict_type = args
            .get("conflict_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if old_path == new_path {
            return Err(MemoryFsError::Validation(
                "old_path and new_path must differ".into(),
            ));
        }

        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            &old_path,
            &self.state.policy,
        )?;
        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            &new_path,
            &self.state.policy,
        )?;
        acl::check(
            &self.state.auth.subject,
            acl::Action::Write,
            "**",
            &self.state.policy,
        )?;

        let old_text = {
            let index = self.state.index.read().unwrap();
            let hash = index
                .get(&old_path)
                .ok_or_else(|| MemoryFsError::NotFound(format!("file {old_path}")))?;
            let bytes = self.state.object_store.get(hash)?;
            String::from_utf8(bytes).map_err(|e| {
                MemoryFsError::Validation(format!("old file is not valid UTF-8: {e}"))
            })?
        };

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
            return Err(MemoryFsError::Conflict(format!(
                "memory {old_id} has status {old_status}, cannot be superseded"
            )));
        }

        let mut new_doc = crate::schema::parse_frontmatter(&new_content)?;
        let new_id = new_doc
            .frontmatter
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| MemoryFsError::Validation("new frontmatter missing id".into()))?
            .to_string();
        if new_id == old_id {
            return Err(MemoryFsError::Validation(
                "new memory id must differ from old memory id".into(),
            ));
        }

        // Cycle / duplicate-id guard. Same idea as the REST handler: scan
        // the current workspace, build an in-memory supersede graph, and
        // ask it to validate. Stays consistent because the index is locked
        // for the duration of validation and commit.
        {
            let index = self.state.index.read().unwrap();
            let graph = crate::supersede::SupersedeGraph::build_from_workspace(
                &index,
                &self.state.object_store,
                "memories/",
            )?;
            graph.validate_supersede(&new_id, &[old_id.as_str()])?;
        }

        let now = chrono::Utc::now().to_rfc3339();
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
            if let Some(ct) = &conflict_type {
                map.insert(
                    "conflict_type".into(),
                    serde_json::Value::String(ct.clone()),
                );
            }
        } else {
            return Err(MemoryFsError::Validation(
                "old frontmatter is not an object".into(),
            ));
        }

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

        let old_hash = self.state.object_store.put(old_rendered.as_bytes())?;
        let new_hash = self.state.object_store.put(new_rendered.as_bytes())?;

        let snapshot = {
            let mut index = self.state.index.write().unwrap();
            index.set(&old_path, old_hash);
            index.set(&new_path, new_hash);
            index
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_str().to_string()))
                .collect()
        };

        let mut log = self.state.commit_log.write().unwrap();
        let parent = log.head().map(|c| c.hash.clone());
        let message = format!("supersede {old_id} -> {new_id}: {reason}");
        let commit = log.commit(
            &self.state.auth.subject,
            &message,
            snapshot,
            parent.as_deref(),
        )?;

        Ok(serde_json::json!({
            "commit_hash": commit.hash.as_str(),
            "old_path": old_path,
            "new_path": new_path,
            "old_memory_id": old_id,
            "new_memory_id": new_id,
            "reason": reason,
        }))
    }

    fn tool_link_entity(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let src = require_str(&args, "src")?;
        let dst = require_str(&args, "dst")?;
        let relation_str = require_str(&args, "relation")?;
        let weight: f32 = args.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;

        let relation = Relation::parse(&relation_str)?;

        let mut graph = self.state.entity_graph.write().unwrap();
        let edge = graph.link(&src, &dst, relation, weight, None)?;

        Ok(serde_json::json!({
            "src": edge.src,
            "dst": edge.dst,
            "relation": relation_str,
            "weight": edge.weight,
        }))
    }

    fn tool_get_provenance(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let memory_id = require_str(&args, "memory_id")?;

        Ok(serde_json::json!({
            "memory_id": memory_id,
            "provenance": {
                "note": "full provenance tracking requires integration with extraction pipeline"
            }
        }))
    }

    fn tool_create_run(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let agent = require_str(&args, "agent")?;

        let trigger_val = args
            .get("trigger")
            .cloned()
            .unwrap_or(serde_json::json!({"kind": "test"}));
        let kind_str = trigger_val["kind"].as_str().unwrap_or("test");
        let kind = match kind_str {
            "user_request" => TriggerKind::UserRequest,
            "scheduled" => TriggerKind::Scheduled,
            "agent_call" => TriggerKind::AgentCall,
            "webhook" => TriggerKind::Webhook,
            _ => TriggerKind::Test,
        };

        let trigger = Trigger {
            kind,
            by: trigger_val["by"].as_str().map(String::from),
            trigger_ref: trigger_val["ref"].as_str().map(String::from),
        };

        let params = StartRunParams {
            agent,
            trigger,
            author: self.state.auth.subject.clone(),
            session_id: args
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(String::from),
            model: args.get("model").and_then(|v| v.as_str()).map(String::from),
            tags: args
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        };

        let mut store = self.state.run_store.write().unwrap();
        let run = store.start(params);

        Ok(serde_json::json!({
            "run_id": run.id,
            "agent": run.agent,
            "status": "running",
            "started_at": run.started_at,
        }))
    }

    fn tool_finish_run(&self, args: serde_json::Value) -> Result<serde_json::Value> {
        let run_id = require_str(&args, "run_id")?;
        let status_str = require_str(&args, "status")?;

        let status = match status_str.as_str() {
            "succeeded" => RunStatus::Succeeded,
            "failed" => RunStatus::Failed,
            "cancelled" => RunStatus::Cancelled,
            "timeout" => RunStatus::Timeout,
            other => {
                return Err(MemoryFsError::Validation(format!(
                    "invalid status: {other}"
                )))
            }
        };

        let params = FinishRunParams {
            status,
            finished_at: args
                .get("finished_at")
                .and_then(|v| v.as_str())
                .map(String::from),
            artifacts: None,
            metrics: None,
            error: args.get("error").and_then(|v| v.as_str()).map(String::from),
            proposed_memories: args
                .get("proposed_memories")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        };

        let mut store = self.state.run_store.write().unwrap();
        let run = store.finish(&run_id, params)?;

        Ok(serde_json::json!({
            "run_id": run.id,
            "status": run.status,
            "finished_at": run.finished_at,
            "finished": true,
        }))
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Paths whose audit trail must survive — overwriting them via plain
/// `write_file` would erase the prior version, defeating the supersede DAG
/// the rest of the system relies on. The model is told to use
/// `memoryfs_supersede_memory` here; the server enforces it.
fn is_append_only_path(path: &str) -> bool {
    path.starts_with("decisions/") || path.starts_with("discoveries/")
}

fn require_str(args: &serde_json::Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| MemoryFsError::Validation(format!("missing required field: {key}")))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn error_to_rpc(e: &MemoryFsError) -> (String, String) {
    match e {
        MemoryFsError::Validation(_) => ("VALIDATION".into(), e.to_string()),
        MemoryFsError::NotFound(_) => ("NOT_FOUND".into(), e.to_string()),
        MemoryFsError::Unauthorized => ("UNAUTHORIZED".into(), e.to_string()),
        MemoryFsError::Forbidden(_) => ("FORBIDDEN".into(), e.to_string()),
        MemoryFsError::Conflict(_) => ("CONFLICT".into(), e.to_string()),
        _ => ("INTERNAL".into(), e.to_string()),
    }
}

fn build_tool_manifest() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "memoryfs_read_file".into(),
            description: "Read a file from the workspace at HEAD.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {
                    "path": { "type": "string" },
                    "at_commit": { "type": "string" },
                    "include_body": { "type": "boolean", "default": true }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_write_file".into(),
            description: "Write a file to staging. Files under decisions/ and \
                          discoveries/ are append-only — overwriting an existing \
                          path is rejected; use memoryfs_supersede_memory \
                          instead, or pass force=true for a typo-only fix."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path", "content"],
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" },
                    "force": {
                        "type": "boolean",
                        "description": "Bypass the append-only guard on decisions/ and discoveries/. Use only for non-semantic edits (typos, formatting)."
                    }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_list_files".into(),
            description: "List files by prefix.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "prefix": { "type": "string" },
                    "limit": { "type": "integer", "default": 50 }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_search".into(),
            description: "Hybrid search across workspace files.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 10 }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_recall".into(),
            description: "Context assembly for agents: search → ACL → read → cite.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 8 },
                    "cite": { "type": "boolean", "default": true }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_remember".into(),
            description: "Create a memory directly (bypassing extraction pipeline).".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["memory_type", "scope", "scope_id", "content", "confidence", "provenance"],
                "properties": {
                    "memory_type": { "type": "string" },
                    "scope": { "type": "string" },
                    "scope_id": { "type": "string" },
                    "content": { "type": "string" },
                    "confidence": { "type": "number" },
                    "provenance": { "type": "object" }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_propose_memory_patch".into(),
            description: "Propose a new or modified memory for review.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["memory"],
                "properties": {
                    "memory": { "type": "object" },
                    "run_id": { "type": "string" },
                    "supersede_target": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_review_memory".into(),
            description: "Approve or reject a pending proposal.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["proposal_id", "decision"],
                "properties": {
                    "proposal_id": { "type": "string" },
                    "decision": { "type": "string", "enum": ["approved", "rejected", "needs_changes"] },
                    "reason": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_supersede_memory".into(),
            description: "Atomically replace one memory with another. The old .md file is preserved with status=superseded; the new memory commits with supersedes=[old_id]. No data is destructively deleted.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["old_path", "new_path", "new_content", "reason"],
                "properties": {
                    "old_path": { "type": "string", "description": "Path of the existing memory file to mark superseded." },
                    "new_path": { "type": "string", "description": "Path where the new memory file will be written." },
                    "new_content": { "type": "string", "description": "Full markdown of the new memory including frontmatter with id." },
                    "reason": { "type": "string", "description": "Why this supersede happened (recorded in commit message)." },
                    "conflict_type": { "type": "string", "enum": ["update", "contradiction", "merge", "none"], "description": "Optional. Recorded on the old memory's frontmatter." }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_commit".into(),
            description: "Publish staged changes as an atomic commit.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message"],
                "properties": {
                    "message": { "type": "string" },
                    "parent_commit": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_revert".into(),
            description: "Forward-only rollback to a target commit.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["target_commit", "reason"],
                "properties": {
                    "target_commit": { "type": "string" },
                    "reason": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_create_run".into(),
            description: "Register the start of an agent run.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["agent", "trigger"],
                "properties": {
                    "agent": { "type": "string" },
                    "trigger": { "type": "object" },
                    "session_id": { "type": "string" }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_finish_run".into(),
            description: "Close a run with status, artifacts, and metrics.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["run_id", "status"],
                "properties": {
                    "run_id": { "type": "string" },
                    "status": { "type": "string", "enum": ["succeeded", "failed", "cancelled", "timeout"] }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_link_entity".into(),
            description: "Create an edge in the entity graph.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["src", "dst", "relation"],
                "properties": {
                    "src": { "type": "string" },
                    "dst": { "type": "string" },
                    "relation": { "type": "string" },
                    "weight": { "type": "number", "default": 1 }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_get_provenance".into(),
            description: "Get full provenance for a memory.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["memory_id"],
                "properties": {
                    "memory_id": { "type": "string" },
                    "include_chain": { "type": "boolean", "default": true }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_log".into(),
            description: "Commit history with filters.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "author": { "type": "string" },
                    "limit": { "type": "integer", "default": 50 }
                }
            }),
        },
        ToolDef {
            name: "memoryfs_diff".into(),
            description: "Diff between two commits.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["from", "to"],
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "format": { "type": "string", "enum": ["json", "unified"], "default": "json" }
                }
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> McpState {
        let dir = tempfile::tempdir().unwrap();
        let obj = ObjectStore::open(dir.path().join("objects")).unwrap();
        let index = InodeIndex::new();
        let commit_log = CommitGraph::new();
        let graph = EntityGraph::new();
        let mut policy = Policy::default();
        policy.default_acl.allow.push(crate::policy::AclRule {
            path: "**".into(),
            subjects: vec!["user:*".into()],
            actions: vec![
                "read".into(),
                "write".into(),
                "list".into(),
                "commit".into(),
                "revert".into(),
                "review".into(),
            ],
        });
        let auth = McpAuth::test("user:alice", "ws_test");

        McpState {
            object_store: Arc::new(obj),
            index: Arc::new(std::sync::RwLock::new(index)),
            commit_log: Arc::new(std::sync::RwLock::new(commit_log)),
            entity_graph: Arc::new(std::sync::RwLock::new(graph)),
            run_store: Arc::new(std::sync::RwLock::new(RunStore::new())),
            policy,
            auth,
            retrieval: None,
            indexer: None,
        }
    }

    #[test]
    fn tool_manifest_has_17_tools() {
        let tools = build_tool_manifest();
        assert_eq!(tools.len(), 17);
    }

    #[test]
    fn tool_names_all_prefixed() {
        let tools = build_tool_manifest();
        for tool in &tools {
            assert!(
                tool.name.starts_with("memoryfs_"),
                "tool {} missing prefix",
                tool.name
            );
        }
    }

    #[test]
    fn tool_manifest_serializes() {
        let tools = build_tool_manifest();
        let json = serde_json::to_value(&tools).unwrap();
        assert!(json.is_array());
        assert_eq!(json.as_array().unwrap().len(), 17);
    }

    #[tokio::test]
    async fn initialize_returns_capabilities() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await;
        assert!(resp.result.is_some());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[tokio::test]
    async fn initialize_ships_behavioral_instructions() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .await;
        let result = resp.result.unwrap();
        let instructions = result["instructions"]
            .as_str()
            .expect("initialize must include behavioral instructions");
        // Spot-check the load-bearing rules are present — recall-first and
        // the supersede guard. If someone deletes them from the markdown,
        // this test surfaces it.
        assert!(instructions.contains("Recall-first"));
        assert!(instructions.contains("memoryfs_supersede_memory"));
        assert!(instructions.contains("decisions/"));
    }

    #[tokio::test]
    async fn write_file_rejects_overwrite_of_existing_decision() {
        let server = McpServer::new(test_state());
        // Initial write succeeds.
        let first = server
            .tool_write_file(serde_json::json!({
                "path": "decisions/db-choice.md",
                "content": "---\ntype: decision\n---\nWe chose Postgres.",
            }))
            .expect("initial write must succeed");
        assert_eq!(first["staged"], true);

        // Overwriting the same path with different content must be rejected.
        let err = server
            .tool_write_file(serde_json::json!({
                "path": "decisions/db-choice.md",
                "content": "---\ntype: decision\n---\nActually MySQL.",
            }))
            .expect_err("overwrite of an existing decision must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("append-only"), "got: {msg}");
        assert!(msg.contains("memoryfs_supersede_memory"), "got: {msg}");
    }

    #[tokio::test]
    async fn write_file_allows_idempotent_rewrite_of_decision() {
        let server = McpServer::new(test_state());
        let body = serde_json::json!({
            "path": "decisions/db-choice.md",
            "content": "---\ntype: decision\n---\nPostgres.",
        });
        server.tool_write_file(body.clone()).unwrap();
        // Same content twice: not an audit-trail violation, the inode just
        // points at the same hash. Must succeed silently.
        server
            .tool_write_file(body)
            .expect("idempotent rewrite must succeed");
    }

    #[tokio::test]
    async fn write_file_force_bypasses_append_only_guard() {
        let server = McpServer::new(test_state());
        server
            .tool_write_file(serde_json::json!({
                "path": "discoveries/cache-bug.md",
                "content": "v1 body",
            }))
            .unwrap();
        server
            .tool_write_file(serde_json::json!({
                "path": "discoveries/cache-bug.md",
                "content": "v1 body with typo fixed",
                "force": true,
            }))
            .expect("force=true must bypass the guard");
    }

    #[tokio::test]
    async fn write_file_guard_does_not_apply_outside_append_only_paths() {
        let server = McpServer::new(test_state());
        server
            .tool_write_file(serde_json::json!({
                "path": "infra/redis.md",
                "content": "host: localhost",
            }))
            .unwrap();
        // facts/infra/events/preferences are mutable in-place — only
        // decisions/ and discoveries/ are guarded.
        server
            .tool_write_file(serde_json::json!({
                "path": "infra/redis.md",
                "content": "host: redis-prod-1",
            }))
            .expect("non-append-only paths must permit overwrite");
    }

    #[tokio::test]
    async fn tools_list_returns_all_tools() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
            .await;
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 17);
    }

    #[tokio::test]
    async fn commit_fires_indexer_when_attached() {
        use crate::indexer::{FileChange, IndexBatch};
        use std::sync::Mutex;

        // Stub indexer captures every batch it sees.
        struct Capture(Arc<Mutex<Vec<FileChange>>>);

        #[async_trait::async_trait]
        impl IndexBatch for Capture {
            async fn run(&self, changes: Vec<FileChange>) -> crate::Result<usize> {
                self.0.lock().unwrap().extend(changes.iter().cloned());
                Ok(changes.iter().map(|_| 1).sum())
            }
        }

        let captured: Arc<Mutex<Vec<FileChange>>> = Arc::new(Mutex::new(Vec::new()));
        let mut state = test_state();
        state.indexer = Some(Arc::new(Capture(captured.clone())));
        let server = McpServer::new(state);

        // Stage a file then commit.
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"infra/db.md","content":"postgres 16"}}}"#,
            )
            .await;
        let commit_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"add db note"}}}"#,
            )
            .await;
        assert!(commit_resp.result.is_some(), "commit should succeed");

        // Indexer fires via tokio::spawn — yield until it runs.
        for _ in 0..50 {
            if !captured.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }

        let batch = captured.lock().unwrap();
        assert!(
            !batch.is_empty(),
            "indexer should have received the post-commit snapshot"
        );
        assert!(
            batch.iter().any(|c| c.file_path == "infra/db.md"),
            "expected the staged file in the batch, got: {:?}",
            batch.iter().map(|c| &c.file_path).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(r#"{"jsonrpc":"2.0","id":3,"method":"unknown","params":{}}"#)
            .await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn invalid_json_returns_parse_error() {
        let server = McpServer::new(test_state());
        let resp = server.handle_message("not json").await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32700);
    }

    #[tokio::test]
    async fn write_and_read_file() {
        let server = McpServer::new(test_state());

        let write_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"test.md","content":"hello world"}}}"#,
            )
            .await;
        assert!(write_resp.result.is_some());

        let read_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"memoryfs_read_file","arguments":{"path":"test.md"}}}"#,
            )
            .await;
        let result = read_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["content"], "hello world");
    }

    #[tokio::test]
    async fn list_files_with_prefix() {
        let server = McpServer::new(test_state());

        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"memories/pref.md","content":"test"}}}"#,
            )
            .await;
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"other/file.md","content":"test"}}}"#,
            )
            .await;

        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memoryfs_list_files","arguments":{"prefix":"memories/"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["count"], 1);
    }

    #[tokio::test]
    async fn remember_creates_memory() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_remember","arguments":{"memory_type":"fact","scope":"user","scope_id":"user:alice","content":"likes Rust","confidence":0.95,"provenance":{"source_file":"test","extracted_at":"2025-01-01T00:00:00Z"}}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["memory_type"], "fact");
        assert!(parsed["path"].as_str().unwrap().starts_with("memories/"));
    }

    #[tokio::test]
    async fn link_entity_tool() {
        let state = test_state();
        let (a, b) = {
            let mut graph = state.entity_graph.write().unwrap();
            let a = graph
                .create_entity(
                    "ws_test",
                    crate::graph::EntityKind::Person,
                    "Alice",
                    vec![],
                    serde_json::json!({}),
                    vec![],
                )
                .unwrap()
                .id
                .to_string();
            let b = graph
                .create_entity(
                    "ws_test",
                    crate::graph::EntityKind::Tool,
                    "Rust",
                    vec![],
                    serde_json::json!({}),
                    vec![],
                )
                .unwrap()
                .id
                .to_string();
            (a, b)
        };

        let server = McpServer::new(state);
        let msg = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"memoryfs_link_entity","arguments":{{"src":"{a}","dst":"{b}","relation":"USES"}}}}}}"#
        );
        let resp = server.handle_message(&msg).await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["relation"], "USES");
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_nonexistent","arguments":{}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
    }

    #[tokio::test]
    async fn create_and_finish_run() {
        let server = McpServer::new(test_state());

        let create_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_create_run","arguments":{"agent":"agent:test","trigger":{"kind":"test"}}}}"#,
            )
            .await;
        let result = create_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let run_id = parsed["run_id"].as_str().unwrap().to_string();
        assert!(run_id.starts_with("run_"));

        let finish_msg = format!(
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"memoryfs_finish_run","arguments":{{"run_id":"{run_id}","status":"succeeded"}}}}}}"#
        );
        let finish_resp = server.handle_message(&finish_msg).await;
        let result = finish_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["finished"], true);
    }

    #[test]
    fn parse_token_user() {
        let (kind, subject) = parse_token("utk_alice").unwrap();
        assert_eq!(kind, "utk_");
        assert_eq!(subject, "user:alice");
    }

    #[test]
    fn parse_token_agent() {
        let (kind, subject) = parse_token("atk_extractor").unwrap();
        assert_eq!(kind, "atk_");
        assert_eq!(subject, "agent:extractor");
    }

    #[test]
    fn parse_token_invalid() {
        assert!(parse_token("invalid_token").is_err());
    }

    #[tokio::test]
    async fn search_finds_matching_files() {
        let server = McpServer::new(test_state());
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"notes/rust-tips.md","content":"Rust borrowing rules"}}}"#,
            )
            .await;
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"notes/python.md","content":"Python GIL"}}}"#,
            )
            .await;

        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memoryfs_search","arguments":{"query":"rust"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["count"], 1);
        assert_eq!(parsed["results"][0], "notes/rust-tips.md");
    }

    #[tokio::test]
    async fn recall_finds_memory_content() {
        let server = McpServer::new(test_state());
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"memories/user/fact_1.md","content":"Alice prefers Rust over Go"}}}"#,
            )
            .await;

        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_recall","arguments":{"query":"rust"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["count"], 1);
        assert!(parsed["memories"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("Rust"));
    }

    #[tokio::test]
    async fn propose_returns_pending() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_propose_memory_patch","arguments":{"memory":{"title":"test","body":"test body"}}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "pending");
        assert!(parsed["proposal_id"].as_str().unwrap().starts_with("prp_"));
    }

    #[tokio::test]
    async fn review_returns_decision() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_review_memory","arguments":{"proposal_id":"prp_test","decision":"approved","reason":"looks good"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["decision"], "approved");
        assert_eq!(parsed["proposal_id"], "prp_test");
    }

    #[tokio::test]
    async fn supersede_returns_new_id() {
        let server = McpServer::new(test_state());

        // Seed an existing active memory file so supersede has something to read.
        let seed_content = "---\nid: mem_old\nstatus: active\ntype: memory\n---\nold body";
        let seed_args = serde_json::json!({
            "name": "memoryfs_write_file",
            "arguments": {"path": "memories/old.md", "content": seed_content},
        });
        let seed_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": seed_args,
        });
        server.handle_message(&seed_req.to_string()).await;

        let new_content = "---\nid: mem_new\nstatus: active\ntype: memory\n---\nnew body";
        let supersede_args = serde_json::json!({
            "name": "memoryfs_supersede_memory",
            "arguments": {
                "old_path": "memories/old.md",
                "new_path": "memories/new.md",
                "new_content": new_content,
                "reason": "outdated info",
                "conflict_type": "update",
            },
        });
        let supersede_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": supersede_args,
        });
        let resp = server.handle_message(&supersede_req.to_string()).await;

        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["old_memory_id"], "mem_old");
        assert_eq!(parsed["new_memory_id"], "mem_new");
        assert_eq!(parsed["old_path"], "memories/old.md");
        assert_eq!(parsed["new_path"], "memories/new.md");
        assert!(!parsed["commit_hash"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn get_provenance_returns_placeholder() {
        let server = McpServer::new(test_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_get_provenance","arguments":{"memory_id":"mem_test123"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["memory_id"], "mem_test123");
        assert!(parsed["provenance"].is_object());
    }

    #[tokio::test]
    async fn commit_and_log() {
        let server = McpServer::new(test_state());
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"test.md","content":"v1"}}}"#,
            )
            .await;

        let commit_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"initial commit"}}}"#,
            )
            .await;
        let result = commit_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["author"], "user:alice");
        assert!(!parsed["commit_hash"].as_str().unwrap().is_empty());

        let log_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memoryfs_log","arguments":{"limit":10}}}"#,
            )
            .await;
        let result = log_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["commits"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["commits"][0]["message"], "initial commit");
    }

    #[tokio::test]
    async fn diff_between_commits() {
        let server = McpServer::new(test_state());

        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"a.md","content":"v1"}}}"#,
            )
            .await;
        let c1_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"c1"}}}"#,
            )
            .await;
        let c1_text = c1_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        let c1: serde_json::Value = serde_json::from_str(&c1_text).unwrap();
        let h1 = c1["commit_hash"].as_str().unwrap().to_string();

        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"b.md","content":"new file"}}}"#,
            )
            .await;
        let c2_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"c2"}}}"#,
            )
            .await;
        let c2_text = c2_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        let c2: serde_json::Value = serde_json::from_str(&c2_text).unwrap();
        let h2 = c2["commit_hash"].as_str().unwrap().to_string();

        let diff_msg = format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"memoryfs_diff","arguments":{{"from":"{h1}","to":"{h2}"}}}}}}"#
        );
        let diff_resp = server.handle_message(&diff_msg).await;
        let result = diff_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        let diff = parsed["diff"].as_array().unwrap();
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0]["type"], "added");
        assert_eq!(diff[0]["path"], "b.md");
    }

    #[tokio::test]
    async fn revert_to_previous_commit() {
        let server = McpServer::new(test_state());

        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"x.md","content":"original"}}}"#,
            )
            .await;
        let c1_resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"first"}}}"#,
            )
            .await;
        let c1_text = c1_resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        let c1: serde_json::Value = serde_json::from_str(&c1_text).unwrap();
        let h1 = c1["commit_hash"].as_str().unwrap().to_string();

        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"x.md","content":"modified"}}}"#,
            )
            .await;
        server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"second"}}}"#,
            )
            .await;

        let revert_msg = format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"memoryfs_revert","arguments":{{"target_commit":"{h1}","reason":"rollback"}}}}}}"#
        );
        let revert_resp = server.handle_message(&revert_msg).await;
        let result = revert_resp.result.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["reverted_to"], h1);
        assert!(!parsed["commit_hash"].as_str().unwrap().is_empty());
    }

    // ── 6.3 Auth tests ──────────────────────────────────────────────────

    fn denied_state() -> McpState {
        let dir = tempfile::tempdir().unwrap();
        let obj = ObjectStore::open(dir.path().join("objects")).unwrap();
        let index = InodeIndex::new();
        let commit_log = CommitGraph::new();
        let graph = EntityGraph::new();
        let mut policy = Policy::default();
        policy.default_acl.allow.push(crate::policy::AclRule {
            path: "public/**".into(),
            subjects: vec!["user:*".into()],
            actions: vec!["read".into(), "list".into()],
        });
        let auth = McpAuth::test("user:bob", "ws_other");

        McpState {
            object_store: Arc::new(obj),
            index: Arc::new(std::sync::RwLock::new(index)),
            commit_log: Arc::new(std::sync::RwLock::new(commit_log)),
            entity_graph: Arc::new(std::sync::RwLock::new(graph)),
            run_store: Arc::new(std::sync::RwLock::new(RunStore::new())),
            policy,
            auth,
            retrieval: None,
            indexer: None,
        }
    }

    #[tokio::test]
    async fn auth_denied_write_to_restricted_path() {
        let server = McpServer::new(denied_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_write_file","arguments":{"path":"secrets/key.md","content":"s3cr3t"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("forbidden"));
    }

    #[tokio::test]
    async fn auth_denied_read_outside_scope() {
        let server = McpServer::new(denied_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_read_file","arguments":{"path":"private/data.md"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("forbidden"));
    }

    #[tokio::test]
    async fn auth_denied_commit_without_write() {
        let server = McpServer::new(denied_state());
        let resp = server
            .handle_message(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memoryfs_commit","arguments":{"message":"sneaky commit"}}}"#,
            )
            .await;
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn parse_token_rejects_empty() {
        assert!(parse_token("").is_err());
    }

    #[test]
    fn parse_token_rejects_no_prefix() {
        assert!(parse_token("plain_token").is_err());
    }

    #[test]
    fn json_rpc_response_serialization() {
        let resp = JsonRpcResponse::success(serde_json::json!(1), serde_json::json!({"ok": true}));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 1);
        assert_eq!(json["result"]["ok"], true);
        assert!(json.get("error").is_none());
    }

    #[test]
    fn json_rpc_error_serialization() {
        let resp = JsonRpcResponse::error(serde_json::json!(2), -32600, "bad request".into());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32600);
        assert_eq!(json["error"]["message"], "bad request");
        assert!(json.get("result").is_none());
    }
}
