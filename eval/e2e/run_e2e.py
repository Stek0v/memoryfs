#!/usr/bin/env python3
"""
End-to-end retrieval test: MemoryFS markdown → Levara gRPC (embed + HNSW) → search.

Prerequisites:
  - Levara running on localhost:50051 (just dev-up)
  - Ollama running with nomic-embed-text-v2-moe (ollama serve)
  - Dataset generated (python eval/e2e/gen_dataset.py)
  - grpcio + grpcio-tools installed (pip install grpcio grpcio-tools)
  - Proto stubs generated (see gen_dataset.py header)

Usage:
  python eval/e2e/run_e2e.py                    # full run
  python eval/e2e/run_e2e.py --max-queries 200  # quick smoke test
  python eval/e2e/run_e2e.py --skip-ingest      # search-only (after prior ingest)
"""

import argparse
import json
import os
import struct
import sys
import time
from pathlib import Path

import grpc
import requests

# Generated from levara.proto
import levara_pb2
import levara_pb2_grpc

LEVARA_GRPC = os.getenv("LEVARA_GRPC", "127.0.0.1:50051")
OLLAMA_URL = os.getenv("OLLAMA_URL", "http://127.0.0.1:11434")
EMBED_MODEL = os.getenv("EMBED_MODEL", "nomic-embed-text-v2-moe")
LEVARA_EMBED_ENDPOINT = os.getenv(
    "LEVARA_EMBED_ENDPOINT", "http://host.docker.internal:11434/api/embed"
)
COLLECTION = "memoryfs_e2e_test"
DIMENSION = 768

DATASET_DIR = Path(__file__).parent / "dataset"
GROUND_TRUTH = Path(__file__).parent / "ground_truth.json"
RESULTS_DIR = Path(__file__).parent / "results"


def get_stub():
    channel = grpc.insecure_channel(
        LEVARA_GRPC,
        options=[
            ("grpc.max_send_message_length", 64 * 1024 * 1024),
            ("grpc.max_receive_message_length", 64 * 1024 * 1024),
        ],
    )
    return levara_pb2_grpc.LevaraServiceStub(channel), channel


def check_services(stub):
    try:
        stub.HasCollection(levara_pb2.HasCollectionReq(name="__probe__"))
    except grpc.RpcError as e:
        if e.code() == grpc.StatusCode.UNAVAILABLE:
            print(f"Levara gRPC not reachable at {LEVARA_GRPC}: {e}")
            sys.exit(1)

    try:
        r = requests.get(f"{OLLAMA_URL}/api/tags", timeout=3)
        models = [m["name"] for m in r.json().get("models", [])]
        if not any(EMBED_MODEL in m for m in models):
            print(f"Model {EMBED_MODEL} not found in Ollama. Available: {models}")
            sys.exit(1)
    except Exception as e:
        print(f"Ollama not reachable at {OLLAMA_URL}: {e}")
        sys.exit(1)

    print(f"  Levara gRPC: OK ({LEVARA_GRPC})")
    print(f"  Ollama: OK ({EMBED_MODEL})")


def embed_batch(texts: list[str], batch_size: int = 32) -> list[list[float]]:
    all_vectors = []
    for i in range(0, len(texts), batch_size):
        batch = texts[i : i + batch_size]
        r = requests.post(
            f"{OLLAMA_URL}/api/embed",
            json={"model": EMBED_MODEL, "input": batch},
        )
        r.raise_for_status()
        data = r.json()
        vecs = data.get("embeddings", [data.get("embedding")])
        all_vectors.extend(vecs)
    return all_vectors


def vec_to_bytes(vec: list[float]) -> bytes:
    return struct.pack(f"{len(vec)}f", *vec)


def ensure_collection(stub):
    resp = stub.HasCollection(levara_pb2.HasCollectionReq(name=COLLECTION))
    if resp.exists:
        print(f"  Collection '{COLLECTION}' exists")
        return
    stub.CreateCollection(
        levara_pb2.CreateCollectionReq(name=COLLECTION)
    )
    print(f"  Created collection '{COLLECTION}'")


def drop_collection(stub):
    try:
        stub.DropCollection(levara_pb2.DropCollectionReq(name=COLLECTION))
        print(f"  Dropped collection '{COLLECTION}'")
    except grpc.RpcError:
        pass


