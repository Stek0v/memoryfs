#!/usr/bin/env bash
# bootstrap.sh — idempotent dev setup for MemoryFS
# Usage: ./scripts/bootstrap.sh [--no-docker] [--no-hooks]
#
# Что делает:
#  1. Проверяет / устанавливает rustup (stable >= 1.75)
#  2. Проверяет Python (>= 3.11) и создаёт .venv
#  3. Устанавливает just, cargo-nextest, ruff, pyright (если отсутствуют)
#  4. Готовит pre-commit hooks
#  5. Опционально стартует docker-compose со зависимостями (Qdrant + Postgres)
#
# Скрипт безопасен для повторного запуска.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

NO_DOCKER=0
NO_HOOKS=0
for arg in "$@"; do
  case "$arg" in
    --no-docker) NO_DOCKER=1 ;;
    --no-hooks)  NO_HOOKS=1 ;;
    -h|--help)
      sed -n '2,16p' "$0"
      exit 0
      ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# ---------- helpers ----------
log()  { printf "\033[1;34m[bootstrap]\033[0m %s\n" "$*"; }
warn() { printf "\033[1;33m[bootstrap]\033[0m %s\n" "$*" >&2; }
fail() { printf "\033[1;31m[bootstrap]\033[0m %s\n" "$*" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

require_min_version() {
  local name="$1" cur="$2" min="$3"
  if ! printf '%s\n%s\n' "$min" "$cur" | sort -V -C; then
    fail "$name $cur < required $min"
  fi
}

# ---------- 1. Rust ----------
log "checking rust toolchain"
if ! have rustup; then
  log "rustup not found — installing"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  # shellcheck source=/dev/null
  source "$HOME/.cargo/env"
fi
rustup update stable >/dev/null
RUST_VERSION="$(rustc --version | awk '{print $2}')"
require_min_version rust "$RUST_VERSION" "1.75.0"
log "rust $RUST_VERSION OK"

# ---------- 2. Python ----------
log "checking python"
if ! have python3; then fail "python3 not found"; fi
PY_VERSION="$(python3 -c 'import sys; print("%d.%d.%d" % sys.version_info[:3])')"
require_min_version python "$PY_VERSION" "3.11.0"
log "python $PY_VERSION OK"

if [ ! -d ".venv" ]; then
  log "creating .venv"
  python3 -m venv .venv
fi
# shellcheck source=/dev/null
source .venv/bin/activate
python -m pip install --quiet --upgrade pip wheel

# ---------- 3. Tools ----------
ensure_cargo_install() {
  local crate="$1" bin="${2:-$1}"
  if ! have "$bin"; then
    log "cargo install $crate"
    cargo install --locked "$crate"
  else
    log "$bin OK"
  fi
}

ensure_cargo_install just
ensure_cargo_install cargo-nextest cargo-nextest

ensure_pip_install() {
  local pkg="$1" bin="${2:-$1}"
  if ! have "$bin"; then
    log "pip install $pkg"
    python -m pip install --quiet "$pkg"
  else
    log "$bin OK"
  fi
}

ensure_pip_install ruff
ensure_pip_install pyright
ensure_pip_install pre-commit

# ---------- 4. pre-commit ----------
if [ "$NO_HOOKS" -eq 0 ]; then
  if [ -f ".pre-commit-config.yaml" ]; then
    log "installing pre-commit hooks"
    pre-commit install --install-hooks
  else
    warn ".pre-commit-config.yaml not found — пропускаю установку хуков"
  fi
fi

# ---------- 5. Docker dev stack ----------
if [ "$NO_DOCKER" -eq 0 ]; then
  if have docker; then
    log "starting dev stack (Qdrant + Postgres) via docker compose"
    if [ -f "scripts/docker-compose.dev.yml" ]; then
      docker compose -f scripts/docker-compose.dev.yml up -d
    else
      warn "scripts/docker-compose.dev.yml not found — пропускаю"
    fi
  else
    warn "docker не найден — dev stack не поднят. Используй --no-docker, либо установи docker"
  fi
fi

log "OK. Запусти 'just' чтобы увидеть доступные команды."
