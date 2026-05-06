# Run result — supersede IDE preference

## Что произошло

Старая память про Neovim (`mem_01HZM2A4B6C8D0E2F4G6H8J0K2`, active с 2026-03-15)
проверена на актуальность. Найдено противоречие новому состоянию (Cursor как основной).

Действие: `memoryfs_supersede_memory` с:

- `conflict_type: contradiction`
- `reason: "Two-week trial confirmed switch from Neovim to Cursor; vim ergonomics preserved via vim-mode."`

## Текущее состояние

- Старая память — **target_locked_pending_review** (нельзя ещё одну supersede пока не
  завершён review текущего предложения).
- Новое предложение — `prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3` в inbox (sensitivity=pii → review).

## Что нужно от user:alice

Открыть inbox и approve/reject:

```bash
memoryfs review prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3 --decision approved
```

После approve:

- Старая получит `superseded_by: [mem_01HZM5P3Q5R7S9T1V3W5X7Y9Z1]`, status=superseded.
- Новая получит `supersedes: [mem_01HZM2A4B6C8D0E2F4G6H8J0K2]`, status=active.
- Оба изменения — в одном атомарном коммите.

## Если reject

- Старая остаётся active.
- Новая остаётся в inbox со статусом rejected (для history), вне active-set.
- Lock с target снимается; можно повторить попытку с другими формулировками.
