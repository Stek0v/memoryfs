# 04 — Задачи с Definition of Done

Структура: задачи сгруппированы по **фазам**. Внутри фазы — нумерация `<phase>.<n>`. Формат каждой задачи:

- **Цель** — что и зачем.
- **Acceptance criteria** — что должно работать.
- **DoD** — расширенные критерии готовности (тесты, доки, метрики).
- **Зависимости** — другие задачи / решения.
- **Эффорт** — t-shirt (S/M/L/XL).

Эффорты — оценочные, до начала спайков. См. также `07-roadmap.md`.

---

## Phase 0 — Foundations & Spikes

Цель фазы: снять ключевые архитектурные риски, зафиксировать схемы, поставить
eval-инфраструктуру **до** начала продуктовой разработки.

### 0.1 Спайк: производительность Markdown-FS на масштабе

- **Цель**: убедиться, что архитектура держит до 100k файлов с p95 read < 50ms.
- **Acceptance**: бенч-скрипт генерирует 100k файлов случайной структурой, измеряет read/write/list/commit.
- **DoD**:
  - Скрипт в `bench/markdownfs_scale/` (Rust + Python harness).
  - Отчёт в `bench/results/0.1.md` с p50/p95/p99.
  - Решение go/no-go по выбранному object-store layout (зафиксировано в ADR-009).
- **Зависимости**: —
- **Эффорт**: M

### 0.2 Спайк: Rust ↔ Python boundary

- **Цель**: выбрать механизм обмена между core и worker (gRPC vs JSON-RPC over HTTP vs ZeroMQ).
- **Acceptance**: 3 прототипа, бенч на throughput и latency.
- **DoD**:
  - Прототипы в `spikes/rust_python_ipc/`.
  - Решение в ADR-003.
  - Метрики p95 < 5ms для round-trip JSON 4 КБ.
- **Эффорт**: M

### 0.3 Eval-инфраструктура (retrieval quality)

- **Цель**: иметь воспроизводимую оценку качества recall до начала индексации.
- **Acceptance**: eval-набор из 200 query/answer пар на синтетических workspace-фикстурах.
- **DoD**:
  - `eval/retrieval/dataset.jsonl` с разметкой relevance per memory.
  - `eval/retrieval/run.py` — runner, выдаёт NDCG@k, MRR, recall@k.
  - CI-шаг: regression-check на eval (фиксируется baseline).
  - Документация в `eval/retrieval/README.md`.
- **Зависимости**: —
- **Эффорт**: L

### 0.4 Eval-инфраструктура (extraction quality)

- **Цель**: оценка precision/recall LLM-экстрактора.
- **Acceptance**: 200 размеченных диалогов с ожидаемыми memory proposals.
- **DoD**:
  - `eval/extraction/dataset.jsonl` (с consent-flag для PII в синтетике).
  - `eval/extraction/run.py` — выдаёт precision, recall, F1 по типам памяти.
  - Baseline зафиксирован.
- **Эффорт**: L

### 0.5 JSON Schemas для frontmatter v1

- **Цель**: финализировать схемы до начала записи реальной памяти.
- **Acceptance**: schemas для memory, conversation, run, decision, entity, proposal.
- **DoD**:
  - Файлы в `schemas/v1/`.
  - Тесты валидации в `schemas/tests/` (positive + negative cases).
  - Документация в `02-data-model.md` синхронизирована со схемами (тест на drift).
- **Эффорт**: M

### 0.6 Killer-demo сценарий зафиксирован

- **Цель**: иметь fixture-сценарий "почему мы выбрали Qdrant" для всех тестов и e2e.
- **Acceptance**: набор файлов + диалогов + ожидаемых memories + ожидаемого recall-ответа.
- **DoD**:
  - `fixtures/killer_demo/` (диалоги, тзшный workspace, expected outputs).
  - E2E-тест прогоняет demo и сравнивает с ожиданием.
- **Эффорт**: S

---

## Phase 1 — Core Workspace (Markdown как источник истины)

Цель: рабочий CLI + REST для read/write/commit/revert/log без extraction и indexing.

### 1.1 Object store (content-addressable)