def ingest_documents(stub, dataset_dir: Path):
    files = sorted(dataset_dir.glob("*.md"))
    print(f"\n=== Ingesting {len(files)} documents ===")

    texts = []
    ids = []
    payloads = []
    for f in files:
        content = f.read_text(encoding="utf-8")
        body_start = content.find("---", 3)
        if body_start != -1:
            body = content[body_start + 3 :].strip()
        else:
            body = content
        texts.append(body)
        ids.append(f.stem)
        payloads.append(json.dumps({"filename": f.name}))

    print(f"  Embedding {len(texts)} documents via Ollama...")
    t0 = time.time()
    vectors = embed_batch(texts, batch_size=32)
    embed_time = time.time() - t0
    print(f"  Embedded in {embed_time:.1f}s ({len(texts) / embed_time:.0f} docs/sec)")

    print(f"  Inserting into Levara via gRPC BatchInsert...")
    t0 = time.time()
    batch_size = 200
    for i in range(0, len(files), batch_size):
        batch_ids = ids[i : i + batch_size]
        batch_vecs = vectors[i : i + batch_size]
        batch_payloads = payloads[i : i + batch_size]

        records = []
        for pid, vec, payload in zip(batch_ids, batch_vecs, batch_payloads):
            records.append(
                levara_pb2.InsertRecord(
                    id=pid,
                    vector=vec,
                    metadata_json=payload,
                )
            )

        stub.BatchInsert(
            levara_pb2.BatchInsertReq(
                collection=COLLECTION,
                records=records,
            )
        )
        done = min(i + batch_size, len(files))
        print(f"    {done}/{len(files)} points inserted")

    insert_time = time.time() - t0
    print(f"  Inserted in {insert_time:.1f}s")
    return embed_time, insert_time


def search_grpc(stub, vector: list[float], top_k: int = 10) -> list[dict]:
    resp = stub.Search(
        levara_pb2.SearchReq(collection=COLLECTION, vector=vector, top_k=top_k)
    )
    return [{"id": r.id, "score": r.score} for r in resp.results]


def run_queries(
    stub, queries: list[dict], max_queries: int | None = None, top_k: int = 10
):
    if max_queries:
        queries = queries[:max_queries]

    print(f"\n=== Running {len(queries)} queries (top_k={top_k}) ===")

    query_texts = [q["query"] for q in queries]
    print(f"  Embedding {len(query_texts)} queries via Ollama...")
    t0 = time.time()
    query_vectors = embed_batch(query_texts, batch_size=32)
    embed_time = time.time() - t0
    print(f"  Embedded in {embed_time:.1f}s")

    hits_at = {1: 0, 3: 0, 5: 0, 10: 0}
    mrr_sum = 0.0
    per_article = {}
    failures = []

    print(f"  Searching via gRPC (throttled to ~80 req/min)...")
    t0 = time.time()
    for i, (q, qvec) in enumerate(zip(queries, query_vectors)):
        expected_file = q["expected_file"]
        expected_id = expected_file.replace(".md", "")

        try:
            results = search_grpc(stub, qvec, top_k=top_k)
        except grpc.RpcError as e:
            if "rate limit" in str(e).lower():
                time.sleep(2.0)
                results = search_grpc(stub, qvec, top_k=top_k)
            else:
                raise

        result_ids = [r["id"] for r in results]

        rank = None
        for j, rid in enumerate(result_ids):
            if rid == expected_id:
                rank = j + 1
                break

        if rank is not None:
            mrr_sum += 1.0 / rank
            for k in hits_at:
                if rank <= k:
                    hits_at[k] += 1
        else:
            failures.append(
                {
                    "query": q["query"],
                    "expected": expected_id,
                    "got_top3": result_ids[:3],
                    "article": q["article"],
                }
            )

        article = q["article"]
        if article not in per_article:
            per_article[article] = {"total": 0, "hit_at_5": 0}
        per_article[article]["total"] += 1
        if rank is not None and rank <= 5:
            per_article[article]["hit_at_5"] += 1

        if (i + 1) % 100 == 0:
            r1 = hits_at[1] / (i + 1)
            r5 = hits_at[5] / (i + 1)
            print(f"    {i + 1}/{len(queries)} — R@1={r1:.3f} R@5={r5:.3f}")

        if i >= 19:
            time.sleep(0.75)

    search_time = time.time() - t0
    n = len(queries)

    metrics = {
        "total_queries": n,
        "top_k": top_k,
        "recall_at_1": hits_at[1] / n,
        "recall_at_3": hits_at[3] / n,
        "recall_at_5": hits_at[5] / n,
        "recall_at_10": hits_at[10] / n,
        "mrr": mrr_sum / n,
        "embed_time_s": embed_time,
        "search_time_s": search_time,
        "avg_query_latency_ms": (search_time / n) * 1000,
    }

    return metrics, per_article, failures


