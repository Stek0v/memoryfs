# 00 — Анализ концепции MemoryFS

## 1. Что предлагается (краткое резюме)

Гибридный продукт: **versioned, human-readable memory workspace для AI-агентов**, где Markdown —
canonical store, а vector/BM25/entity-graph — disposable derived indexes. Объединяет:

- из **mem0**: LLM-extraction, scopes (user/agent/session), multi-signal retrieval, entity linking, append-only;
- из **markdownfs**: Markdown-truth, commits, rollback, permissions, MCP, run records, deterministic reads.

## 2. Сильные стороны концепции

| # | Преимущество | Почему важно |
| --- | -------------- | -------------- |
| 1 | Source of truth — человекочитаемые файлы | Любой инцидент с памятью разбираем без UI и БД |
| 2 | Provenance в frontmatter каждой памяти | Прослеживаемость от ответа до исходного диалога |
| 3 | Append-only с `supersedes` | Temporal reasoning, аудит, откат ошибок |
| 4 | Индексы — derived | Можно перестраивать, менять модель эмбеддингов, экспериментировать без потери данных |
| 5 | MCP как first-class | Прямая интеграция с Claude Code / Cursor / Qwen Code без адаптеров |
| 6 | Run records `/runs/<run-id>/` | Вся работа агента — артефакт с stdout/stderr/result |
| 7 | Permissions на уровне файлов и групп | Multi-agent безопасность из коробки |

## 3. Слабые места и риски концепции

### 3.1 Архитектурные риски

| Риск | Степень | Митигация |
| ------ | --------- | ----------- |
| **Двойная истина**: indexes vs files расходятся | Высокая | Индексация только через event log + reindex job + хеш-проверки |
| **Markdown как БД**: производительность на 100k+ файлов | Высокая | Иерархия + content-addressable layer + bounded indexes; бенчмарки на ранней фазе |
| **Конкурентные записи** агентов в один workspace | Высокая | Append-only + optimistic locking на уровне коммита + явная stream-of-events модель |
| **Frontmatter drift**: схема растёт хаотично | Средняя | Версия схемы (`schema_version`) + migration runner + JSON Schema валидация |
| **Размер репо**: история коммитов растёт линейно | Средняя | GC старых артефактов / pack-файлы / archive policy |
| **LLM-extractor как точка отказа** | Высокая | Review-mode по умолчанию для sensitive скоупов + idempotent extract + degrade-to-raw |

### 3.2 Продуктовые риски

| Риск | Степень | Митигация |
| ------ | --------- | ----------- |
| **Перепроектирование**: хочется и mem0, и Git, и graph DB, и MCP, и UI — сразу | Высокая | Жёсткий MVP scope (см. roadmap), одна killer-demo, остальное вырезать |
| **Конкуренция с Letta/Zep/MemGPT/OpenMemory** | Средняя | Нишевание: **проверяемая локальная память для multi-agent dev**, а не SaaS-platform для всех |
| **Сложность для конечного пользователя**: "почему я должен ревьюить память?" | Средняя | Хорошие defaults: автокоммит для нечувствительных скоупов, review только для PII |
| **Vendor lock-in эмбеддинг-моделей** | Низкая | Плагинный embedder + хранение `embedding_model` в метаданных индекса |

### 3.3 Безопасность и приватность

| Риск | Степень | Митигация |
| ------ | --------- | ----------- |
| **Утечка секретов в память** (API keys, tokens) | Высокая | Pre-extraction redaction + post-extraction secret-scanner + denylist patterns |
| **PII в долгосрочной памяти без согласия** | Высокая | Policy-engine с consent-flag и auto-redaction для shared scopes |
| **Prompt-injection через conversation → memory** | Высокая | Sanitize inputs до extraction; isolate user-content от system-instructions; контракт на формат |
| **Cross-tenant утечка через graph** | Средняя | Жёсткая изоляция workspace_id во всех индексах и graph-запросах |
| **Adversarial memory poisoning** агентом | Средняя | `confidence` threshold + review для записей с малым confidence + diff-review |

### 3.4 Технологические риски

| Риск | Степень | Митигация |
| ------ | --------- | ----------- |
| **Rust + Python boundary**: сериализация, latency | Средняя | Чёткий контракт через JSON-RPC/gRPC, бенчмарки p50/p95 |
| **Vector DB scaling**: миллионы memories | Низкая на MVP | Qdrant/pgvector до 10M; sharding позже |
| **Embedding migration** (модель устарела) | Высокая | Двойной индекс на время миграции; `embedding_version` в метаданных |
| **Graph schema lock-in** | Средняя | Edges как простая таблица на MVP, переход на Kuzu/Neo4j по необходимости |