- **Цель**: SHA-256 объекты + inode-индекс path→hash.
- **Acceptance**: `put(bytes) → hash`, `get(hash) → bytes`, dedupe.
- **DoD**:
  - Tests: дедупликация, целостность, повреждённый объект → ошибка.
  - Bench: 10k put/get на p95 < 5ms.
  - Документация структуры на диске.
- **Зависимости**: 0.1
- **Эффорт**: M

### 1.2 Frontmatter parser/validator

- **Цель**: парсить YAML frontmatter, валидировать против JSON-Schema.
- **Acceptance**: чтение/запись MD-файла с frontmatter; ошибка валидации с указанием поля.
- **DoD**:
  - Tests: 100% веток валидатора, fuzz на YAML edge cases (anchors, multiline, unicode).
  - Бенч: parse 10k файлов < 5 сек.
  - Поддержка миграции `schema_version`.
- **Зависимости**: 0.5
- **Эффорт**: M

### 1.3 Commit graph + log + diff + revert

- **Цель**: Git-style история без зависимости от Git.
- **Acceptance**: `commit`, `log`, `diff`, `revert`.
- **DoD**:
  - Tests: linear history, revert правильности (диффы инверсные), idempotency повторного revert.
  - Concurrent commit → 409 (см. corner case 5.1).
  - Документация формата commit-объекта.
- **Эффорт**: L

### 1.4 ACL guard и policy.yaml

- **Цель**: применять ACL на каждом read/write.
- **Acceptance**: deny побеждает allow; glob-паттерны работают; default deny.
- **DoD**:
  - Tests: 50+ кейсов из `tests/acl/cases.yaml`.
  - Adversarial-тест: попытка обойти через `..`, symlinks, unicode-нормализацию.
  - Кеш ACL invalidates при изменении policy.yaml.
- **Эффорт**: L

### 1.5 REST API (subset)

- **Цель**: рабочий API для file/commit/log/diff/revert.
- **Acceptance**: OpenAPI описывает все endpoints; smoke-tests зелёные.
- **DoD**:
  - Tests: contract tests из openapi.yaml.
  - Auth (bearer) + rate limit middleware.
  - Errors соответствуют формату из §10 API spec.
  - p95 read < 50ms на 10k workspace.
- **Эффорт**: L

### 1.6 CLI (subset)

- **Цель**: `init/status/read/write/commit/log/diff/revert/list`.
- **Acceptance**: команды работают через REST на localhost.
- **DoD**:
  - Tests: e2e через testcontainers.
  - Help-страницы и man-style docs.
  - Exit codes документированы.
- **Эффорт**: M

### 1.7 Pre-redaction (secrets)

- **Цель**: блокировать запись секретов в любые файлы.
- **Acceptance**: regex (API keys, JWT, AWS, OpenAI, GitHub, Slack), entropy-эвристика, denylist patterns.
- **DoD**:
  - Tests: 200+ adversarial-кейсов из `fixtures/secrets/`.
  - False positive rate < 2% на benign-корпусе (bench).
  - Логирование redaction events в audit log.
- **Эффорт**: L

### 1.8 Audit log

- **Цель**: append-only лог всех write/commit/revert/review событий.
- **Acceptance**: формат NDJSON; никогда не теряется при kill -9 (fsync).
- **DoD**:
  - Tests: chaos-test (kill во время записи → восстановление).
  - Опциональная hash-цепочка (`tamper_evident`).
  - Документация форматa.
- **Эффорт**: M

### 1.9 Observability v1

- **Цель**: метрики + structured logs + traces.
- **Acceptance**: Prometheus endpoint; OTLP traces; `trace_id` в логах.
- **DoD**:
  - Дашборд Grafana (`ops/dashboards/`).
  - SLO задокументированы.
  - Smoke-test проверяет наличие метрик.
- **Эффорт**: M

### 1.10 Документация Phase 1

- **Цель**: getting started для разработчика.
- **Acceptance**: можно установить, запустить, сделать первый commit.
- **DoD**:
  - `docs/getting-started.md`.
  - Скрипт `scripts/dev-setup.sh`.
  - Troubleshooting раздел.
- **Эффорт**: S

**Exit-критерии Phase 1:**

