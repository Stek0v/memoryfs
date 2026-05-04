# `commit` — commit DAG

Source: [`src/commit.rs`](../../src/commit.rs)

## What's a commit

```rust
struct Commit {
    hash: CommitHash,                  // sha256 of the canonical encoding
    parent: Option<CommitHash>,        // None = initial
    author: String,                    // subject from auth context
    message: String,
    snapshot: HashMap<String, CommitHash>,  // path → object hash
    timestamp: DateTime<Utc>,
}
```

A commit captures the entire `InodeIndex` snapshot (path → hash) at the moment of the call, not a diff. Diffs are derived on demand by comparing two snapshots.

## Why a DAG, not a chain

Most workflows are linear, but the type allows divergent histories — useful for:

- Background extraction that runs against an older base while the user stages new writes.
- Backup/restore landing a parallel branch that gets merged later.
- Multi-tenant deployments where two subjects independently commit unrelated paths.

The shipped CLI / MCP currently only takes the latest commit's snapshot as the parent for the next commit, but the DAG type is in place.

## API

```rust
let mut log = CommitGraph::new();
let commit = log.commit(author, message, snapshot, parent_hash)?;
log.head();                           // Option<&Commit>
log.get(&hash);                       // Option<&Commit>
log.diff(&from_hash, &to_hash)?;      // Vec<DiffEntry>
log.revert(&target_hash)?;            // restores snapshot, makes new commit
```

## Diff semantics

`diff(from, to)` walks both snapshots and emits one `DiffEntry` per changed path:

- **Added** — path absent in `from`, present in `to`.
- **Removed** — path in `from`, absent in `to`.
- **Modified** — path in both with different hashes.

Equal paths with equal hashes produce no entry.

## Revert semantics

`revert(target)` does NOT rewrite history. It creates a new commit whose snapshot equals the target's snapshot, parented on `head`. The audit log keeps both the original mistake and the revert; the audit chain stays intact.

## Invariants

- Every commit's `parent` (if any) must already exist in the graph — enforced at insert.
- A commit's hash is a function of its content; recomputing must reproduce it.
- `revert` never deletes prior commits.
