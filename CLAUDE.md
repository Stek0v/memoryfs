# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

MemoryFS — verifiable memory workspace for AI agents. Markdown files are the source of truth;
vector/BM25/graph indexes are disposable derivatives. Currently in **Phase 0 (Foundations)**:
schemas, fixtures, validators, and code skeleton are complete; real implementations begin
in Phase 1.

## Commands

All dev commands go through `just` (install: `cargo install just` or `brew install just`).

```bash
just bootstrap          # idempotent: rust, python, just, docker, pre-commit
just env-check          # validate environment (CI + onboarding)
just dev-up             # start Levara (gRPC vector store) + Ollama on host
just dev-down           # stop without data loss
just dev-reset          # stop + delete volumes

just validate-all       # fast (<10s): JSON/YAML/bash syntax, schemas, fixtures, schema-violations
just validate-schemas   # JSON Schema well-formedness + $ref resolution
just validate-fixtures  # frontmatter validation against schemas/v1

just lint               # cargo clippy + ruff + pyright
just fix                # auto-fix: clippy + cargo fmt + ruff

just test-rust          # cargo nextest run --workspace
just test-python        # pytest
just test-contracts     # ACL matrix + supersede invariants
just test-adversarial   # secrets + injection + schema-violations suites
just test               # all of the above
```

Single Rust test: `cargo test -p memoryfs-core -- test_name`
Single Python test: `pytest workers/extractor/tests/test_foo.py::test_name -x`

## Architecture

Pure Rust stack (ADR-003). No Python worker — LLM and embedding accessed via HTTP APIs.

```text
crates/core/     Rust library — storage, ACL, retrieval, embedding, LLM client, vector store
crates/cli/      Rust binary — clap CLI (workspace, memory, review, recall, admin commands)
specs/           Machine-readable contracts (source of truth over prose docs)
  schemas/v1/    JSON Schema 2020-12 (9 schemas: memory, conversation, run, entity, etc.)
  openapi.yaml   REST API v1 (OpenAPI 3.1)
  mcp.tools.json 17 MCP tools with inputSchema
fixtures/        Golden test data (killer-demo happy-path, killer-demo-with-conflict)
tests/adversarial/  Security suites: secrets (50 cases), injection (12 scenarios), schema-violations (20)
```

**Key integrations:**

- **Levara** (`levara.rs`): Primary vector store + embedder backend via gRPC (`tonic`).
  Proto from `github.com/Stek0v/Levara`. Implements `VectorStore` + `Embedder` traits.
  Combined operations: `embed_and_index()` (embed+index in one call), `search_by_text()`
  (embed+search), `hybrid_search()` (vector+BM25 RRF). Replaces separate Qdrant + TEI.
- **Qdrant** (`vector_store.rs`): Legacy vector store via `qdrant-client` crate. Still
  available as `QdrantVectorStore` for environments without Levara.
- **Embedding**: EmbeddingGemma, local inference via HuggingFace TEI (ADR-013). Pluggable `Embedder` trait.
- **LLM extraction**: DeepSeek cloud API (`api.deepseek.com`), OpenAI-compatible (ADR-014). Pluggable `LlmClient` trait.
- **Auth**: JWT tokens via `jsonwebtoken` crate (ADR-005).

Design documents (`00-analysis.md` through `08-adrs.md`, `threat-model.md`) describe architecture,
data model, API, tasks, corner cases, testing strategy, roadmap, and ADRs. These are reference
prose — when they diverge from `specs/schemas/v1/`, the schemas win (tracked in
`specs/DRIFT-REPORT.md`).

## Key conventions

**Machine contracts are the source of truth.** If prose in `02-data-model.md` diverges from
schemas in `specs/schemas/v1/`, it's a bug in the prose. Schema changes must update prose
in the same PR.

**IDs:** Typed ULID wrappers with prefixes — `mem_`, `conv_`, `run_`, `ent_`, `dec_`, `ws_`,
`evt_` (Crockford Base-32, 26 chars). `CommitHash` is SHA-256 hex (64 chars). Defined in
`crates/core/src/ids.rs`.

**Errors:** Unified `MemoryFsError` enum in `crates/core/src/error.rs` with HTTP status + API
code mapping. No `unwrap()`/`panic!()` in production paths.

