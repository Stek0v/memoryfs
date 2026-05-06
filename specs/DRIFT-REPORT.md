# Drift report: prose ⇄ schemas

> Сгенерирован `scripts/crosscheck_data_model.py` на дату 2026-04-30.
> Целевое состояние: 0 drift. Текущее: 37 в 6 примерах фронтматтера.
> Источник истины — схемы в `specs/schemas/v1/`. Дрейф устраняется правкой прозы,
> не схем (если только обнаружится баг в схеме).

## TL;DR

`02-data-model.md` написан на ранней стадии, до того как схемы были стабилизированы.
Использует устаревшую структуру:

| Поле | В прозе | В схеме v1 (правильно) |
| ------ | --------- | ------------------------ |
| `schema_version` | `1` (число) | `"memoryfs/v1"` (строка по pattern) |
| `id` | `mem_01J...` (плейсхолдер) | полный ULID 26 chars Crockford-Base32 |
| `author` | объект `{subject, agent_run}` | строка вида `agent:valeria` |
| `created_at` / `updated_at` | ISO-8601 с `+02:00` | строго UTC с суффиксом `Z` |
| `provenance.source_type` + `source_ref` | объединено в одно поле | разделено: `source_file`, `provenance.run_id`, `provenance.extractor` |
| `provenance` обязательные поля | `source_type`, `source_ref` | `source_file`, `source_commit`, `extracted_at` |
| `entities` | массив строк `[ent_01J...]` | массив объектов `[{id, role}]` |
| `artifacts` (run) | массив имён файлов | объект с типизированными полями (`prompt`, `tool_calls`, ...) |
| `context_refs` (decision) | массив строк | массив объектов `[{kind, ref}]` |
| `redaction_summary.api_key` | свободные счётчики | enum в `categories` + типизированные поля |
| `proposed_memories` | список `mem_*` | список `prp_*` (proposals, не memories) |
| `conflict_type` | `null` или строка | enum включает `"none"`, `null` запрещён |
| `metrics` (run) | `llm_model`, `tokens_in`, `tokens_out` | `model` (отдельно), `tokens_input`, `tokens_output` |

## Полный список нарушений

### Block #1 (untyped — фрагмент базовой схемы)

YAML parse error: содержит литеральные плейсхолдеры `<enum: ...|...>` без кавычек, что
ломает парсер. Это пример-как-документация, не реальный YAML. Решение: либо обернуть
в кавычки, либо сменить codeblock на ` ```text ` чтобы кросс-чекер пропустил.

### Block #2 (memory) — 17 drifts

- `additional_props`: `permissions.review` — поле было в схеме legacy, в v1 называется `review`.
  Оказывается, схема разрешает `review` только если он есть в основной структуре `permissions`.
  Проверить: схема `base.schema.json#/$defs/permissions` имеет `review` — OK. Это
  ложное срабатывание из-за `additionalProperties: false` на уровне всего объекта в прозе.
- `author`: должен быть строкой `agent:valeria`, не `{subject, agent_run}`. agent_run переезжает
  в `provenance.run_id`.
- `created_at`/`updated_at`: TZ должен быть `Z`, не `+02:00`.
- `entities`: должны быть объекты `{id: ent_..., role: "..."}`, не строки.
- `provenance` лишние поля: `commit`, `derived_from`, `source_ref`, `source_type`. Заменить на
  `source_file`, `source_commit`, `source_span` (опц), `run_id` (опц).
- `conflict_type: None` → должно быть `"none"` или поле опущено.

### Block #3 (conversation) — 10 drifts

- `agent_run`, `extraction`, `provenance` как root-level — не в схеме. Должно: `extraction_status`,
  `extraction_run_id`, `extracted_memories` (без обёртки `extraction:`).
- `author` обязателен, в прозе отсутствует.
- `id` плейсхолдер `conv_01J9YQ...` слишком короткий — нужен полный 26-char ULID.
- `redaction_summary.api_key` и т.п. — должны быть в `categories: [api_key, ...]`.
- `schema_version: 1` → `"memoryfs/v1"`.

### Block #4 (run) — 14 drifts

- `errors` корневой — заменить на `error: {...}` (одна ошибка).
- `author` обязателен.
- `artifacts` — объект, не массив. Поля `prompt`, `tool_calls`, `stdout`, `stderr`, `result`,
  `memory_patch` с путями вида `runs/<run_id>/<file>`.
- `metrics`: `llm_model` → `model` (на уровень run), `tokens_in/out` → `tokens_input/output`.
- `proposed_memories` — это `prp_*` (proposals в inbox), не `mem_*`. После approve будут `mem_*`.

### Block #5 (decision) — 8 drifts

- `author` обязателен; `decided_at` обязателен при `status: accepted`.
- `context_refs` — объекты `{kind: "file"|"memory"|"run"|"url", ref: "..."}`, не строки.
- `superseded_by: null` — должно быть пустым массивом `[]` либо опущено.

### Block #6 (entity) — 5 drifts

- `author` обязателен.
- `created_at`/`updated_at` плейсхолдер `'...'` — нужно реальное datetime в UTC.
- `id` плейсхолдер слишком короткий.
- `schema_version: 1` → `"memoryfs/v1"`.

## План устранения

Один PR `prose-sync-with-v1`:

1. Заменить везде `schema_version: 1` → `schema_version: memoryfs/v1`.
2. Все примеры `id:` дополнить до полных 26-char ULID (можно использовать те же,
   что и в killer-demo fixture для консистентности).
3. `author:` сделать строкой формата `subject:identifier`.
4. Все timestamps — UTC c `Z`.
5. `provenance` — переписать пример со всеми обязательными полями: `source_file`,
   `source_commit`, `extracted_at`. `source_ref` и `source_type` удалить.
6. `entities` — объекты с `id` и опциональным `role`.
7. `run.artifacts` — объект, `metrics` — типизированный.
8. `decision.context_refs` — объекты `{kind, ref}`.
9. `conversation.redaction_summary` — структура с `categories: [...]`.
10. Обернуть базовый шаблон фронтматтера в `text` или замэскировать плейсхолдеры,
    чтобы блок не пытался парситься как YAML.

Ожидаемый результат: `python3 scripts/crosscheck_data_model.py` → exit 0.

## Уроки на будущее

1. **Машинные контракты — источник истины.** Каждое расхождение между прозой и схемой
   означает баг в прозе (или баг в схеме, если схема ошибочна). Кросс-чекер запускается
   в CI на каждый PR, изменяющий `02-data-model.md` или `specs/schemas/v1/`.
2. **Yaml-блоки в прозе — это spec-by-example.** Они должны быть валидным YAML и
   валидироваться против тех же схем, что и реальные файлы. Иначе пример обманывает.
3. **Плейсхолдеры в YAML лучше не использовать.** Вместо `id: mem_01J9...` лучше
   `id: mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0  # plausible example` — реальный ID + комментарий.
   Тогда блок parseable и пример непротиворечив.
4. **Pinned plausible IDs.** В fixture'ах и в прозе использовать одни и те же ULID для
   одних и тех же логических сущностей.

## Связь с roadmap

Эта задача — **CC.0.3-prose-sync** в `04-tasks-dod.md` (Phase 0). DoD:
`scripts/crosscheck_data_model.py` exit 0 на все блоки, кросс-чекер встроен в `just validate-all`,
PR-template содержит чек-лист "обновил прозу при изменении schema".
