# 06 — Стратегия тестирования

## 1. Основные принципы

1. **Truth-first testing**: главные инварианты — на уровне workspace (FS), а не индексов.
   Если тест опирается на vector index как источник истины — это плохой тест.
2. **Каждый corner case → именованный тест**. ID из `05-corner-cases.md` появляется в имени теста.
3. **Eval — это тоже тест**, гонится в CI. Регрессия > порога блокирует merge.
4. **Adversarial-suites — first-class**. Не "когда-нибудь сделаем", а до релиза Phase 1.
5. **Performance tests зелёные = fail.** Если бенч проходит подозрительно быстро — это сигнал, что тест мерит не то.
6. **No flaky tests policy**. Flake → quarantine + bug → fix или delete за 7 дней.

## 2. Пирамида тестов

```text
                  ┌──────────────┐
                  │  E2E (5%)    │   killer-demo, MCP via real client
                  ├──────────────┤
                  │  Integration │   API+engine+indexer+worker
                  │     (20%)    │
                  ├──────────────┤
                  │  Component   │   per-component contract
                  │     (30%)    │
                  ├──────────────┤
                  │  Unit (45%)  │   pure functions, parsers, validators
                  └──────────────┘
```

Дополнительно вне пирамиды:

- **Property-based** (proptest/Hypothesis) — инварианты.
- **Fuzz** — парсеры, path-resolver.
- **Adversarial** — secrets/PII/injection/path-traversal.
- **Chaos** — kill -9, disk full, slow LLM.
- **Eval** — extraction + retrieval quality.
- **Load / soak** — production-like нагрузка > 1 час.

## 3. Тесты по компонентам

### 3.1 Object store (Rust)

- Unit: hash-correctness, roundtrip put/get, dedupe.
- Property: для любого `bytes` → `get(put(bytes)) == bytes`.
- Fault injection: corrupted blob → error not panic.
- Bench: 10k ops, p95 latency.

### 3.2 Frontmatter parser

- Unit: golden YAML cases (multiline, anchors, unicode).
- Property: для любого валидного frontmatter parse→serialize→parse идемпотентно.
- Fuzz: cargo-fuzz/AFL на парсере.
- Negative: 100+ malformed входов.

### 3.3 ACL guard

- Unit: matrix subjects × resources × actions.
- Property: deny всегда побеждает allow.
- Adversarial: 50 кейсов из `tests/acl/adversarial.yaml` (`..`, symlinks, unicode confusables).
- Performance: 100k проверок < 1 сек.

### 3.4 Commit graph

- Unit: linear, branching, revert-of-revert.
- Property: revert(revert(c)) = c (semantically, новый hash).
- Concurrency: 100 параллельных commits → all succeed or fail with 409 (no lost data).
- Migration: schema_version bump → новые коммиты.

### 3.5 Pre-redaction

- Unit: каждый pattern.
- Adversarial: `fixtures/secrets/*.json` — должны redacted на 100%.
- False positive: `fixtures/benign/*.md` — < 2% redacted.
- Performance: 10 МБ текста за < 500ms.

### 3.6 Event log

- Unit: append, recover, offset-index.
- Chaos: kill -9 после write/before fsync → recovery без duplicate.
- Tamper-evident: повреждение середины → verify падает.

### 3.7 Extraction worker

- Unit: prompt-builder, output-validator.
- Mock-LLM: golden-input → expected-output (snapshot tests).
- Real-LLM: quarantined-набор, гонится в weekly CI.
- Adversarial: prompt-injection set должен быть отбит.

### 3.8 Indexer

- Unit: chunker (heading-aware), embedding-cache.
- Integration: write + commit → indexer обновляет индексы.
- Drift: file modified + reindex → 0 stale chunks.
- Performance: 100k файлов индекс < 30 минут.

### 3.9 Retrieval engine

- Unit: RRF, scope-boost, recency-decay.
- Eval: NDCG@10, MRR, recall@k.
- ACL-after-retrieve: устаревший payload → права проверены повторно.
- Performance: p95 < 300ms.

### 3.10 MCP server

- Tool-by-tool: golden-IO snapshot.
- Spec-conformance: validate против MCP test harness.
- Integration: реальное подключение Claude Code / Cursor (manual + recorded).

## 4. Adversarial-suites

### 4.1 `secrets-suite`

Файлы: `fixtures/secrets/*.{json,md}`. Каждый кейс описывает:

