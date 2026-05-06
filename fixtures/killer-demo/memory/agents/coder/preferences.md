---
schema_version: memoryfs/v1
id: mem_01HZK4M9F2K3M5N7P9Q1S3T5V7
type: memory
memory_type: preference
scope: agent
scope_id: agent:coder
created_at: "2026-04-30T09:15:03.881Z"
author: agent:architect
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["agent:coder", "user:alice"]
  review: ["user:alice"]
provenance:
  source_file: conversations/2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md
  source_commit: 8f3a6e9c2b1d4a5e7f8c9b0d1a2e3f4c5b6a7d8e9f0c1b2a3d4e5f6a7b8c9d0e
  source_span:
    conv_turn_index: 4
    heading_path: ["Preferences for the coder agent"]
  run_id: run_01HZK4M5J8K1M4N7P0Q3R6S9T2
  extractor: agent:architect
  extracted_at: "2026-04-30T09:14:01.230Z"
  model_version: anthropic/claude-opus-4-7
  prompt_hash: 7a9f3c2e8b5d4f6a1c0e9b8d7c6a5f4e3b2d1c0a9f8e7d6c5b4a3f2e1d0c9b8a
confidence: 0.88
status: active
sensitivity: normal
entities:
  - id: ent_01HZK4M0M3N6P9Q2R5S8T1V4W7
    role: project
tags: [tooling, ci, style]
review_status: not_required
---

# Coder agent — рабочие правила

## Перед коммитом

- `cargo fmt --all` + `cargo clippy --all-targets -- -D warnings` для Rust-частей.
- `ruff check --fix` + `pyright` для Python-частей.
- Все unit-тесты должны проходить локально (`cargo test`, `pytest -x`).

## Стиль кода

- Без panic / unwrap в production-путях; ошибки через `Result<_, MemoryFsError>`.
- Логирование структурированное (`tracing`/`structlog`), без `println!`/`print()`.
- Тестовые fixtures лежат в `fixtures/killer-demo/` — не плодить новые без обсуждения.

## Memory hygiene

- Любая запись в чужой scope (`scope=user|org`) требует `propose_memory_patch`, не `remember`.
- Новые памяти всегда с `confidence` и `provenance.source_span`. Никаких голословных fact'ов.
- При обнаружении конфликта — `supersede_memory` с `conflict_type=update|contradiction`, не молчаливая перезапись.

> Auto-committed: scope=agent, sensitivity=normal, confidence=0.88 ≥ threshold 0.6.
