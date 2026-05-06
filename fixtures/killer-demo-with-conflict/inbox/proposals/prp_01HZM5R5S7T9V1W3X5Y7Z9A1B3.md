---
schema_version: memoryfs/v1
id: prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3
type: proposal
proposal_status: pending_review
proposed_by: agent:architect
proposed_at: "2026-05-15T14:30:11.770Z"
review_required_reason:
  - sensitivity
  - conflict
review_assigned_to:
  - user:alice
expires_at: "2026-05-22T14:30:11.770Z"
permissions:
  read: ["user:alice", "agent:architect"]
  write: ["agent:architect"]
  review: ["user:alice"]
tags: [supersede, ide, pending]
conflict_with:
  - mem_01HZM2A4B6C8D0E2F4G6H8J0K2
proposed_memory:
  schema_version: memoryfs/v1
  type: memory
  memory_type: preference
  scope: user
  scope_id: user:alice
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
    extracted_at: "2026-05-15T14:30:11.770Z"
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
---

# Pending: supersede IDE preference

## Сравнение

| Поле | Старое (target) | Новое (proposed) |
| ------ | ----------------- | ------------------ |
| editor | Neovim | Cursor (with vim-mode) |
| AI completion | n/a | actively used |
| LSP stack | rust-analyzer, pyright | rust-analyzer, pyright (unchanged) |
| Multiplexer | tmux + tmuxp | tmux + tmuxp (unchanged) |
| Fallback | n/a | Neovim (for SSH) |
| confidence | 0.86 | 0.91 |
| created_at | 2026-03-15 | 2026-05-15 |

## Reason от агента

> Two-week trial confirmed switch from Neovim to Cursor; vim ergonomics preserved
> via vim-mode.

## Lock

`mem_01HZM2A4B6C8D0E2F4G6H8J0K2` находится в состоянии `target_locked_pending_review`
до завершения этого review. Нельзя предложить ещё один supersede на тот же target.

## Действия для user:alice

```bash
memoryfs review prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3 --decision approved
```

При approve система выполнит атомарно:

1. Создаст `mem_01HZM5P3Q5R7S9T1V3W5X7Y9Z1` в `memory/users/alice/ide-preference-current.md`.
2. Обновит `mem_01HZM2A4B6C8D0E2F4G6H8J0K2`: status=superseded, superseded_by=[new].
3. Создаст один коммит с двумя изменёнными файлами.
4. Снимет target_locked_pending_review.
