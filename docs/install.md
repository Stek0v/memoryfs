# Install & connect

> 🇷🇺 [Русская версия](install.ru.md)

## Prerequisites

- Rust 1.88+ (`rustup install 1.88`)
- A C linker (Xcode CLT on macOS, build-essential on Debian/Ubuntu) — `tonic-build` compiles protobufs at build time
- Optional: a running [Levara](https://github.com/Stek0v/Levara) cluster for vector retrieval. Without it the binary still works — local `recall` falls back to substring search.

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

You can override any of those via env vars before launching the agent:

```bash
export MEMORYFS_DATA_DIR=/custom/path
export MEMORYFS_WORKSPACE_ID=ws_my_workspace
export MEMORYFS_TOKEN=utk_alice
```

## Wire it into Cursor or other MCP clients

Any MCP-aware client works. The transport is JSON-RPC 2.0 over stdio. Example `mcp.json`:

```json
{
  "mcpServers": {
    "memoryfs": {
      "command": "memoryfs",
      "args": ["mcp"]
    }
  }
}
```

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

## Multi-tenant deployment

For shared deployments, do NOT use local-mode. Provide an explicit `policy.yaml` (schema in `specs/schemas/v1/policy.schema.json`) and JWT-based tokens. See [`examples/policy.local.yaml`](../examples/policy.local.yaml) for the local-mode shape; the multi-tenant equivalent uses subject-scoped allow rules with explicit deny paths.

## Verify the install

After connecting, ask the agent: *"Что есть в моей памяти по этому проекту?"* — it should call `memoryfs_log` and return either entries or an empty list, never an ACL error. If it reports `no allow rule for read on **`, you're on a build older than the local-mode fix; update with `cargo install --git ... --locked --force`.
