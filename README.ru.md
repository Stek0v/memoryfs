# MemoryFS

Верифицируемая per-project память для AI-агентов. Markdown-файлы — источник истины; vector / BM25 / graph индексы — расходный материал. Поставляется одним бинарём, который поднимает REST API и MCP (Model Context Protocol) сервер — любой MCP-совместимый агент (Claude Code, Cursor, кастомные агенты) сразу получает persistent project memory.

> 🇬🇧 English version: [README.md](README.md)

---

## Зачем это нужно

LLM-агенты теряют все решения сразу как сессия завершилась или сработал `/compact`. Чат-история не переживает рестарт процесса; vector DB сам по себе теряет provenance и audit trail. MemoryFS делает `<project>/.memory/` каноническим хранилищем:

- **Markdown — это truth.** Каждая память — `.md` с frontmatter. Индексы (vector, BM25, entity graph) пересобираются из этих файлов — даже если индекс умрёт, данные не пропадут.
- **Append-only audit.** Decisions и discoveries нельзя молча перезаписать. `supersede` сохраняет старую версию со `status: superseded` и привязывает новую через `supersedes: [old_id]`.
- **ACL по умолчанию.** Workspace `Policy` решает кто может read / write / commit на какие пути. Local single-user mode выдаёт текущему пользователю полный доступ; multi-tenant — deny-by-default.
- **Поведенческий контракт едет с сервером.** MCP `initialize.instructions` инструктирует агента когда recall, когда save, когда supersede — никаких per-machine настроек.

## Установка

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked
```

Требуется Rust 1.88+. Ставит один бинарь `memoryfs`.

## Подключение к Claude Code (MCP)

Зарегистрируй глобально — каждый проект автоматически получит память:

```bash
claude mcp add memoryfs --scope user -- memoryfs mcp
```

Всё. Открываешь любой проект — бинарь сам определяет project root через `git rev-parse --show-toplevel` (или cwd), создаёт `<project>/.memory/`, выводит стабильный `workspace_id` из канонического пути и поднимает 17 MCP-инструментов. Поведенческий контракт (recall-first, append-only decisions, supersede-семантика) приходит в `initialize` handshake — см. [`src/mcp_instructions.md`](src/mcp_instructions.md).

Подробнее: [docs/install.md](docs/install.md).

## REST-сервер

```bash
memoryfs serve --bind 127.0.0.1:7777
```

OpenAPI 3.1: [`specs/openapi.yaml`](specs/openapi.yaml).

## Архитектура за 30 секунд

```
.memory/
├── objects/          content-addressable blob store (sha256 → bytes)
├── inode_index       path → текущий hash
├── commit_log        DAG коммитов (parent + snapshot)
├── audit_log         tamper-evident NDJSON
├── decisions/*.md    append-only, только через supersede
├── discoveries/*.md  append-only, только через supersede
├── infra/*.md        изменяемые факты
├── events/YYYY-MM-DD-*.md
└── preferences/*.md
```

Над хранилищем — две поверхности: REST API (axum) и MCP-сервер (JSON-RPC по stdio). Обе проходят через тот же `acl::check`, ту же `Policy`, тот же audit trail. Опциональный retrieval подключает векторный backend ([Levara](https://github.com/Stek0v/Levara) предпочтителен, Qdrant поддерживается) плюс Tantivy BM25, фьюзится через Reciprocal Rank Fusion с ACL post-filter.

Полностью: [docs/architecture.md](docs/architecture.md).

## Документация по компонентам

| Компонент | Что внутри |
|-----------|------------|
| [storage](docs/components/storage.md)     | Object store + inode index |
| [commit](docs/components/commit.md)       | Commit DAG, diff, revert |
| [acl](docs/components/acl.md)             | Path-glob policy engine |
| [mcp](docs/components/mcp.md)             | 17 MCP-tools, instructions, append-only guard |
| [retrieval](docs/components/retrieval.md) | Vector + BM25 + RRF + ACL post-filter |
| [indexing](docs/components/indexing.md)   | Event-driven chunk → embed → upsert |
| [audit](docs/components/audit.md)         | Tamper-evident NDJSON лог |

Интеграции: [Claude Code](docs/integrations/claude-code.md) · [REST API](docs/integrations/rest-api.md) · [Levara](docs/integrations/levara.md)

## Тестирование

```bash
cargo test                                         # вся тест-сюита
cargo test mcp::tests::                            # один модуль
cargo test -- mcp::tests::write_file_rejects_     # один тест
```

End-to-end проверка MCP-поведения вручную: [docs/testing.md](docs/testing.md).

## Статус

Phase 7 (hardening). Storage, commit, ACL, MCP, retrieval, indexing, backup, migration реализованы и покрыты тестами. Eval pipeline (SQuAD-2.0 e2e) и adversarial suites живут в [planning-репо](https://github.com/stek0v/memoryfs-planning) и не входят в этот репо — здесь только deployable subset.

## Лицензия

MIT — см. [LICENSE](LICENSE).
