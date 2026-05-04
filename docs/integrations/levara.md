# Levara backend

[Levara](https://github.com/Stek0v/Levara) is a Rust gRPC vector store + embedder service. It's the preferred retrieval backend for MemoryFS — one round-trip for embed-and-index, one round-trip for embed-and-search, native hybrid (vector + BM25 + fusion) on the server side.

## When to use it

- You want server-side hybrid search (skip the local Tantivy step).
- You want a single component owning embeddings, so MemoryFS doesn't need a separate TEI / OpenAI-compat embedder.
- You're deploying multiple MemoryFS instances against shared retrieval state.

If you're running purely local single-user, the bundled fallback (HTTP embedder + local Tantivy) works fine.

## Wire it up

```bash
# 1. Run Levara
docker run -p 50051:50051 ghcr.io/stek0v/levara:latest

# 2. Point MemoryFS at it
export MEMORYFS_VECTOR_BACKEND=levara
export LEVARA_GRPC_ADDR=http://127.0.0.1:50051
export LEVARA_COLLECTION=memoryfs_default

memoryfs serve
```

The CLI's `serve` and `mcp` commands both pick this up at startup. If `MEMORYFS_VECTOR_BACKEND=qdrant`, the legacy `QdrantVectorStore` is used instead — see `src/vector_store.rs`.

## What MemoryFS gets from Levara

The `levara::LevaraVectorStore` impl provides:

- **`embed_and_index(text, metadata)`** — one RPC instead of `embed → upsert`.
- **`search_by_text(query, top_k)`** — one RPC instead of `embed → search`.
- **`hybrid_search(query, vector_weight, bm25_weight, top_k)`** — server-side RRF.

The retrieval engine (`src/retrieval.rs`) automatically delegates to `HybridSearch` when the backend implements it; otherwise falls back to local fusion.

## Proto

[`proto/levara.proto`](../../proto/levara.proto) — generated at build time by `tonic-build` (see `build.rs`). Updating: replace the proto, `cargo clean -p memoryfs && cargo build` regenerates Rust bindings.

## Embeddings

Levara picks the embedder per collection at create time — typically `EmbeddingGemma` for code/text. MemoryFS never sees raw embedding vectors when using Levara; everything goes through the gRPC interface.
