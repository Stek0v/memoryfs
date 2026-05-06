---
schema_version: memoryfs/v1
id: conv_01HZK4M2A5B8C0D2E5F8G1H4J7
type: conversation
session_id: 2026-04-30-vector-store-discussion
participants:
  - user:alice
  - agent:architect
created_at: "2026-04-30T09:00:12.443Z"
started_at: "2026-04-30T09:00:00.000Z"
finished_at: "2026-04-30T09:11:48.221Z"
author: user:alice
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["user:alice", "agent:architect"]
tags: [memoryfs, vector-store, design]
scope: project
scope_id: project:memoryfs
redacted: true
redaction_summary:
  secrets_redacted: 1
  pii_redacted: 0
  redactor_version: "memoryfs-redact/0.4.2"
  categories: [api_key]
sensitivity: normal
extraction_status: completed
extraction_run_id: run_01HZK4M5J8K1M4N7P0Q3R6S9T2
extracted_memories:
  - mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0
  - mem_01HZK4M9F2K3M5N7P9Q1S3T5V7
  - mem_01HZK4M4D7F9H1J3K5M7N9P1Q3
language: en
turn_count: 6
token_count: 1842
---

# Vector store choice for MemoryFS

## turn 0 (user:alice) — 2026-04-30T09:00:00Z

OK, time to settle the retrieval backend. I want a recommendation today —
LanceDB, Qdrant, or pgvector? We need: hybrid search (dense + sparse), filterable
payload (workspace_id, scope, sensitivity, commit_hash), and we need to support
both local single-user mode and a team server. Test cluster I spun up is
reachable at `https://qdrant.internal.lan:6333` (key was `[REDACTED:api_key]`,
just rotated, doesn't matter).

## turn 1 (agent:architect) — 2026-04-30T09:01:34Z

Quick triage:

- **pgvector** — single store wins on simplicity and ops; loses on hybrid (sparse/BM25
  is an extension, not first-class) and on payload-filtering at scale. Good for
  workspaces under ~50k chunks; mediocre above.
- **LanceDB** — embedded, single-file, very low ops cost. Strong on ANN, weaker on
  filter-heavy queries when payload is rich. No first-class hybrid (sparse) — has
  to be assembled. Cluster story is immature.
- **Qdrant** — hybrid (dense + sparse + payload filtering) out-of-the-box; payload
  index supports our needs; embedded mode covers single-user; cluster covers team.
  Trade-off: an extra service in deployment.

For MemoryFS specifically, the deal-breakers tilt toward Qdrant — we need post-retrieval
ACL filter that runs over `workspace_id`, `scope`, `sensitivity`, and `at_commit`, and
hybrid scores fused across signals.

## turn 2 (user:alice) — 2026-04-30T09:03:12Z

### About me

For context, since you're going to build memories about my preferences:

- I'm staff engineer in Amsterdam. I lead MemoryFS.
- I prefer Rust for systems code, TypeScript for any UI/proto work, Python only for workers.
- I work TDD by default, ADR-style for decisions, two approvers on security PRs.
- Hours: 09:30–18:30 CET, no meetings before 12:00.

Use that for personalization later. Now — does Qdrant cluster mode have anything weird
about commit-bound queries? We need to query "what was indexed at commit X?" so we can
do reproducible recall.

## turn 3 (agent:architect) — 2026-04-30T09:05:01Z

Got it, profile noted. I'll propose memories for these later and you can review.

On commit-bound queries: Qdrant doesn't have a native "at_commit" notion, but our
indexer writes `commit_hash` as a payload field per point, and we keep a tombstone
table mapping `(memory_id, commit_hash)` → active|superseded. So commit-bound recall
becomes a payload filter `commit_hash IN <set of ancestors of target>`.

We need to be careful: the tombstone table is part of the metadata DB, not Qdrant.
If indexer falls behind a commit, we get a brief read-degraded window — we should
expose `lag_seconds.indexer` in `/admin/health`.

## turn 4 (user:alice) — 2026-04-30T09:07:22Z

### Preferences for the coder agent

While we're at it, encode for the coder agent:

- `cargo fmt` + `cargo clippy --all-targets -- -D warnings` before any commit
- `ruff check --fix` + `pyright` for Python
- All tests must pass locally before push
- No `unwrap()` / `panic!` in production code paths
- Structured logging only (`tracing`, `structlog`); no `println!`
- Tests fixtures go under `fixtures/killer-demo/`; don't create new fixture roots
- For cross-scope writes, use `propose_memory_patch`, never `remember`
- On detected conflict, always `supersede_memory` with explicit `conflict_type`

These are working rules, not preferences — non-compliant PRs get auto-rejected.

## turn 5 (agent:architect) — 2026-04-30T09:09:18Z

Acknowledged. I'll record those as `memory_type: preference` in
`memory/agents/coder/` (sensitivity: normal, auto-commit eligible).

For the vector store decision: I'll propose Qdrant baseline as a `memory_type:
constraint` in `memory/projects/memoryfs/`, and write the ADR
(`decisions/0001-vector-store-choice.md`) with full Context / Decision / Consequences.
Both will reference this conversation as `provenance.source_file`.

## turn 6 (user:alice) — 2026-04-30T09:11:00Z

Approved direction. Make the ADR explicit about the trade-offs we accepted (extra
service, not self-contained) so the next maintainer doesn't relitigate. Also flag
that LanceDB stays viable as plug-in for offline workspaces — we don't lock out.

> [conversation closed by user:alice at 2026-04-30T09:11:48Z]
