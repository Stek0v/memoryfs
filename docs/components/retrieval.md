# `retrieval` — multi-signal recall

Source: [`src/retrieval.rs`](../../src/retrieval.rs)

## Pipeline

```
query ──▶ ┌──────────────┐    ┌────────────┐
         │ vector search │    │ BM25       │
         │ (Levara/Qdr.) │    │ (Tantivy)  │
         └──────┬────────┘    └─────┬──────┘
                │                    │
                └─────────┬──────────┘
                          ▼
                  ┌──────────────┐
                  │ Reciprocal   │   k=60 by default
                  │ Rank Fusion  │
                  └──────┬───────┘
                         │
                         ▼
                  ┌──────────────────┐  optional
                  │ entity neighbor  │  via graph
                  │ score            │
                  └──────┬───────────┘
                         │
                         ▼
                  ┌──────────────┐
                  │ recency      │   exp(-Δdays/τ) on created_at
                  │ boost        │
                  └──────┬───────┘
                         │
                         ▼
                  ┌──────────────┐
                  │ ACL          │   re-check every hash
                  │ post-filter  │
                  └──────┬───────┘
                         │
                         ▼
                  ┌──────────────┐
                  │ deterministic│   read objects/<hash> from disk
                  │ disk read    │   not cached chunk text
                  └──────────────┘
```

## Why post-filter ACL

Vector / BM25 indexes don't know who's asking; they return everything they have. Filtering after fusion means:

- **Correctness** — a candidate's path may have been deny-listed since indexing.
- **Auditability** — every reject is logged with the rule that fired.
- **Cheap** — ranking happens once over all candidates, not per-subject.

It's the price of running a single shared index across multiple subjects in multi-tenant deployments. In single-user local mode it's a no-op (everything is allowed) but the code path is identical.

## Reciprocal Rank Fusion (RRF)

```
score(d) = Σ 1 / (k + rank_in_list_i(d))
```

`k = 60` by default (Cormack et al.). The constant matters less than its presence: it dampens the contribution of low-rank hits in any one list, so a doc has to do well in at least one signal to surface.

Weights: vector and BM25 lists can be weighted independently via `vector_weight` / `bm25_weight` query parameters on `/v1/context`.

## Recency boost

`exp(-Δdays / τ)` applied to a doc's RRF score, where `Δdays` is computed from frontmatter `created_at` and `τ` is configurable. Memories without `created_at` (or with malformed values) skip the boost rather than hard-failing.

## Entity expansion

If the entity graph is populated and the query mentions known entity names, the engine pulls their BFS neighbors (depth-bounded) and adds an `entity_score` to the fused score. Disabled when the graph is empty.

## Hybrid backend opt-in

If the configured backend implements `HybridSearch` (Levara does), the engine delegates the parallel vector + BM25 + fusion to the backend and skips the local Tantivy step — fewer round-trips, less memory. Falls back to local fusion automatically when the trait isn't implemented.
