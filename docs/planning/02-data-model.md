# 02 — Модель данных

## 1. Принципы моделирования

1. **Frontmatter = строгая схема, тело = свободный Markdown.** Frontmatter валидируется JSON-Schema, тело — только sanitized.
2. **Все ID — ULID** (`01J...`). Монотонные, K-сортируемые, легко парсить time component.
3. **`schema_version` в каждом файле.** Без неё миграции невозможны.
4. **Provenance — обязательный объект**, не опциональный. Без provenance файл не валиден.
5. **Append-only через `supersedes` / `superseded_by`.** Удаление — только через policy и архивацию.
6. **Entity references — ULID, а не имена.** Имена меняются, ID — нет.

## 2. ID-схемы

| Префикс | Тип | Пример |
| --------- | ----- | -------- |
| `mem_` | Memory record | `mem_01J9YQK7T5...` |
| `conv_` | Conversation | `conv_01J9YQ...` |
| `run_` | Agent run | `run_01J9YQ...` |
| `ent_` | Entity | `ent_01J9YQ...` |
| `dec_` | Decision (ADR-style internal) | `dec_01J9YQ...` |
| `ws_` | Workspace | `ws_01J9YQ...` |
| `evt_` | Event log entry | `evt_01J9YQ...` |
| `commit_` | Commit hash (SHA-256, hex16) | `commit_9f31ab2c4d7e8901` |

## 3. Базовая схема frontmatter

Общие обязательные поля для **любого** файла в workspace:

```yaml
schema_version: 1
id: <prefixed_ULID>
type: <enum>
created_at: <ISO-8601 with TZ>
updated_at: <ISO-8601 with TZ>
author:
  subject: "agent:valeria"   # или user:stek0v
  agent_run: run_01J...      # если author — agent
permissions:
  read: ["owner"]
  write: ["owner"]
provenance:
  source_type: <enum: conversation|run|user_input|migration|import>
  source_ref: <path-or-id>
  commit: <commit_hash | null on first write>
  derived_from: [<id>, ...]   # other memories/files
tags: [<string>, ...]
```

## 4. Тип: Memory

`type: memory`, путь: `/memory/<scope>/<...>.md`

```yaml
schema_version: 1
id: mem_01J9YQK7T5C8YHRKE6TF1H1Z9C
type: memory
memory_type: preference         # preference|fact|goal|skill|relationship|constraint|episodic
scope: user                     # user|agent|session|project|org
scope_id: stek0v                # ID внутри скоупа
created_at: 2026-04-30T12:20:00+02:00
updated_at: 2026-04-30T12:20:00+02:00
author:
  subject: "agent:valeria"
  agent_run: run_01J9YQ...
permissions:
  read: ["owner", "agent:valeria"]
  write: ["owner"]
provenance:
  source_type: conversation
  source_ref: /conversations/2026/04/30/conv_01J9....md
  source_span: { lines: [42, 47] }
  commit: 9f31ab2c4d7e8901
  derived_from: []
confidence: 0.86                # [0.0, 1.0]
status: active                  # active|superseded|archived|disputed
supersedes: []                  # list of memory IDs
superseded_by: null
conflict_type: null             # only if supersedes != []
entities: [ent_01J..., ent_01J...]
tags: ["preference", "devops", "local-llm"]
review:
  required: false
  reviewer: null
  reviewed_at: null
  decision: null                # approved|rejected|deferred
expires_at: null                # для эпизодической памяти
sensitivity: normal             # normal|pii|secret|medical|legal|financial
```

**Тело** — короткое утверждение в первом-третьем лице, не диалог:

```md
User prefers local-first AI infrastructure and uses an RTX 3090 for heavy inference tasks.
```

### 4.1 `memory_type` — таксономия

