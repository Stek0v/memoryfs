# Contributing to MemoryFS

Спасибо за интерес. Этот файл — короткий гид по тому, как сделать PR, который
быстро проходит ревью.

## Бойстрап

```bash
./scripts/bootstrap.sh   # rust, python, just, docker, pre-commit
just env-check           # убедиться, что всё на месте
just dev-up              # qdrant + postgres локально
just validate-all        # должен проходить на main
```

## Принципы

1. **Машинные контракты — источник истины.** Если проза в `02-data-model.md` или
   `03-api-specification.md` расходится со схемами в `specs/schemas/v1/` или
   `specs/openapi.yaml` — это баг в прозе, не в схеме (если только schema не
   ошибочна, тогда обновляем обе).

2. **Каждый PR проходит `just validate-all` локально.** Это быстрая (<10 секунд)
   проверка JSON/YAML/bash-синтаксиса, well-formedness схем, фикстур и
   schema-violations self-test. CI прогонит то же.

3. **Изменение схемы → обновление прозы.** В одном PR. См. `crosscheck_data_model.py`.

4. **Любая регрессия безопасности → новый кейс в adversarial suite.** Если в
   pre-redaction найден gap — добавляется в `tests/adversarial/secrets-suite/corpora.jsonl`.
   Если найден injection — в `injection-suite/scenarios.yaml`. Если найден gap в
   schema — в `schema-violations/violations.jsonl` (с self-test'ом).

5. **Маленькие коммиты, осмысленные сообщения.** Conventional commits:
   `feat(core): add CommitHash::parse`, `fix(redactor): catch JWT alg=none`,
   `docs(02): sync prose with memory.schema.json`.

## Фазы

См. `07-roadmap.md`. На дату 0.4 пакета мы в **Phase 0 (Foundations)**: схемы,
фикстуры, validators, code skeleton, dev-tooling. Phase 1 начнётся, когда:

- `just validate-all` проходит ✓
- `cargo build --workspace` собирается ✓
- `cargo test` зелёный (есть тесты на `ids.rs` и `error.rs`)
- `crosscheck_data_model.py` exit 0 (drift = 0)
- Code-of-conduct и SECURITY.md в репозитории ✓

## Code style

- **Rust:** `cargo fmt` + `clippy --all-targets -- -D warnings`. Без `unwrap()`/`panic!()`
  в production-путях. Структурированное логирование через `tracing`. Errors через
  `thiserror`/`MemoryFsError`.
- **Python:** `ruff` + `pyright`. PEP 8. Async через `anyio`. Pydantic v2 для
  моделей. Без `print` в библиотечном коде — только `structlog`.

## Тесты

- **Unit:** рядом с кодом (`#[cfg(test)] mod tests`).
- **Integration:** `tests/` в каждом crate.
- **Property:** `proptest` для ID-валидации, ACL-evaluator'а, supersede-graph'а.
- **Adversarial:** `tests/adversarial/` — golden-данные.
- **Eval:** `fixtures/*/expected/` — таргеты по recall/precision/NDCG.

```bash
just test                # rust + python + contracts
just test-adversarial    # все три adversarial suite
```

## Размер PR

Цель: <500 строк изменений. Большие PR разбивайте на серию:

1. Schema/contract change (если есть).
2. Implementation.
3. Tests.
4. Docs.

Каждый коммит должен оставлять `just validate-all` зелёным.

## Что делать, если drift checker падает

`02-data-model.md` сейчас отстаёт от схем. Это задокументированный долг
(см. `specs/DRIFT-REPORT.md`). PR, который синхронизирует прозу со схемами, —
welcome (задача `CC.0.3-prose-sync` в Phase 0).

Если ваш PR ломает drift checker — это не блокер для merge **сейчас**, но станет
блокером после PR `CC.0.3`.

## Code of Conduct

Будь добр. Атакуй идеи, не людей. См. `CODE_OF_CONDUCT.md` (TBD — Phase 1).

## Лицензия

PR'ы лицензируются под Apache 2.0 (см. `LICENSE`). Отправляя PR, вы соглашаетесь,
что ваш вклад может быть распространён на этих условиях.
