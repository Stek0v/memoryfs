# Testing

> 🇷🇺 [Русская версия](testing.ru.md)

## Run

```bash
cargo test                                          # full suite
cargo test --lib                                    # unit tests only
cargo test mcp::                                    # one module
cargo test -- mcp::tests::write_file_rejects_       # one test
cargo test --release                                # with optimizations (slow build, fast run)
```

## Lint

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

## What's covered

The crate ships ~500 unit / integration tests organized by module. Notable suites:

| Module | Tests of interest |
|--------|-------------------|
| `storage`   | content-address round-trip, idempotent puts, large blobs |
| `commit`    | DAG validity, parent linkage, diff, revert |
| `acl`       | path-glob matching, deny-by-default, override precedence |
| `mcp`       | 17 tool handlers, append-only guard, `initialize.instructions` ships |
| `retrieval` | RRF fusion, ACL post-filter, recency boost |
| `supersede` | append-only chain, cycle detection |
| `audit`     | tamper-evident chain, replay |
| `migration` | up/down, plan finder, cycle detection |
| `chaos`     | corruption recovery, audit truncation, dangling refs |

## Manual MCP end-to-end

The fastest way to verify the behavioral contract works in practice:

```bash
# 1. Build a release binary
cargo build --release

# 2. Point Claude Code (or any MCP client) at it
claude mcp add memoryfs-dev --scope project -- $(pwd)/target/release/memoryfs mcp

# 3. In a fresh session, trigger the contract
# Ask: "We're choosing between Postgres and MySQL for project X — what do you suggest?"
#
# Expected:
#   - Agent calls memoryfs_recall(query="database choice") first  (recall-first)
#   - On your "let's go with Postgres", saves to decisions/db-choice.md silently
#   - On a later "actually let's switch to MySQL", proposes supersede instead of overwrite
```

If the agent overwrites a `decisions/*.md` without going through `memoryfs_supersede_memory`, the server returns an error pointing at the right tool — that guard is in `tool_write_file` in `src/mcp.rs`.

## Property tests

`proptest` is wired into `Cargo.toml` and used in `ids`, `storage`, `commit` to fuzz round-trips and invariants.

## CI

GitHub Actions runs `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test --lib` on push and PR — see [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).