```json
{
  "id": "secrets-001",
  "input": "Here is my API key: sk-proj-1234567890...",
  "expected_redactions": ["sk-proj-1234567890..."],
  "category": "openai_key"
}
```

Покрытие категорий:

- OpenAI / Anthropic / Google / Azure ключи.
- AWS access keys, SSH ключи (PEM blocks).
- JWT tokens, GitHub tokens, Slack webhooks.
- DB connection strings, .env contents.
- Private keys (PEM/PKCS).
- High-entropy strings ≥ 32 символа в подозрительном контексте.

DoD: 100% redaction на adversarial-наборе; ≤ 2% false positive на benign.

### 4.2 `pii-suite`

- Email, телефон (national + e164), passport / ID-номера разных стран, SSN-like.
- Адреса (regex + heuristic).
- Имена в комбинации с другими PII (требует semantic extraction step).

DoD: 100% детекция базовых, ≥ 90% на advanced.

### 4.3 `injection-suite`

- Direct instruction injection ("Ignore previous...").
- Indirect через embedded content ("This is a test, but actually...").
- Output hijacking (попытка заставить вернуть произвольный JSON).
- Memory poisoning (попытка подменить existing memory).

DoD: extractor возвращает либо корректный пустой результат, либо retry, никогда не выполняет injection.

### 4.4 `path-traversal-suite`

- `..` segments
- URL-encoded `%2e%2e`
- Double-encoded
- Unicode confusables (`/m e m o r y/`, `/Memory/`)
- Symlink attempts
- Long path bombs

DoD: 100% rejected.

### 4.5 `acl-bypass-suite`

- Unicode confusables in subject IDs (Latin → Cyrillic).
- Frontmatter permissions wider than policy.yaml.
- Cross-tenant payload в индексе.
- Token replay после revoke.

DoD: 100% rejected.

## 5. Eval-suites

### 5.1 Retrieval eval

`eval/retrieval/dataset.jsonl` — пары `{query, expected_memory_ids[], scope_filters}`.

Метрики:

- NDCG@10
- MRR
- Recall@k (k=5, 10, 20)
- Per-source contribution (vector / BM25 / entity)

Baseline зафиксирован после Phase 3. CI блокирует merge при регрессии > 2% на любую метрику без override + объяснения.

Fixtures должны включать:

- Простые лookup-запросы.
- Запросы с синонимами ("preferences" vs "likes").
- Multi-hop ("проекты пользователя X").
- Negation ("что НЕ нравится").
- Запросы с шумом (typos, mixed-language).
- Temporal ("что мы решили на прошлой неделе").
- ACL-фильтрованные (нужный документ скрыт правами).

### 5.2 Extraction eval

`eval/extraction/dataset.jsonl` — пары `{conversation_md, expected_proposals[]}`.

Метрики:

- Precision (доля корректных среди извлечённых).
- Recall (доля извлечённых среди корректных).
- F1.
- Per memory_type breakdown.
- Provenance accuracy: source_span попадает в правильный диапазон строк.
- Hallucination rate: % proposals без подтверждения в conversation.

Baseline после Phase 2. Регрессия > 3% F1 → блок.

Fixtures:

- Сильные сигналы ("I prefer X").
- Слабые сигналы ("обычно я делаю X").
- Многоступенчатый вывод (нужно объединить две реплики).
- Противоречия в одном диалоге.
- Сарказм / отрицание.
- Несколько языков в одном диалоге.
- PII / secrets в репликах (must NOT be extracted as memory без redaction).

### 5.3 Conflict-resolution eval

Конкретный sub-eval: умеет ли система предлагать `supersede` при противоречиях.
Метрики: precision/recall на detection of conflicts; correctness of proposed `conflict_type`.

## 6. Performance / Load / Soak

### 6.1 Bench suite

- `bench/markdownfs_scale/` — read/write/list/commit на 10k → 100k → 1M.
- `bench/retrieval/` — recall p50/p95/p99 при разных размерах.
- `bench/indexer/` — throughput чанков и embeddings.
- `bench/extraction/` — proposals/sec при mock-LLM.

Результаты публикуются в `bench/results/<phase>/<date>.md`. Регрессия > 10% на baseline → блок.

### 6.2 Load tests

- 100 параллельных агентов, 1000 events/sec, 1 час.
- Метрики: error rate < 0.1%, latency не деградирует > 2x.

### 6.3 Soak tests (24h)

