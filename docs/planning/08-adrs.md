# 08 — Architecture Decision Records

Краткий формат: **Context → Decision → Consequences → Alternatives**.

Все ADR — в статусе **proposed** до явного одобрения архитектором + 1 ревьюером. После одобрения — `accepted`.

---

## ADR-001 — Markdown как источник истины

**Status**: proposed (must accept до Phase 1)

**Context**:
Системы памяти для AI-агентов часто построены вокруг vector DB как единственного хранилища.
Это даёт быстрый recall, но проигрывает по верифицируемости: нельзя открыть запись в редакторе,
увидеть контекст, прочитать историю изменений.
markdownfs предлагает противоположный подход — Markdown-файлы с frontmatter, version control, ACL.

**Decision**:
Источник истины — **Markdown-файлы** в иерархическом workspace. Vector index, BM25 index,
entity graph — производные индексы, перестраиваемые из workspace + event log.

**Consequences**:

- Любая запись памяти проверяема человеком без специальных инструментов.
- Полная reconstruct-ability индексов.
- Diff/revert работают как в Git.
- Provenance не теряется при смене embedder/модели.
− Производительность на больших workspace требует bench (Phase 0.1).
− Индексация — eventually consistent, требует drift-monitoring.

**Alternatives considered**:

- **Vector-first** (mem0-подобный): отвергнут из-за слабой верифицируемости.
- **Single SQLite DB с blob-телами**: отвергнут — теряет human-readability и портируемость.
- **Гибрид с произвольным выбором**: отвергнут — концептуальный долг и неоднозначность инвариантов.

---

## ADR-002 — Append-only memory с supersedes

**Status**: proposed

**Context**:
Память агентов должна обновляться при изменениях ("раньше предпочитал X, теперь Y").
Прямая перезапись теряет историю и возможность temporal reasoning.

**Decision**:
Память — **append-only**. Вместо update — новая запись + ссылка `supersedes` на старую;
старая получает `superseded_by` и `status: superseded`.

**Consequences**:

- Полная история изменений.
- Возможность temporal reasoning ("когда мы перешли с X на Y").
- Откат ошибочной записи — простая операция.
- Audit для compliance.
− Storage растёт. Митигация: archive после N supersedes + GC strategy.
− Усложнение retrieval (фильтрация active по умолчанию).

**Alternatives**:

- **Mutable memory**: отвергнут.
- **Mutable + history table**: усложнение схемы без преимуществ.

---

## ADR-003 — Чистый Rust (без Python worker)

**Status**: accepted

**Context**:
Core (workspace engine, ACL, commit graph) — характеризуется hot-path latency и предсказуемой
памятью. LLM extraction вызывается через HTTP API (DeepSeek cloud), embedding — локальный
inference через HTTP. Оба случая не требуют Python-экосистемы — достаточно HTTP-клиента на Rust.

**Decision**:

- **Весь код — на Rust**. Один бинарник, единый стек.
- **LLM extraction**: HTTP-клиент (`reqwest`) к DeepSeek API (`api.deepseek.com`), OpenAI-compatible формат.
- **Embedding**: HTTP-клиент к локальному inference серверу (HuggingFace TEI / Ollama), конфигурируемый endpoint.
- Спайк 0.2 (Rust↔Python IPC) — **отменён**, не нужен.
- Python worker (`workers/extractor/`) — **удаляется** из проекта.

**Consequences**:

- Один бинарник — проще deploy, CI, debug.
- Убирается IPC overhead и сериализация между языками.
- Одна команда, один стек, один IDE.
- Нет version-drift между Rust и Python зависимостями.
− Потеря доступа к Python ML-библиотекам (NER, tokenizers). Митигация: HTTP API к inference-серверам.
− Rust HTTP-клиент менее удобен для быстрого прототипирования LLM-промптов. Митигация: конфиг промптов в YAML/Markdown файлах.

**Alternatives considered**:

- **Rust core + Python worker** (первоначальный ADR-003): отвергнут — LLM и embedding доступны
  через HTTP API, Python не добавляет ценности.
- **Go core + Python worker**: отвергнут ранее.

**Supersedes**: оригинальный ADR-003 (Rust + Python).

---

## ADR-004 — Vector store: pgvector, pluggable для Levara

**Status**: accepted

**Context**:
Нужен vector store с filter-by-metadata, persistence, SQL-интеграцией. PostgreSQL уже
используется для metadata (commits, inode-index, edges). Держать отдельный vector DB
(Qdrant) — лишний процесс.

**Decision**:

