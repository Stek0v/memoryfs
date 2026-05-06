---
schema_version: memoryfs/v1
id: mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0
type: memory
memory_type: preference
scope: user
scope_id: user:alice
created_at: "2026-04-30T09:14:22.317Z"
author: agent:architect
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["user:alice"]
  review: ["user:alice"]
provenance:
  source_file: conversations/2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md
  source_commit: 8f3a6e9c2b1d4a5e7f8c9b0d1a2e3f4c5b6a7d8e9f0c1b2a3d4e5f6a7b8c9d0e
  source_span:
    conv_turn_index: 2
    heading_path: ["About me"]
  run_id: run_01HZK4M5J8K1M4N7P0Q3R6S9T2
  extractor: agent:architect
  extracted_at: "2026-04-30T09:13:58.044Z"
  model_version: anthropic/claude-opus-4-7
  prompt_hash: 7a9f3c2e8b5d4f6a1c0e9b8d7c6a5f4e3b2d1c0a9f8e7d6c5b4a3f2e1d0c9b8a
confidence: 0.92
status: active
sensitivity: pii
entities:
  - id: ent_01HZK4M0A3B6C9D2E5F8G1H4J7
    role: subject
tags: [profile, languages, location, role]
review_status: approved
reviewed_by: user:alice
reviewed_at: "2026-04-30T09:18:11.902Z"
---

# Профиль Alice — рабочие предпочтения

Alice — staff engineer, базируется в Амстердаме, текущий проект — MemoryFS.

## Языки и стек

- Системный код — Rust. Любит сильный тайпчекер, `cargo clippy -- -D warnings` в CI.
- Frontend / прототипы — TypeScript, никогда plain JS.
- Скрипты и worker'ы — Python 3.11+.

## Стиль работы

- TDD по умолчанию для core-логики; для UI — visual review допустим.
- Code review требует минимум двух approvers для security-чувствительных PR.
- Документирование решений через ADR (`decisions/000N-<slug>.md`).

## Расписание

- Будни 09:30–18:30 CET, фокус-блоки до 12:00 без встреч.
- Ревью PR — после 16:00.

> Извлечено агентом `architect` из разговора `conv_01HZK4M2A5B8C0D2E5F8G1H4J7`,
> turn 2 ("About me"). Потребовался review (sensitivity=pii).