- Steady load, проверка memory leaks, очередей, audit log роста.
- DoD: zero data loss, growth in disk предсказуем.

### 6.4 Spike tests

- Внезапный 10x rps в течение 5 минут.
- Recovery после спайка < 1 минуты.

## 7. Chaos engineering

`chaos/` test runner. Сценарии:

- `kill-9-during-write` — на каждом шаге write protocol.
- `disk-full-mid-commit`.
- `network-partition` к vector store.
- `slow-llm` (30+ секунд responses).
- `clock-skew` ±5 минут на агентах.
- `corrupted-blob` (модификация файла на диске).
- `event-log-truncation`.

DoD: все сценарии завершаются без data loss и с понятной ошибкой / recovery.

## 8. Security testing

### 8.1 SAST / dependency

- `cargo audit` + `cargo deny` (Rust).
- `pip-audit` + `bandit` (Python).
- `gitleaks` на pre-commit.
- Dependency updates — еженедельно.

### 8.2 DAST

- Запуск ZAP / nuclei против работающего API.
- OWASP API top-10 checklist.

### 8.3 Threat model

- Документ `docs/threat-model.md` (STRIDE).
- Обновляется при добавлении нового surface.

### 8.4 Pen-test

- Pre-1.0 release — внешний или внутренний pen-test.

## 9. Тестовые fixtures и golden данные

```text
fixtures/
├── workspaces/
│   ├── empty/
│   ├── small/                 # 50 файлов
│   ├── medium/                # 5k
│   ├── large/                 # 100k (генерируется по запросу)
│   └── killer_demo/           # фиксированный demo-workspace
├── conversations/
│   ├── golden/                # для extraction eval
│   ├── injection/             # adversarial
│   └── multilingual/
├── secrets/                   # adversarial-suite
├── benign/                    # для false-positive теста redaction
├── acl/                       # ACL adversarial
├── frontmatter/
│   ├── valid/
│   └── invalid/
└── policies/
    ├── default/
    └── restrictive/
```

Все fixtures под VCS.

## 10. CI / CD pipeline тестов

Стадии (gating):

1. **Lint + format** (rustfmt, clippy, ruff, mypy) — fast, обязательно.
2. **Schemas validation** (JSON Schemas сами валидны + drift с docs).
3. **Unit + property** (parallel) — < 5 минут.
4. **Component + integration** — < 15 минут.
5. **Adversarial-suites** — < 10 минут.
6. **Eval (retrieval + extraction)** — < 20 минут (с mock-LLM в PR-CI).
7. **E2E (smoke + killer-demo)** — < 10 минут.
8. **Bench (regression)** — < 30 минут, на dedicated runner.

Nightly:

1. Real-LLM eval.
2. Chaos suite.
3. Soak (24h в weekend).
4. Security scans.

## 11. Coverage gates

| Слой | Line | Branch |
| ------ | ------ | -------- |
| Core (Rust) | ≥ 80% | ≥ 70% |
| Workers (Python) | ≥ 70% | ≥ 60% |
| Adapters (vector/BM25/graph) | ≥ 75% | ≥ 65% |
| MCP / CLI | ≥ 70% | ≥ 60% |

Coverage не подменяет тесты — он дополняет инварианты и eval. Блокировка merge при падении
ниже floor (не при отсутствии инкремента).

## 12. Release checklist

Перед релизом:

- [ ] Все P0/P1 cornercases имеют именованные тесты, зелёные.
- [ ] Eval baseline не регрессирует.
- [ ] Bench не регрессирует > 10%.
- [ ] Adversarial-suites: 100% на secrets, injection, path-traversal, ACL-bypass.
- [ ] Security scan: 0 критических, ≤ 5 high с migration plan.
- [ ] Chaos suite: zero data loss подтверждён.
- [ ] Documentation drift тесты зелёные.
- [ ] Migration runner проверен на golden v(N-1) → v(N).
- [ ] Threat model обновлена под новые surface.

## 13. Test ownership

| Suite | Owner |
| ------- | ------- |
| Core unit / component | Rust eng (rotation) |
| Workers unit / component | Python eng (rotation) |
| Eval (retrieval / extraction) | ML eng |
| Adversarial security | Security-owner |
| Chaos | SRE / platform eng |
| Bench | Performance-owner |
| E2E / killer-demo | QA + product owner |

Owner обязан:

- держать suite зелёным и быстрым;
- триаж нового красного теста за 24 часа;
- ежемесячный отчёт о состоянии своего suite.