**Rust style:** `cargo fmt` + `clippy -D warnings`. Structured logging via `tracing`. Errors via
`thiserror`/`MemoryFsError`. MSRV 1.88, edition 2021. Async traits via `async-trait`.

**Commits:** Conventional format — `feat(core): add CommitHash::parse`,
`fix(redactor): catch JWT alg=none`, `docs(02): sync prose with memory.schema.json`.

**Security regressions:** Every new gap adds a test case to the relevant adversarial suite (`tests/adversarial/`).

## CI pipeline

10 jobs in `.github/workflows/ci.yml`:

1. **validate** — JSON/YAML/bash syntax, schema well-formedness, fixture validation,
   schema-violations self-test, drift report (informational)
2. **fmt** — `cargo fmt --check` (instant, no compilation)
3. **rust** — clippy + nextest (clippy compiles, no separate build step)
4. **coverage** — `cargo-llvm-cov` with `--fail-under-lines 80` (CC.1 gate), uploads lcov artifact
5. **msrv** — `cargo check` on Rust 1.88 (declared minimum version)
6. **security** — `cargo audit` for known dependency vulnerabilities
7. **adversarial** — secrets/injection suites (placeholders until Phase 1.7/2)
8. **api-specs** — spectral lint OpenAPI, MCP tools.json self-consistency, API drift check (CC.4, informational)
9. **markdown** — markdownlint + lychee link check (informational, not in gate)
10. **ci-gate** — requires: validate, fmt, rust, coverage, msrv, security, adversarial, api-specs

Every commit must leave `just validate-all` green. Pre-commit hooks
(`.pre-commit-config.yaml`) run 10 validators including gitleaks, cargo-fmt, ruff,
schema validation.

## What's implemented vs. stub

**Implemented:** `ids.rs` (ULID types), `error.rs` (error hierarchy), `schema.rs` (frontmatter
parser + JSON Schema validator), `storage.rs` (content-addressable object store + inode index),
`commit.rs` (commit log with DAG, diff, revert), `acl.rs` (path-glob ACL engine),
`redaction.rs` (secret detection + redaction, 20+ patterns), `audit.rs` (tamper-evident log),
`event_log.rs` (append-only NDJSON bus + consumer offsets), `embedder.rs` (trait + `HttpEmbedder`
for OpenAI-compatible endpoints), `vector_store.rs` (trait + `QdrantVectorStore` via gRPC),
`levara.rs` (`LevaraVectorStore` + `LevaraEmbedder` via gRPC — primary backend, replaces
Qdrant + TEI; combined `embed_and_index()`, `search_by_text()`, `hybrid_search()`; proto
generated from `github.com/Stek0v/Levara`, 13 tests),
`bm25.rs` (Tantivy full-text index with field boosts), `chunker.rs` (heading-aware markdown
splitter with overlap), `indexer.rs` (event-driven worker: chunk → embed → upsert),
`reindex.rs` (full rebuild with progress + checkpoint resume), `extraction.rs` +
`extraction_worker.rs` (LLM-based memory extraction), `inbox.rs` (proposal review queue),
`memory_policy.rs` (auto-commit/review decisions), `post_scan.rs` (post-extraction secret scan),
`supersede.rs` (memory replacement DAG with cycle detection), `observability.rs` (metrics +
tracing middleware), `policy.rs` (workspace policy config), `api.rs` (REST API skeleton with
tests), CLI structure in `main.rs` (clap commands, no handlers), all JSON Schemas, fixtures,
adversarial test data, validation scripts.

**Phase 4 (Retrieval + Context API):** `llm.rs` (`LlmClient` trait + `OpenAiCompatibleClient`
for DeepSeek/OpenAI-compatible APIs), `retrieval.rs` (multi-signal engine: parallel vector +
BM25 queries, Reciprocal Rank Fusion, scope/recency boost, ACL post-filter with audit logging,
deterministic file read from FS), `api.rs` updated with `/v1/context` endpoint and
`ContextRetriever` trait object for type-erased retrieval.

**Phase 5 (Entity Graph):** `graph.rs` (in-memory entity store with `EntityKind` enum,
`Relation` enum (16 types), CRUD, dedupe by canonical_name+kind, alias merge, BFS neighbor
traversal with depth limit and relation filter), `api.rs` updated with 5 entity endpoints
(create, get, search, link, neighbors). `ids.rs` updated with Serialize/Deserialize on all
prefixed ID types. `entity_extraction.rs` (NER via LLM + entity linking with Levenshtein
fuzzy matching, dedupe threshold 0.8, first-word fallback search). `retrieval.rs` updated
with entity-aware fusion: `entity_score` in `SourceScores`, entity graph neighbor expansion,
configurable `ENTITY_BOOST_WEIGHT`.

