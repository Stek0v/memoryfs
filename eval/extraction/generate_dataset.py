#!/usr/bin/env python3
"""Generate a synthetic extraction eval dataset.

Produces eval/extraction/dataset.jsonl with 200 annotated conversations,
each with expected memory proposals (structured output the extractor
should produce).

Usage: python3 eval/extraction/generate_dataset.py
"""

import json
import random
from pathlib import Path

random.seed(43)

MEMORY_TYPES = ["decision", "fact", "preference", "action_item", "insight"]
SENSITIVITY = ["normal", "normal", "normal", "pii", "secret"]
TOPICS = [
    "database choice", "API design", "auth strategy", "caching layer",
    "deployment pipeline", "monitoring stack", "testing approach",
    "error handling policy", "logging format", "schema migration",
    "rate limiting", "feature flags", "code review process",
    "incident response", "backup strategy", "data retention",
    "access control model", "encryption approach", "webhook design",
    "versioning strategy",
]

ENTITIES = [
    ("Alice", "Person"), ("Bob", "Person"), ("Carlos", "Person"),
    ("Diana", "Person"), ("Eve", "Person"),
    ("Qdrant", "Tool"), ("PostgreSQL", "Tool"), ("Redis", "Tool"),
    ("Kafka", "Tool"), ("Docker", "Tool"), ("Kubernetes", "Tool"),
    ("Project Alpha", "Project"), ("Project Beta", "Project"),
    ("Platform Team", "Team"), ("Security Team", "Team"),
]


def make_conversation(idx: int) -> dict:
    """Create a synthetic conversation with expected extractions."""
    topic = TOPICS[idx % len(TOPICS)]
    mem_type = MEMORY_TYPES[idx % len(MEMORY_TYPES)]
    sensitivity = SENSITIVITY[idx % len(SENSITIVITY)]
    entity1 = random.choice(ENTITIES)
    entity2 = random.choice([e for e in ENTITIES if e != entity1])

    # Build conversation turns
    turns = [
        {
            "role": "user",
            "content": f"We need to decide on {topic}. {entity1[0]} suggested using {entity2[0]} for this.",
        },
        {
            "role": "assistant",
            "content": f"That makes sense. {entity1[0]}'s recommendation aligns with our requirements. "
                        f"Let me note that the team decided to go with {entity2[0]} for {topic}.",
        },
    ]

    # Add complexity for some conversations
    if idx % 3 == 0:
        turns.append({
            "role": "user",
            "content": f"Actually, we also need to consider the impact on {random.choice(TOPICS)}.",
        })
        turns.append({
            "role": "assistant",
            "content": f"Good point. I'll add that as a related concern. "
                        f"The decision about {topic} affects our approach to that as well.",
        })

    # PII injection for some
    if sensitivity == "pii":
        turns[0]["content"] += f" My email is {entity1[0].lower()}@example.com and phone is 555-0{idx:03d}."

    # Secret injection for some
    if sensitivity == "secret":
        turns[0]["content"] += f" The API key is sk-{'a' * 32} but don't store that."

    # Expected extraction
    expected_proposals = [
        {
            "type": mem_type,
            "title": f"Decision: {topic}",
            "content": f"{entity1[0]} recommended {entity2[0]} for {topic}. Team agreed.",
            "sensitivity": sensitivity,
            "tags": [topic.split()[0].lower()],
            "entities": [
                {"name": entity1[0], "kind": entity1[1]},
                {"name": entity2[0], "kind": entity2[1]},
            ],
        }
    ]

    # Multi-memory conversations
    if idx % 5 == 0:
        extra_topic = TOPICS[(idx + 7) % len(TOPICS)]
        expected_proposals.append({
            "type": "action_item",
            "title": f"TODO: follow up on {extra_topic}",
            "content": f"Need to investigate {extra_topic} as follow-up.",
            "sensitivity": "normal",
            "tags": [extra_topic.split()[0].lower()],
            "entities": [],
        })

    return {
        "id": f"conv_{idx:04d}",
        "turns": turns,
        "expected_proposals": expected_proposals,
        "sensitivity": sensitivity,
        "has_pii": sensitivity == "pii",
        "has_secret": sensitivity == "secret",
    }


def main():
    conversations = [make_conversation(i) for i in range(200)]

    out_dir = Path(__file__).parent
    dataset_path = out_dir / "dataset.jsonl"
    with open(dataset_path, "w") as f:
        for conv in conversations:
            f.write(json.dumps(conv, ensure_ascii=False) + "\n")

    # Stats
    total_proposals = sum(len(c["expected_proposals"]) for c in conversations)
    pii_count = sum(1 for c in conversations if c["has_pii"])
    secret_count = sum(1 for c in conversations if c["has_secret"])
    multi = sum(1 for c in conversations if len(c["expected_proposals"]) > 1)

    print(f"Generated {len(conversations)} conversations -> {dataset_path}")
    print(f"  Total expected proposals: {total_proposals}")
    print(f"  PII conversations: {pii_count}")
    print(f"  Secret conversations: {secret_count}")
    print(f"  Multi-proposal conversations: {multi}")


if __name__ == "__main__":
    main()
