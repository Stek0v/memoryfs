# Security Policy

## Поддерживаемые версии

| Версия | Поддержка |
| -------- | ----------- |
| 0.x | Только последний минор |
| 1.x | После релиза — все патчи в течение 12 месяцев |

## Сообщить об уязвимости

**НЕ создавайте public issue для security-багов.**

Напишите на `security@memoryfs.dev` с темой `[security] <краткое описание>`.

Минимально включите:

- Версия / commit hash, на котором воспроизвели
- Описание уязвимости
- Шаги воспроизведения (или PoC)
- Оценка impact'а (раскрытие данных / обход ACL / RCE / DoS)
- Опционально: предложенный fix

Что вы получите:

- Acknowledgement в течение **72 часов**
- Triage-решение в течение **7 дней**
- Регулярные апдейты по статусу как минимум раз в 7 дней
- Кредит в release notes, если вы хотите быть упомянутым

## Объём

В scope:

- Утечка данных через retrieval / API / extraction
- Обход ACL и policy (E1–E10 в `threat-model.md`)
- Подделка provenance (S3) или audit log (T2, R3, R4)
- Pre-redaction-обходы (I1, I7)
- Prompt injection, ведущий к записи нелегитимной памяти (T5)
- RCE / privilege escalation в core / cli / extractor
- Отравление зависимостей supply-chain

Не в scope (по `threat-model.md` v1.0):

- Атаки от привилегированного OS-root на хосте
- Глобальный пассивный наблюдатель TLS
- Side-channel внутри LLM провайдера
- Attacks через прямую правку filesystem в обход API (мы рекомендуем не давать
  прямой FS-доступ; если вы это делаете и получили compromise — это ожидаемо)

## Что мы делаем для безопасности

- 9-схемная валидация фронтматтера + 20-кейсный adversarial schema-violations test
- 50+ pre-redaction кейсов в `tests/adversarial/secrets-suite/`
- 12 prompt-injection сценариев в `tests/adversarial/injection-suite/`
- STRIDE-анализ в `threat-model.md` с явными митигациями
- Pen-test pre-1.0 — независимый аудит ≥5 человеко-дней
- Append-only audit log с опциональным hash chain (ADR-011)
- Review-by-default для PII / secret / medical / legal / financial

## CVE и публикация

После fix'а мы:

1. Публикуем CVE через GitHub Security Advisories
2. Релизим патч-версию
3. Сообщаем рекомендуемый upgrade path
4. Указываем reporter'а в release notes (если разрешено)
