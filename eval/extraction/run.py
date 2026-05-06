#!/usr/bin/env python3
"""Extraction quality evaluation runner.

Computes precision, recall, F1 of memory proposals against ground truth.
Evaluates by memory type and sensitivity classification.

Usage:
  # Against live server
  python3 eval/extraction/run.py --endpoint http://127.0.0.1:7777

  # Dry run (mock: perfect extraction baseline)
  python3 eval/extraction/run.py --mock

  # Save baseline
  python3 eval/extraction/run.py --mock --save-baseline
"""

import argparse
import json
import sys
from pathlib import Path

EVAL_DIR = Path(__file__).parent
DATASET = EVAL_DIR / "dataset.jsonl"
BASELINE = EVAL_DIR / "baseline.json"


def load_dataset() -> list[dict]:
    with open(DATASET) as f:
        return [json.loads(line) for line in f if line.strip()]


def title_similarity(a: str, b: str) -> float:
    """Simple Jaccard similarity on word sets."""
    words_a = set(a.lower().split())
    words_b = set(b.lower().split())
    if not words_a or not words_b:
        return 0.0
    return len(words_a & words_b) / len(words_a | words_b)


def match_proposals(expected: list[dict], actual: list[dict], threshold: float = 0.3) -> tuple[int, int, int]:
    """Match actual proposals to expected by title similarity.

    Returns (true_positives, false_positives, false_negatives).
    """
    matched_expected = set()
    matched_actual = set()

    for i, exp in enumerate(expected):
        best_score = 0.0
        best_j = -1
        for j, act in enumerate(actual):
            if j in matched_actual:
                continue
            score = title_similarity(exp["title"], act.get("title", ""))
            if score > best_score:
                best_score = score
                best_j = j
        if best_score >= threshold and best_j >= 0:
            matched_expected.add(i)
            matched_actual.add(best_j)

    tp = len(matched_expected)
    fp = len(actual) - len(matched_actual)
    fn = len(expected) - len(matched_expected)
    return tp, fp, fn


def check_sensitivity(expected: list[dict], actual: list[dict]) -> dict:
    """Check sensitivity classification accuracy."""
    correct = 0
    total = 0
    for exp in expected:
        exp_sens = exp.get("sensitivity", "normal")
        # Find matching actual
        for act in actual:
            if title_similarity(exp["title"], act.get("title", "")) >= 0.3:
                act_sens = act.get("sensitivity", "normal")
                total += 1
                if act_sens == exp_sens:
                    correct += 1
                break
    return {"correct": correct, "total": total, "accuracy": correct / total if total > 0 else 0.0}


def check_entity_extraction(expected: list[dict], actual: list[dict]) -> dict:
    """Check entity extraction recall."""
    expected_entities = set()
    actual_entities = set()

    for exp in expected:
        for ent in exp.get("entities", []):
            expected_entities.add(ent["name"].lower())

    for act in actual:
        for ent in act.get("entities", []):
            actual_entities.add(ent.get("name", "").lower())

    if not expected_entities:
        return {"precision": 1.0, "recall": 1.0, "f1": 1.0}

    tp = len(expected_entities & actual_entities)
    precision = tp / len(actual_entities) if actual_entities else 0.0
    recall = tp / len(expected_entities)
    f1 = 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0.0

    return {"precision": precision, "recall": recall, "f1": f1}


def mock_extract(conversation: dict) -> list[dict]:
    """Mock extractor — returns expected proposals with slight noise."""
    proposals = []
    for exp in conversation["expected_proposals"]:
        proposal = dict(exp)
        # Simulate imperfect extraction: sometimes miss sensitivity
        if conversation.get("has_secret") and len(proposals) == 0:
            proposal["sensitivity"] = "normal"  # simulate miss
        proposals.append(proposal)
    return proposals


