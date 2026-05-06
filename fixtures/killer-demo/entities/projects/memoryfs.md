---
schema_version: memoryfs/v1
id: ent_01HZK4M0M3N6P9Q2R5S8T1V4W7
type: entity
entity_kind: project
canonical_name: MemoryFS
aliases:
  - memoryfs
  - mfs
created_at: "2026-04-30T08:55:00.000Z"
author: user:alice
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["user:alice", "agent:architect"]
provenance:
  source_file: entities/projects/memoryfs.md
  source_commit: 4d2e6a8b1c3f5e7a9b0d2c4e6f8a1b3d5c7e9f0a2b4d6c8e0f1a3b5d7c9e1f3a
  extracted_at: "2026-04-30T08:55:00.000Z"
  extractor: user:alice
sensitivity: normal
scope: org
scope_id: org:memoryfs
external_refs:
  - system: github
    id: memoryfs/memoryfs
attributes:
  description: Verifiable memory workspace for AI agents
  status: active
  primary_language: Rust
  vector_store: Qdrant
  license: Apache-2.0
---

# MemoryFS

Гибрид markdown-as-truth и mem0-style memory intelligence. См. `decisions/0001-*` для
ключевого решения по retrieval backend.

## Активные памяти проекта

- `mem_01HZK4M4D7F9H1J3K5M7N9P1Q3` — vector store choice (constraint).

## Активные ADR

- `dec_01HZK4M3M6N9P2Q5R8S1T4V7X0` — ADR-0001 Qdrant baseline.
