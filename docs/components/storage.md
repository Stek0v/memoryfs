# `storage` — content-addressable object store

Source: [`src/storage.rs`](../../src/storage.rs)

## Shape

Two pieces:

- **`ObjectStore`** — `objects/<sha256>` files on disk. Writes are atomic (temp file + rename); the same content always hashes to the same path, so duplicate writes are free.
- **`InodeIndex`** — in-memory `path → CommitHash` map. The path can be any UTF-8 string; nothing forces a directory layout, but the rest of the system (MCP append-only guard, retrieval scope filter) relies on the convention `decisions/`, `discoveries/`, `infra/`, etc.

## Why content-addressable

Three properties fall out for free:

1. **Integrity check** — re-hash the bytes, compare to the path; any drift is a corrupted disk.
2. **Deduplication** — two memories with identical content share a single object on disk.
3. **Time travel** — commits store path → hash maps; replaying any commit means looking up the hashes, not diffing files.

## API surface

```rust
let store = ObjectStore::open(data_dir.join("objects"))?;
let hash = store.put(content_bytes)?;        // CommitHash
let bytes = store.get(&hash)?;               // Vec<u8>
store.has(&hash)                             // bool

let mut index = InodeIndex::new();
index.set("decisions/db.md", hash.clone());
let current = index.get("decisions/db.md"); // Option<&CommitHash>
let all_paths = index.paths();              // Vec<&str>
```

## Invariants

- **`put` is idempotent** by hash: writing the same bytes twice is a no-op on disk.
- **Hashes are SHA-256 hex (lowercase, 64 chars)**, validated by `CommitHash::parse`.
- **`get` returns the same bytes that were `put`** — round-trip is property-tested.
- The store has **no concept of paths** — only the inode index does. This keeps the object layer pure.

## Failure modes

- Disk corruption: `get` returns the bytes whose hash doesn't match the requested one → `MemoryFsError::Storage`.
- Missing file: `get` on an unknown hash → `MemoryFsError::NotFound`.
- The inode index is in-memory; persistence is the caller's responsibility (commit log replays it on startup, or `reindex` rebuilds from disk).
