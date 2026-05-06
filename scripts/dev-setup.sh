#!/usr/bin/env bash
# dev-setup.sh — lightweight developer environment setup for MemoryFS.
#
# Checks prerequisites, builds the workspace, runs tests, and optionally
# starts the Docker dev stack. Idempotent — safe to run repeatedly.
#
# Usage:
#   bash scripts/dev-setup.sh              # full setup
#   bash scripts/dev-setup.sh --no-docker  # skip Docker services
#   bash scripts/dev-setup.sh --check      # only verify, don't build

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m'

ok()   { printf "${GREEN}✓${NC} %s\n" "$1"; }
warn() { printf "${YELLOW}!${NC} %s\n" "$1"; }
fail() { printf "${RED}✗${NC} %s\n" "$1"; }

SKIP_DOCKER=false
CHECK_ONLY=false

for arg in "$@"; do
  case "$arg" in
    --no-docker) SKIP_DOCKER=true ;;
    --check)     CHECK_ONLY=true ;;
  esac
done

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_DIR"

echo "MemoryFS dev-setup"
echo "=================="
echo ""

# ── Check prerequisites ──

ERRORS=0

if command -v rustc &>/dev/null; then
  RUST_VER=$(rustc --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')
  RUST_MINOR=$(echo "$RUST_VER" | cut -d. -f2)
  if [ "$RUST_MINOR" -ge 88 ]; then
    ok "Rust $RUST_VER (>= 1.88)"
  else
    fail "Rust $RUST_VER — need >= 1.88. Run: rustup update stable"
    ERRORS=$((ERRORS + 1))
  fi
else
  fail "Rust not found. Install: https://rustup.rs"
  ERRORS=$((ERRORS + 1))
fi

if command -v cargo &>/dev/null; then
  ok "cargo"
else
  fail "cargo not found"
  ERRORS=$((ERRORS + 1))
fi

if command -v just &>/dev/null; then
  ok "just $(just --version 2>/dev/null | head -1)"
else
  warn "just not found. Install: cargo install just"
fi

if command -v python3 &>/dev/null; then
  PY_VER=$(python3 --version | grep -oE '[0-9]+\.[0-9]+')
  PY_MINOR=$(echo "$PY_VER" | cut -d. -f2)
  if [ "$PY_MINOR" -ge 11 ]; then
    ok "Python $PY_VER (>= 3.11)"
  else
    warn "Python $PY_VER — recommended >= 3.11 for validation scripts"
  fi
else
  warn "Python 3 not found — validation scripts won't work"
fi

if ! $SKIP_DOCKER; then
  if command -v docker &>/dev/null; then
    ok "Docker $(docker --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
  else
    warn "Docker not found — dev services (Postgres, TEI) won't start"
  fi
fi

echo ""

if [ $ERRORS -gt 0 ]; then
  fail "$ERRORS required tool(s) missing. Fix above errors and re-run."
  exit 1
fi

if $CHECK_ONLY; then
  ok "All checks passed."
  exit 0
fi

# ── Build ──

echo "Building workspace..."
cargo build --workspace 2>&1
ok "Build succeeded"
echo ""

# ── Tests ──

echo "Running tests..."
cargo test --workspace 2>&1
ok "All tests passed"
echo ""

# ── Validation ──

if command -v python3 &>/dev/null; then
  echo "Running contract validations..."
  if command -v just &>/dev/null; then
    just validate-all 2>&1
  else
    python3 scripts/validate_schemas.py
    python3 scripts/validate_fixtures.py
  fi
  ok "Validations passed"
  echo ""
fi

# ── Docker ──

if ! $SKIP_DOCKER && command -v docker &>/dev/null; then
  echo "Starting dev services..."
  docker compose -f scripts/docker-compose.dev.yml up -d 2>&1
  ok "Dev services running"
  echo ""
  echo "  PostgreSQL (pgvector): 127.0.0.1:5433  db=memoryfs user=memoryfs"
  echo "  Embedding (TEI):       127.0.0.1:8090"
  echo ""
fi

# ── Done ──

echo "=================="
ok "Dev environment ready!"
echo ""
echo "Next steps:"
echo "  cargo run -p memoryfs-cli -- health --url http://127.0.0.1:3000"
echo "  See docs/getting-started.md for a walkthrough."
