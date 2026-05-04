# Architecture

> 🇷🇺 [Русская версия](architecture.ru.md)

MemoryFS is a single Rust binary that owns one persistent project memory store and exposes it through two surfaces — REST and MCP — sharing the same access control, audit, and retrieval pipeline.

## High-level diagram

```
┌─────────────────────────────────────────────────────────────────┐
│  Agent (Claude Code, Cursor, custom)                            │
└────────────┬───────────────────────────────────┬────────────────┘
             │ JSON-RPC over stdio              │ HTTP
             ▼                                  ▼
┌──────────────────────┐            ┌──────────────────────┐
│  mcp::McpServer      │            │  api::router (axum)  │
│  17 tools            │            │  REST v1             │
└──────────┬───────────┘            └──────────┬───────────┘
           │                                   │
           └───────────┬───────────────────────┘
                       ▼
              ┌────────────────┐
              │ acl::check     │  policy + audit gate
              └───────┬────────┘
                      │
       ┌──────────────┼─────────────┬──────────────┐
       ▼              ▼             ▼              ▼
  ┌─────────┐   ┌──────────┐  ┌──────────┐   ┌──────────┐
  │ storage │   │ commit   │  │ supersede│   │retrieval │
  │ objects │   │ DAG      │  │ replace  │   │ vec+BM25 │
  │ + inode │   │ + diff   │  │ + audit  │   │ + RRF    │
  └────┬────┘   └────┬─────┘  └────┬─────┘   └────┬─────┘
       │             │             │              │
       └────────┬────┴─────────────┴──────────────┘
                ▼
        ┌──────────────┐         ┌─────────────────┐
        │ event_log    │ ──────▶ │ indexer worker  │
        │ NDJSON bus   │         │ chunk → embed   │
        └──────────────┘         │ → upsert        │
                                 └────────┬────────┘
                                          ▼
                                 ┌─────────────────┐
                                 │ Levara / Qdrant │
                                 │ + Tantivy BM25  │
                                 └─────────────────┘
```

## Data model

Every memory is a `.md` file with YAML frontmatter validated against `specs/schemas/v1/memory.schema.json`. IDs are typed ULID wrappers with prefixes (`mem_`, `dec_`, `ent_`, `run_`, `evt_`, `prp_`, `conv_`, `ws_`) so a `RunId` can never be passed where a `MemoryId` is expected.

Storage is content-addressable: `objects/<sha256>` holds the bytes, `inode_index` maps `path → current sha256`, `commit_log` is a DAG where each commit captures a parent + a path→hash snapshot. This is the same shape git uses internally; it gives free deduplication, free integrity check (path → hash → bytes round-trip), and free time travel.

## Source of truth invariant

If `objects/`, `inode_index`, the BM25 index, or the vector store ever disagree, the rebuild path is: **scan `decisions/`, `discoveries/`, `infra/`, etc. on disk → re-derive everything else.** No index is canonical.

## Two surfaces, one gate

Every operation — REST or MCP — passes through `acl::check(subject, action, path, policy)` before touching the store. The check is path-glob based (`memory/user/**`, `decisions/*.md`) with deny-by-default. Local single-user mode uses `Policy::local_user(subject)` which grants the current user `**`; multi-tenant deployments load policy from `.memory/policy.yaml` (schema in `specs/schemas/v1/policy.schema.json`).

Every check, every write, every supersede produces an `audit_log` entry — append-only NDJSON with a tamper-evident hash chain (`audit.rs`).

## Behavioral contract

The MCP server's `initialize` response includes an `instructions` field — a markdown contract telling the agent **when** to call which tool: recall-first before recommendations, save without asking on explicit decisions, supersede instead of overwrite. Source: [`src/mcp_instructions.md`](../src/mcp_instructions.md).

Two append-only paths (`decisions/`, `discoveries/`) are enforced server-side: a plain `write_file` overwriting an existing file in those prefixes is rejected with an error pointing at `memoryfs_supersede_memory`. The `force=true` parameter exists as an escape hatch for typo fixes.

## Retrieval pipeline

`/v1/context` (REST) and `memoryfs_recall` (MCP) both run through `retrieval::RetrievalEngine`:

1. Parallel vector search (Levara / Qdrant) and BM25 search (Tantivy).
2. Reciprocal Rank Fusion combines the two ranked lists.
3. Optional entity graph neighbor expansion adds `entity_score`.
4. Recency boost via exponential decay on `created_at` from frontmatter.
5. ACL post-filter — every candidate hash is re-checked against the caller's subject before returning.
6. Final hit list is read deterministically from disk (`objects/<hash>` not the cached chunk text) so the response is reproducible.

## Indexing pipeline

Writes never block on indexing. `event_log` is an append-only NDJSON bus with consumer offsets; the indexer worker tails it, chunks new files (`chunker.rs` — heading-aware with overlap, optional `document_title` prepend), embeds chunks via `embedder::Embedder`, and upserts to the vector backend. `MemoryId::from_path` derives a deterministic ULID from the file path, so re-indexing is idempotent — old chunks for a path are replaced, never piled up.

## Module map

| Module | Responsibility |
|--------|----------------|
| `ids` | Typed ULID wrappers, `CommitHash` |
| `error` | Unified `MemoryFsError` + HTTP/MCP status mapping |
| `schema` | Frontmatter parser + JSON Schema validator |
| `storage` | Content-addressable object store + inode index |
| `commit` | Commit DAG, diff, revert |
| `acl` | Path-glob policy engine |
| `policy` | Workspace policy types + loader |
| `redaction` | Secret detection (20+ patterns) + redaction |
| `audit` | Tamper-evident NDJSON log |
| `event_log` | Append-only event bus + consumer offsets |
| `embedder` | `Embedder` trait + `HttpEmbedder` |
| `vector_store` | `VectorStore` trait + Qdrant impl |
| `levara` | Primary backend: vector + embed + hybrid via gRPC |
| `bm25` | Tantivy full-text |
| `chunker` | Heading-aware markdown splitter |
| `indexer` | Event-driven worker |
| `reindex` | Full rebuild with checkpoint |
| `retrieval` | Multi-signal engine + RRF + ACL post-filter |
| `graph` | Entity graph (CRUD, BFS) |
| `entity_extraction` | NER via LLM + linking |
| `extraction` / `extraction_worker` | Memory extraction from runs |
| `inbox` | Proposal review queue |
| `memory_policy` | Auto-commit / require-review decisions |
| `post_scan` | Post-extraction secret scan |
| `supersede` | Memory replacement DAG, cycle detection |
| `runs` | Agent run lifecycle |
| `mcp` | JSON-RPC 2.0 server, 17 tool handlers, `initialize` instructions |
| `api` | REST router (axum) |
| `backup` | Full workspace backup + restore + verify |
| `migration` | Schema migration runner with up/down |
| `observability` | Metrics + tracing middleware |
| `llm` | `LlmClient` trait + OpenAI-compatible client |
