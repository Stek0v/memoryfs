# Scripts

Скрипты для onboarding и dev-окружения MemoryFS. Все идемпотентны.

## bootstrap.sh

Полная установка / обновление dev-окружения. Проверяет и при необходимости ставит:

- rustup + stable Rust ≥1.75
- Python ≥3.11 + .venv
- `just`, `cargo-nextest`
- `ruff`, `pyright`, `pre-commit`
- pre-commit hooks (если есть `.pre-commit-config.yaml`)
- Docker dev-stack (Qdrant + Postgres) через `docker-compose.dev.yml`

Использование:

```bash
./scripts/bootstrap.sh                # всё
./scripts/bootstrap.sh --no-docker    # без поднятия Qdrant/Postgres
./scripts/bootstrap.sh --no-hooks     # без pre-commit
```

Безопасен для повторных запусков. Не модифицирует системные пакеты, кроме того, что
делает `cargo install` / `pip install` в пользовательский профиль.

## env-check.sh

Проверяет окружение без модификаций. Подходит для CI и для onboarding-чек-листа.

Коды выхода:

| Код | Значение |
| ----- | ---------- |
| 0 | Все обязательные инструменты на месте, версии достаточны. |
| 1 | Отсутствует обязательный инструмент или версия слишком старая. |
| 2 | Только опциональные warning'и (docker, pre-commit). |

```bash
./scripts/env-check.sh
echo $?
```

## docker-compose.dev.yml

Поднимает локальный стек:

- **Qdrant 1.11** на `127.0.0.1:6333` (REST) / `:6334` (gRPC).
  Volume `qdrant_storage`. Healthcheck — `/healthz`.
- **Postgres 16** на `127.0.0.1:5433`. БД `memoryfs`, пользователь `memoryfs`,
  пароль `dev_only_password` (НЕ использовать в продакшне). Volume `postgres_data`.

Запуск/остановка:

```bash
docker compose -f scripts/docker-compose.dev.yml up -d
docker compose -f scripts/docker-compose.dev.yml down
docker compose -f scripts/docker-compose.dev.yml down -v  # с очисткой томов
```

Tantivy/BM25 в dev — in-process, отдельный сервис не нужен.

## Что должно появиться позже (TBD)

- `seed-fixtures.sh` — загрузить `fixtures/killer-demo/` в локальный сервер (Phase 0.5).
- `regen-schemas.sh` — пересборка типов из JSON Schemas (Phase 0).
- `bench-retrieval.sh` — прогон retrieval-eval против fixture (Phase 4).
- `release.sh` — сборка артефактов релиза (Phase 7).
