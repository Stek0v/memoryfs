# Contributing

Short version: open an issue for anything non-trivial before sending a PR. Run `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` locally before pushing.

## Dev loop

```bash
git clone https://github.com/stek0v/memoryfs
cd memoryfs
cargo build              # ~1 min cold (tonic-build compiles protos)
cargo test --lib         # ~1s after build
cargo run -- mcp         # try the MCP server locally
```

## Code style

- `cargo fmt` is enforced in CI.
- `cargo clippy --all-targets -- -D warnings` is enforced in CI.
- MSRV is 1.88, declared in `rust-toolchain.toml`.
- Async traits via `async-trait`. No `unwrap()` / `panic!()` in production paths — use `MemoryFsError`.
- Structured logging via `tracing`.
- Errors are `thiserror`-based variants on `MemoryFsError`. Map to HTTP/MCP status codes via `error::status_code`.

## Tests

- Unit tests live in `#[cfg(test)] mod tests` blocks at the bottom of each module.
- Use `tempfile::tempdir()` for any test that touches disk.
- Property tests via `proptest` for round-trips and invariants.
- Don't mock storage in retrieval / commit tests — use the real `ObjectStore` against a tempdir. Mock-vs-real divergence is a real source of bugs.

## Commit messages

Conventional commits: `feat(scope): subject`, `fix(scope): subject`, `docs(scope): subject`. Scope is the module or area (`mcp`, `acl`, `retrieval`, `docs`). Body: explain *why*, not *what* — the diff already shows what changed.

## What changes need a corresponding spec update

If your PR touches:

- A schema → update `specs/schemas/v1/*.json` and any fixtures.
- An API endpoint → update `specs/openapi.yaml`.
- An MCP tool → update `specs/mcp.tools.json` and the manifest in `src/mcp.rs` (the `tool_manifest_has_17_tools` test will catch a forgotten count).
- The behavioral contract → edit `src/mcp_instructions.md` (tests assert load-bearing strings are present).

## Reporting security issues

Email instead of opening a public issue. Use the address listed at https://github.com/stek0v/memoryfs/security.
