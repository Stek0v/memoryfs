---
schema_version: memoryfs/v1
id: mem_01HZK4M4D7F9H1J3K5M7N9P1Q3
type: memory
memory_type: constraint
scope: project
scope_id: project:memoryfs
created_at: "2026-04-30T09:22:47.501Z"
author: agent:architect
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["user:alice", "agent:architect"]
  review: ["user:alice"]
provenance:
  source_file: decisions/0001-vector-store-choice.md
  source_commit: 9b4c7e0d3a6f1e2c5b8d1a4f7c0e3b6d9a2f5c8e1b4d7a0c3f6e9b2d5a8c1f4e
  source_span:
    heading_path: ["Decision"]
  run_id: run_01HZK4M5J8K1M4N7P0Q3R6S9T2
  extractor: agent:architect
  extracted_at: "2026-04-30T09:22:31.117Z"
  model_version: anthropic/claude-opus-4-7
  prompt_hash: 7a9f3c2e8b5d4f6a1c0e9b8d7c6a5f4e3b2d1c0a9f8e7d6c5b4a3f2e1d0c9b8a
confidence: 0.95
status: active
sensitivity: normal
entities:
  - id: ent_01HZK4M0M3N6P9Q2R5S8T1V4W7
    role: project
tags: [architecture, vector-store, qdrant, decision-summary]
review_status: not_required
---

# MemoryFS — vector store

**Выбор:** Qdrant как baseline для v1.0.

**Альтернативы рассматривались:** LanceDB, pgvector.

**Ключевые причины:**

1. Hybrid поиск (dense + sparse + filtering) из коробки — критично для signals fusion.
2. Payload filtering по произвольным полям (workspace_id, scope, sensitivity, commit) — нужно для post-retrieval ACL.
3. Cluster-режим для team-server, embedded-режим для local — обе аудитории закрыты одной зависимостью.
4. Активный maintainer-status, gRPC + REST.

**Trade-off, осознанно принятый:**

- Дополнительная инфраструктурная зависимость (отдельный сервис) — приемлемо, потому что есть embedded mode.
- Не self-contained как LanceDB — но LanceDB слабее в hybrid и filter-heavy сценариях.

См. полный ADR в `decisions/0001-vector-store-choice.md`.
