# Adversarial Test Suites

Наборы регрессионных тестов, защищающие от классов уязвимостей, которые
обнаруживаются раз в несколько лет, но если пропущены — последствия серьёзные.

## Обзор

| Suite | Защищает от | Размер | Source-of-truth |
| ------- | ------------ | -------- | ----------------- |
| [`secrets-suite/`](./secrets-suite/) | Утечка секретов через pre-redaction (I1, I7 в threat-model) | 200+ кейсов | corpora.jsonl |
| [`injection-suite/`](./injection-suite/) | Prompt injection в текст файлов (T5, Adv-A/I) | 60+ сценариев | scenarios.yaml |
| [`schema-violations/`](./schema-violations/) | Принятие невалидного фронтматтера (R5) | 80+ примеров | violations.jsonl |

## Принципы

1. **Каждый кейс — отдельная регрессия.** Когда в продакшне обнаруживается утечка/обход —
   немедленно добавляется кейс в suite. Новый код проходит весь suite до merge.

2. **Положительный и отрицательный контроль.** В каждом suite есть `positive.jsonl`
   (должно быть отловлено) и `negative.jsonl` (не должно быть ложных срабатываний).
   Метрика — F1 ≥0.95 на positive, false-positive rate <1% на negative.

3. **Suite — golden-данные.** Не зависят от инфраструктуры (нет docker, нет LLM).
   Запускаются за секунды в CI. Гейтят merge.

4. **Авторство.** Каждый кейс имеет `id`, `source` (где найден), `added_at`. Кейсы
   из публичных CVE/research — с ссылкой. Кейсы из internal incidents — с redacted
   контекстом.

## Использование

```bash
just test-adversarial            # все три suite
just test-adversarial-secrets    # только secrets
just test-adversarial-injection  # только injection
just test-adversarial-schema     # только schema-violations
```

В CI: каждая suite — отдельный job, fail-fast=false, чтобы видеть все падения сразу.

## Метрики качества

- **secrets-suite F1 ≥0.95** на positive (recall ловли секретов).
- **secrets-suite false-positive ≤1%** на negative (не редактим то, что не секрет).
- **injection-suite resilience ≥0.90** — extractor не извлекает память из инструкций
  внутри текста.
- **schema-violations precision = 1.0** — каждый файл из positive должен быть отвергнут;
  ни один из negative — нет.

Метрики проверяются `scripts/run_adversarial.py` и публикуются в `out/adversarial-report.json`.

## Связь с другими документами

- `threat-model.md` §4.4 (I1, I7), §4.2 (T5), §4.3 (R5)
- `04-tasks-dod.md` 1.7 (pre-redaction DoD)
- `06-testing-strategy.md` §adversarial
