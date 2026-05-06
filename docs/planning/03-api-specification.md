# 03 — Спецификация API

## 1. Общие правила

- **Транспорты**: REST/JSON (HTTP/1.1 + HTTP/2), MCP (stdio + sse), CLI поверх REST.
- **Версия API** в URL: `/v1/...`. Следующая мажорка — `/v2/...`, обе живут параллельно во время миграции.
- **Аутентификация**: Bearer token в `Authorization: Bearer <token>`. Токены — два типа:
  `user_token` и `agent_token`, разделены префиксом (`utk_*`, `atk_*`).
- **Workspace scope**: явно через header `X-Workspace-Id: ws_01J...` или через path-сегмент
  в admin-API. Отсутствие → ошибка 400.
- **Idempotency**: write-эндпоинты принимают `Idempotency-Key` header, кешируют ответ 24 часа.
- **Pagination**: cursor-based. `?cursor=<opaque>&limit=<int, default 50, max 500>`. Ответ — `{items, next_cursor}`.
- **Time**: все времена ISO-8601 с TZ. Сервер всегда возвращает UTC, принимает любые TZ.
- **Errors**: стандартизированный формат (см. §10).
- **Rate limiting**: per-token, заголовки `X-RateLimit-*` в ответе.
- **Request size**: 10 МБ per body, 64 МБ для bulk.

## 2. Аутентификация и токены

```http
POST /v1/auth/token
Content-Type: application/json

{
  "subject": "agent:valeria",
  "scopes": ["read", "propose", "commit"],
  "ttl_seconds": 86400
}
```

Ответ:

```json
{ "token": "atk_01J...", "expires_at": "2026-05-01T12:00:00Z" }
```

Token revocation: `DELETE /v1/auth/token/{token_id}`.

## 3. Workspace endpoints

### `POST /v1/workspaces`

Создать workspace. Тело:

```json
{ "name": "personal-ai", "owner_subject": "user:stek0v" }
```

### `GET /v1/workspaces/{id}`

Метаданные и состояние.

### `POST /v1/workspaces/{id}/commit`

```json
{
  "message": "Add user local-LLM preferences",
  "paths": ["/memory/users/stek0v/preferences.md"],
  "author_subject": "agent:valeria"
}
```

Ответ: `{ "commit": "9f31ab2c4d7e8901" }`.

### `POST /v1/workspaces/{id}/revert`

```json
{ "commit": "9f31ab2c4d7e8901", "message": "revert: PII leak" }
```

Ответ: `{ "commit": "<new_commit>" }`.

### `GET /v1/workspaces/{id}/log`

Параметры: `path?`, `limit`, `cursor`. Возвращает commit-объекты.

### `GET /v1/workspaces/{id}/diff?from=<c1>&to=<c2>`

Diff между двумя коммитами; default `to=HEAD`.

## 4. File endpoints

### `GET /v1/files?path=/memory/users/stek0v/preferences.md`

Возвращает:

```json
{
  "path": "...",
  "file_id": "mem_01J...",
  "type": "memory",
  "frontmatter": { "...": "..." },
  "body": "User prefers...",
  "object_hash": "sha256:...",
  "last_commit": "9f31ab..."
}
```

### `PUT /v1/files`

```json
{
  "path": "/memory/users/stek0v/preferences.md",
  "frontmatter": { "...": "..." },
  "body": "...",
  "auto_commit": false
}
```

Если `auto_commit: false`, файл записывается, но коммит не создаётся (нужен отдельный `/commit`).

### `DELETE /v1/files`

Запрещено напрямую. Возвращает `405 Method Not Allowed` со ссылкой на `/v1/files/archive`.

### `POST /v1/files/archive`

Перемещает файл в `/archive/...` с маркером `status: archived` в frontmatter.

### `GET /v1/files/list?path=/memory/users/&recursive=true`

Listing с ACL-фильтрацией.

## 5. Memory endpoints

### `POST /v1/memory/propose`

Предложить память (используется extraction-воркером).

```json
{
  "workspace_id": "ws_01J...",
  "proposals": [
    {
      "memory_type": "preference",
      "scope": "user",
      "scope_id": "stek0v",
      "text": "User prefers local-first AI infrastructure.",
      "entities": [{"name": "RTX 3090", "kind": "tool"}],
      "confidence": 0.91,
      "source": {
        "type": "conversation",
        "ref": "conv_01J...",
        "span": {"lines": [42, 47]}
      },
      "agent_run": "run_01J..."
    }
  ]
}
```

Ответ:

```json
{
  "results": [
    {
      "proposal_id": "mem_01J...",
      "status": "auto_committed | pending_review | rejected",
      "commit": "9f31ab..." | null,
      "review_required_reason": "sensitivity:pii" | null
    }
  ]
}
```

### `GET /v1/memory/{id}`

Полная запись + provenance.

### `GET /v1/memory/search`

