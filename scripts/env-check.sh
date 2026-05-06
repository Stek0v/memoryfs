#!/usr/bin/env bash
# env-check.sh — verify dev environment without modifying anything.
# Exit code:
#   0  всё в порядке
#   1  отсутствует обязательный инструмент или версия ниже минимальной
#   2  предупреждения (опциональные инструменты)

set -uo pipefail

OK=0
WARN=0
FAIL=0

green() { printf "\033[1;32m%s\033[0m\n" "$*"; }
yellow(){ printf "\033[1;33m%s\033[0m\n" "$*"; }
red()   { printf "\033[1;31m%s\033[0m\n" "$*"; }

check() {
  local name="$1" cmd="$2" min="${3:-}" extract="${4:-}"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    red "✗ $name: not found"
    FAIL=$((FAIL+1))
    return
  fi
  local v=""
  if [ -n "$extract" ]; then
    v="$($extract 2>/dev/null || echo "")"
  fi
  if [ -n "$min" ] && [ -n "$v" ]; then
    if printf '%s\n%s\n' "$min" "$v" | sort -V -C; then
      green "✓ $name: $v (>= $min)"
      OK=$((OK+1))
    else
      red "✗ $name: $v < required $min"
      FAIL=$((FAIL+1))
    fi
  else
    green "✓ $name: $v"
    OK=$((OK+1))
  fi
}

warn_check() {
  local name="$1" cmd="$2"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    yellow "! $name: not found (optional)"
    WARN=$((WARN+1))
  else
    green "✓ $name"
    OK=$((OK+1))
  fi
}

echo "=== required ==="
check "rustc"       rustc       "1.75.0"  'rustc --version | awk "{print \$2}"'
check "cargo"       cargo       ""        'cargo --version | awk "{print \$2}"'
check "python3"     python3     "3.11.0"  'python3 -c "import sys; print(\"%d.%d.%d\" % sys.version_info[:3])"'
check "git"         git         "2.30.0"  'git --version | awk "{print \$3}"'

echo ""
echo "=== build tooling ==="
check "just"          just          ""  'just --version | awk "{print \$2}"'
check "cargo-nextest" cargo-nextest ""  'cargo-nextest --version 2>&1 | awk "{print \$2}"'

echo ""
echo "=== python tooling ==="
check "ruff"    ruff    ""  'ruff --version | awk "{print \$2}"'
check "pyright" pyright ""  'pyright --version | awk "{print \$2}"'

echo ""
echo "=== optional ==="
warn_check "docker"           docker
warn_check "docker compose"   docker # compose subcommand check below
warn_check "qdrant client"    qdrant
warn_check "pre-commit"       pre-commit

echo ""
echo "=== summary ==="
green "ok=$OK"
[ "$WARN" -gt 0 ] && yellow "warn=$WARN"
[ "$FAIL" -gt 0 ] && red    "fail=$FAIL"

if [ "$FAIL" -gt 0 ]; then
  exit 1
elif [ "$WARN" -gt 0 ]; then
  exit 2
fi
exit 0