1. CLI делает init/write/commit/log/revert на 100k файлов без падений.
2. ACL покрыт тестами и adversarial-suite.
3. Secret pre-redaction работает по eval-набору.
4. p95 read < 50ms подтверждён бенчем.
5. Killer-demo workspace проходит smoke-test.

---

## Phase 2 — Memory Extraction Worker

Цель: воркер на Python, который из conversation.md создаёт memory proposals.

### 2.1 Event log + worker bus

- **Цель**: append-only event log + потребление воркером.
- **Acceptance**: events durable, at-least-once delivery, idempotency через event_id.
- **DoD**:
  - Tests: повторная доставка идемпотентна.
  - Recovery после kill -9.
  - Bench: 1000 events/sec write.
- **Эффорт**: M

### 2.2 Extraction prompt контракт

- **Цель**: prompt для LLM, который возвращает строгий JSON proposals.
- **Acceptance**: schema валидируется; невалидный output → retry с диагностикой.
- **DoD**:
  - `prompts/extraction/v1.md` зафиксирован.
  - Tests на golden диалоги: stable output (или объяснимый drift).
  - Eval Phase 0.4 проходит с baseline F1 ≥ 0.7.
- **Эффорт**: L

### 2.3 Extraction worker

- **Цель**: Python worker → читает события, вызывает LLM, постит в `/v1/memory/propose`.
- **Acceptance**: дроп LLM не блокирует пайплайн (degraded mode), ретраи с экспоненциальным backoff.
- **DoD**:
  - Tests: с mock LLM, с реальным local LLM, с timeout, с partial output.
  - Метрики: extraction latency, success rate, queue depth.
  - Конфигурируемый embedder/LLM endpoint.
- **Эффорт**: L

### 2.4 Policy-engine для memory

- **Цель**: классифицировать sensitivity, решать auto-commit vs review.
- **Acceptance**: policy.yaml применяется; sensitive scopes идут в inbox.
- **DoD**:
  - Tests: 50+ policy-кейсов.
  - Документация policy DSL.
  - Health check: некорректный policy.yaml → старт блокируется.
- **Эффорт**: M

### 2.5 Inbox и review API

- **Цель**: `/inbox/proposals/` + `POST /v1/memory/review`.
- **Acceptance**: review approve/reject; approved proposals записываются в финальный путь и коммитятся.
- **DoD**:
  - Tests: review-flow e2e.
  - UI-агностичность (CLI достаточно).
  - Подсчёт review queue size в метриках.
- **Эффорт**: M

### 2.6 Post-extraction secret scan

- **Цель**: повторная проверка proposal'а на секреты (LLM может пропустить).
- **Acceptance**: при срабатывании — proposal помечен `sensitivity:secret` и блокируется.
- **DoD**:
  - Tests: adversarial-suite (секреты в произвольных формах).
  - Метрики: post-scan hit rate.
- **Эффорт**: M

### 2.7 Supersede engine

- **Цель**: автоматическое или ручное вытеснение старой памяти.
- **Acceptance**: `POST /v1/memory/{id}/supersede`; супер-связи валидны (нет циклов).
- **DoD**:
  - Tests: cycle detection, корректный обновлённый граф.
  - Audit log фиксирует supersede события.
- **Эффорт**: M

**Exit-критерии Phase 2:**

1. Eval extraction F1 ≥ 0.7 на baseline-наборе.
2. Sensitive scopes никогда не auto-commit'ятся (adversarial-suite).
3. Worker outage не теряет события.
4. Review-flow проходит e2e на killer-demo.

---

## Phase 3 — Indexing (Vector + BM25)

Цель: derived-индексы для быстрого recall.

### 3.1 Heading-aware chunker

- **Цель**: разбить Markdown по структуре заголовков с учётом overlap.
- **Acceptance**: chunk metadata содержит heading_path, char range, окружающий контекст.
- **DoD**:
  - Tests: 50+ markdown fixtures (вложенные списки, code blocks, tables).
  - Edge cases: пустой документ, только frontmatter, длинные параграфы > 4k токенов.
- **Эффорт**: M

### 3.2 Embedding pipeline

