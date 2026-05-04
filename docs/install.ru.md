# Установка и подключение

> 🇬🇧 [English version](install.md)

## Зависимости

- Rust 1.88+ (`rustup install 1.88`)
- C-линкер (Xcode CLT на macOS, build-essential на Debian/Ubuntu) — `tonic-build` компилит protobuf'ы при сборке
- Опционально: запущенный [Levara](https://github.com/Stek0v/Levara) кластер для vector retrieval. Без него бинарь работает — local `recall` фолбэкается на substring.

## Сборка и установка

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked
```

Бинарь падает в `~/.cargo/bin/memoryfs`. Проверь:

```bash
memoryfs --version
```

## Подключение к Claude Code (рекомендуется)

Регистрируем глобально — каждый проект в каждом workspace получает память автоматически:

```bash
claude mcp add memoryfs --scope user -- memoryfs mcp
```

Что происходит при первом использовании в проекте:

1. Claude Code поднимает `memoryfs mcp` как stdio-чайлд процесс.
2. Бинарь зовёт `git rev-parse --show-toplevel` (или фолбэк на cwd) — находит project root.
3. Создаёт `<project>/.memory/` если нет.
4. Выводит стабильный `workspace_id` из канонического пути проекта (SHA-256 → ULID).
5. Ставит local-mode subject (`utk_<твой-юзер>`) и выдаёт ему полный доступ через `Policy::local_user`.
6. Возвращает поведенческий контракт через `initialize.instructions` — см. [`src/mcp_instructions.md`](../src/mcp_instructions.md).

Любое из этого можно переопределить через env перед запуском агента:

```bash
export MEMORYFS_DATA_DIR=/custom/path
export MEMORYFS_WORKSPACE_ID=ws_my_workspace
export MEMORYFS_TOKEN=utk_alice
```

## Подключение к Cursor / другим MCP-клиентам

Любой MCP-совместимый клиент работает. Транспорт — JSON-RPC 2.0 по stdio. Пример `mcp.json`:

```json
{
  "mcpServers": {
    "memoryfs": {
      "command": "memoryfs",
      "args": ["mcp"]
    }
  }
}
```

## REST режим

Поднять долгоживущий REST-сервер (для не-MCP интеграций или удалённого доступа):

```bash
memoryfs serve --bind 127.0.0.1:7777 \
  --data-dir ~/projects/foo/.memory
```

OpenAPI: [`specs/openapi.yaml`](../specs/openapi.yaml). Health check:

```bash
curl http://127.0.0.1:7777/v1/health
```

## Multi-tenant deployment

Для shared-деплоев local-mode НЕ используй. Положи явный `policy.yaml` (схема — `specs/schemas/v1/policy.schema.json`) и JWT-токены. См. [`examples/policy.local.yaml`](../examples/policy.local.yaml) для local-mode формы; multi-tenant эквивалент — subject-scoped allow-правила с явными deny-путями.

## Проверка установки

После подключения спроси агента: *«Что есть в моей памяти по этому проекту?»* — он должен дёрнуть `memoryfs_log` и вернуть либо записи, либо пустой список, но никогда ACL-ошибку. Если выпало `no allow rule for read on **` — у тебя сборка до local-mode фикса; обнови через `cargo install --git ... --locked --force`.
