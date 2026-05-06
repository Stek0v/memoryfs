# killer-demo-with-conflict

Второй fixture, демонстрирующий жизненный цикл памяти через **supersede**: одна и та же
информация (предпочтение IDE Alice) меняется со временем, но история сохраняется.

## Сценарий

1. **2026-03-15** — Alice устанавливает новый лаптоп, обсуждает с architect IDE setup.
   Создаётся `mem_01HZM2A4B6C8D0E2F4G6H8J0K2` (Neovim, status=active).
2. **Два месяца спустя, 2026-05-15** — Alice пробует Cursor, через две недели решает
   переехать. В разговоре с architect просит обновить память.
3. Architect находит существующую память через `memoryfs_search`, видит противоречие,
   вызывает `memoryfs_supersede_memory` с `conflict_type=contradiction`.
4. Поскольку `sensitivity=pii`, новое предложение уходит в **inbox**, target получает
   `target_locked_pending_review`.
5. Alice approves через `memoryfs_review`. **Один атомарный коммит** делает:
   - старую memory: `status: superseded`, `superseded_by: [new]`
   - новую memory: `status: active`, `supersedes: [old]`
   - proposal: `proposal_status: approved`

## Состав

```text
killer-demo-with-conflict/
├── .memoryfs/
│   └── policy.yaml
├── memory/users/alice/
│   ├── ide-preference.md                  # mem_...K2  status=superseded
│   └── ide-preference-current.md          # mem_...A3  status=active
├── conversations/2026/05/15/
│   └── conv_01HZM5K0M2N4P6Q8R0S2T4V6X8.md # source for new memory
├── runs/run_01HZM5N4P6Q8R0S2T4V6X8Z0B2/
│   ├── index.md
│   ├── prompt.md
│   ├── tool_calls.jsonl
│   └── result.md
├── inbox/proposals/
│   └── prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3.md  # state: pending_review (или approved/rejected в тестах)
└── expected/
    └── supersede_invariants.yaml          # golden expectations для тестов
```

## Что тестирует fixture

| Аспект | Где |
| -------- | ----- |
| Двусторонняя связь supersedes ↔ superseded_by | INV STRUCT-1, STRUCT-2 |
| DAG (никаких циклов) | INV STRUCT-3, adv_supersede_cycle |
| target_locked_pending_review | adv_double_supersede_locked |
| Атомарность апдейта (один коммит на 2 файла) | state_after_approve.expected_commit |
| Retrieval по умолчанию скрывает superseded | retrieval_queries.rq_default_only_active |
| Explicit запрос истории включает superseded | rq_explicit_history |
| Provenance цепочка | rq_provenance_chain |
| Reject не меняет старую память | state_after_reject |
| Self-supersede блокируется на schema-валидации | adv_self_supersede |

## Plausible IDs

| Тип | ID |
| ----- | ---- |
| Workspace | `ws_01HZM5N2P4Q6R8T0V2X4Z6B8D0` |
| Old memory (superseded) | `mem_01HZM2A4B6C8D0E2F4G6H8J0K2` |
| New memory (active) | `mem_01HZM5P3Q5R7S9T1V3W5X7Y9Z1` |
| Source conversation | `conv_01HZM5K0M2N4P6Q8R0S2T4V6X8` |
| Run | `run_01HZM5N4P6Q8R0S2T4V6X8Z0B2` |
| Proposal | `prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3` |
| Entity Alice | `ent_01HZM1A3B5C7D9E1F3G5H7J9K1` |

## Использование в тестах

```bash
# Загрузить fixture в нужном состоянии
just load-fixture killer-demo-with-conflict --state pending_review
# → policy + старая memory + conversation + run + pending proposal

just load-fixture killer-demo-with-conflict --state approved
# → старая superseded, новая active, proposal approved

just load-fixture killer-demo-with-conflict --state rejected
# → старая active, новая отсутствует, proposal rejected (история сохранена)

# Прогонять тесты
just test-supersede --fixture killer-demo-with-conflict
```