Параметры: `query`, `scope`, `scope_id`, `memory_type`, `tags`, `since`, `until`, `min_confidence`.
Ответ:

```json
{
  "items": [
    {
      "memory": { "...": "..." },
      "scores": {
        "vector": 0.82, "bm25": 0.41, "entity": 0.77, "final": 0.86
      },
      "matched_chunks": [{"heading_path": ["..."], "snippet": "..."}]
    }
  ],
  "next_cursor": null
}
```

### `POST /v1/memory/{id}/supersede`

```json
{
  "new_text": "User now prefers ...",
  "conflict_type": "preference_changed",
  "source": {"type": "conversation", "ref": "conv_01J..."},
  "confidence": 0.93
}
```

### `POST /v1/memory/review`

```json
{
  "proposal_id": "mem_01J...",
  "decision": "approved | rejected | deferred",
  "reviewer_subject": "user:stek0v",
  "comment": "OK to commit",
  "edits": { "text": "edited version" }
}
```

### `GET /v1/memory/inbox`

Список pending-review proposals.

## 6. Context (recall) endpoint

### `GET /v1/context`

Параметры:

- `query` (string, required)
- `scopes` (array): `user:stek0v`, `agent:valeria`, `project:picoclaw`
- `limit` (int, default 8, max 50)
- `min_confidence` (float, default 0.5)
- `cite` (bool, default true) — включить ссылки на файлы и коммиты
- `include_body` (bool, default true)
- `time_window` (object): `{since, until}`

Ответ:

```json
{
  "query": "what does Stek0v prefer for local AI?",
  "candidates": [
    {
      "memory_id": "mem_01J...",
      "text": "User prefers local-first AI infrastructure.",
      "source_file": "/memory/users/stek0v/preferences.md",
      "source_commit": "9f31ab2c4d7e8901",
      "confidence": 0.91,
      "retrieval": {
        "vector_score": 0.82,
        "bm25_score": 0.41,
        "entity_score": 0.77,
        "final_score": 0.86,
        "rank": 1
      },
      "permissions_read": ["owner", "agent:valeria"]
    }
  ],
  "trace_id": "abc123"
}
```

## 7. Run endpoints

### `POST /v1/runs`

Зарегистрировать новый run.

```json
{
  "agent": "agent:valeria",
  "trigger": {"source_type": "conversation", "source_ref": "conv_01J..."},
  "metadata": {}
}
```

Возвращает `run_id` + structured paths для записи артефактов.

### `PATCH /v1/runs/{id}`

Дополнить артефактами / финализировать (`status: completed`).

### `GET /v1/runs/{id}`

Полные данные run + список артефактов.

## 8. Entity endpoints

### `POST /v1/entities`

Создать entity. С автоматическим dedupe по `canonical_name + kind` (warn, не fail).

### `GET /v1/entities/{id}`

### `GET /v1/entities/search?q=...&kind=...`

### `POST /v1/entities/{id}/link`

```json
{
  "target_id": "mem_01J...",
  "target_kind": "memory",
  "relation": "MENTIONS",
  "weight": 1.0
}
```

### `GET /v1/entities/{id}/neighbors?relation=PREFERS&depth=2`

## 9. Admin endpoints

- `POST /v1/admin/reindex?scope=...` — полная переиндексация.
- `POST /v1/admin/migrate-schema?to=2` — миграция схем.
- `GET /v1/admin/health` — health probe.
- `GET /v1/admin/metrics` — Prometheus metrics.
- `GET /v1/admin/audit?since=...` — выборка из audit log.

## 10. Формат ошибок

Все ошибки — `application/json`:

```json
{
  "error": {
    "code": "PERMISSION_DENIED",
    "message": "Subject agent:valeria cannot write /memory/users/alice/",
    "trace_id": "abc123",
    "details": { "subject": "agent:valeria", "resource": "/memory/users/alice/" }
  }
}
```

| HTTP | code | Когда |
| ------ | ------ | ------- |
| 400 | `INVALID_REQUEST` | Невалидный JSON / missing field |
| 400 | `SCHEMA_VALIDATION` | Frontmatter не соответствует JSON-Schema |
| 401 | `UNAUTHENTICATED` | Нет токена / просрочен |
| 403 | `PERMISSION_DENIED` | ACL fail |
| 404 | `NOT_FOUND` | File / memory / run не существуют |
| 409 | `CONFLICT` | Concurrent write / supersede cycle |
| 409 | `SUPERSEDE_CYCLE` | Цикл в supersedes |
| 412 | `PRECONDITION_FAILED` | If-Match не совпал |
| 422 | `POLICY_REJECTED` | Memory отвергнут policy engine |
| 422 | `SENSITIVE_REQUIRES_REVIEW` | Не auto-commit, нужен review |
| 423 | `LOCKED` | Workspace в режиме миграции |
| 429 | `RATE_LIMITED` | Превышен лимит |
| 500 | `INTERNAL` | Баг — trace_id обязательно |
| 503 | `WORKER_UNAVAILABLE` | Extraction worker недоступен |

