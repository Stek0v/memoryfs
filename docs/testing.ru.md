# Тестирование

> 🇬🇧 [English version](testing.md)

## Запуск

```bash
cargo test                                          # полный набор
cargo test --lib                                    # только юнит-тесты
cargo test mcp::                                    # один модуль
cargo test -- mcp::tests::write_file_rejects_       # один тест
cargo test --release                                # с оптимизациями (долгая сборка, быстрый прогон)
```

## Линтеры

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## Что покрыто

В крейте ~500 unit / integration тестов, разбитых по модулям. Основные:

| Модуль | Что проверяется |
|--------|-----------------|
| `storage`   | round-trip контент-адресации, идемпотентность put, большие блобы |
| `commit`    | валидность DAG, связь с родителем, diff, revert |
| `acl`       | сопоставление glob-путей, deny-by-default, приоритет правил |
| `mcp`       | 17 tool handler-ов, append-only guard, `initialize.instructions` |
| `retrieval` | RRF-фьюжн, ACL post-filter, recency boost |
| `supersede` | append-only цепочка, детект циклов |
| `audit`     | tamper-evident цепочка, replay |
| `migration` | up/down, поиск плана, детект циклов |
| `chaos`     | восстановление после повреждений, обрыв audit, висячие ссылки |

## Ручной MCP end-to-end

Самый быстрый способ убедиться, что поведенческий контракт работает на практике:

```bash
# 1. Собрать release-бинарь
cargo build --release

# 2. Подключить к Claude Code (или любому MCP-клиенту)
claude mcp add memoryfs-dev --scope project -- $(pwd)/target/release/memoryfs mcp

# 3. В новой сессии триггернуть контракт
# Спросите: «Выбираем между Postgres и MySQL для проекта X — что посоветуешь?»
#
# Ожидаемое поведение:
#   - Агент сначала вызывает memoryfs_recall(query="database choice")  (recall-first)
#   - На «давай Postgres» — молча сохраняет в decisions/db-choice.md
#   - Позже на «нет, переходим на MySQL» — предлагает supersede, а не перезапись
```

Если агент перезаписывает `decisions/*.md` напрямую (минуя `memoryfs_supersede_memory`), сервер вернёт ошибку с указанием правильного инструмента — гард живёт в `tool_write_file` в `src/mcp.rs`.

## Property-тесты

`proptest` подключён в `Cargo.toml` и используется в `ids`, `storage`, `commit` для фаззинга round-trip и инвариантов.

## CI

GitHub Actions гоняет `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test --lib` на push и PR — см. [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).