- **Цель**: pluggable embedder с metadata (model, version).
- **Acceptance**: embed batch; кеш по object_hash + chunk_index + model.
- **DoD**:
  - Tests: cache hit, model change → reindex.
  - Bench: 1k chunks < 30 сек на baseline-модели.
- **Эффорт**: M

### 3.3 Vector index (Qdrant adapter)

- **Цель**: upsert/search/delete с metadata-фильтрами.
- **Acceptance**: filters работают по workspace_id, scope, scope_id, tags.
- **DoD**:
  - Tests: большие partition, миграция между моделями (двойной индекс).
  - Документация: как добавить другой backend (pgvector).
- **Эффорт**: L

### 3.4 BM25 index (Tantivy adapter)

- **Цель**: full-text с правильным boost'ом.
- **Acceptance**: search по title/heading/body/tags; phrase queries; russian + english analyzers.
- **DoD**:
  - Tests: russian inflections, code-tokens, hyphenation, mixed languages.
  - Bench: index 100k chunks < 5 минут.
- **Эффорт**: M

### 3.5 Indexer worker (event-driven)

- **Цель**: реагировать на commit events → обновлять индексы.
- **Acceptance**: при revert индексы тоже откатываются.
- **DoD**:
  - Tests: race-condition между commit и reindex.
  - Метрики: drift между FS и индексами (background checker).
  - Recovery: full reindex по запросу.
- **Эффорт**: L

### 3.6 Reindex (полная)

- **Цель**: восстановление любого индекса с нуля из workspace.
- **Acceptance**: `POST /v1/admin/reindex` с прогрессом.
- **DoD**:
  - Tests: после reindex все запросы из eval-набора возвращают то же.
  - Bench: 100k файлов < 30 минут.
  - Возобновляемость (checkpoint).
- **Эффорт**: M

**Exit-критерии Phase 3:**

1. NDCG@10 на eval ≥ baseline + 10% по сравнению с only-vector.
2. Drift между FS и индексами = 0 на killer-demo.
3. Reindex восстанавливает индексы корректно.

---

## Phase 4 — Retrieval Engine + Context API

### 4.1 Multi-signal retrieval

- **Цель**: параллельные запросы в vector + BM25 + entity.
- **Acceptance**: Reciprocal Rank Fusion + scope/recency boost.
- **DoD**:
  - Tests: synthetic queries, ablation per source.
  - Метрики: per-source hit rate.
- **Эффорт**: L

### 4.2 ACL filter после retrieval

- **Цель**: payload-filter не доверяем — финальная проверка прав на API-слое.
- **Acceptance**: даже если payload устарел, leaked данные не попадают в ответ.
- **DoD**:
  - Tests: adversarial — устаревший index payload + новые ACL.
  - Audit log при denied access.
- **Эффорт**: M

### 4.3 `/v1/context` endpoint

- **Цель**: финальный API для агентов.
- **Acceptance**: возвращает provenance, scores, snippets.
- **DoD**:
  - Tests: e2e на killer-demo.
  - p95 < 300ms на 100k workspace.
  - Документация с примерами для агентов.
- **Эффорт**: M

### 4.4 Deterministic file read после fusion

- **Цель**: top-N файлов читаются из FS, не из payload.
- **Acceptance**: snippets берутся из актуального файла.
- **DoD**:
  - Tests: changed file → updated snippet.
  - Bench: чтение топ-8 файлов < 50ms.
- **Эффорт**: S

**Exit-критерии Phase 4:**

1. p95 recall < 300ms.
2. Eval NDCG@10 ≥ target.
3. ACL adversarial-suite зелёная.

---

## Phase 5 — Entity Graph

### 5.1 Entity store + edges

- **Цель**: SQL-таблица entities + edges (по схеме из 02).
- **Acceptance**: CRUD entities; link/unlink edges.
- **DoD**:
  - Tests: dedupe by canonical_name, alias merge.
  - Migration story.
- **Эффорт**: M

### 5.2 Entity extraction (NER + linking)

- **Цель**: при extraction: сущности → existing entity или новый proposal.
- **Acceptance**: dedupe ≥ 80% на eval entity-set.
- **DoD**:
  - Tests на NER edge cases (transliteration, abbreviations).
  - Confidence для linking.
