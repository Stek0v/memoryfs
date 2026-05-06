# Memory patch — run_01HZK4M5J8K1M4N7P0Q3R6S9T2

Состояние памяти до и после run. Формат: per-memory action + краткая дельта.

## Создано

### + mem_01HZK4M9F2K3M5N7P9Q1S3T5V7 (auto-committed)

```yaml
memory_type: preference
scope: agent
scope_id: agent:coder
sensitivity: normal
confidence: 0.88
provenance.source_file: conversations/2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md
provenance.source_span.conv_turn_index: 4
```

Reason for auto-commit:

- sensitivity == normal
- confidence (0.88) >= 0.7
- scope == agent (allowed for auto)
- no conflict (memoryfs_search returned 0 items)

### + mem_01HZK4M4D7F9H1J3K5M7N9P1Q3 (auto-committed)

```yaml
memory_type: constraint
scope: project
scope_id: project:memoryfs
sensitivity: normal
confidence: 0.95
provenance.source_file: decisions/0001-vector-store-choice.md
```

Reason for auto-commit: same predicates as above, scope=project allowed.

## Предложено

### ? prp_01HZK4M8X1Y2Z3A4B5C6D7E8F9 → mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0 (pending)

```yaml
memory_type: preference
scope: user
scope_id: user:alice
sensitivity: pii          # ← вынуждает review
confidence: 0.92
provenance.source_file: conversations/2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md
provenance.source_span.conv_turn_index: 2
review_required_reason:
  - sensitivity
review_assigned_to: ["user:alice"]
expires_at: 2026-05-07T09:13:58Z   # +168h по policy
```

## Не предложено (rejected at planner stage)

- API-ключ из turn 0 — поглощён pre-redaction'ом, маркер `[REDACTED:api_key]` не попал
  в кандидатов. (Если бы попал — fail-closed бы заблокировал запись секрета.)
