# Fixtures

Тестовые workspace'ы, которые служат golden-данными для разработки и регрессионных тестов.

## Состав

| Fixture | Что демонстрирует | Базовый коммит |
| --------- | ------------------- | ---------------- |
| [`killer-demo/`](./killer-demo/) | end-to-end happy-path: conversation → extraction → ADR + 3 memory (одна в review, две auto-committed); pre-redaction; provenance | 9b4c7e0d… |
| [`killer-demo-with-conflict/`](./killer-demo-with-conflict/) | supersede lifecycle: старая память (Neovim) → новая (Cursor) с conflict_type=contradiction; target_locked_pending_review; атомарный апдейт двух файлов | 5e6f7081… |

Каждый fixture снабжён директорией `expected/` с golden-данными для тестов:

| Файл | Назначение |
| ------ | ----------- |
| `killer-demo/expected/acl_matrix.json` | 27 case-of-truth для ACL-проверок: subject × path × action → allowed/denied. Включает path-traversal, unicode-homoglyph, anonymous, cross-user изоляцию. |
| `killer-demo/expected/retrieval_queries.yaml` | 12 запросов с ожидаемым top-1, must_include, must_exclude. Считаемые метрики: recall@5/10, NDCG@10, precision@5, MRR. |
| `killer-demo-with-conflict/expected/supersede_invariants.yaml` | 4 структурных инварианта (двусторонняя связь, DAG, conflict_type-симметрия), 3 финальных состояния (pending/approved/rejected), 3 retrieval-запроса, 3 adversarial-проверки (LOCKED, SUPERSEDE_CYCLE, self-supersede). |

## killer-demo

Реалистичный workspace с одной end-to-end историей: Alice и agent:architect обсуждают
выбор vector-store, agent извлекает три памяти (одна — pii в review, две — auto-committed),
формализует решение в ADR-0001.

Назначение fixture:

- **schema validation** — все файлы должны валидироваться против `specs/schemas/v1/*.json`.
- **provenance roundtrip** — каждая memory должна резолвиться в свой source_file и source_span.
- **acl golden** — тесты ACL прогоняют known subjects (`user:alice`, `agent:coder`,
  `agent:reviewer`, `agent:architect`, `agent:unknown`) через каждый путь и сверяют
  allowed/denied с `expected/acl_matrix.json` (TBD на Phase 0.5).
- **extraction eval** — gold standard для precision/recall extraction'а: зная conversation,
  каких памятей мы ждём.
- **retrieval eval** — query → expected paths/memories: см. `expected/retrieval_queries.yaml` (TBD).
- **commit graph** — при загрузке fixture тесты пересоздают коммиты по той же временной
  последовательности.

## Структура

```text
killer-demo/
├── .memoryfs/
│   └── policy.yaml                                       # workspace policy
├── memory/
│   ├── users/alice/profile.md                            # mem_...Y0  (pii, approved)
│   ├── agents/coder/preferences.md                       # mem_...V7  (auto)
│   └── projects/memoryfs/vector-store-choice.md          # mem_...Q3  (auto)
├── conversations/
│   └── 2026/04/30/conv_01HZK4M2A5B8C0D2E5F8G1H4J7.md     # source, redacted=true
├── decisions/
│   └── 0001-vector-store-choice.md                       # dec_...X0  (accepted)
├── runs/
│   └── run_01HZK4M5J8K1M4N7P0Q3R6S9T2/
│       ├── index.md
│       ├── prompt.md
│       ├── tool_calls.jsonl
│       ├── result.md
│       └── memory_patch.md
├── entities/
│   ├── people/alice.md                                   # ent_...J7
│   └── projects/memoryfs.md                              # ent_...W7
└── inbox/
    └── proposals/prp_01HZK4M8X1Y2Z3A4B5C6D7E8F9.md      # pending review
```

## Plausible IDs (fixed for reproducibility)

| Тип | ID |
| ----- | ---- |
| Workspace | `ws_01HZK4M0Z3Y6X9W2V5N8T1S4R7` |
| Conversation | `conv_01HZK4M2A5B8C0D2E5F8G1H4J7` |
| Run | `run_01HZK4M5J8K1M4N7P0Q3R6S9T2` |
| ADR | `dec_01HZK4M3M6N9P2Q5R8S1T4V7X0` |
| Profile memory (pending) | `mem_01HZK4M7N5P8Q9R3T6V8W2X5Y0` |
| Coder preferences | `mem_01HZK4M9F2K3M5N7P9Q1S3T5V7` |
| Vector store memory | `mem_01HZK4M4D7F9H1J3K5M7N9P1Q3` |
| Pending proposal | `prp_01HZK4M8X1Y2Z3A4B5C6D7E8F9` |
| Entity Alice | `ent_01HZK4M0A3B6C9D2E5F8G1H4J7` |
| Entity MemoryFS | `ent_01HZK4M0M3N6P9Q2R5S8T1V4W7` |

> Все ID валидны по Crockford-Base32 (алфавит без I, L, O, U).

## Статус валидации

На дату 2026-04-30 все 9 файлов фронтматтера + `policy.yaml` проходят валидацию
против `specs/schemas/v1/*.schema.json` без ошибок (Draft 2020-12, разрешение `$ref`
через `referencing` registry). Это проверяется CI-задачей `just validate-fixtures`
(Phase 0).

## Использование

```bash
# Загрузить fixture в локальный сервер
memoryfs workspace import \
  --from fixtures/killer-demo/ \
  --workspace-id ws_01HZK4M0Z3Y6X9W2V5N8T1S4R7

# Запустить schema validation
just validate-fixtures

# Запустить retrieval eval против fixture
just eval-retrieval --fixture killer-demo
```

## Что демонстрирует fixture

1. **Markdown = truth.** Удалить любой derived-индекс — всё восстановится из markdown.
2. **Append-only.** ADR ссылается на conversation, conversation на run, run на proposal —
   ни один файл никогда не перезаписывает другой.
3. **Provenance.** Любую память можно проследить до конкретного turn в исходной conversation.
4. **Pre-redaction в действии.** API-ключ в turn 0 заменён на `[REDACTED:api_key]`,
   `redaction_summary.secrets_redacted == 1`.
5. **Review-by-default.** PII память не была auto-committed — попала в `inbox/proposals/`.
6. **Auto-commit для безопасных категорий.** Coder preferences и project constraint
   попали сразу в active.
7. **Граф сущностей.** Alice → OWNS → MemoryFS, Alice → WROTE → ADR-0001.
8. **Reproducibility.** prompt_hash зафиксирован в provenance — promo же запуск даст тот же prompt.

## Расширение fixture'а

Не плодить новые fixtures без обсуждения. Когда нужны новые сценарии:

- **Конфликт памятей** — `fixtures/killer-demo-with-conflict/` (Phase 1.6 task).
- **Multi-tenant ACL** — `fixtures/multi-tenant/` (Phase 5).
- **Migration v1 → v2** — `fixtures/migration-v1-v2/` (Phase 0 task).

Каждый новый fixture требует `expected/` директории с golden ожиданиями.
