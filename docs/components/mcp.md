# `mcp` — MCP server

Source: [`src/mcp.rs`](../../src/mcp.rs), [`src/mcp_instructions.md`](../../src/mcp_instructions.md)

## Transport

JSON-RPC 2.0 over stdio. Each line on stdin is one message; each response is one line on stdout. Implemented in `McpServer::run_stdio()`. The Claude Code / Cursor / generic MCP host launches the binary as a child process and pipes both ends.

## `initialize` handshake

The server returns:

```json
{
  "protocolVersion": "2024-11-05",
  "capabilities": { "tools": { "listChanged": false } },
  "serverInfo": { "name": "memoryfs", "version": "..." },
  "instructions": "<full behavioral contract>"
}
```

The `instructions` field is the load-bearing piece — clients (Claude Code does this) inject it into the model's system prompt. It tells the agent **when** to use which tool. Source: [`src/mcp_instructions.md`](../../src/mcp_instructions.md). Edit there, recompile, the contract ships.

The contract enforces:

- **Recall-first**: before any architectural recommendation, call `memoryfs_recall`.
- **When to save without asking**: explicit user decision → save silently; root cause found → save as discovery; new infra → save as fact; etc.
- **Path conventions**: one record per file, predictable layout (`decisions/<slug>.md` etc.).
- **Findability**: include searchable terms in body content (recall is over embeddings, not slugs).
- **Supersede, never overwrite**.
- **Don't save**: code paths, conversation play-by-play, TodoWrite-style task progress.

## 17 tools

Manifest matches [`specs/mcp.tools.json`](../../specs/mcp.tools.json):

| Tool | Purpose |
|------|---------|
| `memoryfs_read_file`, `memoryfs_write_file`, `memoryfs_list_files` | File CRUD |
| `memoryfs_commit`, `memoryfs_revert`, `memoryfs_log`, `memoryfs_diff` | Commit ops |
| `memoryfs_search`, `memoryfs_recall` | Search / semantic recall |
| `memoryfs_remember`, `memoryfs_propose_memory_patch`, `memoryfs_review_memory`, `memoryfs_supersede_memory` | Memory lifecycle |
| `memoryfs_link_entity`, `memoryfs_get_provenance` | Entity graph |
| `memoryfs_create_run`, `memoryfs_finish_run` | Agent run lifecycle |

## Append-only guard

Plain `memoryfs_write_file` to a path under `decisions/` or `discoveries/` is **rejected** if the path already exists with different content:

```json
{ "code": -32602, "message": ".../db-choice.md already exists and is append-only. Decisions and discoveries preserve their audit trail — use memoryfs_supersede_memory to record the new version. For a typo-only fix, retry with force=true." }
```

Idempotent rewrites (same content) succeed silently. `force=true` exists as the typo-fix escape hatch. Other prefixes (`infra/`, `events/`, `preferences/`, `facts/`) are mutable in place.

This pairs with the `instructions` contract: the model is told to use `supersede`; the server enforces what the model might forget.

## Auth

`McpAuth::from_env()` reads `MEMORYFS_TOKEN` and `MEMORYFS_WORKSPACE_ID`. Tokens prefixed `utk_` mean a user subject; `atk_` means an agent subject. Local-mode bootstrap (in `cmd_mcp` in `src/main.rs`) auto-sets both before constructing the auth context, so nothing manual is required.

## Adding a tool

1. Append a `ToolDef` to `build_tool_manifest()` in `src/mcp.rs` with name, description, JSON-Schema for input.
2. Add the dispatch arm in `handle_tools_call`.
3. Implement `tool_xxx` — call `acl::check` first, then operate on `state`.
4. Mirror in [`specs/mcp.tools.json`](../../specs/mcp.tools.json) so external clients see the contract.
5. Cover with a test in `mcp::tests`.

The tool count assertion (`tool_manifest_has_17_tools`) catches forgotten manifest entries.