## 4. Конкурентный ландшафт

| Решение | Сильное | Слабое для нашего юзкейса |
| --------- | --------- | --------------------------- |
| **mem0** | Зрелый retrieval, scopes, entity linking | Vector-first, слабая верифицируемость, не Markdown-native |
| **Letta (MemGPT)** | OS-метафора памяти, paging, archival | Сложная модель, не Markdown, не Git-style |
| **Zep** | Graph + temporal knowledge, fact extraction | SaaS-first, тяжело локально, не human-readable файлы |
| **OpenMemory MCP** | MCP-нативный, локальный | Минимальная intelligence, нет provenance/commit |
| **markdownfs** | Markdown truth, Git-style, MCP | Нет memory intelligence: что запомнить, как связать |
| **Obsidian + плагины** | Human-readable, графы заметок | Нет agent-API, нет provenance, не append-only |
| **Git + flat files** | Полная проверяемость | Нет retrieval, нет extraction, нет permissions |

**Незанятая ниша**: проверяемая, локальная, Markdown-native, MCP-первая память для multi-agent
dev-сценариев. Все конкуренты либо жертвуют верифицируемостью, либо memory intelligence,
либо локальностью.

## 5. Ключевые допущения, требующие проверки

Каждое допущение → отдельная задача в `04-tasks-dod.md` с критерием falsify.

| ID | Допущение | Как проверять |
| ---- | ----------- | --------------- |
| A1 | Markdown-truth не упрётся в производительность до 100k файлов | Бенчмарк read/write/index 10k → 100k → 1M |
| A2 | LLM-extraction-quality достаточна для unattended режима в нечувствительных скоупах | Eval-набор: precision/recall на 200 размеченных диалогах |
| A3 | Multi-signal retrieval (vector+BM25+graph) даёт прирост NDCG@10 ≥ 15% vs только vector | A/B на бенчмарке retrieval |
| A4 | MCP-агенты будут писать в workspace без обхода API | Аудит логов в demo-сценарии |
| A5 | Append-only history не взорвёт диск за 6 месяцев активной работы | Симуляция нагрузки + measure storage growth |
| A6 | Review-flow (для sensitive scopes) не убьёт UX | User study на 5+ разработчиках |

## 6. Критические "не делать" (anti-scope для MVP)

Чтобы не утонуть в фичах, фиксируем что **НЕ** входит в MVP:

- ❌ Web UI / dashboard (только CLI + MCP).
- ❌ Multi-tenant SaaS (single-tenant local-first).
- ❌ Полноценная graph DB — стартуем с edges-таблицы.
- ❌ Distributed mode / replication.
- ❌ Streaming embeddings / online learning.
- ❌ Federated workspaces.
- ❌ Mobile clients.
- ❌ Auto-merge AI-конфликтов памяти (только supersede + review).
- ❌ Custom DSL для policies (берём YAML + jsonlogic-like).
- ❌ Plugin SDK (после стабилизации core).

## 7. Главный продуктовый тезис

> **AI-агент не "помнит" — он оставляет память как проверяемый артефакт, который можно открыть,
> процитировать, откатить, перенести.**

Это формирует все архитектурные ставки: верифицируемость > удобства, провенанс > компактности,
локальность > облачности, явный коммит > молчаливой записи.

## 8. Рекомендация

**Идти в реализацию**, но со следующими корректировками концепции:

1. **Выкинуть из MVP graph DB как сервис** — заменить таблицей edges. Графовая БД появится
   в Phase 5+ только если entity-сценарии докажут необходимость.
2. **Сделать review-flow обязательным** для скоупов: `user_profile`, `medical`, `legal`, `finance`,
   `secrets`. Для остальных — auto-commit с возможностью отката.
3. **Зафиксировать схему frontmatter v1** до начала extraction-работ. Миграции — через `schema_version`.
4. **Eval-набор retrieval — Phase 0**, до начала индексации. Иначе нечем измерять качество.
5. **Bench-suite — Phase 0**, чтобы все архитектурные решения были измеряемы.
6. **Killer-demo сценарий** ("почему мы выбрали Qdrant?") — fixture для всех тестов и продаж.
7. **Безопасность — не Phase 7, а сквозная**: secret-scanner и redaction должны быть в Phase 1.

См. `07-roadmap.md` для детализации.
