# Security

## Threat model

MemoryFS handles AI agent memories that may contain PII, credentials,
medical/legal/financial data, and proprietary information. The threat model
assumes compromised agents, malicious inputs, and insider threats.

See `threat-model.md` for the full analysis.

## Authentication

JWT-based authentication with two token types:

- **User tokens** (`utk_` prefix): full access, issued to human operators
- **Agent tokens** (`atk_` prefix): scoped access, issued to AI agents

Tokens are validated via the `jsonwebtoken` crate (ADR-005). The `alg=none`
attack vector is explicitly rejected.

## Authorization (ACL)

Path-glob access control engine (`acl.rs`). Rules are evaluated top-to-bottom
with first-match semantics:

```text
(pattern: "memories/private/**", principal: "agent:*", action: Read, effect: Deny)
(pattern: "memories/**",         principal: "*",       action: Read, effect: Allow)
```

Actions: `Read`, `Write`, `Review`, `Admin`.

Every ACL denial is recorded in the audit log with the requesting principal,
target path, and attempted action.

## Sensitive memory handling

Memories classified as sensitive (`pii`, `secret`, `medical`, `legal`,
`financial`) go through a mandatory review process:

1. Agent proposes memory via `remember` / `propose`
2. Memory enters the inbox (`inbox.rs`) as a proposal
3. `MemoryPolicy` evaluates — auto-commit for safe content, review-required
   for sensitive classifications
4. Reviewer approves or rejects via `review` tool
5. Only approved memories enter the active set

## Secret detection and redaction

The redaction engine (`redaction.rs`) scans content pre-commit for 20+
secret patterns:

- AWS access keys and secret keys
- GitHub/GitLab tokens
- JWT tokens
- API keys (generic patterns)
- Private keys (RSA, EC, etc.)
- Database connection strings
- OAuth tokens and refresh tokens
- Slack webhooks
- Basic auth credentials in URLs

Detected secrets are redacted (replaced with `[REDACTED:<type>]`) before
storage. The original content is never persisted.

Post-extraction scan (`post_scan.rs`) provides a second pass after LLM
extraction to catch secrets that might have been introduced by the
extraction process.

## Audit trail

Tamper-evident audit log (`audit.rs`):

- Append-only NDJSON format
- Optional SHA-256 hash chain (each entry includes hash of previous)
- Per-event fsync for crash safety
- Records: file writes, commits, reverts, reviews, ACL denials, redactions

Chain verification: `AuditLog::verify_chain()` detects any modification
to historical entries.

## Adversarial test suites

Located in `tests/adversarial/`:

- **Secrets suite** (50 cases): validates redaction of all supported
  credential patterns, including edge cases and false positives
- **Injection suite** (12 scenarios): tests for prompt injection,
  frontmatter injection, path traversal, and XSS in memory content
- **Schema violations** (20 cases): malformed frontmatter, invalid
  field values, missing required fields

## Chaos engineering

The chaos test suite (`chaos.rs`, 25 tests) validates data integrity
under adverse conditions:

- Object store corruption detection and recovery
- Commit graph conflict detection under concurrent writes
- Audit log tamper detection and truncation handling
- Backup integrity verification with corrupt objects
- Migration rollback safety

## Security conventions

- No `unwrap()` or `panic!()` in production code paths
- All errors route through `MemoryFsError` with appropriate HTTP status codes
- Pre-commit hooks run `gitleaks` for secret scanning
- Every new security gap adds a test case to the adversarial suite
- PRs touching `policy/` or `redaction/` require security owner review
