# Operations Guide

## Running the server

```bash
# Start dependencies (Levara — requires Ollama on host)
just dev-up

# Build and run the REST server
cargo run -p memoryfs -- serve --bind 127.0.0.1:7777 --data-dir .memoryfs

# Or via just (if configured)
just serve
```

The server creates `.memoryfs/` for workspace data (object store, indexes).

### Dependencies (docker-compose)

`just dev-up` starts Levara (requires Ollama running on the host):

| Service | Port | Purpose |
|---------|------|---------|
| Levara (gRPC) | 127.0.0.1:50051 | Vector store + embedder backend (HNSW) |
| Levara (HTTP) | 127.0.0.1:8080 | Swagger UI, `/metrics` |
| Ollama (host) | 127.0.0.1:11434 | Embedding model (`nomic-embed-text-v2-moe`) |

Levara builds from `../../Levara/Levara` by default. Override with
`LEVARA_REPO_PATH` env var if your Levara checkout is elsewhere.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `MEMORYFS_ENDPOINT` | `http://127.0.0.1:7777` | API base URL (for CLI client) |
| `MEMORYFS_WORKSPACE_ID` | — | Workspace ID for CLI/MCP |
| `MEMORYFS_TOKEN` | — | Bearer token (`utk_`/`atk_` prefix) |
| `MEMORYFS_LOG` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`) |
| `MEMORYFS_DATA_DIR` | `.memoryfs` | Data directory (for MCP server) |
| `LEVARA_GRPC_URL` | `http://127.0.0.1:50051` | Levara gRPC endpoint |
| `LEVARA_EMBED_ENDPOINT` | `http://127.0.0.1:8090/v1/embeddings` | Embedding API for Levara to proxy |
| `LEVARA_EMBED_MODEL` | `google/embedding-gemma` | Embedding model name |
| `DEEPSEEK_API_KEY` | — | DeepSeek API key for LLM extraction |

## MCP server

```bash
# Set required env vars
export MEMORYFS_TOKEN="utk_your_token_here"
export MEMORYFS_WORKSPACE_ID="ws_your_workspace_id"

# Run MCP stdio server
cargo run -p memoryfs -- mcp
```

For Claude Code integration, add to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "memoryfs": {
      "command": "memoryfs",
      "args": ["mcp"],
      "env": {
        "MEMORYFS_TOKEN": "utk_...",
        "MEMORYFS_WORKSPACE_ID": "ws_..."
      }
    }
  }
}
```

## Backup and restore

```rust
use memoryfs_core::backup::{create_backup, restore_backup, verify_backup, BackupParams};

// Create backup
create_backup(&BackupParams {
    backup_dir: &path,
    workspace_id: "ws_prod",
    object_store: &store,
    index: &index,
    commit_graph: &graph,
    entity_graph: &entities,
    audit_log_path: Some(&audit_path),
    event_log_path: Some(&event_path),
})?;

// Verify integrity
let result = verify_backup(&backup_dir)?;
assert!(result.is_ok());

// Restore to a new store
let restored = restore_backup(&backup_dir, &target_store)?;
```

Backup output is a directory with `manifest.json` and raw object files.
`verify_backup()` checks for missing objects, hash mismatches, and commit
graph validity.

## Schema migration

```rust
use memoryfs_core::migration::{MigrationRunner, SchemaState};

let runner = MigrationRunner::default_chain();
let mut state = SchemaState::new("memoryfs/v1");

// Migrate to v2
runner.migrate(&store, &mut index, &mut state, "memoryfs/v2")?;

// Rollback last migration
runner.rollback(&store, &mut index, &mut state)?;
```

Migrations transform frontmatter in indexed files. The runner plans a path
between versions and applies transforms sequentially.

## Monitoring

MemoryFS uses `tracing` for structured logging and exports Prometheus
metrics via the observability module.

Key log targets:

- `memoryfs_core::storage` — object store operations
- `memoryfs_core::commit` — commit graph mutations
- `memoryfs_core::retrieval` — query execution and scoring
- `memoryfs_core::mcp` — MCP tool dispatching
- `memoryfs_core::acl` — access control decisions

Set `MEMORYFS_LOG=debug` for detailed output, `trace` for full request tracing.

## Health check

```bash
memoryfs health
# or
curl http://127.0.0.1:7777/v1/admin/health
```

## Development

```bash
just bootstrap          # one-time setup
just env-check          # verify environment
just dev-up             # start Qdrant + Postgres
just test               # run all tests
just lint               # clippy + ruff + pyright
just validate-all       # schema + fixture validation
```

### Running a single test

```bash
cargo test -p memoryfs-core -- test_name
cargo test -p memoryfs-core -- chaos       # run chaos suite
```

## Troubleshooting

**Server fails to bind**: Check if port 7777 is in use (`lsof -i :7777`).
Use `--bind 0.0.0.0:8080` for a different port.

**MCP auth error**: Ensure `MEMORYFS_TOKEN` starts with `utk_` or `atk_`
and `MEMORYFS_WORKSPACE_ID` starts with `ws_`.

**Object store corruption**: Run `verify_backup()` on the data directory.
Corrupt objects can be recovered via `delete()` + `put()` (re-hashing).

**Stale indexes**: Indexes are derived data. Rebuild with a full reindex
if they diverge from the object store.
