# Установка и подключение

> 🇬🇧 [English version](install.md)

## TL;DR — полный стек одной командой

MemoryFS + Levara (vector backend) + Ollama (embedder) + MCP подключён к Claude Code:

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked --force && (docker ps -a --format '{{.Names}}' | grep -q '^levara$' || docker run -d --name levara -p 50051:50051 -p 8080:8080 ghcr.io/stek0v/levara:latest) && (command -v ollama >/dev/null || curl -fsSL https://ollama.com/install.sh | sh) && ollama pull nomic-embed-text && claude mcp add memoryfs --scope user --env LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051 -- memoryfs mcp
```

Что происходит по шагам:

1. `cargo install` — собирает и кладёт `memoryfs` в `~/.cargo/bin/`. `--force` нужен чтобы можно было перезапускать команду для апгрейда.
2. `docker run … levara` — поднимает Levara на `:50051` (gRPC) и `:8080` (HTTP). Пропускается, если контейнер с именем `levara` уже есть.
3. `ollama install + pull` — ставит Ollama (если ещё не стоит), качает `nomic-embed-text` (~274 МБ, 768-dim) — совпадает с дефолтами MemoryFS, env-переменные ставить не надо.
4. `claude mcp add … --env LEVARA_GRPC_ENDPOINT=…` — регистрирует `memoryfs mcp` глобально для Claude Code; env-переменная подключает Levara прямо к MCP, и каждый коммит автоматически индексируется в векторы, а recall возвращает семантические попадания, а не substring-матчи.

Нужно: Rust 1.88+ (`rustup install 1.88`), Docker, [Claude Code CLI](https://docs.claude.com/en/docs/claude-code).

> **Что делает env-переменная:** без `LEVARA_GRPC_ENDPOINT` MCP работает в чисто файловом режиме (recall = substring, без векторов). С ней — MCP на старте поднимает Levara-клиента, после каждого commit фоном гоняет индексер, а recall идёт через гибрид vector+BM25. Бинарь и workflow те же — просто recall богаче. Детали — [`docs/integrations/levara.ru.md`](integrations/levara.ru.md).

Минимальная установка (только MCP, без Levara + Ollama):

```bash
cargo install --git https://github.com/stek0v/memoryfs --locked && claude mcp add memoryfs --scope user -- memoryfs mcp
```

## Зависимости

- Rust 1.88+ (`rustup install 1.88`)
- C-линкер (Xcode CLT на macOS, build-essential на Debian/Ubuntu) — `tonic-build` компилит protobuf'ы при сборке
- Опционально: запущенный [Levara](https://github.com/Stek0v/Levara) для семантического поиска. Используется и `memoryfs serve` (REST), и `memoryfs mcp` (когда выставлен `LEVARA_GRPC_ENDPOINT`). Без него оба режима откатываются на substring-recall по файловому дереву. Полная установка в [`docs/integrations/levara.ru.md`](integrations/levara.ru.md).

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
7. Если выставлен `LEVARA_GRPC_ENDPOINT` — поднимает Levara-движок recall + фоновый индексер; recall идёт через hybrid vector+BM25, каждый коммит автоматически индексируется. Иначе откатывается на substring-recall по файловому дереву.

Любое из этого можно переопределить через env перед запуском агента:

```bash
export MEMORYFS_DATA_DIR=/custom/path
export MEMORYFS_WORKSPACE_ID=ws_my_workspace
export MEMORYFS_TOKEN=utk_alice
export LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051   # включить семантический recall
```

Подключить Levara к уже зарегистрированному MCP-инстансу:

```bash
claude mcp remove memoryfs --scope user
claude mcp add memoryfs --scope user --env LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051 -- memoryfs mcp
```

## Подключение к Cursor / другим MCP-клиентам

Любой MCP-совместимый клиент работает. Транспорт — JSON-RPC 2.0 по stdio. Пример `mcp.json`:

```json
{
  "mcpServers": {
    "memoryfs": {
      "command": "memoryfs",
      "args": ["mcp"],
      "env": {
        "LEVARA_GRPC_ENDPOINT": "http://127.0.0.1:50051"
      }
    }
  }
}
```

Убери блок `env`, если хочешь чисто файловый режим (substring recall, без Levara).

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

Если нужен семантический поиск (vector + BM25 hybrid) вместо substring-фолбэка — натрави `memoryfs serve` на [Levara](https://github.com/Stek0v/Levara). Полная установка, env-переменные и траблшутинг — в [`docs/integrations/levara.ru.md`](integrations/levara.ru.md):

```bash
export LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051
memoryfs serve --bind 127.0.0.1:7777 --data-dir ~/projects/foo/.memory
```

## Multi-tenant deployment

Для shared-деплоев local-mode НЕ используй. Положи явный `policy.yaml` (схема — `specs/schemas/v1/policy.schema.json`) и JWT-токены. См. [`examples/policy.local.yaml`](../examples/policy.local.yaml) для local-mode формы; multi-tenant эквивалент — subject-scoped allow-правила с явными deny-путями.

## Проверка установки

После подключения спроси агента: *«Что есть в моей памяти по этому проекту?»* — он должен дёрнуть `memoryfs_log` и вернуть либо записи, либо пустой список, но никогда ACL-ошибку. Если выпало `no allow rule for read on **` — у тебя сборка до local-mode фикса; обнови через `cargo install --git ... --locked --force`.
