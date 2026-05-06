# Getting Started with MemoryFS

MemoryFS is a verifiable memory workspace for AI agents. This guide walks you
through setup, first workspace operations, and common troubleshooting.

## Prerequisites

| Tool | Minimum version | Check |
|------|----------------|-------|
| Rust (via rustup) | 1.88 | `rustc --version` |
| just | any | `just --version` |
| Docker + Compose | Docker 24+ | `docker --version` |
| Python | 3.11+ | `python3 --version` |

## Quick setup

```bash
# Clone and enter the repo
git clone https://github.com/memoryfs/memoryfs.git
cd memoryfs

# One-command bootstrap (installs Rust toolchain, Python venv, pre-commit hooks, starts Docker)
just bootstrap

# Verify everything is in place
just env-check
```

Or run the dev-setup script directly:

```bash
bash scripts/dev-setup.sh
```

To skip Docker (e.g. CI environments without a daemon):

```bash
just bootstrap-no-docker
```

## Build and test

```bash
# Build the workspace
cargo build --workspace

# Run all Rust tests
cargo test --workspace

# Or via just (includes contract and adversarial suites)
just test

# Lint
just lint

# Auto-fix formatting + clippy
just fix
```

### Running a single test

```bash
# Rust: run a specific test by name
cargo test -p memoryfs-core -- test_name

# With output visible
cargo test -p memoryfs-core -- test_name --nocapture
```

## First workspace session

Once built, use the CLI to interact with a workspace:

```bash
# Build the CLI
cargo build -p memoryfs-cli

# Check server health (requires a running API server)
cargo run -p memoryfs-cli -- health --url http://127.0.0.1:3000

# Write a memory file
cargo run -p memoryfs-cli -- write \
  --url http://127.0.0.1:3000 \
  --path memory/first.md \
  --content "---
type: memory
status: active
---
My first memory."

# Commit staged changes
cargo run -p memoryfs-cli -- commit \
  --url http://127.0.0.1:3000 \
  --message "add first memory"

# Read it back
cargo run -p memoryfs-cli -- read \
  --url http://127.0.0.1:3000 \
  --path memory/first.md

# View commit log
cargo run -p memoryfs-cli -- log --url http://127.0.0.1:3000

# List files
cargo run -p memoryfs-cli -- list --url http://127.0.0.1:3000
```

## Infrastructure

The dev Docker stack provides:

| Service | Address | Purpose |
|---------|---------|---------|
| PostgreSQL 16 (pgvector) | `127.0.0.1:5433` | Vector store, audit |
| Embedding TEI | `127.0.0.1:8090` | Local embedding inference |

```bash
just dev-up      # start services
just dev-down    # stop (data preserved)
just dev-reset   # stop + delete volumes
```

## Project layout

```text
crates/core/     Rust library: storage, ACL, commits, retrieval, observability
crates/cli/      Rust binary: workspace CLI (clap)
specs/           Machine contracts: JSON Schemas, OpenAPI, MCP tools
fixtures/        Golden test data (killer-demo workspaces)
tests/adversarial/  Security suites: secrets, injection, schema violations
scripts/         Bootstrap, validation, Docker compose
```

## Validation

Run all contract validations (fast, no cargo needed):

```bash
just validate-all
```

This checks JSON/YAML syntax, schema well-formedness, fixture frontmatter validation,
and schema-violation self-tests.

## Troubleshooting

### `rustc --version` shows < 1.88

```bash
rustup update stable
rustup default stable
```

### `just: command not found`

```bash
cargo install just
# or
brew install just
```

### Docker services won't start

Check that Docker daemon is running:

```bash
docker info
```

If port conflicts occur (5433, 8090), stop conflicting services or edit
`scripts/docker-compose.dev.yml` to use different ports.

### `cargo test` fails with linker errors

Ensure you have the system C toolchain installed:

```bash
# macOS
xcode-select --install

# Linux (Debian/Ubuntu)
sudo apt install build-essential
```

### `just validate-all` fails on Python imports

The validation scripts need Python 3.11+ with `pyyaml` and `jsonschema`:

```bash
pip3 install pyyaml jsonschema
```

Or run `just bootstrap` which sets up a venv with all dependencies.

### Pre-commit hooks fail

```bash
pre-commit install
pre-commit run --all-files
```

If `gitleaks` fails, check that you haven't accidentally committed secrets.
See `tests/adversarial/secrets-suite/` for the detection patterns.

### Tests pass locally but CI fails

1. Check CI logs for the failing job (validate, rust, adversarial, api-specs).
2. Run the exact failing command locally: `just validate-all`, `just lint`, `just test`.
3. Ensure your Rust toolchain matches CI (stable, MSRV 1.88).

### Observability / metrics not appearing

The `/v1/metrics` endpoint serves Prometheus metrics. Ensure:

1. The server was started with metrics initialization (`init_metrics()`).
2. You've made at least one request (metrics are created on first use).
3. Hit `GET /v1/metrics` to see the Prometheus text output.
