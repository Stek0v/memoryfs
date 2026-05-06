---
schema_version: memoryfs/v1
id: mem_01HZM2A4B6C8D0E2F4G6H8J0K2
type: memory
memory_type: preference
scope: user
scope_id: user:alice
created_at: "2026-03-15T10:22:14.510Z"
updated_at: "2026-05-15T14:31:08.221Z"
author: agent:architect
permissions:
  read: ["user:alice", "agent:architect"]
  write: ["user:alice"]
  review: ["user:alice"]
provenance:
  source_file: conversations/2026/03/15/conv_01HZM2A0B2C4D6E8F0G2H4J6K8.md
  source_commit: 1a2b3c4d5e6f7081920a1b2c3d4e5f60718293a4b5c6d7e8f9012345678901a2
  source_span:
    conv_turn_index: 1
    heading_path: ["IDE setup"]
  run_id: run_01HZM2C5D7E9F1G3H5J7K9M1N3
  extractor: agent:architect
  extracted_at: "2026-03-15T10:22:00.331Z"
  model_version: anthropic/claude-opus-4-7
  prompt_hash: 1c0e9b8d7c6a5f4e3b2d1c0a9f8e7d6c5b4a3f2e1d0c9b8a7a9f3c2e8b5d4f6a
confidence: 0.86
status: superseded
sensitivity: pii
superseded_by:
  - mem_01HZM5P3Q5R7S9T1V3W5X7Y9Z1
conflict_type: contradiction
entities:
  - id: ent_01HZM1A3B5C7D9E1F3G5H7J9K1
    role: subject
tags: [ide, tooling, workflow]
review_status: approved
reviewed_by: user:alice
reviewed_at: "2026-03-15T10:35:42.119Z"
---

# Alice — IDE preference (SUPERSEDED 2026-05-15)

> Эта память была актуальна с 2026-03-15 по 2026-05-15. Заменена памятью
> `mem_01HZM5P3Q5R7S9T1V3W5X7Y9Z1` после смены IDE.

Alice использует **Neovim** в качестве основного редактора:

- LSP через `nvim-lspconfig`, `rust-analyzer` для Rust, `pyright` для Python.
- Plugin manager — `lazy.nvim`.
- Не использует JetBrains/VSCode — считает, что vim-моторика быстрее на её workflow.
- Терминал tmux + tmuxp для проектных сессий.

## История

- 2026-03-15 — записано впервые, после установки нового лаптопа.
- 2026-05-15 — superseded после двух недель работы с Cursor; см. новую память.