- **Эффорт**: L

### 5.3 Entity-aware retrieval

- **Цель**: при recall — расширять поиск через entity neighbors.
- **Acceptance**: entity_score добавлен в fusion.
- **DoD**:
  - A/B на eval: лучше или нет.
  - Контроль раздувания результатов.
- **Эффорт**: M

### 5.4 Entity API

- **Цель**: `/v1/entities/*` (см. API spec §8).
- **Acceptance**: все endpoints.
- **DoD**:
  - Tests + docs.
- **Эффорт**: S

**Exit-критерии Phase 5:**

1. Entity dedupe rate ≥ 80% на eval.
2. Multi-hop ("какие проекты связаны с пользователем?") работает.
3. Improvement в NDCG@10 не отрицательный.

---

## Phase 6 — MCP Server

### 6.1 MCP server (stdio + sse)

- **Цель**: MCP-tool manifest + транспорт.
- **Acceptance**: подключение из Claude Code и Cursor работает.
- **DoD**:
  - Tests против MCP test harness.
  - Документация для подключения.
- **Эффорт**: M

### 6.2 Все 17 MCP tools (см. API spec §11)

- **Цель**: полный набор tools.
- **Acceptance**: каждый tool имеет тест на golden input/output.
- **DoD**:
  - Contract tests.
  - Examples в докe.
- **Эффорт**: L

### 6.3 MCP authn/authz

- **Цель**: agent_token-based auth для MCP.
- **Acceptance**: agent работает только в своих скоупах.
- **DoD**:
  - Tests на cross-workspace попытки.
- **Эффорт**: M

**Exit-критерии Phase 6:**

1. Demo с Claude Code / Cursor: чтение/запись/recall.
2. Tool latency p95 < 200ms.

---

## Phase 7 — Hardening

### 7.1 Backup / restore

- **Цель**: snapshot + restore workspace.
- **Acceptance**: restore воспроизводит state байт-в-байт.
- **DoD**:
  - Tests: restore → reindex → eval не деградирует.
- **Эффорт**: M

### 7.2 Migration runner

- **Цель**: schema_version migrations.
- **Acceptance**: при старте сервиса миграции применяются автоматически.
- **DoD**:
  - Tests на golden-фикстурах v1 → v2.
  - Откат миграции (best effort).
- **Эффорт**: M

### 7.3 Chaos engineering suite

- **Цель**: kill -9, disk full, OOM, slow LLM.
- **Acceptance**: zero data loss, корректное восстановление.
- **DoD**:
  - Test runbook + автоматизация.
- **Эффорт**: L

### 7.4 Performance hardening

- **Цель**: устранить hot spots по профилировке.
- **Acceptance**: p99 в 2x от p95 на load-тесте.
- **DoD**:
  - Профили в `bench/profiles/`.
- **Эффорт**: L

### 7.5 Документация v1.0

- **Цель**: полная dev + ops + integration документация.
- **Acceptance**: новый разработчик за 2 часа делает PR.
- **DoD**:
  - `docs/architecture.md`, `docs/operations.md`, `docs/integrations/*`, `docs/security.md`.
- **Эффорт**: M

**Exit-критерии Phase 7:**

1. Все SLO достигнуты.
2. Chaos suite зелёная.
3. Документация полная и актуальная.

---

## Cross-cutting tasks (всегда живые)

### CC.1 Coverage gates

- Core (Rust): ≥ 80% line, ≥ 70% branch.
- Workers (Python): ≥ 70% line.
- В CI: regression блокирует merge.

### CC.2 Security review каждый PR

- Pre-commit: secret-scan, schema-lint.
- Каждый PR с изменением policy/redaction → ревью security-owner.

### CC.3 Eval regression

- Eval (retrieval + extraction) гонится на каждом PR.
- Регрессия > 2% по NDCG → блокирует merge без явного override.

### CC.4 Документация в PR

- Любое изменение публичного API → обновление docs в том же PR.
- Test на drift openapi.yaml ↔ реализация.

### CC.5 ADR при существенных решениях

- Любая новая зависимость, новый паттерн ACL, изменение схемы → ADR в `08-adrs.md`.
