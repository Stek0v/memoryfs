# `audit` — tamper-evident log

Source: [`src/audit.rs`](../../src/audit.rs)

## Shape

Append-only NDJSON at `audit_log`. Each line:

```json
{
  "id": "evt_01J...",
  "timestamp": "2026-05-04T12:34:56Z",
  "subject": "user:alice",
  "action": "write",
  "path": "decisions/db.md",
  "decision": "allow",
  "rule": "memory/** allow user:alice",
  "prev_hash": "abc123...",
  "this_hash": "def456..."
}
```

## Hash chain

Each entry's `this_hash` is `sha256(prev_hash || canonical_json(entry_minus_this_hash))`. `prev_hash` of the first entry is the empty string. To verify the log:

```rust
let report = AuditLog::verify(path)?;
assert!(report.valid);
assert_eq!(report.entries_checked, n);
```

Any single tampered or removed line breaks the chain at that point and downstream entries fail verification. Detection is reliable; restoration is not — once tampered, the only fix is restore from backup.

## What gets logged

Every `acl::check` (allow and deny), every commit, every supersede, every backup / restore / migration event. The audit module is the canonical timeline for "who did what when, and was it allowed."

## Replay

`AuditLog::replay(path)` returns the parsed event stream. Useful for:

- Reconstructing the inode index from scratch (events carry the path → hash mapping).
- Debugging "who deleted X" — search for the matching event.
- Compliance reports — filter by subject and time window.

## Performance

Append is O(1) — one disk write per event. Verification is O(n) — must walk the chain. For deployments with millions of events, the chunked verification mode (`verify_range`) checks a window without full replay.
