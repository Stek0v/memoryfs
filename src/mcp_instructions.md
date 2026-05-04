MemoryFS — per-project verifiable memory for this workspace. Markdown files are the source of truth; vector/BM25/graph indexes are disposable derivatives.

## Recall-first
Before any architectural recommendation about THIS project ("I suggest X", "let's go with Y", "we should use Z"), call `memoryfs_recall` with the topic. If a prior decision exists, surface it and either follow it or explicitly propose a supersede. Don't recommend in a vacuum when memory may already hold the answer.

## When to save without being asked
- User states a decision ("we'll use Postgres", "not X") → `memoryfs_remember` as `type=decision`
- Root cause of a bug found after investigation (insight, not just the fix) → `type=discovery` with symptom + cause + fix
- New service / endpoint / version / IP / port → `type=fact` at `infra/<topic>.md`
- Significant milestone shipped → `type=event` at `events/YYYY-MM-DD-<slug>.md` (absolute date, never "today")
- User corrects approach for this project → `type=preference`, `scope=project`
- Time-bounded item (deadline, freeze, dependency window) → `type=event` with absolute date

## Path conventions (one record per file — supersede needs a target)
- `decisions/<slug>.md`
- `discoveries/<slug>.md`
- `facts/<slug>.md`  or  `infra/<topic>.md`
- `events/YYYY-MM-DD-<slug>.md`
- `preferences/<topic>.md`

## Findability
Recall searches content embeddings, not the slug. Put into the body the terms a future-you would naturally type as a query (synonyms, the technology name, the problem domain). A decision titled "db-choice" should mention "database", "Postgres", "SQL" in the body so it is findable.

## Supersede, never overwrite
- Decision reversed → `memoryfs_supersede_memory`. The server REJECTS plain `memoryfs_write_file` overwriting an existing `decisions/*.md` or `discoveries/*.md` — that guardrail enforces the audit trail. For a typo-only fix, pass `force=true` to `write_file`.
- Fact obsolete (version bumped, IP changed) → supersede.
- Discovery disproven by new info → supersede.

## Confirm vs save silently
- User explicit ("let's do X", "use Y") → save silently, mention the path in one line
- Inferred from discussion → confirm in one sentence, then save
- Reversal of a prior decision → ALWAYS confirm + supersede, never silent overwrite

## Don't save
- Step-by-step task progress — that is what TodoWrite is for
- Code snippets or file paths — git/grep is authoritative and ages badly on rename
- Conversation play-by-play ("we discussed A then B") — only the conclusion
- Duplicates — `memoryfs_search` before `write_file` / `remember`
