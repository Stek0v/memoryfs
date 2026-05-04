//! MemoryFS CLI — `serve` starts the REST server, other commands talk to it.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(
    name = "memoryfs",
    version,
    about = "MemoryFS — verifiable memory workspace for AI agents",
    long_about = None,
)]
struct Cli {
    /// Workspace ID.
    #[arg(long, env = "MEMORYFS_WORKSPACE_ID", global = true)]
    workspace: Option<String>,

    /// API endpoint.
    #[arg(
        long,
        env = "MEMORYFS_ENDPOINT",
        default_value = "http://127.0.0.1:7777",
        global = true
    )]
    endpoint: String,

    /// Bearer token (utk_/atk_).
    #[arg(long, env = "MEMORYFS_TOKEN", hide_env_values = true, global = true)]
    token: Option<String>,

    /// Log level.
    #[arg(long, env = "MEMORYFS_LOG", default_value = "info", global = true)]
    log: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the REST API server.
    Serve {
        /// Address to bind (e.g. 0.0.0.0:7777).
        #[arg(long, default_value = "127.0.0.1:7777")]
        bind: String,
        /// Path to workspace data directory.
        #[arg(long, default_value = ".memoryfs")]
        data_dir: String,
    },
    /// Run the MCP server over stdio (for Claude Code, Cursor, etc.).
    Mcp,
    /// Initialize a new workspace.
    Init { name: String },
    /// Show workspace status (HEAD commit, file count).
    Status,
    /// Read a file from the workspace.
    Read {
        path: String,
        #[arg(long)]
        at_commit: Option<String>,
    },
    /// Write a file to the workspace (stages for commit).
    Write {
        path: String,
        /// Read content from this local file.
        #[arg(long)]
        from_file: String,
    },
    /// List files in the workspace.
    List {
        #[arg(long)]
        prefix: Option<String>,
    },
    /// Create a commit from staged changes.
    Commit {
        /// Commit message.
        #[arg(short, long)]
        message: String,
        /// Expected parent commit (optimistic concurrency).
        #[arg(long)]
        parent: Option<String>,
    },
    /// Show commit log.
    Log {
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Diff between two commits.
    Diff { from: String, to: String },
    /// Revert to a prior commit.
    Revert {
        /// Target commit hash to revert to.
        target: String,
        /// Reason for reverting.
        #[arg(long)]
        reason: String,
    },
    /// Health check.
    Health,
}

struct HttpClient {
    base: String,
    token: Option<String>,
    workspace: Option<String>,
    http: reqwest::Client,
}

impl HttpClient {
    fn new(cli: &Cli) -> Self {
        Self {
            base: cli.endpoint.trim_end_matches('/').to_string(),
            token: cli.token.clone(),
            workspace: cli.workspace.clone(),
            http: reqwest::Client::new(),
        }
    }

    fn ws_id(&self) -> Result<&str> {
        self.workspace
            .as_deref()
            .context("--workspace or MEMORYFS_WORKSPACE_ID required")
    }

    fn auth_header(&self) -> Result<String> {
        self.token
            .as_ref()
            .map(|t| format!("Bearer {t}"))
            .context("--token or MEMORYFS_TOKEN required")
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut req = self.http.get(&url);
        if let Ok(auth) = self.auth_header() {
            req = req.header("authorization", auth);
        }
        let resp = req.send().await.context("request failed")?;
        let status = resp.status();
        let body: Value = resp.json().await.context("invalid JSON response")?;
        if !status.is_success() {
            let code = body["code"].as_str().unwrap_or("UNKNOWN");
            let msg = body["message"].as_str().unwrap_or("unknown error");
            bail!("{code}: {msg}");
        }
        Ok(body)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut req = self.http.post(&url).json(body);
        if let Ok(auth) = self.auth_header() {
            req = req.header("authorization", auth);
        }
        let resp = req.send().await.context("request failed")?;
        let status = resp.status();
        let json: Value = resp.json().await.context("invalid JSON response")?;
        if !status.is_success() {
            let code = json["code"].as_str().unwrap_or("UNKNOWN");
            let msg = json["message"].as_str().unwrap_or("unknown error");
            bail!("{code}: {msg}");
        }
        Ok(json)
    }

