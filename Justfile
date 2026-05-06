# MemoryFS — development command runner
#
# Usage: `just` (список), `just <recipe>`
# Установка: `cargo install just` или `brew install just`

# По умолчанию печатаем список рецептов
default:
    @just --list --unsorted

# ------------------------------------------------------------------
# Окружение
# ------------------------------------------------------------------

# Полный bootstrap (rust, python, just, docker stack)
bootstrap:
    bash scripts/bootstrap.sh

# Bootstrap без docker
bootstrap-no-docker:
    bash scripts/bootstrap.sh --no-docker

# Проверить окружение (для CI и onboarding)
env-check:
    bash scripts/env-check.sh

# Поднять docker-stack (Levara + Ollama на хосте) для local dev
dev-up:
    docker compose -f scripts/docker-compose.dev.yml up -d --build
    @echo ""
    @echo "Levara gRPC:          127.0.0.1:50051 (vector store + embedder)"
    @echo "Levara HTTP:          http://127.0.0.1:8080 (Swagger, metrics)"

# Остановить docker-stack без потери данных
dev-down:
    docker compose -f scripts/docker-compose.dev.yml down

# Снести docker-stack вместе с volume'ами (clean slate)
dev-reset:
    docker compose -f scripts/docker-compose.dev.yml down -v

# ------------------------------------------------------------------
# Валидация контрактов и fixtures
# ------------------------------------------------------------------

# Базовая JSON/YAML/bash sanity-проверка
validate-syntax:
    @echo "→ JSON syntax"
    @python3 -c "import json,sys,glob; [json.load(open(f)) for f in glob.glob('specs/**/*.json', recursive=True)]"
    @echo "  OK"
    @echo "→ YAML syntax"
    @python3 -c "import yaml,glob; [yaml.safe_load(open(f)) for f in ['specs/openapi.yaml', *glob.glob('fixtures/**/*.yaml', recursive=True), *glob.glob('scripts/*.yml')]]"
    @echo "  OK"
    @echo "→ Bash syntax"
    @bash -n scripts/bootstrap.sh
    @bash -n scripts/env-check.sh
    @bash -n scripts/dev-setup.sh
    @echo "  OK"

# Валидация фронтматтеров fixtures против JSON Schemas v1
validate-fixtures:
    python3 scripts/validate_fixtures.py

# Валидация всех схем v1 на well-formedness (рекурсивно)
validate-schemas:
    python3 scripts/validate_schemas.py

# Проверка дрейфа между прозой 02-data-model.md и схемами
crosscheck-data-model:
    python3 scripts/crosscheck_data_model.py

# Self-test adversarial schema-violations
test-schema-violations:
    python3 scripts/test_schema_violations.py

# Все валидации одной командой (быстрая, без cargo)
validate-all: validate-syntax validate-schemas validate-fixtures test-schema-violations
    @echo ""
    @echo "✓ all validations passed"

# ------------------------------------------------------------------
# Adversarial test suites
# ------------------------------------------------------------------

# Прогон всех adversarial-наборов
test-adversarial: test-adversarial-secrets test-adversarial-injection test-adversarial-schema

# Secrets pre-redaction (placeholder — будет реализовано в Phase 1.7)
test-adversarial-secrets:
    @echo "TODO Phase 1.7: run scripts/test_secrets_suite.py against tests/adversarial/secrets-suite/corpora.jsonl"

# Prompt injection (placeholder — Phase 2)
test-adversarial-injection:
    @echo "TODO Phase 2: run integration test feeding scenarios.yaml to extractor"

# Schema violations (уже работает)
test-adversarial-schema: test-schema-violations

# ------------------------------------------------------------------
# Тесты (placeholder для Phase 0+; будут реализованы по мере появления кода)
# ------------------------------------------------------------------

# Unit + integration тесты Rust core
test-rust:
    cargo nextest run --workspace

# Schema validation tests + ACL invariants
test-contracts:
    python3 scripts/test_acl_matrix.py
    python3 scripts/test_supersede_invariants.py

# Все тесты подряд
test: test-rust test-contracts

# ------------------------------------------------------------------
# Качество кода
# ------------------------------------------------------------------

# Lint всего
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Auto-fix
fix:
    cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged
    cargo fmt --all

# ------------------------------------------------------------------
# Утилиты
# ------------------------------------------------------------------

# Показать структуру пакета
tree:
    @find . -type f -not -path './target/*' -not -path './.venv/*' -not -path './node_modules/*' -not -path './.git/*' | sort

# Подсчитать строки в документации/контрактах
stats:
    @echo "Documentation lines:"
    @find . -name "*.md" -not -path "./target/*" -not -path "./.venv/*" | xargs wc -l | tail -1
    @echo ""
    @echo "Specs lines:"
    @find specs -type f | xargs wc -l | tail -1
    @echo ""
    @echo "Fixtures count:"
    @find fixtures -type f | wc -l

# Собрать пакет в zip
package:
    cd .. && zip -r memoryfs-planning-package.zip memoryfs-planning/ -x "*/target/*" "*/.venv/*" "*/node_modules/*" "*/.git/*"
    @echo "→ ../memoryfs-planning-package.zip"
