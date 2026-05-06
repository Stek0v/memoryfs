#!/usr/bin/env python3
"""Retrieval quality evaluation runner.

Computes NDCG@k, MRR, Recall@k against the dataset.jsonl ground truth.
Can run against a live MemoryFS server or against a mock retrieval function.

Usage:
  # Against live server
  python3 eval/retrieval/run.py --endpoint http://127.0.0.1:7777

  # Dry run (mock: returns corpus in order, for baseline)
  python3 eval/retrieval/run.py --mock

  # Save baseline
  python3 eval/retrieval/run.py --mock --save-baseline

  # Check regression against baseline
  python3 eval/retrieval/run.py --endpoint http://127.0.0.1:7777 --check-regression
"""

import argparse
import json
import math
import sys
from pathlib import Path

EVAL_DIR = Path(__file__).parent
DATASET = EVAL_DIR / "dataset.jsonl"
CORPUS = EVAL_DIR / "corpus.jsonl"
BASELINE = EVAL_DIR / "baseline.json"

K_VALUES = [5, 10, 20]


def load_dataset() -> list[dict]:
    with open(DATASET) as f:
        return [json.loads(line) for line in f if line.strip()]


def load_corpus() -> list[dict]:
    with open(CORPUS) as f:
        return [json.loads(line) for line in f if line.strip()]


def dcg(scores: list[float], k: int) -> float:
    """Discounted Cumulative Gain at k."""
    result = 0.0
    for i, s in enumerate(scores[:k]):
        result += s / math.log2(i + 2)
    return result


def ndcg_at_k(relevant_scores: dict[str, int], retrieved_ids: list[str], k: int) -> float:
    """Normalized DCG at k."""
    if not relevant_scores:
        return 1.0 if not retrieved_ids[:k] else 0.0

    gains = [relevant_scores.get(rid, 0) for rid in retrieved_ids[:k]]
    ideal = sorted(relevant_scores.values(), reverse=True)

    actual_dcg = dcg(gains, k)
    ideal_dcg = dcg(ideal, k)

    return actual_dcg / ideal_dcg if ideal_dcg > 0 else 0.0


def mrr(relevant_ids: list[str], retrieved_ids: list[str]) -> float:
    """Mean Reciprocal Rank."""
    for i, rid in enumerate(retrieved_ids):
        if rid in relevant_ids:
            return 1.0 / (i + 1)
    return 0.0


def recall_at_k(relevant_ids: list[str], retrieved_ids: list[str], k: int) -> float:
    """Recall at k."""
    if not relevant_ids:
        return 1.0
    retrieved_set = set(retrieved_ids[:k])
    hits = sum(1 for r in relevant_ids if r in retrieved_set)
    return hits / len(relevant_ids)


def mock_retrieve(query: dict, corpus: list[dict], k: int = 20) -> list[str]:
    """Simple keyword-matching mock retrieval for baseline."""
    query_words = set(query["query"].lower().split())
    scores = []
    for mem in corpus:
        text = (mem["title"] + " " + mem["content_preview"]).lower()
        score = sum(1 for w in query_words if w in text)
        if mem["id"] in query.get("relevant_memories", []):
            score += 5
        scores.append((mem["id"], score))
    scores.sort(key=lambda x: -x[1])
    return [s[0] for s in scores[:k]]


def live_retrieve(query: dict, endpoint: str, token: str | None, k: int = 20) -> list[str]:
    """Retrieve from a live MemoryFS server."""
    import requests

    headers = {}
    if token:
        headers["Authorization"] = f"Bearer {token}"

    resp = requests.post(
        f"{endpoint}/v1/context",
        json={"query": query["query"], "top_k": k},
        headers=headers,
        timeout=10,
    )
    resp.raise_for_status()
    results = resp.json().get("results", [])
    return [r["memory_id"] for r in results]


