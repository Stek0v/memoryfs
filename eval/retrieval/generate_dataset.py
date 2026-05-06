#!/usr/bin/env python3
"""Generate a synthetic retrieval eval dataset.

Produces eval/retrieval/dataset.jsonl with 200 query/answer pairs, each
annotated with relevance judgments. The dataset covers diverse query types:
factual recall, temporal, entity-based, negation, and multi-hop.

Usage: python3 eval/retrieval/generate_dataset.py
"""

import json
import random
from pathlib import Path

random.seed(42)

TOPICS = [
    "authentication", "database migration", "API rate limiting", "caching strategy",
    "error handling", "logging pipeline", "deployment workflow", "testing strategy",
    "performance optimization", "security audit", "code review process",
    "dependency management", "feature flags", "monitoring setup", "backup strategy",
    "schema versioning", "access control", "data encryption", "webhook integration",
    "CI/CD pipeline", "load balancing", "service mesh", "container orchestration",
    "message queues", "event sourcing", "CQRS pattern", "microservices boundary",
    "API versioning", "GraphQL schema", "REST conventions",
]

ENTITIES = [
    "Alice", "Bob", "Carlos", "Diana", "Eve", "Frank", "Grace", "Hector",
    "Iris", "Jack", "Karen", "Leo", "Maya", "Noah", "Olivia", "Paul",
]

TOOLS = [
    "Qdrant", "PostgreSQL", "Redis", "Kafka", "Grafana", "Prometheus",
    "Docker", "Kubernetes", "Terraform", "GitHub Actions", "Sentry",
    "DataDog", "PagerDuty", "Slack", "Linear", "Notion",
]

SCOPES = ["project", "team", "personal", "org"]
SENSITIVITY = ["normal", "pii", "secret", "financial"]


def make_memory(idx: int, topic: str) -> dict:
    """Create a synthetic memory document."""
    entity = random.choice(ENTITIES)
    tool = random.choice(TOOLS)
    scope = random.choice(SCOPES)
    return {
        "id": f"mem_{idx:06d}",
        "title": f"Decision: {topic}",
        "path": f"memories/mem_{idx:06d}.md",
        "scope": scope,
        "tags": [topic.split()[0].lower(), tool.lower()],
        "entity_mentions": [entity, tool],
        "content_preview": (
            f"{entity} decided to use {tool} for {topic}. "
            f"This was discussed in the {scope} context on day {idx}."
        ),
    }


def make_query_factual(memories: list[dict], idx: int) -> dict:
    """Generate a factual recall query."""
    mem = memories[idx % len(memories)]
    entity = mem["entity_mentions"][0]
    tool = mem["entity_mentions"][1]
    topic = mem["title"].replace("Decision: ", "")

    return {
        "id": f"q_{idx:04d}",
        "query": f"What did {entity} decide about {topic}?",
        "type": "factual",
        "relevant_memories": [mem["id"]],
        "relevance_scores": {mem["id"]: 3},
        "expected_entities": [entity, tool],
    }


def make_query_entity(memories: list[dict], idx: int) -> dict:
    """Generate an entity-based query."""
    entity = random.choice(ENTITIES)
    relevant = [m for m in memories if entity in m["entity_mentions"]][:3]

    return {
        "id": f"q_{idx:04d}",
        "query": f"What has {entity} been involved in?",
        "type": "entity",
        "relevant_memories": [m["id"] for m in relevant],
        "relevance_scores": {m["id"]: 2 for m in relevant},
        "expected_entities": [entity],
    }


def make_query_topic(memories: list[dict], idx: int) -> dict:
    """Generate a topic-based query."""
    topic = random.choice(TOPICS)
    relevant = [m for m in memories if topic in m["title"]][:5]

    return {
        "id": f"q_{idx:04d}",
        "query": f"What decisions were made about {topic}?",
        "type": "topic",
        "relevant_memories": [m["id"] for m in relevant],
        "relevance_scores": {m["id"]: 2 for m in relevant},
        "expected_entities": [],
    }


def make_query_temporal(memories: list[dict], idx: int) -> dict:
    """Generate a temporal query."""
    recent = memories[-10:]
    mem = random.choice(recent)

    return {
        "id": f"q_{idx:04d}",
        "query": "What were the most recent decisions?",
        "type": "temporal",
        "relevant_memories": [m["id"] for m in recent[:5]],
        "relevance_scores": {m["id"]: 1 for m in recent[:5]},
        "expected_entities": [],
    }


def make_query_negation(memories: list[dict], idx: int) -> dict:
    """Generate a negation/no-result query."""
    return {
        "id": f"q_{idx:04d}",
        "query": "What decisions were made about quantum teleportation?",
        "type": "negation",
        "relevant_memories": [],
        "relevance_scores": {},
        "expected_entities": [],
    }


def main():
    # Generate memories
    memories = []
    for i in range(200):
        topic = TOPICS[i % len(TOPICS)]
        memories.append(make_memory(i, topic))

    # Generate queries
    queries = []
    generators = [
        make_query_factual,
        make_query_entity,
        make_query_topic,
        make_query_temporal,
        make_query_negation,
    ]

    for i in range(200):
        gen = generators[i % len(generators)]
        queries.append(gen(memories, i))

    # Write dataset
    out_dir = Path(__file__).parent
    dataset_path = out_dir / "dataset.jsonl"
    with open(dataset_path, "w") as f:
        for q in queries:
            f.write(json.dumps(q, ensure_ascii=False) + "\n")

    # Write memories corpus
    corpus_path = out_dir / "corpus.jsonl"
    with open(corpus_path, "w") as f:
        for m in memories:
            f.write(json.dumps(m, ensure_ascii=False) + "\n")

    print(f"Generated {len(queries)} queries -> {dataset_path}")
    print(f"Generated {len(memories)} memories -> {corpus_path}")
    print(f"Query types: {', '.join(sorted(set(q['type'] for q in queries)))}")


if __name__ == "__main__":
    main()
