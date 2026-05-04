# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial public release of the deployable subset.
- Single-crate layout: `memoryfs` (lib + bin) replaces the prior `memoryfs-core` + `memoryfs` workspace split from the planning repo.
- MCP server (`memoryfs mcp`) with 17 tools matching `specs/mcp.tools.json`.
- REST server (`memoryfs serve`) implementing `specs/openapi.yaml`.
- `initialize.instructions` ships the behavioral contract — recall-first, path conventions, supersede-only for decisions/discoveries, what-not-to-save.
- Server-side append-only guard on `decisions/` and `discoveries/`: plain `write_file` is rejected with a pointer to `memoryfs_supersede_memory`. Escape hatch: `force=true`.
- `Policy::local_user(subject)` for single-user MCP mode (auto-grants the running user full access; redaction stays fail-closed).
- Local-mode bootstrap in `cmd_mcp`: auto-detects project root via `git rev-parse --show-toplevel`, derives `workspace_id` from the canonical path, creates `.memory/` if missing.
- Deterministic `MemoryId::from_path` for idempotent re-indexing.
- Documentation: architecture (EN + RU), install (EN + RU), per-component docs, integration guides for Claude Code / REST / Levara, manual e2e checklist.

### Notes
- This repo is the deployable subset of [memoryfs-planning](https://github.com/stek0v/memoryfs-planning) — design docs, eval pipeline, adversarial suites, fixtures, and the Python extractor stub stay there.
