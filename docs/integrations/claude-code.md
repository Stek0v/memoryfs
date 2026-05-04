# Claude Code integration

## Register

```bash
claude mcp add memoryfs --scope user -- memoryfs mcp
```

`--scope user` means every Claude Code session, in every project, on this machine, gets the server. Use `--scope project` if you want it only in one project (creates `.mcp.json` at project root).

Verify:

```bash
claude mcp list
```

You should see `memoryfs ✓ connected`.

## What appears in the project

First time the agent calls any `memoryfs_*` tool in a project, the server creates:

```
<project>/.memory/
├── objects/
├── audit_log
├── event_log/
└── inode_index
```

Add to `.gitignore` if you want memory local-only, or commit it if the team should share decisions. Both work — content-addressable storage handles cross-machine merges naturally (no metadata, no machine-specific paths inside).

## Behavioral contract

Delivered via the MCP `initialize.instructions` field. The agent gets:

- **Recall-first**: before any architectural recommendation, call `memoryfs_recall`.
- Save without asking on explicit decisions, root causes, new infra, milestones, preferences, deadlines.
- Path conventions: `decisions/<slug>.md`, `discoveries/<slug>.md`, `infra/<topic>.md`, `events/YYYY-MM-DD-<slug>.md`, `preferences/<topic>.md`.
- `memoryfs_supersede_memory` instead of overwriting decisions/discoveries.
- Don't save: code paths, conversation play-by-play, TodoWrite-style task progress.

Source: [`src/mcp_instructions.md`](../../src/mcp_instructions.md). Edit there to tweak.

## Override env

The auto-bootstrap detects everything from the project, but each value can be overridden:

```bash
export MEMORYFS_DATA_DIR=/custom/path
export MEMORYFS_WORKSPACE_ID=ws_my_workspace
export MEMORYFS_TOKEN=utk_alice
```

Set these before launching `claude` to inject a different identity or data dir.

## Troubleshooting

- **`recall` returns "no allow rule for read on **"** — old binary, missing the local-mode policy fix. Update with `cargo install --git ... --locked --force`.
- **`memoryfs_log` is empty after writes** — check the agent is actually calling `memoryfs_commit` after `memoryfs_write_file` (writes are staged until commit).
- **MCP connection fails on launch** — check `claude mcp` reports "connected"; if not, run `memoryfs mcp` directly in a shell and look for stderr output (the binary prints `memoryfs mcp: project=... workspace=... data=...` on startup).