def evaluate(conversations: list[dict], extract_fn) -> dict:
    """Run evaluation across all conversations."""
    total_tp = 0
    total_fp = 0
    total_fn = 0

    sensitivity_stats = {"correct": 0, "total": 0}
    entity_stats = {"precision": [], "recall": [], "f1": []}
    by_type: dict[str, dict] = {}

    pii_detected = {"total": 0, "caught": 0}
    secret_detected = {"total": 0, "caught": 0}

    for conv in conversations:
        expected = conv["expected_proposals"]
        actual = extract_fn(conv)

        tp, fp, fn = match_proposals(expected, actual)
        total_tp += tp
        total_fp += fp
        total_fn += fn

        # Sensitivity check
        sens = check_sensitivity(expected, actual)
        sensitivity_stats["correct"] += sens["correct"]
        sensitivity_stats["total"] += sens["total"]

        # Entity check
        ent = check_entity_extraction(expected, actual)
        entity_stats["precision"].append(ent["precision"])
        entity_stats["recall"].append(ent["recall"])
        entity_stats["f1"].append(ent["f1"])

        # PII/secret tracking
        if conv.get("has_pii"):
            pii_detected["total"] += 1
            for act in actual:
                if act.get("sensitivity") == "pii":
                    pii_detected["caught"] += 1
                    break

        if conv.get("has_secret"):
            secret_detected["total"] += 1
            for act in actual:
                if act.get("sensitivity") == "secret":
                    secret_detected["caught"] += 1
                    break

        # Per-type
        for exp in expected:
            mtype = exp.get("type", "unknown")
            by_type.setdefault(mtype, {"tp": 0, "fp": 0, "fn": 0})

    # Aggregate
    precision = total_tp / (total_tp + total_fp) if (total_tp + total_fp) > 0 else 0.0
    recall = total_tp / (total_tp + total_fn) if (total_tp + total_fn) > 0 else 0.0
    f1 = 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0.0

    avg = lambda lst: sum(lst) / len(lst) if lst else 0.0

    return {
        "precision": precision,
        "recall": recall,
        "f1": f1,
        "tp": total_tp,
        "fp": total_fp,
        "fn": total_fn,
        "sensitivity_accuracy": sensitivity_stats["correct"] / sensitivity_stats["total"]
            if sensitivity_stats["total"] > 0 else 0.0,
        "entity_precision": avg(entity_stats["precision"]),
        "entity_recall": avg(entity_stats["recall"]),
        "entity_f1": avg(entity_stats["f1"]),
        "pii_detection_rate": pii_detected["caught"] / pii_detected["total"]
            if pii_detected["total"] > 0 else 0.0,
        "secret_detection_rate": secret_detected["caught"] / secret_detected["total"]
            if secret_detected["total"] > 0 else 0.0,
    }


def print_results(results: dict):
    print("\n=== Extraction Evaluation Results ===\n")
    print(f"  Precision = {results['precision']:.4f}")
    print(f"  Recall    = {results['recall']:.4f}")
    print(f"  F1        = {results['f1']:.4f}")
    print(f"  TP={results['tp']}  FP={results['fp']}  FN={results['fn']}")
    print()
    print(f"  Sensitivity accuracy = {results['sensitivity_accuracy']:.4f}")
    print(f"  PII detection rate   = {results['pii_detection_rate']:.4f}")
    print(f"  Secret detection rate= {results['secret_detection_rate']:.4f}")
    print()
    print(f"  Entity precision = {results['entity_precision']:.4f}")
    print(f"  Entity recall    = {results['entity_recall']:.4f}")
    print(f"  Entity F1        = {results['entity_f1']:.4f}")
    print()


def main():
    parser = argparse.ArgumentParser(description="Extraction quality eval")
    parser.add_argument("--endpoint", help="Live server endpoint")
    parser.add_argument("--token", help="Bearer token")
    parser.add_argument("--mock", action="store_true", help="Use mock extractor")
    parser.add_argument("--save-baseline", action="store_true", help="Save as baseline")
    args = parser.parse_args()

    if not DATASET.exists():
        print("Dataset not found. Run generate_dataset.py first.")
        sys.exit(1)

    conversations = load_dataset()

    if args.mock or not args.endpoint:
        extract_fn = mock_extract
        print("Using mock extractor")
    else:
        print(f"Live extraction not yet implemented (would use {args.endpoint})")
        sys.exit(1)

    results = evaluate(conversations, extract_fn)
    print_results(results)

    if args.save_baseline:
        with open(BASELINE, "w") as f:
            json.dump(results, f, indent=2)
        print(f"Baseline saved to {BASELINE}")

    # Check F1 threshold from DoD
    if results["f1"] < 0.7:
        print(f"WARNING: F1 {results['f1']:.4f} below 0.7 target")

    return 0


if __name__ == "__main__":
    sys.exit(main())
