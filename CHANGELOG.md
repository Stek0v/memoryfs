# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed (2026-05-06 — monorepo consolidation)
- `stek0v/memoryfs` is now the **single canonical home** for the project. The previously-separate planning repository [`stek0v/LevaraOs`](https://github.com/stek0v/LevaraOs) has been archived and its contents merged here. Production engine (`crates/`), spec contracts (`specs/`), planning prose (`docs/planning/`), test fixtures (`fixtures/`), adversarial test suites (`tests/adversarial/`), Python workers (`workers/`), eval harnesses (`eval/`), benchmarks (`bench/`), and dev tooling (`Justfile`, `scripts/`, lint configs) all live here now.
- Crate layout switched to a Cargo workspace with members `crates/core` (lib `memoryfs`, package `memoryfs-core`) and `crates/cli` (bin `memoryfs`, package `memoryfs-cli`). `cargo install --git https://github.com/stek0v/memoryfs --locked` continues to work without a `--bin` flag (`default-members = ["crates/cli"]`).
- `CONTRIBUTING.md` replaced with the more comprehensive 96-line version from the former planning repo.

### Added
- Initial public release.
- MCP server (`memoryfs mcp`) with 17 tools matching `specs/mcp.tools.json`.
- REST server (`memoryfs serve`) implementing `specs/openapi.yaml`.
- `initialize.instructions` ships the behavioral contract — recall-first, path conventions, supersede-only for decisions/discoveries, what-not-to-save.
- Server-side append-only guard on `decisions/` and `discoveries/`: plain `write_file` is rejected with a pointer to `memoryfs_supersede_memory`. Escape hatch: `force=true`.
- `Policy::local_user(subject)` for single-user MCP mode (auto-grants the running user full access; redaction stays fail-closed).
- Local-mode bootstrap in `cmd_mcp`: auto-detects project root via `git rev-parse --show-toplevel`, derives `workspace_id` from the canonical path, creates `.memory/` if missing.
- Deterministic `MemoryId::from_path` for idempotent re-indexing.
- Documentation: architecture (EN + RU), install (EN + RU), per-component docs, integration guides for Claude Code / REST / Levara, manual e2e checklist.

