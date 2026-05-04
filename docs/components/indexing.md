# `indexer` — event-driven indexing

Source: [`src/indexer.rs`](../../src/indexer.rs), [`src/event_log.rs`](../../src/event_log.rs), [`src/chunker.rs`](../../src/chunker.rs), [`src/reindex.rs`](../../src/reindex.rs)

## Why event-driven

Writes never block on indexing. The hot path (`tool_write_file`, `POST /v1/files`) only:

1. Puts content into `objects/<hash>`.
2. Updates the inode index.
3. Appends an event to the NDJSON event log.

That's it. The indexer worker tails the event log on its own schedule and does the slow work (chunk, embed, upsert) in the background.

## Event log

Append-only NDJSON at `event_log/`. Each consumer (the indexer is one, but the API allows others) maintains its own offset file, so multiple consumers don't need coordination. New events past the offset get processed; nothing is ever rewritten.

## Chunking

`chunker.rs` does heading-aware markdown splitting:

- Splits at `#`, `##`, etc. boundaries first.
- Within a section, falls back to a target token count with overlap.
- Optionally prepends `document_title` to each chunk's body so embeddings carry document context (richer recall on short queries).

Chunk IDs are derived: `MemoryId::from_path(path)` produces a deterministic ULID from the file path. Re-indexing the same file under a different content hash produces the same chunk ID prefix, so the upsert replaces prior chunks instead of accumulating duplicates. This is the fix for the "ID rerolling" bug — see commit `c11e509`.

## Embed + upsert

For each chunk:

```rust
let vec = embedder.embed(chunk.text).await?;
vector_store.upsert(chunk.id, vec, chunk.metadata).await?;
bm25_index.add(chunk.id, &chunk.text)?;
```

The metadata carries `path`, `scope`, `created_at` (from frontmatter), and any sensitivity labels — used by retrieval's recency boost and ACL post-filter.

## Full reindex

`reindex.rs` rebuilds everything from disk:

1. Walks the inode index (or `decisions/`, `discoveries/`, etc. if the index is gone).
2. Reads each object, chunks, embeds, upserts.
3. Writes a checkpoint after every N files so a crash mid-rebuild can resume.

Used after a backup restore or schema migration.

## Failure mode

If the embedder is unreachable (Levara down, network blip), the worker logs and retries with backoff. The event log and inode index are unaffected — recall just misses the new chunks until the worker catches up. The `auto_index: false` policy switch turns the worker off entirely, useful for offline or air-gapped setups.
