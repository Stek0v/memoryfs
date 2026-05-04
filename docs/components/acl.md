# `acl` — access control

Source: [`src/acl.rs`](../../src/acl.rs), [`src/policy.rs`](../../src/policy.rs)

## The check

Every operation on the store goes through:

```rust
acl::check(subject, action, path, &policy)?;
```

Returns `Ok(())` or `MemoryFsError::Acl` with the rule that denied. Both REST and MCP layers call this before any storage mutation; nothing in the system is supposed to write past the check unless the test bypasses storage entirely.

## Policy shape

```yaml
schema_version: memoryfs/v1
default_acl:
  deny_by_default: true
  allow:
    - path: "memory/user/**"
      subjects: ["user:alice"]
      actions: ["read", "write", "list"]
    - path: "decisions/**"
      subjects: ["user:*"]
      actions: ["read"]
  deny:
    - path: "infra/secrets/**"
      subjects: ["*"]
redaction:
  fail_closed: true
review:
  require_review_for: ["pii", "secret", "medical", "legal", "financial"]
  low_confidence_threshold: 0.6
  scope_org_requires_review: true
  review_ttl_hours: 168
indexing:
  auto_index: true
```

Full schema: [`specs/schemas/v1/policy.schema.json`](../../specs/schemas/v1/policy.schema.json).

## Path matching

Glob style:

- `**` matches any depth, any path components.
- `*` matches a single path segment.
- Literal segments match themselves (`decisions/db.md`).

Allow rules grant; deny rules override. Subject matching supports `user:alice` (literal) and `user:*` (wildcard).

## Local mode shortcut

```rust
let policy = Policy::local_user("user:alice");
```

Produces a policy that allows the named subject `read|write|list|review|commit|revert` on `**`, while keeping `redaction.fail_closed = true` and the default sensitive-content review list. This is what `memoryfs mcp` uses by default — there's no auth boundary inside a single-user process.

## Multi-tenant

Load explicitly:

```rust
let yaml = std::fs::read_to_string(".memory/policy.yaml")?;
let policy = Policy::from_yaml(&yaml)?;
```

The policy file is part of the workspace; redeploying the binary doesn't change permissions.

## What gets audited

Every `acl::check` (allow or deny) is appended to the audit log with:

- subject, action, path
- policy decision (allow / deny + matching rule)
- timestamp + chain hash

Tampering is detectable on replay (see [`audit`](audit.md)).
