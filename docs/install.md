# Install & connect

> 🇷🇺 [Русская версия](install.ru.md)

## TL;DR — full-stack one-liner

MemoryFS + Levara (vector backend) + Ollama (embedder) + MCP wired into Claude Code:

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked --force && (docker ps -a --format '{{.Names}}' | grep -q '^levara$' || docker run -d --name levara -p 50051:50051 -p 8080:8080 ghcr.io/stek0v/levara:latest) && (command -v ollama >/dev/null || curl -fsSL https://ollama.com/install.sh | sh) && ollama pull nomic-embed-text && claude mcp add memoryfs --scope user --env LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051 -- memoryfs mcp
```

What it does, step by step:

1. `cargo install` — builds and drops `memoryfs` into `~/.cargo/bin/`. `--force` lets you re-run safely to upgrade.
2. `docker run … levara` — starts Levara on `:50051` (gRPC) and `:8080` (HTTP). Skipped if a container named `levara` already exists.
3. `ollama install + pull` — installs Ollama (skipped if already present), downloads `nomic-embed-text` (~274 MB, 768-dim) — matches MemoryFS's defaults, no env vars needed.
4. `claude mcp add … --env LEVARA_GRPC_ENDPOINT=…` — registers `memoryfs mcp` globally for Claude Code; the env var wires Levara directly into MCP, so every commit auto-indexes to vectors and recall returns semantic hits, not substring matches.

Prerequisites: Rust 1.88+ (`rustup install 1.88`), Docker, the [Claude Code CLI](https://docs.claude.com/en/docs/claude-code).

> **What the env var does:** without `LEVARA_GRPC_ENDPOINT`, MCP runs in pure-file mode (substring recall, no vectors). With it set, MCP spins up a Levara client on startup, fires a background indexer after every commit, and recall goes through hybrid vector+BM25. Same binary, same workflow — just a richer recall path. Details in [`docs/integrations/levara.md`](integrations/levara.md).

For a minimal install (just MCP, skip Levara + Ollama):

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked && claude mcp add memoryfs --scope user -- memoryfs mcp
```

## Prerequisites

- Rust 1.88+ (`rustup install 1.88`)
- A C linker (Xcode CLT on macOS, build-essential on Debian/Ubuntu) — `tonic-build` compiles protobufs at build time
- Optional: a running [Levara](https://github.com/Stek0v/Levara) instance for semantic search. Used by both `memoryfs serve` (REST) and `memoryfs mcp` (when `LEVARA_GRPC_ENDPOINT` is set). Without it, both modes fall back to substring recall on the file tree. Full setup in [`docs/integrations/levara.md`](integrations/levara.md).

## Build & install

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked
```

The binary lands in `~/.cargo/bin/memoryfs`. Verify:

```bash
memoryfs --version
```

## Wire it into Claude Code (recommended)

Register globally — every project in every workspace gets memory automatically:

```bash
claude mcp add memoryfs --scope user -- memoryfs mcp
```

What happens on first use in a project:

1. Claude Code launches `memoryfs mcp` as a stdio child process.
2. The binary calls `git rev-parse --show-toplevel` (or falls back to cwd) to find the project root.
3. Creates `<project>/.memory/` if missing.
4. Derives a stable `workspace_id` from the canonical project path (SHA-256 truncated to a ULID).
5. Sets a local-mode subject (`utk_<your-username>`) and grants it full access via `Policy::local_user`.
6. Returns the behavioral contract via `initialize.instructions` — see [`src/mcp_instructions.md`](../src/mcp_instructions.md).
7. If `LEVARA_GRPC_ENDPOINT` is set, builds a Levara-backed retrieval engine + background indexer; recall uses hybrid vector+BM25 and every commit auto-indexes. Otherwise falls back to substring recall over the file tree.

You can override any of those via env vars before launching the agent:

```bash
export MEMORYFS_DATA_DIR=/custom/path
export MEMORYFS_WORKSPACE_ID=ws_my_workspace
export MEMORYFS_TOKEN=utk_alice
export LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051   # opt into semantic recall
```

To wire Levara into an already-registered MCP entry (after the fact):

```bash
claude mcp remove memoryfs --scope user
claude mcp add memoryfs --scope user --env LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051 -- memoryfs mcp
```

## Wire it into Cursor or other MCP clients

Any MCP-aware client works. The transport is JSON-RPC 2.0 over stdio. Example `mcp.json`:

```json
{
  "mcpServers": {
    "memoryfs": {
      "command": "memoryfs",
      "args": ["mcp"],
      "env": {
        "LEVARA_GRPC_ENDPOINT": "http://127.0.0.1:50051"
      }
    }
  }
}
```

Drop the `env` block to run pure-file mode (substring recall, no Levara).

## REST mode

Run a long-lived REST server (useful for non-MCP integrations or remote access):

```bash
memoryfs serve --bind 127.0.0.1:7777 \
  --data-dir ~/projects/foo/.memory
```

OpenAPI spec: [`specs/openapi.yaml`](../specs/openapi.yaml). Health check:

```bash
curl http://127.0.0.1:7777/v1/health
```

If you want semantic search (vector + BM25 hybrid) instead of substring fallback, point `memoryfs serve` at a [Levara](https://github.com/Stek0v/Levara) instance — full setup, env vars, and troubleshooting in [`docs/integrations/levara.md`](integrations/levara.md):

```bash
export LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051
memoryfs serve --bind 127.0.0.1:7777 --data-dir ~/projects/foo/.memory
```

## Multi-tenant deployment

For shared deployments, do NOT use local-mode. Provide an explicit `policy.yaml` (schema in `specs/schemas/v1/policy.schema.json`) and JWT-based tokens. See [`examples/policy.local.yaml`](../examples/policy.local.yaml) for the local-mode shape; the multi-tenant equivalent uses subject-scoped allow rules with explicit deny paths.

## Verify the install

After connecting, ask the agent: *"Что есть в моей памяти по этому проекту?"* — it should call `memoryfs_log` and return either entries or an empty list, never an ACL error. If it reports `no allow rule for read on **`, you're on a build older than the local-mode fix; update with `cargo install --git ... --locked --force`.