## 11. MCP tools

Имена tools (snake_case, prefix `memoryfs_`):

| Tool | Назначение | Минимальные параметры |
| ------ | ----------- | ---------------------- |
| `memoryfs_read_file` | Прочитать файл по path | `path` |
| `memoryfs_write_file` | Записать файл (без commit) | `path`, `frontmatter`, `body` |
| `memoryfs_list_files` | Listing | `path`, `recursive` |
| `memoryfs_search` | Полнотекстовый/семантический поиск файлов | `query`, `scope` |
| `memoryfs_recall` | Recall с fusion | `query`, `scopes`, `limit` |
| `memoryfs_remember` | Высокоуровневое API: предложить память + опционально commit | `text`, `scope`, `scope_id`, `confidence` |
| `memoryfs_propose_memory_patch` | Предложить набор memory updates | массив proposals |
| `memoryfs_review_memory` | Утвердить / отклонить proposal | `proposal_id`, `decision` |
| `memoryfs_supersede_memory` | Заменить старую память новой | `old_id`, `new_text`, `conflict_type` |
| `memoryfs_commit` | Создать commit | `message`, `paths` |
| `memoryfs_revert` | Revert commit | `commit` |
| `memoryfs_create_run` | Зарегистрировать run | `agent`, `trigger` |
| `memoryfs_finish_run` | Закрыть run + прикрепить артефакты | `run_id`, `artifacts` |
| `memoryfs_link_entity` | Связать entity ↔ memory/run | `src_id`, `dst_id`, `relation` |
| `memoryfs_get_provenance` | Полная цепочка провенанса | `id` |
| `memoryfs_log` | История коммитов | `path?`, `limit` |
| `memoryfs_diff` | Diff между коммитами | `from`, `to` |

### Контракт ответа MCP

Каждый tool возвращает:

```json
{
  "ok": true,
  "data": { ... },
  "trace_id": "abc123",
  "warnings": []
}
```

или:

```json
{
  "ok": false,
  "error": { "code": "...", "message": "...", "trace_id": "..." }
}
```

## 12. CLI команды

CLI — тонкий клиент над REST. Команды:

```text
memoryfs init [--name NAME] [--root PATH]
memoryfs status
memoryfs read PATH
memoryfs write PATH --type memory --scope user --scope-id stek0v ...
memoryfs commit -m "MSG" [PATHS...]
memoryfs revert COMMIT
memoryfs log [--path PATH] [--limit N]
memoryfs diff FROM TO
memoryfs search QUERY [--scope SCOPE]
memoryfs recall QUERY [--scopes ...] [--cite]
memoryfs remember "TEXT" --scope user --scope-id stek0v --confidence 0.9
memoryfs review LIST | APPROVE ID | REJECT ID
memoryfs run start --agent NAME --trigger SRC
memoryfs run finish ID
memoryfs entity create --kind project --name "Picoclaw"
memoryfs entity link SRC DST --relation MENTIONS
memoryfs admin reindex [--scope SCOPE]
memoryfs admin migrate-schema --to N
memoryfs admin health
```

## 13. Примеры рабочих сценариев

### 13.1 Агент предлагает память после диалога

```text
1. POST /v1/runs                                  → run_01J...
2. POST /v1/events {kind:"conversation.append"}   → evt_...
3. (worker async) POST /v1/memory/propose         → mem_01J... pending_review (sensitivity:pii)
4. GET  /v1/memory/inbox                          → ...
5. POST /v1/memory/review {decision:"approved"}   → committed, commit_hash
6. PATCH /v1/runs/run_01J... {status:"completed"}
```

### 13.2 Recall при следующем диалоге

```text
1. GET /v1/context?query=...&scopes=user:stek0v,project:picoclaw
2. → 3 memories с provenance + scores
3. Агент строит ответ, цитируя source_file и commit
```

### 13.3 Откат ошибочной памяти

```text
1. GET /v1/memory/mem_01J...                      → проверка
2. POST /v1/workspaces/ws.../revert {commit: ...}
3. Indexer автоматически удалит соответствующий чанк (event-driven)
```

## 14. Backwards compatibility

- В рамках мажорной версии API: добавление полей — безопасно; переименование/удаление — break.
- Удаляемые поля помечаются `deprecated_at` (header `X-Deprecation-Notice`) ≥ 6 месяцев до удаления.
- Изменения в frontmatter-схеме = bump `schema_version` + миграция.

## 15. OpenAPI и schemas — артефакты

- `openapi.yaml` — машиночитаемая спецификация (генерируется из кода или поддерживается вручную с тестом drift).
- `schemas/memory.schema.json`, `schemas/conversation.schema.json` и т.д. — JSON-Schema для frontmatter.
- `mcp.tools.json` — манифест MCP tools.

Все три артефакта обязательны до начала Phase 2.