def print_report(
    metrics: dict,
    per_article: dict,
    failures: list,
    embed_time: float,
    insert_time: float,
):
    print("\n" + "=" * 60)
    print("E2E RETRIEVAL RESULTS")
    print("=" * 60)

    print(f"\nQueries:       {metrics['total_queries']}")
    print(f"Recall@1:      {metrics['recall_at_1']:.3f}")
    print(f"Recall@3:      {metrics['recall_at_3']:.3f}")
    print(f"Recall@5:      {metrics['recall_at_5']:.3f}")
    print(f"Recall@10:     {metrics['recall_at_10']:.3f}")
    print(f"MRR:           {metrics['mrr']:.3f}")

    print(f"\nIngest:        {embed_time:.1f}s embed + {insert_time:.1f}s insert")
    print(
        f"Search:        {metrics['search_time_s']:.1f}s total, "
        f"{metrics['avg_query_latency_ms']:.1f}ms avg/query"
    )

    print(f"\n--- Per-article Recall@5 (worst 10) ---")
    sorted_articles = sorted(
        per_article.items(),
        key=lambda x: x[1]["hit_at_5"] / max(x[1]["total"], 1),
    )
    for title, stats in sorted_articles[:10]:
        rate = stats["hit_at_5"] / max(stats["total"], 1)
        print(f"  {rate:.0%} ({stats['hit_at_5']}/{stats['total']})  {title}")
    if len(sorted_articles) > 10:
        print(f"  ... ({len(sorted_articles) - 10} more articles)")

    if failures:
        print(f"\n--- Sample failures ({len(failures)} total) ---")
        for f in failures[:10]:
            print(f"  Q: {f['query'][:80]}")
            print(f"    expected: {f['expected']}")
            print(f"    got:      {f['got_top3']}")
            print()


def save_results(metrics: dict, per_article: dict, failures: list):
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    ts = time.strftime("%Y%m%d_%H%M%S")
    out = RESULTS_DIR / f"e2e_{ts}.json"
    with open(out, "w") as f:
        json.dump(
            {
                "timestamp": ts,
                "config": {
                    "collection": COLLECTION,
                    "embed_model": EMBED_MODEL,
                    "dimension": DIMENSION,
                    "levara_grpc": LEVARA_GRPC,
                },
                "metrics": metrics,
                "per_article": per_article,
                "sample_failures": failures[:50],
            },
            f,
            indent=2,
            ensure_ascii=False,
        )
    print(f"\nResults saved to {out}")


def main():
    parser = argparse.ArgumentParser(description="E2E retrieval test via Levara gRPC")
    parser.add_argument(
        "--max-queries", type=int, default=None, help="Limit queries (default: all)"
    )
    parser.add_argument(
        "--skip-ingest", action="store_true", help="Skip ingestion (reuse existing)"
    )
    parser.add_argument(
        "--top-k", type=int, default=10, help="Top-K for search (default: 10)"
    )
    parser.add_argument(
        "--fresh", action="store_true", help="Drop and recreate collection"
    )
    args = parser.parse_args()

    if not GROUND_TRUTH.exists():
        print("Ground truth not found. Run: python eval/e2e/gen_dataset.py")
        sys.exit(1)

    stub, channel = get_stub()
    check_services(stub)

    with open(GROUND_TRUTH) as f:
        gt = json.load(f)

    embed_time = 0.0
    insert_time = 0.0

    if not args.skip_ingest:
        if args.fresh:
            drop_collection(stub)
        ensure_collection(stub)
        embed_time, insert_time = ingest_documents(stub, DATASET_DIR)
    else:
        print("Skipping ingest (--skip-ingest)")

    metrics, per_article, failures = run_queries(
        stub, gt["queries"], max_queries=args.max_queries, top_k=args.top_k
    )

    print_report(metrics, per_article, failures, embed_time, insert_time)
    save_results(metrics, per_article, failures)
    channel.close()


if __name__ == "__main__":
    main()