    async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut req = self.http.put(&url).json(body);
        if let Ok(auth) = self.auth_header() {
            req = req.header("authorization", auth);
        }
        let resp = req.send().await.context("request failed")?;
        let status = resp.status();
        let json: Value = resp.json().await.context("invalid JSON response")?;
        if !status.is_success() {
            let code = json["code"].as_str().unwrap_or("UNKNOWN");
            let msg = json["message"].as_str().unwrap_or("unknown error");
            bail!("{code}: {msg}");
        }
        Ok(json)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log);

    match cli.command {
        Command::Serve { bind, data_dir } => cmd_serve(&bind, &data_dir).await,
        Command::Mcp => cmd_mcp().await,
        _ => cmd_client(cli).await,
    }
}

// ── Server commands ──────────────────────────────────────────────────────

async fn cmd_serve(bind: &str, data_dir: &str) -> Result<()> {
    use memoryfs::api;
    use memoryfs::bm25::Bm25Index;
    use memoryfs::commit::CommitGraph;
    use memoryfs::embedder::{HttpEmbedder, HttpEmbedderConfig};
    use memoryfs::event_log::{ConsumerOffset, EventLog};
    use memoryfs::graph::EntityGraph;
    use memoryfs::indexer::{FileChange, Indexer, IndexerConfig};
    use memoryfs::levara::{LevaraClient, LevaraVectorStore};
    use memoryfs::policy::{AclRule, Policy};
    use memoryfs::storage::{InodeIndex, ObjectStore};
    use memoryfs::MemoryId;

    let data_path = std::path::Path::new(data_dir);
    std::fs::create_dir_all(data_path)
        .with_context(|| format!("failed to create data dir: {data_dir}"))?;

    let store =
        ObjectStore::open(data_path.join("objects")).context("failed to open object store")?;
    let index = InodeIndex::new();
    let graph = CommitGraph::new();
    let entity_graph = EntityGraph::new();
    let mut policy = Policy::default();
    if let Ok(path) = std::env::var("MEMORYFS_POLICY_FILE") {
        let yaml = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read policy file: {path}"))?;
        policy = Policy::from_yaml(&yaml).context("failed to parse policy file")?;
    } else if std::env::var("MEMORYFS_DEV_ALLOW_ALL")
        .ok()
        .as_deref()
        .is_some_and(|v| matches!(v, "1" | "true" | "yes"))
    {
        policy.default_acl.allow.push(AclRule {
            path: "**".into(),
            subjects: vec!["*".into()],
            actions: vec![
                "read".into(),
                "write".into(),
                "review".into(),
                "commit".into(),
                "revert".into(),
                "list".into(),
            ],
        });
    }
    let workspace_id = std::env::var("MEMORYFS_WORKSPACE_ID").unwrap_or_else(|_| "default".into());

    let event_log = Arc::new(
        EventLog::open(data_path.join("events.ndjson")).context("failed to open event log")?,
    );

    let state = Arc::new(api::AppState {
        store,
        index: RwLock::new(index),
        graph: RwLock::new(graph),
        staging: RwLock::new(BTreeMap::new()),
        policy,
        metrics_handle: None,
        retrieval: None,
        entity_graph: RwLock::new(entity_graph),
        event_log: Some(event_log.clone()),
        workspace_id: workspace_id.clone(),
    });

    // ── Indexer background task (optional — runs when LEVARA_GRPC_ENDPOINT is set) ──
    let levara_grpc = std::env::var("LEVARA_GRPC_ENDPOINT").ok();
    if let Some(grpc_url) = levara_grpc {
        let embed_endpoint =
            std::env::var("EMBEDDING_ENDPOINT").unwrap_or_else(|_| "http://localhost:11434".into());
        let embed_model =
            std::env::var("EMBEDDING_MODEL").unwrap_or_else(|_| "nomic-embed-text".into());
        let embed_dims: usize = std::env::var("EMBEDDING_DIMENSIONS")
            .unwrap_or_else(|_| "768".into())
            .parse()
            .unwrap_or(768);
        let collection = std::env::var("LEVARA_COLLECTION").unwrap_or_else(|_| "memoryfs".into());

        let mut levara_client =
            LevaraClient::connect(&grpc_url, &embed_endpoint, &embed_model, embed_dims)
                .await
                .context("failed to connect to Levara gRPC")?;
        if let Ok(http_base) = std::env::var("LEVARA_HTTP_ENDPOINT") {
            levara_client = levara_client.with_http_base(http_base);
        }

        let vector_store = Arc::new(LevaraVectorStore::new(levara_client.clone(), collection));
        vector_store
            .ensure_collection()
            .await
            .context("failed to ensure Levara collection")?;

        let embedder = Arc::new(HttpEmbedder::new(HttpEmbedderConfig {
            endpoint: embed_endpoint,
            model: embed_model,
            dimension: embed_dims,
            batch_size: 64,
        }));

        let bm25_index = Arc::new(
            Bm25Index::open(&data_path.join("bm25"))
                .unwrap_or_else(|_| Bm25Index::in_memory().expect("BM25 in-memory")),
        );

        let indexer = Arc::new(Indexer::new(
            embedder,
            vector_store,
            bm25_index,
            IndexerConfig::default(),
        ));

        let consumer = ConsumerOffset::open(data_path.join("indexer.offset"))
            .context("failed to open indexer consumer offset")?;

        let idx_event_log = event_log.clone();
        let idx_state = state.clone();
        let idx_ws = workspace_id.clone();

        tokio::spawn(async move {
            use memoryfs::event_log::EventKind;

            tracing::info!("indexer background task started");
            loop {
                let from_offset = consumer.get().unwrap_or(0);
                let events = match idx_event_log.read_from(from_offset) {
                    Ok(e) => e,
                    Err(e) => {
                        tracing::error!("indexer read_from error: {e}");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                let mut processed = 0u64;
                for event in &events {
                    let is_indexable = matches!(
                        event.kind,
                        EventKind::CommitCreated
                            | EventKind::CommitReverted
                            | EventKind::MemoryAutoCommitted
                            | EventKind::MemoryApproved
                            | EventKind::MemorySuperseded
                    );
                    if !is_indexable {
                        processed = event.offset + 1;
                        continue;
                    }

                    let changes: Vec<FileChange> = {
                        let index = idx_state.index.read().unwrap();
                        index
                            .iter()
                            .map(|(path, hash)| {
                                let content = idx_state
                                    .store
                                    .get(hash)
                                    .ok()
                                    .map(|data| String::from_utf8_lossy(&data).to_string());
                                FileChange {
                                    // Deterministic per-path ID so re-indexing a
                                    // file across cycles upserts over its prior
                                    // chunks instead of stacking new ones under
                                    // a fresh ULID. Without this, a supersede
                                    // commit can't evict the old `status:active`
                                    // chunks and they keep crowding out new
                                    // content in top-K search.
                                    memory_id: MemoryId::from_path(path),
                                    file_path: path.to_string(),
                                    workspace_id: idx_ws.clone(),
                                    content,
                                    commit: event.target.clone(),
                                }
                            })
                            .collect()
                    };

                    if !changes.is_empty() {
                        match indexer.index_batch(&changes).await {
                            Ok(results) => {
                                let total_chunks: usize =
                                    results.iter().map(|r| r.chunk_count).sum();
                                tracing::info!(
                                    "indexed {} files ({} chunks) for commit {}",
                                    changes.len(),
                                    total_chunks,
                                    &event.target
                                );
                            }
                            Err(e) => {
                                tracing::error!("indexer batch error: {e}");
                            }
                        }
                    }

                    processed = event.offset + 1;
                }

                if processed > from_offset {
                    let _ = consumer.commit(processed);
                }

                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        });

        tracing::info!("indexer connected to Levara at {grpc_url}");
    } else {
        tracing::warn!("LEVARA_GRPC_ENDPOINT not set — indexing disabled, search won't work");
    }

    let app = api::router(state);
    let app = app.layer(tower_http::cors::CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;
    tracing::info!("listening on {bind}");

    axum::serve(listener, app).await.context("server error")?;

    Ok(())
}

async fn cmd_mcp() -> Result<()> {
    use memoryfs::commit::CommitGraph;
    use memoryfs::graph::EntityGraph;
    use memoryfs::mcp::{McpAuth, McpServer, McpState};
    use memoryfs::policy::Policy;
    use memoryfs::runs::RunStore;
    use memoryfs::storage::{InodeIndex, ObjectStore};

    // Local-mode bootstrap: when launched as an MCP child by Claude Code /
    // Cursor, no env is set up by hand. Detect the project root, place the
    // data dir inside it, and derive a workspace name from the directory.
    // All three values can still be overridden via env for non-local setups.
    let project_root = resolve_project_root()?;
    let data_dir = std::env::var("MEMORYFS_DATA_DIR")
        .unwrap_or_else(|_| project_root.join(".memory").to_string_lossy().into_owned());
    if std::env::var("MEMORYFS_WORKSPACE_ID").is_err() {
        let ws = derive_workspace_id(&project_root);
        // SAFETY: cmd_mcp() runs single-threaded before any tokio task spawns.
        // McpAuth::from_env() reads it on the next line.
        std::env::set_var("MEMORYFS_WORKSPACE_ID", &ws);
    }
    if std::env::var("MEMORYFS_TOKEN").is_err() {
        // Local single-user MCP has no meaningful auth boundary — the process
        // already runs as the user. Use a stable subject so audit logs are
        // readable; users who want a real token just set the env var.
        let user = std::env::var("USER").unwrap_or_else(|_| "local".into());
        std::env::set_var("MEMORYFS_TOKEN", format!("utk_{user}"));
    }

    let auth = McpAuth::from_env()
        .context("MCP requires MEMORYFS_TOKEN and MEMORYFS_WORKSPACE_ID environment variables")?;

    eprintln!(
        "memoryfs mcp: project={} workspace={} data={}",
        project_root.display(),
        auth.workspace_id,
        data_dir,
    );

    let data_path = std::path::Path::new(&data_dir);
    std::fs::create_dir_all(data_path)
        .with_context(|| format!("failed to create data dir: {data_dir}"))?;

    let store =
        ObjectStore::open(data_path.join("objects")).context("failed to open object store")?;

    let policy = Policy::local_user(&auth.subject);
    let state = McpState {
        object_store: Arc::new(store),
        index: Arc::new(std::sync::RwLock::new(InodeIndex::new())),
        commit_log: Arc::new(std::sync::RwLock::new(CommitGraph::new())),
        entity_graph: Arc::new(std::sync::RwLock::new(EntityGraph::new())),
        run_store: Arc::new(std::sync::RwLock::new(RunStore::new())),
        policy,
        auth,
        retrieval: None,
    };

    let server = McpServer::new(state);
    server.run_stdio().await.context("MCP server error")?;

    Ok(())
}

// ── Client commands ──────────────────────────────────────────────────────

async fn cmd_client(cli: Cli) -> Result<()> {
    let client = HttpClient::new(&cli);

    match cli.command {
        Command::Health => {
            let resp = client.get("/v1/admin/health").await?;
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }

        Command::Init { name } => {
            let body = serde_json::json!({"name": name});
            let resp = client.post("/v1/workspaces", &body).await?;
            let id = resp["id"].as_str().unwrap_or("?");
            println!("workspace created: {id}");
        }

        Command::Status => {
            let ws = client.ws_id()?;
            let resp = client.get(&format!("/v1/workspaces/{ws}")).await?;
            let head = resp["head_commit"].as_str().unwrap_or("(none)");
            let name = resp["name"].as_str().unwrap_or("?");
            println!("workspace: {name}");
            println!("head: {head}");
        }

        Command::Read { path, at_commit: _ } => {
            let resp = client.get(&format!("/v1/files/{path}")).await?;
            let content = resp["content"].as_str().unwrap_or("");
            print!("{content}");
        }

        Command::Write { path, from_file } => {
            let content = std::fs::read_to_string(&from_file)
                .with_context(|| format!("failed to read {from_file}"))?;
            let body = serde_json::json!({"content": content});
            let resp = client.put(&format!("/v1/files/{path}"), &body).await?;
            let hash = resp["staged_hash"].as_str().unwrap_or("?");
            let bytes = resp["bytes"].as_u64().unwrap_or(0);
            println!("staged {path} ({bytes} bytes, hash: {hash})");
        }

        Command::List { prefix } => {
            let mut url = "/v1/files".to_string();
            if let Some(ref p) = prefix {
                url = format!("{url}?prefix={p}");
            }
            let resp = client.get(&url).await?;
            if let Some(items) = resp["items"].as_array() {
                for item in items {
                    let path = item["path"].as_str().unwrap_or("?");
                    let size = item["size"].as_u64().unwrap_or(0);
                    println!("{path}  ({size} bytes)");
                }
                if items.is_empty() {
                    println!("(no files)");
                }
            }
        }

        Command::Commit { message, parent } => {
            let ws = client.ws_id()?;
            let mut body = serde_json::json!({"message": message});
            if let Some(p) = parent {
                body["parent_commit"] = Value::String(p);
            }
            let resp = client
                .post(&format!("/v1/workspaces/{ws}/commit"), &body)
                .await?;
            let hash = resp["hash"].as_str().unwrap_or("?");
            println!("committed: {hash}");
            println!("message: {message}");
        }

        Command::Log { limit } => {
            let ws = client.ws_id()?;
            let resp = client
                .get(&format!("/v1/workspaces/{ws}/log?limit={limit}"))
                .await?;
            if let Some(items) = resp["items"].as_array() {
                for item in items {
                    let hash = item["hash"].as_str().unwrap_or("?");
                    let msg = item["message"].as_str().unwrap_or("");
                    let author = item["author"].as_str().unwrap_or("?");
                    let ts = item["created_at"].as_str().unwrap_or("?");
                    println!("{} {} ({}, {})", &hash[..12], msg, author, ts);
                }
                if items.is_empty() {
                    println!("(no commits)");
                }
            }
        }

        Command::Diff { from, to } => {
            let ws = client.ws_id()?;
            let resp = client
                .get(&format!("/v1/workspaces/{ws}/diff?from={from}&to={to}"))
                .await?;
            if let Some(entries) = resp["entries"].as_array() {
                for entry in entries {
                    let change = entry["change"].as_str().unwrap_or("?");
                    let path = entry["path"].as_str().unwrap_or("?");
                    println!("{change}: {path}");
                }
                if entries.is_empty() {
                    println!("(no changes)");
                }
            }
        }

        Command::Revert { target, reason } => {
            let ws = client.ws_id()?;
            let body = serde_json::json!({
                "target_commit": target,
                "reason": reason
            });
            let resp = client
                .post(&format!("/v1/workspaces/{ws}/revert"), &body)
                .await?;
            let hash = resp["hash"].as_str().unwrap_or("?");
            println!("reverted to {target}");
            println!("new commit: {hash}");
        }

        Command::Serve { .. } | Command::Mcp => unreachable!(),
    }

    Ok(())
}

fn init_logging(level: &str) {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(level))
        .try_init();
}

/// Resolve the project root for local-mode MCP. Order:
/// 1. `MEMORYFS_PROJECT_ROOT` env (explicit override)
/// 2. `git rev-parse --show-toplevel` from cwd (most projects are git repos)
/// 3. cwd (fallback for non-git directories)
fn resolve_project_root() -> Result<std::path::PathBuf> {
    if let Ok(p) = std::env::var("MEMORYFS_PROJECT_ROOT") {
        return Ok(std::path::PathBuf::from(p));
    }

    if let Ok(out) = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return Ok(std::path::PathBuf::from(s));
            }
        }
    }

    std::env::current_dir().context("failed to read current directory")
}

/// Derive a workspace ID from the project root path.
///
/// Format: `<slug>_<hash>` where `slug` is the directory basename (lowercased,
/// non-alphanumerics replaced with `-`) and `hash` is the first 8 hex chars
/// of SHA-256 over the absolute path. The slug keeps things human-readable in
/// audit logs and Levara's collection metadata; the hash suffix prevents two
/// projects with the same basename (e.g. two clones at different paths) from
/// silently sharing memory.
fn derive_workspace_id(root: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};

    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let basename = canonical
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".into());

    let slug: String = basename
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "workspace".into()
    } else {
        slug
    };

    let hash = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let suffix: String = hash.iter().take(4).map(|b| format!("{:02x}", b)).collect();

    format!("{slug}_{suffix}")
}
