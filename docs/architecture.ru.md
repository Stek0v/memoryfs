# Архитектура

> 🇬🇧 [English version](architecture.md)

MemoryFS — один Rust-бинарь, владеющий persistent project memory store, и две поверхности (REST и MCP), которые делят общий access control, audit и retrieval.

## Высокоуровневая схема

```
┌─────────────────────────────────────────────────────────────────┐
│  Агент (Claude Code, Cursor, кастомный)                         │
└────────────┬───────────────────────────────────┬────────────────┘
             │ JSON-RPC по stdio                │ HTTP
             ▼                                  ▼
┌──────────────────────┐            ┌──────────────────────┐
│  mcp::McpServer      │            │  api::router (axum)  │
│  17 tools            │            │  REST v1             │
└──────────┬───────────┘            └──────────┬───────────┘
           │                                   │
           └───────────┬───────────────────────┘
                       ▼
              ┌────────────────┐
              │ acl::check     │  policy + audit gate
              └───────┬────────┘
                      │
       ┌──────────────┼─────────────┬──────────────┐
       ▼              ▼             ▼              ▼
  ┌─────────┐   ┌──────────┐  ┌──────────┐   ┌──────────┐
  │ storage │   │ commit   │  │ supersede│   │retrieval │
  │ objects │   │ DAG      │  │ replace  │   │ vec+BM25 │
  │ + inode │   │ + diff   │  │ + audit  │   │ + RRF    │
  └────┬────┘   └────┬─────┘  └────┬─────┘   └────┬─────┘
       │             │             │              │
       └────────┬────┴─────────────┴──────────────┘
                ▼
        ┌──────────────┐         ┌─────────────────┐
        │ event_log    │ ──────▶ │ indexer worker  │
        │ NDJSON шина  │         │ chunk → embed   │
        └──────────────┘         │ → upsert        │
                                 └────────┬────────┘
                                          ▼
                                 ┌─────────────────┐
                                 │ Levara / Qdrant │
                                 │ + Tantivy BM25  │
                                 └─────────────────┘
```

## Модель данных

Каждая память — `.md` с YAML frontmatter, валидируется по `specs/schemas/v1/memory.schema.json`. ID — типизированные обёртки над ULID с префиксами (`mem_`, `dec_`, `ent_`, `run_`, `evt_`, `prp_`, `conv_`, `ws_`): `RunId` нельзя передать туда, где ждут `MemoryId`.

Storage — content-addressable: `objects/<sha256>` хранит байты, `inode_index` мапит `path → текущий sha256`, `commit_log` — DAG, в каждом коммите parent + snapshot path→hash. Та же форма, что внутри git: бесплатная дедупликация, бесплатная проверка целостности (path → hash → bytes round-trip), бесплатный time travel.

## Инвариант источника истины

Если `objects/`, `inode_index`, BM25-индекс или vector store разъехались — путь восстановления один: **сканируем `decisions/`, `discoveries/`, `infra/` и т.д. с диска → пересобираем всё остальное.** Ни один индекс не канон.

## Две поверхности, один gate

Каждая операция — REST или MCP — проходит через `acl::check(subject, action, path, policy)` до того как тронет хранилище. Проверка по path-glob (`memory/user/**`, `decisions/*.md`), deny-by-default. Local single-user mode использует `Policy::local_user(subject)` — текущему юзеру `**`; multi-tenant грузит политику из `.memory/policy.yaml` (схема — `specs/schemas/v1/policy.schema.json`).

Каждая проверка, каждая запись, каждый supersede пишет запись в `audit_log` — append-only NDJSON с tamper-evident hash chain (`audit.rs`).

## Поведенческий контракт

В `initialize` MCP-сервера есть поле `instructions` — markdown-контракт, который говорит агенту **когда** дёргать какой инструмент: recall-first перед рекомендацией, save-без-вопроса при явном решении, supersede вместо overwrite. Источник: [`src/mcp_instructions.md`](../src/mcp_instructions.md).

Два append-only пути (`decisions/`, `discoveries/`) защищены на сервере: обычный `write_file` поверх существующего файла в этих префиксах будет отклонён с ошибкой, указывающей на `memoryfs_supersede_memory`. Параметр `force=true` — escape hatch для опечаток.

## Retrieval pipeline

`/v1/context` (REST) и `memoryfs_recall` (MCP) оба идут через `retrieval::RetrievalEngine`:

1. Параллельно vector search (Levara / Qdrant) и BM25 (Tantivy).
2. Reciprocal Rank Fusion объединяет два рейтинга.
3. Опционально расширение через entity graph (`entity_score`).
4. Recency boost — экспоненциальный decay по `created_at` из frontmatter.
5. ACL post-filter — каждый кандидат re-checked под subject вызывающего перед возвратом.
6. Финальные хиты читаются с диска детерминистично (`objects/<hash>`, не из кешированного chunk text) — ответ воспроизводим.

## Indexing pipeline

Запись никогда не блокируется индексацией. `event_log` — append-only NDJSON шина с consumer offsets; indexer worker её таилит, chunks новые файлы (`chunker.rs` — heading-aware с overlap, опциональный `document_title` prepend), embed через `embedder::Embedder`, upsert в vector backend. `MemoryId::from_path` выводит детерминистичный ULID из path — re-indexing идемпотентен, старые chunks для path заменяются, а не накапливаются.

## Карта модулей

| Модуль | Ответственность |
|--------|-----------------|
| `ids` | Типизированные ULID-обёртки, `CommitHash` |
| `error` | Унифицированный `MemoryFsError` + маппинг в HTTP/MCP |
| `schema` | Frontmatter parser + JSON Schema validator |
| `storage` | Content-addressable object store + inode index |
| `commit` | Commit DAG, diff, revert |
| `acl` | Path-glob policy engine |
| `policy` | Типы workspace policy + loader |
| `redaction` | Детект секретов (20+ паттернов) + redaction |
| `audit` | Tamper-evident NDJSON лог |
| `event_log` | Append-only event bus + consumer offsets |
| `embedder` | `Embedder` trait + `HttpEmbedder` |
| `vector_store` | `VectorStore` trait + Qdrant impl |
| `levara` | Основной backend: vector + embed + hybrid через gRPC |
| `bm25` | Tantivy full-text |
| `chunker` | Heading-aware markdown splitter |
| `indexer` | Event-driven worker |
| `reindex` | Полный rebuild с checkpoint |
| `retrieval` | Multi-signal engine + RRF + ACL post-filter |
| `graph` | Entity graph (CRUD, BFS) |
| `entity_extraction` | NER через LLM + linking |
| `extraction` / `extraction_worker` | Memory extraction из runs |
| `inbox` | Очередь review proposals |
| `memory_policy` | Решения auto-commit / require-review |
| `post_scan` | Пост-extraction скан секретов |
| `supersede` | Memory replacement DAG, детект циклов |
| `runs` | Lifecycle agent runs |
| `mcp` | JSON-RPC 2.0 сервер, 17 tool handlers, `initialize` instructions |
| `api` | REST router (axum) |
| `backup` | Full workspace backup + restore + verify |
| `migration` | Schema migration runner с up/down |
| `observability` | Metrics + tracing middleware |
| `llm` | `LlmClient` trait + OpenAI-compatible client |
