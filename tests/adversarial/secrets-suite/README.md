# secrets-suite

Регрессионный набор для **pre-redaction** компонента. Защищает от утечки
секретов через conversation-файлы и любые другие пути, где text от пользователя
попадает в storage.

## Содержимое

- [`corpora.jsonl`](./corpora.jsonl) — 50 представительных кейсов (расширяется до 200+).
  Каждая строка — JSON-объект с полями `id`, `category`, `subcategory`, `input`,
  `expect_redacted`, `source`, `added_at`, опц. `notes`.

## Категории

| Категория | Покрытие |
| ----------- | ---------- |
| `api_key` | OpenAI (sk-/sk-proj-), Anthropic (sk-ant-), GitHub (ghp_/gho_/ghs_), Slack (xoxb-/xoxp-), AWS, Google, Twilio, SendGrid, Stripe |
| `jwt` | HS256, RS256, alg=none, в логах, в YAML, в JSON |
| `ssh_key` | RSA, OpenSSH, EC PEM-блоки |
| `password` | Plain в URL (basic auth), connection strings, явная разметка ("My password is..."), в config-файлах |
| `credit_card` | Visa, Amex (с пробелами и без) |
| `iban` | DE, NL формат |
| `ssn` | US format |
| `high_entropy` | Hex (sha256-like), base64-обёрнутые, контекстуальные (после "commit", "sha256=" — не секрет) |

## Подмножества

- **Positive (`expect_redacted: true`)**: ловить ОБЯЗАТЕЛЬНО. F1 ≥0.95 на этих.
- **Negative (`expect_redacted: false`)**: НЕ редактим. False-positive ≤1%. Включает
  публичные publishable-ключи (Stripe `pk_test`), идентификаторы (org-id, UUID, ULID,
  git-commit-hash).
- **Borderline**: явно помечены `notes: "borderline"` — обсуждаемые случаи.

## Расширение

Новый кейс добавляется через PR с обоснованием в `notes` и ссылкой в `source`.
ID `sec_NNN` назначается по порядку. Категории расширяются через ADR.

## Запуск

```bash
just test-adversarial-secrets
```

Реализация — `crates/redactor` (Rust). Пока это спецификация — реальный test
harness будет в Phase 1.7 (см. `04-tasks-dod.md`).

## Метрики (целевые)

| Метрика | Цель | Замер |
| --------- | ------ | ------- |
| Recall (precision-redacted positive) | ≥95% | (true_pos) / (true_pos + false_neg) на `expect_redacted: true` |
| False-positive rate | ≤1% | (false_pos) / (false_pos + true_neg) на `expect_redacted: false` |
| F1 | ≥0.95 | F1 при threshold по высокоэнтропийному правилу |

## Что делать при regression

1. Кейс падает → найти конкретный паттерн.
2. Если детектор пропустил positive — расширить regex/правило, добавить unit-тест,
   снова прогнать всю suite (важно: не сломать negative).
3. Если детектор поймал negative — сузить правило или добавить контекст-фильтр
   ("hash" слово рядом с hex64 → не секрет), снова прогнать.
4. PR с диффом по `corpora.jsonl` (если кейс добавлен) и кодом редактора.

## Связь с threat-model

- I1 — утечка API ключа в conversation
- I7 — утечка через extraction worker во внешний LLM (если sensitive не вырезан pre-extraction)
- T5 — memory poisoning через "помни что мой пароль X"
