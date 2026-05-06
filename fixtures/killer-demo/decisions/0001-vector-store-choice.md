---
schema_version: memoryfs/v1
id: dec_01HZK4M3M6N9P2Q5R8S1T4V7X0
type: decision
adr_number: 1
title: "Qdrant as baseline vector store for MemoryFS v1.0"
status: accepted
deciders:
  - user:alice
  - agent:architect
created_at: "2026-04-30T09:20:11.450Z"
decided_at: "2026-04-30T09:22:31.117Z"
author: agent:architect
permissions:
  read: ["user:alice", "agent:coder", "agent:reviewer", "agent:architect"]
  write: ["user:alice", "agent:architect"]
  review: ["user:alice"]
tags: [retrieval, vector-store, hybrid-search]
scope: project
scope_id: project:memoryfs
context_refs:
  - kind: file
    ref: conversations/2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md
  - kind: memory
    ref: mem_01HZK4M4D7F9H1J3K5M7N9P1Q3
  - kind: run
    ref: run_01HZK4M5J8K1M4N7P0Q3R6S9T2
consequences_summary: >
  Qdrant принят как baseline. LanceDB остаётся viable плагин для offline. Дополнительная
  инфра-зависимость (отдельный сервис) принята осознанно ради hybrid+payload-filter.
---

# ADR-0001: Qdrant as baseline vector store for MemoryFS v1.0

- **Status:** Accepted
- **Date:** 2026-04-30
- **Deciders:** user:alice, agent:architect
- **Source conversation:** `conversations/2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md`

## Context

MemoryFS retrieval требует:

1. Hybrid поиск — dense (embedding) + sparse (BM25) + payload filtering, объединённые в один
   ранжированный список.
2. Богатый payload-фильтр по полям `workspace_id`, `scope`, `scope_id`, `sensitivity`,
   `commit_hash`, `author`. Эти фильтры применяются на retrieval-time и затем ещё раз
   post-retrieval ACL — производительность фильтрации критична.
3. Поддержку двух режимов:
   - **local single-user** — embedded, нулевая инфра-зависимость.
   - **team-server** — кластерный, replication, persistence.
4. Reproducible recall: запрос вида "что было активным на коммите X" должен сводиться к payload-фильтру.

Рассмотренные альтернативы:

- **pgvector** — единая БД для metadata + vector упрощает ops; но sparse/BM25 — extension,
  не first-class; payload filtering при росте набора деградирует. Подходит до ~50k chunks,
  слабее выше.
- **LanceDB** — embedded, single-file, минимальный ops-овод; сильный ANN; слабее на
  filter-heavy запросах; hybrid собирается вручную; cluster-режим незрелый.
- **Qdrant** — hybrid из коробки; payload index покрывает наши нужды; embedded mode
  закрывает local; cluster — team. Cost: отдельный сервис в деплое.

## Decision

Qdrant выбран baseline для v1.0. Конкретно:

- Local-mode — Qdrant embedded (in-process через FFI-биндинги, без отдельного процесса).
- Team-mode — Qdrant как отдельный сервис в docker-compose / k8s.
- Каждая memory chunk пишется в Qdrant с payload `{workspace_id, memory_id, commit_hash,
  scope, scope_id, sensitivity, author, tags}`.
- Фильтр `sensitivity != "secret"` применяется на индексации (для secret вообще не кладём).
- Hybrid score: dense (cosine) + sparse (BM25 от Tantivy, отдельный сервис) + graph score
  (Postgres) → линейная комбинация в Workspace Engine.

## Consequences

### Положительные

- Один движок закрывает hybrid + payload, минимум собственной фьюжн-логики.
- Проверенный production-grade движок для обеих аудиторий (local + team).
- Reproducible recall сводится к стандартному payload-фильтру.

### Отрицательные / trade-off

- Дополнительная инфра-зависимость в team-режиме. Принято осознанно — embedded mode
  снимает нагрузку для local пользователей.
- Не self-contained как LanceDB. Принято — для self-contained сценариев планируем
  LanceDB как plug-in (см. "Открытые вопросы").
- Sparse-сигнал реализуется отдельно (Tantivy), а не нативно в Qdrant. Снижает
  атомарность обновления индекса; компенсируется eventual consistency и lag-метрикой.

### Нейтральные

- Embedding-провайдер абстрагирован за интерфейсом — возможна смена без касания Qdrant.
- Миграции Qdrant collection (например, при смене embedding-модели) выполняются через
  `/admin/reindex` с background-задачей.

## Открытые вопросы (post-decision)

- **OQ-1 (Phase 3):** LanceDB как plug-in для offline-workspaces — приоритет уточнить
  после первого user-research пакета. Не блокирует v1.0.
- **OQ-2 (Phase 5):** Atomicity миграции collection при смене embedding-модели —
  требуется либо blue/green collections, либо downtime-окно. Решить в Phase 5.

## Связанные памяти

- `mem_01HZK4M4D7F9H1J3K5M7N9P1Q3` — короткая выдержка для retrieval.