**Phase 6 (MCP Server):** `mcp.rs` (JSON-RPC 2.0 dispatcher over stdio, `McpServer` with
`run_stdio()` transport loop, `McpState` shared state, `McpAuth` with token parsing
(`utk_`/`atk_` prefixes), 17 tool handlers matching `specs/mcp.tools.json`: file CRUD
(`read_file`, `write_file`, `list_files`), search/recall, memory management (`remember`,
`propose`, `review`, `supersede`), commit operations (`commit`, `revert`, `log`, `diff`),
entity graph (`link_entity`, `get_provenance`), agent runs (`create_run`, `finish_run`),
ACL-gated with `Policy`-based access control, 18 tests).

**Phase 7 (Hardening):** `backup.rs` (full workspace backup/restore — JSON manifest +
raw object files in directory tree, `BackupParams` struct, `create_backup()`, `restore_backup()`
returning `RestoredState`, `verify_backup()` with integrity checks for missing/corrupt objects
and commit graph validity, entity graph reconstruction from serialized JSON, 7 tests).
`migration.rs` (schema migration runner — `Migration` struct with up/down transform functions,
`SchemaState` tracking version + applied history, `MigrationRunner` with `plan()` path-finding,
`migrate()` applying transforms to all indexed files, `rollback()` for last migration,
built-in v1→v2 chain, cycle detection, 9 tests). `graph.rs` updated with `all_entities()`
and `all_edges()` accessors for backup serialization.

**Runs module:** `runs.rs` (in-memory `RunStore` — `start()`/`finish()`/`get()`/`list()`
with full run lifecycle: `TriggerKind` enum, `RunStatus` with terminal detection,
`RunMetrics`, `Artifacts`, serialization to frontmatter markdown, 13 tests). MCP tools
`create_run`/`finish_run` now use `RunStore` instead of inline stubs.

**CLI + REST server:** `main.rs` — `memoryfs serve` starts axum HTTP server on
`127.0.0.1:7777` (configurable `--bind`), `memoryfs mcp` runs MCP stdio server,
all other subcommands are HTTP client commands (health, init, status, read, write,
list, commit, log, diff, revert).

**Phase 7.3 (Chaos engineering):** `chaos.rs` (25 tests covering object store
corruption detection and recovery, commit graph stress and conflict detection,
audit log tamper/truncation handling, event log reopen and consumer offsets,
backup integrity with corrupt objects, migration rollback safety, index
consistency with dangling refs, run store bulk operations, large payloads,
unicode paths, entity graph bulk operations).

**Phase 7.5 (Documentation):** `docs/architecture.md` (system overview, core
modules, data flow, security model, key invariants), `docs/operations.md`
(server setup, MCP config, backup/restore, migration, monitoring, troubleshooting),
`docs/security.md` (threat model, auth, ACL, sensitive memory handling, secret
detection, audit trail, adversarial suites, chaos tests),
`docs/integrations/` (mcp.md, rest-api.md, embedding.md, llm.md).

**Benchmark (Phase 0.1 + 7.4):** `crates/core/tests/bench_scale.rs` — 100k file
scale test with p50/p95/p99 latency assertions. SLO: p95 read < 50ms (actual:
0.191ms), p95 write < 50ms (actual: 0.318ms). Results in `bench/results/0.1.md`.

**API drift test (CC.4):** `scripts/check_api_drift.py` — compares OpenAPI spec
paths against implemented axum routes. Runs in CI (informational). Use `--strict`
to fail on missing implementations.

**Eval infrastructure (Phase 0.3, 0.4):** `eval/retrieval/` — 200 query/answer
pairs across 5 types, `run.py` computes NDCG@k/MRR/Recall@k with `--check-regression`
(2% threshold). `eval/extraction/` — 200 annotated conversations with 240 expected
proposals, `run.py` computes precision/recall/F1 by type and sensitivity. Both have
mock baselines and `--save-baseline` mode. Extraction prompt contract in
`prompts/extraction/v1.md`.