| Тип | Пример | Время жизни |
| ----- | -------- | ------------- |
| `preference` | "Prefers Vim over VS Code" | долгая |
| `fact` | "Lives in Amsterdam" | долгая |
| `goal` | "Hitting 5k MRR by Q3" | средняя, истекает |
| `skill` | "Strong in Rust, learning Zig" | долгая |
| `relationship` | "Reports to Alice" | средняя |
| `constraint` | "No cloud LLMs for medical data" | долгая, критичная |
| `episodic` | "Discussed Qdrant choice on 2026-04-30" | короткая |

## 5. Тип: Conversation

`type: conversation`, путь: `/conversations/YYYY/MM/DD/<conv_id>.md`

```yaml
schema_version: 1
id: conv_01J9YQ...
type: conversation
created_at: 2026-04-30T12:00:00+02:00
updated_at: 2026-04-30T12:35:00+02:00
participants:
  - "user:stek0v"
  - "agent:valeria"
session_id: ses_01J9...
agent_run: run_01J9...
permissions:
  read: ["user:stek0v", "agent:valeria"]
  write: ["agent:valeria"]
provenance:
  source_type: user_input
  source_ref: cli
redacted: true                  # true если применялся redactor
redaction_summary:
  api_key: 0
  email: 1
  phone: 0
extraction:
  status: completed             # pending|in_progress|completed|failed
  worker_run: run_01J9...
  proposals: [mem_01J9..., mem_01J9...]
tags: ["devops", "memory-design"]
```

**Тело** — Markdown с турнамерками:

```md
### Turn 1 — user
...

### Turn 2 — agent:valeria
...
```

## 6. Тип: Run

`type: run`, путь: `/runs/<run_id>/`

`metadata.md`:

```yaml
schema_version: 1
id: run_01J9YQ...
type: run
agent: "agent:valeria"
started_at: 2026-04-30T12:20:00+02:00
finished_at: 2026-04-30T12:21:14+02:00
status: completed               # pending|running|completed|failed|aborted
trigger:
  source_type: conversation
  source_ref: conv_01J9...
permissions:
  read: ["agent:valeria", "user:stek0v"]
  write: ["agent:valeria"]
artifacts:
  - prompt.md
  - tool_calls.md
  - stdout.md
  - stderr.md
  - result.md
  - memory_patch.md
metrics:
  duration_ms: 74320
  tokens_in: 4123
  tokens_out: 891
  cost_usd: 0.0234
  llm_model: "claude-opus-4-7"
errors: []
proposed_memories: [mem_01J9..., mem_01J9...]
```

`memory_patch.md` — описание предлагаемых изменений памяти, со ссылками на `mem_*` ID,
которые либо ушли в auto-commit, либо в review-очередь.

## 7. Тип: Decision (ADR-style)

`type: decision`, путь: `/decisions/adr-<NNNN>-<slug>.md`

```yaml
schema_version: 1
id: dec_01J9YQ...
type: decision
adr_number: 12
title: "Use Qdrant as vector store"
status: accepted                # proposed|accepted|deprecated|superseded
created_at: 2026-04-30T12:00:00+02:00
deciders: ["user:stek0v"]
context_refs: [conv_01J9..., run_01J9...]
supersedes: []
superseded_by: null
permissions:
  read: ["group:devs"]
  write: ["user:stek0v"]
tags: ["adr", "vector-db"]
```

**Тело** — стандарт ADR:

```md
## Context
...
## Decision
...
## Consequences
...
## Alternatives considered
...
```

## 8. Тип: Entity

`type: entity`, путь: `/entities/<kind>/<entity_id>.md`

```yaml
schema_version: 1
id: ent_01J9YQ...
type: entity
entity_kind: project            # person|project|tool|concept|site|org
canonical_name: "Picoclaw"
aliases: ["picoclaw", "pico-claw", "PicoClaw"]
created_at: ...
updated_at: ...
permissions:
  read: ["group:devs"]
  write: ["user:stek0v"]
attributes:
  domain: "multi-agent dev"
  status: active
tags: ["project"]
```

