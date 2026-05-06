# Specs

Машинно-валидируемые контракты MemoryFS.

## Содержимое

| Файл | Назначение | Stage |
| ------ | ----------- | ------- |
| [`openapi.yaml`](./openapi.yaml) | REST API v1 (OpenAPI 3.1) | draft |
| [`mcp.tools.json`](./mcp.tools.json) | 17 MCP-tools с inputSchema | draft |
| [`schemas/v1/base.schema.json`](./schemas/v1/base.schema.json) | Общие $defs (ulid, provenance, permissions, sensitivity, …) | draft |
| [`schemas/v1/memory.schema.json`](./schemas/v1/memory.schema.json) | Frontmatter памяти | draft |
| [`schemas/v1/conversation.schema.json`](./schemas/v1/conversation.schema.json) | Frontmatter диалога | draft |
| [`schemas/v1/run.schema.json`](./schemas/v1/run.schema.json) | Frontmatter запуска агента | draft |
| [`schemas/v1/decision.schema.json`](./schemas/v1/decision.schema.json) | Frontmatter ADR | draft |
| [`schemas/v1/entity.schema.json`](./schemas/v1/entity.schema.json) | Frontmatter сущности графа | draft |
| [`schemas/v1/proposal.schema.json`](./schemas/v1/proposal.schema.json) | Frontmatter pending proposal | draft |
| [`schemas/v1/policy.schema.json`](./schemas/v1/policy.schema.json) | `policy.yaml` workspace | draft |
| [`schemas/v1/event.schema.json`](./schemas/v1/event.schema.json) | Запись в NDJSON event/audit log (37 категорий, опционально hash chain) | draft |

## Принципы

1. **Schemas — source of truth для frontmatter.** Любое расхождение между прозой
   в `02-data-model.md` и схемой — баг в прозе.
2. **OpenAPI описывает обёртки запросов/ответов.** Содержимое frontmatter в response
   ссылается на JSON Schemas, а не дублирует их.
3. **Версионирование одно сквозное:** `memoryfs/v1` для всех схем. При major bump
   директория `v2/` появляется параллельно, `v1/` остаётся ≥6 месяцев (deprecation policy).

## Как валидировать

```bash
# Один файл (например, fixture):
ajv validate -s specs/schemas/v1/memory.schema.json \
             -d <(yq '.frontmatter // ."---"' fixtures/killer-demo/memory/users/alice/profile.md)

# Все fixtures:
just validate-fixtures  # → проходит каждый md, парсит фронтматтер, валидирует против схемы по type
```

В CI (Phase 0): `just validate-schemas` запускает sketch-validator на golden-наборе
из `fixtures/` + adversarial-набор из `tests/adversarial/schema-violations/`.

## Что отсутствует и появится позже

- **Edges schema** — пока графовые рёбра хранятся только в metadata DB; если/когда переедут в файлы —
  отдельная схема.
- **CLI-spec** — пока документируется в прозе в `03-api-specification.md`. Вынесем
  отдельным machine-readable контрактом в Phase 6, если появится потребность в
  кросс-language CLI клиентах.
