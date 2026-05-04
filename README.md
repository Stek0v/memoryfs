# MemoryFS

Verifiable per-project memory for AI agents. Markdown files are the source of truth; vector / BM25 / graph indexes are disposable derivatives. Ships as a single binary that exposes a REST API and an MCP (Model Context Protocol) server, so any MCP-aware agent — Claude Code, Cursor, custom agents — picks up persistent project memory the moment it's attached.

> 🇷🇺 Русская версия: [README.ru.md](README.ru.md)

---

## Why this exists

LLM coding agents lose every decision the moment a session ends or `/compact` triggers. Notebooks and chat history don't survive process restarts; a vector DB on its own loses provenance and audit trail. MemoryFS treats each project's `.memory/` folder as the canonical record:

- **Markdown is truth.** Every memory is a `.md` file with frontmatter. Indexes (vector, BM25, entity graph) are rebuilt from these files — corrupt or lose them and the system recovers.
- **Append-only audit.** Decisions and discoveries cannot be silently overwritten. `supersede` records the new version while preserving the old one with `status: superseded`.
- **ACL by default.** A workspace `Policy` decides who can read / write / commit which paths. Local single-user mode auto-grants the current user; multi-tenant deployments use deny-by-default.
- **Behavioral contract ships with the server.** The MCP `initialize.instructions` field tells the agent when to recall, when to save, when to supersede — no per-machine onboarding.

## Install

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked
```

Requires Rust 1.88+. Pulls a single binary named `memoryfs`.

## Use it from Claude Code (MCP)

Register globally so every project gets memory automatically:

```bash
claude mcp add memoryfs --scope user -- memoryfs mcp
```

That's it. Open any project; the binary auto-detects the project root via `git rev-parse --show-toplevel` (or cwd), creates `<project>/.memory/`, derives a stable `workspace_id` from the canonical path, and starts serving 17 MCP tools. The behavioral contract (recall-first, append-only decisions, supersede semantics) is delivered in the `initialize` handshake — see [`src/mcp_instructions.md`](src/mcp_instructions.md).

A more detailed walkthrough lives in [docs/install.md](docs/install.md).

## Run the REST server

```bash
memoryfs serve --bind 127.0.0.1:7777
```

OpenAPI 3.1 spec: [`specs/openapi.yaml`](specs/openapi.yaml).

## Architecture in 30 seconds

```
.memory/
├── objects/          content-addressable blob store (sha256 → bytes)
├── inode_index       path → current hash
├── commit_log        DAG of commits (parent + snapshot)
├── audit_log         tamper-evident NDJSON
├── decisions/*.md    append-only, supersede-only
├── discoveries/*.md  append-only, supersede-only
├── infra/*.md        mutable facts
├── events/YYYY-MM-DD-*.md
└── preferences/*.md
```

Two surfaces wrap that store: a REST API (axum) and an MCP server (JSON-RPC over stdio). Both share the same `acl::check` gate, the same `Policy`, the same audit trail. Optional retrieval pipeline plugs in a vector backend ([Levara](https://github.com/Stek0v/Levara) preferred, Qdrant supported) plus Tantivy BM25, fused via Reciprocal Rank Fusion with ACL post-filter.

Full breakdown: [docs/architecture.md](docs/architecture.md).

## Component docs

| Component | What it owns |
|-----------|--------------|
| [storage](docs/components/storage.md)     | Object store + inode index |
| [commit](docs/components/commit.md)       | Commit DAG, diff, revert |
| [acl](docs/components/acl.md)             | Path-glob policy engine |
| [mcp](docs/components/mcp.md)             | 17 MCP tools, instructions, append-only guard |
| [retrieval](docs/components/retrieval.md) | Vector + BM25 + RRF + ACL post-filter |
| [indexing](docs/components/indexing.md)   | Event-driven chunk → embed → upsert |
| [audit](docs/components/audit.md)         | Tamper-evident NDJSON log |

Integrations: [Claude Code](docs/integrations/claude-code.md) · [REST API](docs/integrations/rest-api.md) · [Levara](docs/integrations/levara.md)

## Testing

```bash
cargo test                                         # full suite
cargo test mcp::tests::                            # one module
cargo test -- mcp::tests::write_file_rejects_     # one test
```

Manual end-to-end checklist for MCP behavior: [docs/testing.md](docs/testing.md).

## Status

Phase 7 (hardening). Storage, commit, ACL, MCP, retrieval, indexing, backup, migration are implemented and covered by tests. The eval pipeline (SQuAD-2.0 e2e) and adversarial suites live in the [planning repo](https://github.com/stek0v/memoryfs-planning) and are not bundled here — this repo is the deployable subset.

## License

MIT — see [LICENSE](LICENSE).
