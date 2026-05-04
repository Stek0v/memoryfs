# Levara backend

> 🇬🇧 [English version](levara.md)

[Levara](https://github.com/Stek0v/Levara) — Rust gRPC vector store + embedder. Это предпочтительный retrieval-бэкенд для MemoryFS: один RPC на embed-and-index, один на embed-and-search, нативный hybrid (vector + BM25 + fusion) на стороне сервера.

## Когда Levara реально нужна

| Режим | Нужна Levara? |
|-------|---------------|
| `memoryfs mcp` и хочешь семантический recall (hybrid vector + BM25) | **Да**, выстави `LEVARA_GRPC_ENDPOINT` в env MCP-инстанса. |
| `memoryfs mcp` и ASCII/substring recall устраивает | **Нет.** Не выставляй env — MCP пойдёт чисто файлово. |
| `memoryfs serve` (REST) + нужен семантический поиск | **Да**, выстави `LEVARA_GRPC_ENDPOINT` перед запуском. |
| Multi-tenant деплой с общим retrieval | **Да.** |

Когда MCP получает `LEVARA_GRPC_ENDPOINT`, на старте поднимается Levara-клиент + фоновый индексер; каждый коммит автоматически индексируется, и `memoryfs_recall` идёт через hybrid vector+BM25 вместо substring-сканирования.

## 1. Поднять Levara

### Docker (быстрее всего)

```bash
docker run -d --name levara \
  -p 50051:50051 \
  -p 8080:8080 \
  ghcr.io/stek0v/levara:latest
```

Открывает:
- `50051` — gRPC (обязательный для MemoryFS)
- `8080` — HTTP write path (опционально; включает shared-pool записи для REST-сервера)

### Из исходников

```bash
git clone https://github.com/Stek0v/Levara
cd Levara
cargo run --release -- --grpc-bind 0.0.0.0:50051 --http-bind 0.0.0.0:8080
```

### Проверить, что поднялась

```bash
grpcurl -plaintext localhost:50051 list             # должен показать `levara.v1.LevaraService`
curl  http://localhost:8080/api/v1/health           # 200
```

`grpcurl` нет — `brew install grpcurl` / `apt install grpcurl`.

## 2. Поднять embedder

Levara сама не делает эмбеддинги — проксирует во внешний сервис. Выбери, что у тебя уже крутится:

### Вариант A — Ollama (zero-config локально)

```bash
brew install ollama   # или: curl -fsSL https://ollama.com/install.sh | sh
ollama pull nomic-embed-text          # 274 MB, 768-dim
ollama serve                          # слушает http://localhost:11434
```

Совпадает с дефолтами MemoryFS — env переменные для embedder можно вообще не ставить.

### Вариант B — TEI (Text Embeddings Inference, под GPU)

```bash
docker run -d --name tei -p 8081:80 \
  ghcr.io/huggingface/text-embeddings-inference:latest \
  --model-id google/embeddinggemma-300m
```

И:
```bash
export EMBEDDING_ENDPOINT=http://localhost:8081
export EMBEDDING_MODEL=google/embeddinggemma-300m
export EMBEDDING_DIMENSIONS=768
```

### Вариант C — OpenAI-совместимый API

Что угодно с OpenAI `/v1/embeddings` (vLLM, LM Studio, сам OpenAI):
```bash
export EMBEDDING_ENDPOINT=https://api.openai.com/v1
export EMBEDDING_MODEL=text-embedding-3-small
export EMBEDDING_DIMENSIONS=1536
```

> **Размерность должна совпадать в трёх местах.** Если модель отдаёт 1536, а MemoryFS думает 768 — gRPC `Index` будет отбивать каждый вектор как shape error.

## 3. Подключить MemoryFS

### MCP (Claude Code, Cursor)

Прокинь Levara прямо в env MCP-инстанса — Claude Code пробросит env в чайлд-процесс:

```bash
claude mcp add memoryfs --scope user \
  --env LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051 \
  --env LEVARA_COLLECTION=memoryfs \
  -- memoryfs mcp
```

На старте MCP видит env и:

1. Открывает gRPC-канал к Levara (если не подключился — warning в лог, MCP продолжает работать на substring-фолбэке).
2. Делает `ensure_collection(LEVARA_COLLECTION)` — идемпотентно.
3. Собирает `RetrievalEngine` (Levara vector + локальный BM25 fusion) для `memoryfs_recall`.
4. После каждого успешного `memoryfs_commit` спавнит one-shot задачу индексера — fire-and-forget, не блокирует ответ tool'а.

### REST (`memoryfs serve`)

Тот же env, плюс опциональный HTTP endpoint:

```bash
export LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051
export LEVARA_COLLECTION=memoryfs                # опционально; default "memoryfs"
export LEVARA_HTTP_ENDPOINT=http://127.0.0.1:8080  # опционально; включает HTTP write path

memoryfs serve --bind 127.0.0.1:7777 --data-dir ~/projects/foo/.memory
```

При первом запросе MemoryFS:

1. Открывает gRPC-канал к `LEVARA_GRPC_ENDPOINT` (фейлится сразу, если не достучался).
2. Зовёт `ensure_collection(LEVARA_COLLECTION)` — идемпотентно, создаёт коллекцию если нет.
3. Поднимает фоновую indexer-таску, которая раз в 2 с тащит хвост event log. На каждый `CommitCreated` / `MemoryAutoCommitted` / `MemorySuperseded` гоняет батч изменённых файлов через `BatchEmbedAndIndex`.
4. Переключает retrieval на `LevaraVectorStore::hybrid_search` — серверный RRF-фьюжн вектор + BM25.

В логах увидишь:

```
INFO indexer connected to Levara at http://127.0.0.1:50051
INFO indexer background task started
INFO indexed 3 files (12 chunks) for commit c0ffee...
```

Если переменная **не** выставлена:
```
WARN LEVARA_GRPC_ENDPOINT not set — indexing disabled, search won't work
```
…REST-сервер всё равно стартует, но `/v1/recall` будет работать только substring.

## 4. Полный список env-переменных

| Переменная | Обязательная? | По умолчанию | Назначение |
|------------|---------------|--------------|------------|
| `LEVARA_GRPC_ENDPOINT` | **да** | — | gRPC URL, например `http://localhost:50051` |
| `LEVARA_COLLECTION` | нет | `memoryfs` | Имя коллекции / namespace HNSW |
| `LEVARA_HTTP_ENDPOINT` | нет | — | HTTP base Levara; включает shared-pool записи через `/api/v1/batch_insert` |
| `EMBEDDING_ENDPOINT` | нет | `http://localhost:11434` | Куда Levara/MemoryFS проксируют embed-запросы |
| `EMBEDDING_MODEL` | нет | `nomic-embed-text` | Имя модели для embed-запросов |
| `EMBEDDING_DIMENSIONS` | нет | `768` | Размерность вектора; должна совпадать с тем, что отдаёт модель |

## 5. Проверка end-to-end

После того как заиндексировался хотя бы один коммит:

```bash
curl -X POST http://127.0.0.1:7777/v1/recall \
  -H 'content-type: application/json' \
  -d '{"query": "database choice", "top_k": 5}'
```

Ожидаешь `results: [...]` с полем `score`. Если пришло `results: []`, а данные точно есть — проверь:

1. Логи indexer-а — реально ли он отстрелял `index_batch`.
2. Размер коллекции — `grpcurl -plaintext localhost:50051 levara.v1.LevaraService/CountPoints -d '{"collection": "memoryfs"}'` должно быть > 0.
3. Несовпадение размерности — частый silent failure; перезапусти всё с одинаковым `EMBEDDING_DIMENSIONS`.

## 6. Что MemoryFS получает от Levara

`levara::LevaraVectorStore` даёт:

- **`embed_and_index(text, metadata)`** — один RPC вместо `embed → upsert`.
- **`search_by_text(query, top_k)`** — один RPC вместо `embed → search`.
- **`hybrid_search(query, vector_weight, bm25_weight, top_k)`** — серверный RRF.

Retrieval-движок ([src/retrieval.rs](../../src/retrieval.rs)) автоматически делегирует в `HybridSearch`, если бэкенд это умеет; иначе — локальная фьюжн.

## 7. Откатиться на Qdrant

Levara — preferred бэкенд, но в trait `VectorStore` есть и Qdrant impl — пригодится, если у тебя уже крутится Qdrant и заводить второй сервис не хочется.

```bash
# Не выставлять LEVARA_GRPC_ENDPOINT.
# Текущий `memoryfs serve` подключает только Levara — нужен патч main.rs
# или форк, чтобы прокинуть QdrantVectorStore.
```

`QdrantVectorStore` живёт в [src/vector_store.rs:79](../../src/vector_store.rs:79); подключение в `cmd_serve` — правка строк на 20 в `src/main.rs`.

## 8. Proto-контракт

[`proto/levara.proto`](../../proto/levara.proto) — генерится `tonic-build` на сборке (см. [`build.rs`](../../build.rs)). Обновление: подменить .proto, `cargo clean -p memoryfs && cargo build` пересоздаст Rust-биндинги.

Сырые embedding-векторы MemoryFS никогда не видит при работе с Levara — всё идёт через gRPC.

## 9. Траблшутинг

| Симптом | Вероятная причина | Чинить |
|---------|-------------------|--------|
| `failed to connect to Levara` на старте | не тот порт / Levara не запущена | `docker ps`, `curl localhost:8080/api/v1/health` |
| `failed to ensure Levara collection` | коллекция уже есть с другой размерностью | дропнуть коллекцию в Levara, перезапустить |
| `index_batch error: invalid vector dimension` | mismatch `EMBEDDING_DIMENSIONS` | выровнять три точки: вывод модели, env, коллекция |
| `recall` пустой для заведомо проиндексированных данных | indexer ещё не догнал | подождать 2 с (poll-цикл) или глянуть логи indexer-а |
| Warning `LEVARA_GRPC_ENDPOINT not set` | переменная не пробросилась в shell, который запустил `memoryfs` | `export` или `env LEVARA_GRPC_ENDPOINT=... memoryfs serve` |
