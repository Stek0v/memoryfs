---
schema_version: memoryfs/v1
id: prp_01HZK4M8X1Y2Z3A4B5C6D7E8F9
type: proposal
proposal_status: pending_review
proposed_by: agent:architect
proposed_at: "2026-04-30T09:13:58.044Z"
review_required_reason:
  - sensitivity
review_assigned_to:
  - user:alice
expires_at: "2026-05-07T09:13:58.044Z"
permissions:
  read: ["user:alice", "agent:architect"]
  write: ["agent:architect"]
  review: ["user:alice"]
tags: [extraction, profile, pending]
proposed_memory:
  schema_version: memoryfs/v1
  type: memory
  memory_type: preference
  scope: user
  scope_id: user:alice
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
---

# Proposal: profile of Alice

Pending review (sensitivity=pii).

## Predлагаемое содержимое

Alice — staff engineer, базируется в Амстердаме, лидирует MemoryFS.

- Языки: Rust (систем), TypeScript (UI/proto), Python (workers).
- Стиль: TDD по умолчанию, ADR для решений, два approver'а на security PR.
- Часы: 09:30–18:30 CET, фокус-блоки до 12:00 без встреч.

## Почему review

- `sensitivity == pii` → policy.review.sensitive_requires_review.
- `confidence` (0.92) выше threshold, но это не отменяет требование review для pii.

## Reviewer'у

Если содержимое корректно — `memoryfs review prp_01HZK4M8X1Y2Z3A4B5C6D7E8F9 --decision approved`.
Если нужны правки — `--decision needs_changes --reason "..."`.
Если отказать — `--decision rejected --reason "..."` (в audit фиксируется причина).
