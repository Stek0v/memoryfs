#!/usr/bin/env python3
"""Convert SQuAD 2.0 dev set into MemoryFS markdown memories + ground truth."""

import json
import hashlib
import os
import sys
from pathlib import Path

SQUAD_PATH = "/tmp/squad-dev-v2.0.json"
OUT_DIR = Path(__file__).parent / "dataset"
GROUND_TRUTH = Path(__file__).parent / "ground_truth.json"


def slug(text: str, max_len: int = 60) -> str:
    s = text.lower().replace(" ", "-")
    s = "".join(c for c in s if c.isalnum() or c == "-")
    return s[:max_len].rstrip("-")


def stable_id(article_title: str, para_idx: int) -> str:
    h = hashlib.sha256(f"{article_title}:{para_idx}".encode()).hexdigest()[:16]
    return f"mem_{h}"


def main():
    if not os.path.exists(SQUAD_PATH):
        print(f"Download SQuAD first: curl -sL -o {SQUAD_PATH} "
              "https://rajpurkar.github.io/SQuAD-explorer/dataset/dev-v2.0.json")
        sys.exit(1)

    with open(SQUAD_PATH) as f:
        data = json.load(f)

    OUT_DIR.mkdir(parents=True, exist_ok=True)

    ground_truth = []
    file_count = 0

    for article in data["data"]:
        title = article["title"].replace("_", " ")
        art_slug = slug(title)

        for para_idx, para in enumerate(article["paragraphs"]):
            context = para["context"]
            mem_id = stable_id(title, para_idx)
            filename = f"{art_slug}_{para_idx:03d}.md"

            md = (
                f"---\n"
                f"id: {mem_id}\n"
                f"title: \"{title} §{para_idx}\"\n"
                f"kind: fact\n"
                f"sensitivity: public\n"
                f"source: squad-v2.0/{art_slug}\n"
                f"article: \"{title}\"\n"
                f"paragraph_index: {para_idx}\n"
                f"---\n\n"
                f"# {title}\n\n"
                f"{context}\n"
            )

            (OUT_DIR / filename).write_text(md, encoding="utf-8")
            file_count += 1

            for qa in para["qas"]:
                if qa.get("is_impossible", False):
                    continue

                answers = qa["answers"]
                if not answers:
                    continue

                ground_truth.append({
                    "query": qa["question"],
                    "expected_file": filename,
                    "expected_mem_id": mem_id,
                    "article": title,
                    "paragraph_index": para_idx,
                    "answer_text": answers[0]["text"],
                    "qa_id": qa["id"],
                })

    with open(GROUND_TRUTH, "w", encoding="utf-8") as f:
        json.dump({
            "dataset": "squad-v2.0-dev",
            "total_files": file_count,
            "total_queries": len(ground_truth),
            "queries": ground_truth,
        }, f, indent=2, ensure_ascii=False)

    print(f"Generated {file_count} markdown files in {OUT_DIR}")
    print(f"Generated {len(ground_truth)} ground-truth queries in {GROUND_TRUTH}")


if __name__ == "__main__":
    main()