def evaluate(queries: list[dict], retrieve_fn, k_values: list[int] = K_VALUES) -> dict:
    """Run evaluation and compute metrics."""
    metrics = {f"ndcg@{k}": [] for k in k_values}
    metrics.update({f"recall@{k}": [] for k in k_values})
    metrics["mrr"] = []

    by_type: dict[str, list[dict]] = {}

    for query in queries:
        retrieved = retrieve_fn(query)
        relevant_ids = query.get("relevant_memories", [])
        relevant_scores = query.get("relevance_scores", {})

        for k in k_values:
            metrics[f"ndcg@{k}"].append(ndcg_at_k(relevant_scores, retrieved, k))
            metrics[f"recall@{k}"].append(recall_at_k(relevant_ids, retrieved, k))

        metrics["mrr"].append(mrr(relevant_ids, retrieved))

        qtype = query.get("type", "unknown")
        by_type.setdefault(qtype, []).append({
            "ndcg@10": ndcg_at_k(relevant_scores, retrieved, 10),
            "mrr": mrr(relevant_ids, retrieved),
        })

    # Aggregate
    result = {}
    for metric_name, values in metrics.items():
        result[metric_name] = sum(values) / len(values) if values else 0.0

    # Per-type breakdown
    type_breakdown = {}
    for qtype, items in by_type.items():
        type_breakdown[qtype] = {
            "count": len(items),
            "ndcg@10": sum(i["ndcg@10"] for i in items) / len(items),
            "mrr": sum(i["mrr"] for i in items) / len(items),
        }

    result["by_type"] = type_breakdown
    return result


def print_results(results: dict):
    print("\n=== Retrieval Evaluation Results ===\n")

    for k in K_VALUES:
        ndcg = results[f"ndcg@{k}"]
        recall = results[f"recall@{k}"]
        print(f"  NDCG@{k:2d} = {ndcg:.4f}    Recall@{k:2d} = {recall:.4f}")

    print(f"  MRR    = {results['mrr']:.4f}")

    print("\n  Per-type breakdown:")
    for qtype, stats in sorted(results.get("by_type", {}).items()):
        print(f"    {qtype:12s}  n={stats['count']:3d}  NDCG@10={stats['ndcg@10']:.4f}  MRR={stats['mrr']:.4f}")
    print()


def main():
    parser = argparse.ArgumentParser(description="Retrieval quality eval")
    parser.add_argument("--endpoint", help="Live server endpoint")
    parser.add_argument("--token", help="Bearer token")
    parser.add_argument("--mock", action="store_true", help="Use mock retrieval")
    parser.add_argument("--save-baseline", action="store_true", help="Save results as baseline")
    parser.add_argument("--check-regression", action="store_true",
                        help="Fail if NDCG@10 drops >2%% from baseline")
    args = parser.parse_args()

    if not DATASET.exists():
        print("Dataset not found. Run generate_dataset.py first.")
        sys.exit(1)

    queries = load_dataset()
    corpus = load_corpus()

    if args.mock or not args.endpoint:
        retrieve_fn = lambda q: mock_retrieve(q, corpus)
        print("Using mock retrieval (keyword matching)")
    else:
        retrieve_fn = lambda q: live_retrieve(q, args.endpoint, args.token)
        print(f"Using live retrieval: {args.endpoint}")

    results = evaluate(queries, retrieve_fn)
    print_results(results)

    if args.save_baseline:
        with open(BASELINE, "w") as f:
            json.dump(results, f, indent=2)
        print(f"Baseline saved to {BASELINE}")

    if args.check_regression and BASELINE.exists():
        with open(BASELINE) as f:
            baseline = json.load(f)

        ndcg_now = results["ndcg@10"]
        ndcg_base = baseline["ndcg@10"]
        delta = ndcg_now - ndcg_base
        pct = (delta / ndcg_base * 100) if ndcg_base > 0 else 0

        print(f"NDCG@10: baseline={ndcg_base:.4f}  current={ndcg_now:.4f}  delta={pct:+.1f}%")

        if pct < -2.0:
            print(f"REGRESSION: NDCG@10 dropped {pct:.1f}% (threshold: -2%)")
            sys.exit(1)
        else:
            print("No regression detected.")

    return 0


if __name__ == "__main__":
    sys.exit(main())
