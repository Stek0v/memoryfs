---
schema_version: memoryfs/v1
id: run_01HZM5N4P6Q8R0S2T4V6X8Z0B2
type: run
agent: agent:architect
session_id: 2026-05-15-cursor-switch
started_at: "2026-05-15T14:06:00.140Z"
finished_at: "2026-05-15T14:30:51.044Z"
trigger:
  kind: user_request
  by: user:alice
  ref: conv_01HZM5K0M2N4P6Q8R0S2T4V6X8
author: agent:architect
permissions:
  read: ["user:alice", "agent:architect"]
  write: ["agent:architect"]
tags: [supersede, ide, conflict]
status: succeeded
artifacts:
  prompt:        runs/run_01HZM5N4P6Q8R0S2T4V6X8Z0B2/prompt.md
  tool_calls:    runs/run_01HZM5N4P6Q8R0S2T4V6X8Z0B2/tool_calls.jsonl
  result:        runs/run_01HZM5N4P6Q8R0S2T4V6X8Z0B2/result.md
metrics:
  duration_ms: 1491904
  tokens_input: 4812
  tokens_output: 2103
  tool_calls: 5
  memories_proposed: 1
  memories_committed: 0
  cost_usd: 0.09
model: anthropic/claude-opus-4-7
proposed_memories:
  - prp_01HZM5R5S7T9V1W3X5Y7Z9A1B3
consumed_memories:
  - mem_01HZM2A4B6C8D0E2F4G6H8J0K2
consumed_files:
  - conversations/2026/05/15/conv_01HZM5K0M2N4P6Q8R0S2T4V6X8.md
---

# Run: supersede Alice's IDE preference

Supersede-операция для существующей памяти. Demonstrates:

1. **Supersede vs новая запись:** агент сначала ищет существующую память, обнаруживает
   противоречие, выбирает supersede а не новый propose.
2. **Цепочка supersedes/superseded_by** ведётся в обе стороны — старая получает
   `superseded_by`, новая `supersedes`.
3. **Review требуется** — sensitivity=pii. Новая память попадает в inbox даже при
   высоком confidence.
4. **conflict_type=contradiction** — заменили инструмент полностью, не уточнили факт.

## Артефакты

- [prompt.md](./prompt.md)
- [tool_calls.jsonl](./tool_calls.jsonl)
- [result.md](./result.md)
