---
schema_version: memoryfs/v1
id: mem_01HZM5P3Q5R7S9T1V3W5X7Y9Z1
type: memory
memory_type: preference
scope: user
scope_id: user:alice
created_at: "2026-05-15T14:31:08.221Z"
author: agent:architect
permissions:
  read: ["user:alice", "agent:architect"]
  write: ["user:alice"]
  review: ["user:alice"]
provenance:
  source_file: conversations/2026/05/15/conv_01HZM5K0M2N4P6Q8R0S2T4V6X8.md
  source_commit: 5e6f70819203a4b5c6d7e8f9012345678901a2b3c4d5e6f70819203a4b5c6d7e
  source_span:
    conv_turn_index: 3
    heading_path: ["IDE update"]
  run_id: run_01HZM5N4P6Q8R0S2T4V6X8Z0B2
  extractor: agent:architect
  extracted_at: "2026-05-15T14:30:51.044Z"
  model_version: anthropic/claude-opus-4-7
  prompt_hash: 2b3c4d5e6f70819203a4b5c6d7e8f9012345678901a2b3c4d5e6f7081920304a
confidence: 0.91
status: active
sensitivity: pii
supersedes:
  - mem_01HZM2A4B6C8D0E2F4G6H8J0K2
conflict_type: contradiction
entities:
  - id: ent_01HZM1A3B5C7D9E1F3G5H7J9K1
    role: subject
tags: [ide, tooling, workflow]
review_status: approved
reviewed_by: user:alice
reviewed_at: "2026-05-15T14:42:18.770Z"
review_decision_reason: "Подтверждаю, что перешла на Cursor. Старая память про Neovim — устарела."
---

# Alice — IDE preference (ACTIVE 2026-05-15)

> Эта память заменяет `mem_01HZM2A4B6C8D0E2F4G6H8J0K2` (Neovim, до 2026-05-15).
> conflict_type=contradiction — alice сменила инструмент.

Alice использует **Cursor** как основной редактор:

- AI-completion активно используется для прототипирования и refactor.
- Vim-mode включён — мышечная память от Neovim сохраняется.
- LSP-стек тот же (`rust-analyzer`, `pyright`).
- Tmux/tmuxp по-прежнему для проектных сессий.
- Параллельно держит Neovim как fallback для удалённой работы по SSH.

## Что НЕ изменилось (наследуется логически)

- Языковые предпочтения (Rust для систем, TS для UI, Python для workers) — см.
  отдельную память `mem_01HZM2X8Y0Z2A4B6C8D0E2F4G6` (если есть).
- Tmux setup — тот же.

## Что нужно reviewer'у

Если это решение временное (test drive Cursor) — `--decision needs_changes` с
заметкой "review again in 30 days". Если устойчивый switch — approve.
