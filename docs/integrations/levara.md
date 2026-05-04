# Levara backend

> 🇷🇺 [Русская версия](levara.ru.md)

[Levara](https://github.com/Stek0v/Levara) is a Rust gRPC vector store + embedder service. It's the preferred retrieval backend for MemoryFS — one round-trip for embed-and-index, one round-trip for embed-and-search, native hybrid (vector + BM25 + fusion) on the server side.

## When you actually need Levara

| Mode | Levara needed? |
|------|----------------|
| `memoryfs mcp` and you want semantic recall (hybrid vector + BM25) | **Yes**, set `LEVARA_GRPC_ENDPOINT` in the MCP entry's env. |
| `memoryfs mcp` and ASCII/substring recall is good enough | **No.** Don't set the env var; MCP runs pure-file. |
| `memoryfs serve` (REST) + you want semantic search | **Yes**, set `LEVARA_GRPC_ENDPOINT` before launching. |
| Multi-tenant deployment with shared retrieval | **Yes.** |

When MCP gets `LEVARA_GRPC_ENDPOINT` it spins up a Levara client + background indexer at startup; every commit then auto-indexes and `memoryfs_recall` uses hybrid vector+BM25 instead of substring scanning.

## 1. Run Levara

### Docker (fastest)

```bash
docker run -d --name levara \
  -p 50051:50051 \
  -p 8080:8080 \
  ghcr.io/stek0v/levara:latest
```

This exposes:
- `50051` — gRPC (mandatory for MemoryFS)
- `8080` — HTTP write path (optional; enables shared-pool writes for the REST server)

### From source

```bash
git clone https://github.com/Stek0v/Levara
cd Levara
cargo run --release -- --grpc-bind 0.0.0.0:50051 --http-bind 0.0.0.0:8080
```

### Verify it's up

```bash
grpcurl -plaintext localhost:50051 list             # should list `levara.v1.LevaraService`
curl  http://localhost:8080/api/v1/health           # should return 200
```

If `grpcurl` isn't installed: `brew install grpcurl` / `apt install grpcurl`.

## 2. Run an embedder

Levara doesn't ship an embedder — it proxies to one. Pick whichever you have running:

### Option A — Ollama (zero-config local)

```bash
brew install ollama   # or: curl -fsSL https://ollama.com/install.sh | sh
ollama pull nomic-embed-text          # 274 MB, 768-dim
ollama serve                          # listens on http://localhost:11434
```

This matches MemoryFS's defaults — you don't need to set any embedder env vars.

### Option B — TEI (Text Embeddings Inference, GPU-friendly)

```bash
docker run -d --name tei -p 8081:80 \
  ghcr.io/huggingface/text-embeddings-inference:latest \
  --model-id google/embeddinggemma-300m
```

Then export:
```bash
export EMBEDDING_ENDPOINT=http://localhost:8081
export EMBEDDING_MODEL=google/embeddinggemma-300m
export EMBEDDING_DIMENSIONS=768
```

### Option C — OpenAI-compatible API

Anything that speaks the OpenAI `/v1/embeddings` shape works (vLLM, LM Studio, OpenAI itself):
```bash
export EMBEDDING_ENDPOINT=https://api.openai.com/v1
export EMBEDDING_MODEL=text-embedding-3-small
export EMBEDDING_DIMENSIONS=1536
```

> **Dimension must match across all three.** If your embedder outputs 1536 dims and you tell MemoryFS 768, the gRPC `Index` call will reject every vector with a shape error.

## 3. Connect MemoryFS

### MCP (Claude Code, Cursor)

Wire Levara directly into the MCP entry — Claude Code passes the env to the child process:

```bash
claude mcp add memoryfs --scope user \
  --env LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051 \
  --env LEVARA_COLLECTION=memoryfs \
  -- memoryfs mcp
```

On startup MCP detects the env and:

1. Opens a gRPC channel to Levara (warning logged on failure; MCP keeps running with substring fallback).
2. Calls `ensure_collection(LEVARA_COLLECTION)` — idempotent.
3. Builds a `RetrievalEngine` (Levara vector + local BM25 fusion) for `memoryfs_recall`.
4. Spawns a one-shot indexer task after every successful `memoryfs_commit` — fire-and-forget, doesn't block the tool response.

### REST (`memoryfs serve`)

Same env, plus optional HTTP endpoint:

```bash
export LEVARA_GRPC_ENDPOINT=http://127.0.0.1:50051
export LEVARA_COLLECTION=memoryfs                # optional; default "memoryfs"
export LEVARA_HTTP_ENDPOINT=http://127.0.0.1:8080  # optional; enables HTTP write path

memoryfs serve --bind 127.0.0.1:7777 --data-dir ~/projects/foo/.memory
```

On the first request MemoryFS will:

1. Open a gRPC channel to `LEVARA_GRPC_ENDPOINT` (fails fast on connect error).
2. Call `ensure_collection(LEVARA_COLLECTION)` — idempotent; creates the collection if missing.
3. Spawn a background indexer task that tails the event log every 2 s. On each `CommitCreated` / `MemoryAutoCommitted` / `MemorySuperseded` event, it batches changed files through `BatchEmbedAndIndex`.
4. Switch retrieval over to `LevaraVectorStore::hybrid_search` — server-side RRF fusion of vector + BM25.

You'll see this in the logs:

```
INFO indexer connected to Levara at http://127.0.0.1:50051
INFO indexer background task started
INFO indexed 3 files (12 chunks) for commit c0ffee...
```

If the env var is **not** set:
```
WARN LEVARA_GRPC_ENDPOINT not set — indexing disabled, search won't work
```
…the REST server still starts, but `/v1/recall` will only do substring search.

## 4. Full env-var reference

| Variable | Required? | Default | Purpose |
|----------|-----------|---------|---------|
| `LEVARA_GRPC_ENDPOINT` | **yes** | — | gRPC URL, e.g. `http://localhost:50051` |
| `LEVARA_COLLECTION` | no | `memoryfs` | Per-collection HNSW namespace |
| `LEVARA_HTTP_ENDPOINT` | no | — | Levara HTTP base; enables shared-pool HTTP writes via `/api/v1/batch_insert` |
| `EMBEDDING_ENDPOINT` | no | `http://localhost:11434` | Where Levara/MemoryFS proxy embedding requests |
| `EMBEDDING_MODEL` | no | `nomic-embed-text` | Model name forwarded to the embedder |
| `EMBEDDING_DIMENSIONS` | no | `768` | Vector dimension; must match what the model produces |

## 5. Verify end-to-end

After ingesting at least one commit:

```bash
curl -X POST http://127.0.0.1:7777/v1/recall \
  -H 'content-type: application/json' \
  -d '{"query": "database choice", "top_k": 5}'
```

Expected: `results: [...]` with `score` fields. If you get `results: []` and you know the data is there, check:

1. Indexer logs — did it actually run `index_batch`?
2. Levara collection — `grpcurl -plaintext localhost:50051 levara.v1.LevaraService/CountPoints -d '{"collection": "memoryfs"}'` should be > 0.
3. Dimension mismatch — common silent failure mode; restart everything with consistent `EMBEDDING_DIMENSIONS`.

## 6. What MemoryFS gets from Levara

The `levara::LevaraVectorStore` impl provides:

- **`embed_and_index(text, metadata)`** — one RPC instead of `embed → upsert`.
- **`search_by_text(query, top_k)`** — one RPC instead of `embed → search`.
- **`hybrid_search(query, vector_weight, bm25_weight, top_k)`** — server-side RRF.

The retrieval engine ([src/retrieval.rs](../../src/retrieval.rs)) automatically delegates to `HybridSearch` when the backend implements it; otherwise falls back to local fusion.

## 7. Switching back to Qdrant

Levara is the preferred backend, but the `VectorStore` trait has a Qdrant impl too — useful if you already run Qdrant and don't want a second service.

```bash
# Don't set LEVARA_GRPC_ENDPOINT.
# Instead provide a custom main.rs path or fork the binary —
# the current `memoryfs serve` only wires Levara.
```

The `QdrantVectorStore` lives in [src/vector_store.rs:79](../../src/vector_store.rs:79); wiring it into `cmd_serve` is a ~20-line change against `src/main.rs`.

## 8. Proto contract

[`proto/levara.proto`](../../proto/levara.proto) — generated at build time by `tonic-build` (see [`build.rs`](../../build.rs)). Updating: replace the proto, `cargo clean -p memoryfs && cargo build` regenerates the Rust bindings.

MemoryFS never sees raw embedding vectors when using Levara — everything goes through the gRPC interface.

## 9. Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `failed to connect to Levara` on startup | wrong port / Levara not running | check `docker ps`, `curl localhost:8080/api/v1/health` |
| `failed to ensure Levara collection` | collection exists with wrong dimension | drop the collection in Levara, restart |
| `index_batch error: invalid vector dimension` | `EMBEDDING_DIMENSIONS` mismatch | align all three: model output, env var, collection |
| `recall` returns empty for known-indexed data | indexer hasn't caught up yet | wait 2 s (poll cycle) or check indexer logs |
| `LEVARA_GRPC_ENDPOINT not set` warning | env var missing in the shell that launched `memoryfs` | `export` it, or use `env LEVARA_GRPC_ENDPOINT=... memoryfs serve` |