**Тело** — свободное описание сущности на Markdown, человекочитаемое.

## 9. Тип: Memory Proposal (inbox)

`type: memory_proposal`, путь: `/inbox/proposals/<ULID>.md`

Структура почти идентична `memory`, но `status: pending_review`. После одобрения файл
перемещается в финальный путь и `status: active`, frontmatter дополняется reviewer-полями.

## 10. Event log — формат

Append-only NDJSON (одно событие на строку):

```json
{
  "evt_id": "evt_01J9YQ...",
  "ts": "2026-04-30T12:20:00.123+02:00",
  "kind": "conversation.append|memory.proposed|memory.committed|memory.superseded|workspace.commit|workspace.revert|review.decided|index.reindex_started|index.reindex_completed",
  "workspace_id": "ws_01J...",
  "subject": "agent:valeria",
  "trace_id": "abc123",
  "payload": { "...": "schema-specific" }
}
```

Свойства:

- offset-индекс (бинарный) рядом с файлом для быстрого seek;
- ротация по размеру (например, 256 МБ → новый segment);
- compaction только для `index.*` событий, остальные — навсегда;
- хеш-цепочка: каждое событие включает hash предыдущего (опционально, опция `tamper_evident: true` в `.memoryfs/config.yaml`).

## 11. Граф — модель

Простая edges-таблица (MVP):

```sql
CREATE TABLE entities (
    id          TEXT PRIMARY KEY,         -- ent_01J...
    workspace_id TEXT NOT NULL,
    kind        TEXT NOT NULL,
    canonical_name TEXT NOT NULL,
    file_path   TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL
);

CREATE TABLE edges (
    id          TEXT PRIMARY KEY,         -- edge_01J...
    workspace_id TEXT NOT NULL,
    src_id      TEXT NOT NULL,            -- entity / memory / run / decision
    src_kind    TEXT NOT NULL,
    dst_id      TEXT NOT NULL,
    dst_kind    TEXT NOT NULL,
    relation    TEXT NOT NULL,            -- prefers|wrote|derived_from|mentions|supersedes|...
    weight      REAL DEFAULT 1.0,
    provenance_commit TEXT,
    created_at  TIMESTAMPTZ NOT NULL
);

CREATE INDEX idx_edges_src ON edges(src_id, relation);
CREATE INDEX idx_edges_dst ON edges(dst_id, relation);
CREATE INDEX idx_edges_ws  ON edges(workspace_id);
```

Реляции (controlled vocabulary, расширяемая через config):

```text
PREFERS, AVOIDS, USES, KNOWS, OWNS, MEMBER_OF,
WROTE, REVIEWED, DERIVED_FROM, MENTIONS, REFERENCES,
SUPERSEDES, CONFLICTS_WITH, RELATES_TO,
PRODUCED, CONSUMED
```

## 12. Vector index — метаданные чанка

Каждая запись в Qdrant/pgvector:

```json
{
  "id": "<chunk_uuid>",
  "vector": [...],
  "payload": {
    "workspace_id": "ws_01J...",
    "file_path": "/memory/users/stek0v/preferences.md",
    "file_id": "mem_01J...",
    "file_type": "memory",
    "memory_type": "preference",
    "scope": "user",
    "scope_id": "stek0v",
    "heading_path": ["root", "Tools"],
    "chunk_index": 0,
    "char_start": 0,
    "char_end": 312,
    "commit": "9f31ab2c4d7e8901",
    "embedding_model": "text-embedding-3-small",
    "embedding_version": 1,
    "permissions_read": ["owner", "agent:valeria"],
    "tags": ["preference", "devops"],
    "indexed_at": "2026-04-30T12:21:00Z"
  }
}
```

`permissions_read` дублируется в payload для filter-time ACL, но **финальный ACL-чек** делается
на API-слое после deterministic read — payload может устареть.

## 13. BM25 index — поля документа