- **pgvector** расширение в PostgreSQL 16 — единственный vector store для MVP.
- Trait `VectorStore` с абстракцией: `upsert`, `search`, `delete`, `reindex`.
- Первая имплементация — `PgVectorStore`.
- Архитектура готовится к подключению **Levara** (https://github.com/levara)
  как альтернативного backend'а — trait проектируется с учётом этого.
- Qdrant убирается из стека.

**Consequences**:

- Один PostgreSQL процесс на всё (metadata + vectors) — проще deploy.
- Transactional consistency: commit + vector upsert в одной транзакции.
- SQL-фильтрация по metadata нативная.
- Подготовка к Levara через pluggable trait.
− pgvector менее оптимизирован для ANN, чем dedicated vector DB. Митигация: HNSW-индексы, batch upsert.
− Нужен pgvector extension в Docker-образе.

**Alternatives considered**:

- **Qdrant** (оригинальный ADR-004): отвергнут — лишний процесс, нет транзакционной консистентности с metadata.
- **Weaviate**: тяжелее, GraphQL API избыточный.
- **LanceDB / Chroma**: embeddable; не совместимы с Levara roadmap.

**Supersedes**: оригинальный ADR-004 (Qdrant как baseline).

---

## ADR-005 — Authn: JWT токены, два класса subjects

**Status**: accepted

**Context**:
Agents и users — разные subjects с разными жизненными циклами и scopes. Нужна stateless верификация для масштабируемости.

**Decision**:

- **JWT** токены в `Authorization: Bearer` заголовке. Crate `jsonwebtoken`.
- Prefixed `sub` claim: `user:<id>` или `agent:<id>`.
- Agent токены — обязательный `exp` (max 7 дней default), short-lived.
- User токены — refresh-token flow: access (15 мин) + refresh (30 дней).
- Revocation через deny-list в PostgreSQL (проверяется на hot-path если `jti` в deny-list).
- Signing: HMAC-SHA256 (HS256) для MVP; RS256 как опция для team-deploy.

**Consequences**:

- Stateless верификация — быстрее, чем DB-lookup на каждый запрос.
- Стандартный формат, совместимость с middleware-экосистемой.
- Claims (scope, workspace_id, permissions) внутри токена — меньше DB-запросов.
− Revocation требует deny-list (не instant, но acceptable с коротким TTL).
− JWT payload видим (не секретный) — не класть PII в claims.

**Alternatives considered**:

- **Opaque tokens + DB-lookup** (оригинальный ADR-005): отвергнут — DB-lookup на каждый запрос, не масштабируется.
- **OAuth 2.0 full flow**: добавится для team-deploy, но не в MVP.

**Supersedes**: оригинальный ADR-005 (opaque bearer tokens).

---

## ADR-006 — ULID для всех ID

**Status**: proposed

**Context**:
Нужны ID, которые: монотонные во времени, K-сортируемые, не требуют центрального координатора, человекочитаемые.

**Decision**:
**ULID** (26 chars Crockford base32) для всех ID, с typed-prefix:

- `mem_<ULID>`, `conv_<ULID>`, `run_<ULID>`, `ent_<ULID>`, `dec_<ULID>`, `ws_<ULID>`, `evt_<ULID>`.
- Commit hash — отдельная схема: `commit_<sha256-hex16>`.

**Consequences**:

- Sortability по времени → удобство pagination и log-чтения.
- No collisions при разумной нагрузке.
- Префикс делает тип читаемым.
− 26 chars vs 22 для UUID — длиннее.

**Alternatives**:

- **UUIDv7**: эквивалент по свойствам, но читаемость ниже. Acceptable, но ULID удобнее в логах.
- **UUIDv4**: нет sortability.
- **Snowflake**: требует координатора.

---

## ADR-007 — Permissions: ACL в `policy.yaml` + per-file overrides

**Status**: proposed

**Context**:
Разные пользователи и агенты имеют разные права. Нужна модель, которую можно ревьюить как код.

**Decision**:

- **Глобальный** policy.yaml с rules `subject + resource glob + actions`.
- **Default deny.**
- **Per-file overrides** в frontmatter (`permissions.read/write`), но не шире глобально дозволенного.
- ACL валидируется при write и при read.

**Consequences**:

- Audit policy через diff.
- Понятно где правда.
- Per-file flexibility.
− YAML с глоб-паттернами требует тестов.
− Кеш ACL должен инвалидироваться при изменении.

**Alternatives**:

- **OPA/Rego**: мощнее, но overhead для MVP.
- **Чистый per-file**: децентрализация → сложно ревьюить.

---

## ADR-008 — Memory extraction trust model: review-by-default для sensitive

**Status**: proposed

**Context**:
LLM может выдумать или неверно классифицировать факты. Полностью доверять auto-commit
опасно для PII / medical / legal / financial.

**Decision**:
Auto-commit разрешён **только** для `sensitivity: normal`. Для всего остального —
обязательный review через inbox. Конфиг переопределяется в policy.yaml,
но default — restrictive.

**Consequences**:

- Защита от hallucinated PII.
- Audit trail на каждое sensitive решение.
- Отказоустойчивость к ошибкам extractor.
− Лишний UX-шаг для пользователя.
− Review queue может расти. Митигация: метрики + alerting.

**Alternatives**:

- **Auto-commit всегда** (mem0-style): отвергнут (см. risks 5.1, 5.2).
- **Manual для всего**: убивает UX.

---

## ADR-009 — Object layout: content-addressable + per-workspace inode index

**Status**: proposed (зависит от Phase 0.1 спайка)

**Context**:
Хранение Markdown-файлов в плоском FS не масштабируется (limits на directory size,
slow listing). Нужна структура, которая держит 100k+ файлов.

**Decision**:

- Тело файлов — content-addressable: `objects/<sha256[:2]>/<sha256[2:]>`.
- Path-resolution через inode-индекс (SQLite/Postgres): `(workspace_id, path) → object_hash`.
- Папка `<workspace>/memory/` и т.д. — это **логические** пути; на диске реальная структура — objects + inode.
- Совместимость: команда `memoryfs export` восстанавливает плоскую читаемую структуру для пользователя.

**Consequences**:

- Дедупликация одинаковых тел.
- Listing via DB-индекс — O(log n).
- Стабильно при больших workspace.
− Файлы напрямую в FS не открываются — нужен `memoryfs read` или export.

**Alternatives**:

- **Плоский FS**: отвергнут — лимиты ОС и медленный listing.
- **Git as backend**: тяжелее, не оптимизирован под наш юзкейс.

**Compatibility note**: на маленьких workspace (< 5k файлов) можно поддержать "flat mode"
для удобства. Решается после Phase 0.1.

---

## ADR-010 — MCP transport: stdio + sse

**Status**: proposed

**Context**:
MCP — основной канал для агентов (Claude Code, Cursor). Спецификация поддерживает stdio
(локальные процессы) и SSE/HTTP (удалённые серверы).

**Decision**:

- **stdio** для local-first (Claude Code, Cursor локально подключаются).
- **SSE** для team-server режима (Phase 7+).
- WebSocket — не поддерживаем в v1.0 (overhead, мало где нужно).
- Все MCP tools — за тем же authn что и REST.

**Consequences**:

- Стандартный набор для MCP экосистемы.
- Простой local deploy.
− Два транспорта для поддержки.

**Alternatives**:

- **Только stdio**: ограничивает team-deploy.
- **Только SSE**: усложняет local-only сетап.

---

## ADR-011 — Append-only event log с hash-цепочкой как опция

**Status**: proposed

**Context**:
Все workspace mutations должны быть отслеживаемы. Audit для compliance — ключевая фича.

**Decision**:

- **NDJSON event log**, append-only, fsync на каждое событие.
- Опция `tamper_evident: true` в `.memoryfs/config.yaml` включает hash-цепочку (каждое событие хранит hash предыдущего).
- Команда `memoryfs admin audit verify` проверяет цепочку.

**Consequences**:

- Непрерывный audit trail.
- Tamper-evidence бесплатно при включении.
− fsync на каждое событие = производительность; митигация: батчинг с верхней границей задержки.
− verify на больших логах медленный; периодический sealing с подписью (post-MVP).

**Alternatives**:

- **Без event log**: невозможно для audit.
- **Полноценный blockchain**: overkill.

---

## ADR-012 — Schema versioning через `schema_version` + migration runner

**Status**: proposed

**Context**:
Frontmatter будет эволюционировать. Без явного версионирования — невозможны безопасные миграции.

**Decision**:

- Каждый файл имеет `schema_version: <int>`.
- Запуск сервиса проверяет: если есть файлы с `schema_version < current`, запускается migration runner.
- Миграции — упорядоченные шаги (`migrations/v1_to_v2.rs/.py`), идемпотентные.
- Каждая миграция — атомарный коммит с автором `system:migration`.
- Невозможность миграции (нет шага) → старт сервиса блокируется с понятной ошибкой.

**Consequences**:

- Безопасные миграции.
- Audit миграций как обычных коммитов.
− Каждое изменение схемы требует миграции.

**Alternatives**:

- **No versioning, schema-on-read**: фрагильно.
- **Database-style migrations** только для metadata: не покрывает frontmatter.

---

## ADR-013 — Embedding: EmbeddingGemma локально, pluggable

**Status**: accepted

**Context**:
Embedding нужен для vector-индексации (Phase 3) и retrieval (Phase 4). Облачные embedding API
(OpenAI, Cohere) — платные и зависят от network. Для batch-операций (reindex 100k файлов)
локальный inference дешевле и быстрее.

**Decision**:

- **EmbeddingGemma** (Google) как default embedding модель, запускается **локально**.
- Inference сервер: HuggingFace Text Embeddings Inference (TEI) или Ollama в Docker.
- Trait `Embedder` в `crates/core`:

  ```rust
  #[async_trait]
  pub trait Embedder: Send + Sync {
      async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
      fn dimension(&self) -> usize;
      fn model_id(&self) -> &str;
  }
  ```

- Первая имплементация: `HttpEmbedder` — generic HTTP-клиент к любому OpenAI-compatible endpoint.
- Model + endpoint конфигурируются в `.memoryfs/config.yaml`.
- При смене модели — автоматический reindex (Phase 3.6).

**Consequences**:

- Без оплаты за токены, без rate limits.
- Быстрый batch embedding (GPU локально).
- Pluggable — можно подключить OpenAI, Cohere, или кастомную модель.
- Offline-capable.
− Требуется GPU (или CPU с ожиданием) на dev-машине.
− Docker-образ TEI ~2GB.

**Alternatives considered**:

- **OpenAI text-embedding-3-small**: облачный, платный, зависимость от network.
- **nomic-embed-text**: хороший, но EmbeddingGemma показывает лучше на MTEB.
- **bge-base**: устаревший.

---

## ADR-014 — LLM extraction: DeepSeek через облачный API

**Status**: accepted

**Context**:
Memory extraction требует мощной языковой модели для анализа conversation и генерации
structured proposals. Локальный inference крупных LLM (>30B параметров) требует серьёзного GPU.

**Decision**:

- **DeepSeek** через `api.deepseek.com` (OpenAI-compatible API).
- Trait `LlmClient` в `crates/core`:

  ```rust
  #[async_trait]
  pub trait LlmClient: Send + Sync {
      async fn chat(&self, messages: &[Message], schema: Option<&JsonSchema>) -> Result<String>;
      fn model_id(&self) -> &str;
  }
  ```

- Первая имплементация: `OpenAiCompatibleClient` — работает с любым OpenAI-compatible
  endpoint (DeepSeek, OpenAI, Ollama, vLLM).
- API key в env var `DEEPSEEK_API_KEY` или `.memoryfs/secrets.yaml` (encrypted).
- Structured output через JSON mode / function calling.
- Retry: exponential backoff (3 попытки, max 30s).
- Model name конфигурируется: default `deepseek-chat`.

**Consequences**:

- Мощная модель без локального GPU.
- Дешевле чем OpenAI/Anthropic для extraction-задач.
- OpenAI-compatible — легко переключить на другого провайдера.
− Зависимость от внешнего API (network, availability).
− Стоимость при большом volume. Митигация: batch requests, кеширование extraction-результатов.

**Alternatives considered**:

- **Anthropic Claude**: дороже, но потенциально лучше quality. Может быть добавлен как альтернативный backend.
- **OpenAI GPT-4o-mini**: дороже DeepSeek при сопоставимом quality на extraction.
- **Локальный DeepSeek**: требует мощный GPU; оставляем как опцию через тот же trait.

---

## Список ADR (живой)

| ID | Название | Status | Phase до которой |
| ---- | ---------- | -------- | ------------------ |
| ADR-001 | Markdown как источник истины | proposed | Phase 0 |
| ADR-002 | Append-only memory | proposed | Phase 0 |
| ADR-003 | Чистый Rust (без Python worker) | accepted | Phase 0 |
| ADR-004 | pgvector, pluggable для Levara | accepted | Phase 0 |
| ADR-005 | JWT токены, два класса subjects | accepted | Phase 1 |
| ADR-006 | ULID для всех ID | proposed | Phase 0 |
| ADR-007 | ACL: policy.yaml + overrides | proposed | Phase 1 |
| ADR-008 | Review-by-default для sensitive | proposed | Phase 2 |
| ADR-009 | Object layout: content-addressable | proposed | Phase 1 |
| ADR-010 | MCP transport: stdio + sse | proposed | Phase 6 |
| ADR-011 | Event log с hash-chain опцией | proposed | Phase 1 |
| ADR-012 | Schema versioning | proposed | Phase 1 |
| ADR-013 | EmbeddingGemma локально, pluggable | accepted | Phase 3 |
| ADR-014 | DeepSeek cloud extraction | accepted | Phase 2 |

Новые ADR добавляются по правилу из `04-tasks-dod.md` CC.5: любая новая зависимость,
новый паттерн ACL, изменение схемы → ADR.

## Шаблон для новых ADR

```md
## ADR-NNN — Название

**Status**: proposed | accepted | deprecated | superseded by ADR-XXX

**Context**:
<2-4 предложения о проблеме>

**Decision**:
<что решено>

**Consequences**:
+ плюсы
− минусы

**Alternatives considered**:
- <вариант>: <почему отвергнут>
```
