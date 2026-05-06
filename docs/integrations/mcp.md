# MCP Integration

MemoryFS exposes a JSON-RPC 2.0 server over stdio implementing the
Model Context Protocol. This enables Claude Code, Cursor, and other
MCP-capable tools to interact with MemoryFS workspaces directly.

## Setup

```bash
export MEMORYFS_TOKEN="utk_your_token"
export MEMORYFS_WORKSPACE_ID="ws_your_workspace"
memoryfs mcp
```

## Available tools (17)

Full schemas: `specs/mcp.tools.json`

### File operations

| Tool | Description |
|------|-------------|
| `read_file` | Read file content by path |
| `write_file` | Write/update a file (stages for commit) |
| `list_files` | List files with optional prefix filter |

### Search and recall

| Tool | Description |
|------|-------------|
| `search` | Full-text + vector hybrid search |
| `recall` | Context retrieval with multi-signal fusion |

### Memory management

| Tool | Description |
|------|-------------|
| `remember` | Create a new memory (may auto-commit or queue for review) |
| `propose` | Propose a memory for review |
| `review` | Approve or reject a proposed memory |
| `supersede` | Replace one memory with another (creates DAG edge) |

### Commit operations

| Tool | Description |
|------|-------------|
| `commit` | Create a commit from staged changes |
| `revert` | Revert workspace to a prior commit |
| `log` | View commit history |
| `diff` | Compare two commits |

### Entity graph

| Tool | Description |
|------|-------------|
| `link_entity` | Create a relation between two entities |
| `get_provenance` | Trace entity provenance through the graph |

### Agent runs

| Tool | Description |
|------|-------------|
| `create_run` | Start tracking an agent run |
| `finish_run` | Complete a run with status and artifacts |

## Auth

Tokens are passed via `MEMORYFS_TOKEN`. Two types:

- `utk_` — user tokens, full workspace access
- `atk_` — agent tokens, scoped by ACL rules

The MCP server validates tokens on every tool call and enforces ACL
rules based on the token's principal.

## Error handling

MCP errors follow JSON-RPC 2.0 conventions:

| Code | Meaning |
|------|---------|
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32000` | Application error (check `data.code` for API error code) |

Application error codes match the REST API: `NOT_FOUND`, `FORBIDDEN`,
`CONFLICT`, `VALIDATION_ERROR`, etc.