Tantivy (или эквивалент):

```text
id              TEXT  STORED
workspace_id    TEXT  INDEXED FAST
file_path       TEXT  STORED
file_type       TEXT  INDEXED
scope           TEXT  INDEXED
scope_id        TEXT  INDEXED
title           TEXT  INDEXED  (boost: 2.0)
heading         TEXT  INDEXED  (boost: 1.5)
body            TEXT  INDEXED
tags            TEXT  INDEXED FAST
commit          TEXT  STORED
indexed_at      DATE  INDEXED
```

## 14. Метаданные DB (SQLite/Postgres)

```sql
CREATE TABLE workspaces (
    id           TEXT PRIMARY KEY,
    root_path    TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL,
    config_hash  TEXT NOT NULL
);

CREATE TABLE files (
    workspace_id TEXT NOT NULL,
    path         TEXT NOT NULL,
    file_id      TEXT NOT NULL,             -- mem_01J... etc
    type         TEXT NOT NULL,
    object_hash  TEXT NOT NULL,
    last_commit  TEXT NOT NULL,
    schema_version INT NOT NULL,
    permissions_hash TEXT NOT NULL,
    PRIMARY KEY (workspace_id, path)
);

CREATE TABLE commits (
    hash         TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    parent       TEXT,
    author       TEXT NOT NULL,
    message      TEXT,
    created_at   TIMESTAMPTZ NOT NULL,
    tree_hash    TEXT NOT NULL
);

CREATE INDEX idx_files_id  ON files(file_id);
CREATE INDEX idx_commits_ws ON commits(workspace_id, created_at DESC);
```

## 15. Schema versioning

- Каждое изменение схемы — bump `schema_version` (major).
- Миграционный runner на старте: для всех файлов с `schema_version < current` запускает соответствующий migration step.
- Миграция = новый коммит с `commit.message: "migrate schema 1 → 2"` и `author: "system:migration"`.
- Отсутствие миграции для нового мажора = старт сервиса блокируется.

## 16. Sensitivity и redaction

Поле `sensitivity` определяет:

- какие redaction-правила применяются;
- требуется ли review;
- кто в default permissions;
- попадает ли в default-shared контекст.

| `sensitivity` | Read default | Review | Index | Notes |
| --------------- | -------------- | -------- | ------- | ------- |
| `normal` | `["owner", "agent:*"]` (workspace-level) | no | yes | стандарт |
| `pii` | `["owner"]` | yes | yes (encrypted-at-rest) | redact в логах |
| `secret` | `["owner"]` | yes | **no** (или only-hash) | автоматический block в proposals |
| `medical` | `["owner"]` | yes | yes (audit on each read) | спец. policy |
| `legal` | `["owner", "group:legal"]` | yes | yes | |
| `financial` | `["owner", "group:finance"]` | yes | yes | |

## 17. Примеры supersede и conflict

**Memory v1** (старая):

```yaml
id: mem_001
status: superseded
superseded_by: mem_002
```

```md
User prefers VS Code for coding.
```

**Memory v2** (новая):

```yaml
id: mem_002
status: active
supersedes: [mem_001]
conflict_type: preference_changed
provenance:
  source_type: conversation
  source_ref: conv_01J9X...
  commit: a1b2c3...
```

```md
User now prefers Cursor with Claude Code over VS Code.
```

Поле `conflict_type` ∈ {`preference_changed`, `fact_corrected`, `goal_dropped`, `skill_evolved`,
`relationship_changed`, `disambiguation`, `merge`}.

## 18. Anti-patterns в данных (запрещено)

- Изменять frontmatter без bump `updated_at` и без коммита.
- Удалять файл из workspace вне `archive` flow.
- Хранить секреты в теле памяти (только зашифрованный reference, либо отказ).
- Использовать имена в edges вместо ID.
- Создавать память без `provenance.source_type`.
- Создавать `supersedes` cycle (валидируется при write).
