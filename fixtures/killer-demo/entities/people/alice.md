---
schema_version: memoryfs/v1
id: ent_01HZK4M0A3B6C9D2E5F8G1H4J7
type: entity
entity_kind: person
canonical_name: Alice
aliases:
  - alice
  - "@alice"
created_at: "2026-04-30T08:55:00.000Z"
author: user:alice
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["user:alice"]
provenance:
  source_file: entities/people/alice.md
  source_commit: 4d2e6a8b1c3f5e7a9b0d2c4e6f8a1b3d5c7e9f0a2b4d6c8e0f1a3b5d7c9e1f3a
  extracted_at: "2026-04-30T08:55:00.000Z"
  extractor: user:alice
sensitivity: pii
scope: org
scope_id: org:memoryfs
external_refs:
  - system: github
    id: alice-eng
attributes:
  role: staff_engineer
  location: Amsterdam
  timezone: Europe/Amsterdam
  primary_languages: [Rust, TypeScript, Python]
---

# Alice

Staff engineer, lead of MemoryFS project. Subject of `mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0`
(profile memory).

## Связи (актуальные)

- `OWNS` → `ent_01HZK4M0M3N6P9Q2R5S8T1V4W7` (project: MemoryFS)
- `WROTE` → `dec_01HZK4M3M6N9P2Q5R8S1T4V7X0` (ADR-0001)
- `WROTE` → `conv_01HZK4M2A5B8C0D2E5F8G1H4J7` (conversation)
