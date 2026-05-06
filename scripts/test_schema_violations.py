#!/usr/bin/env python3
"""
test_schema_violations.py — прогоняет tests/adversarial/schema-violations/violations.jsonl.
Каждая строка содержит фронтматтер, который ДОЛЖЕН быть отвергнут указанной схемой.
Если schema случайно пропустит — это ложно-отрицательное срабатывание (баг в схеме).

Exit codes:
    0 — все violations корректно отвергнуты
    1 — schema accepted чего-то, что должна была отвергнуть
    2 — нет зависимостей
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

try:
    from jsonschema import Draft202012Validator
    from referencing import Registry, Resource
except ImportError as exc:
    print(f"missing dependency: {exc}", file=sys.stderr)
    sys.exit(2)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    schemas_dir = root / "specs" / "schemas" / "v1"
    cases_path = root / "tests" / "adversarial" / "schema-violations" / "violations.jsonl"

    schemas = {}
    for f in schemas_dir.glob("*.schema.json"):
        with f.open() as fh:
            schemas[f.name] = json.load(fh)

    registry = Registry()
    for name, s in schemas.items():
        registry = registry.with_resource(name, Resource.from_contents(s))

    validators = {
        name: Draft202012Validator(s, registry=registry)
        for name, s in schemas.items()
    }

    failures = 0
    total = 0
    with cases_path.open() as fh:
        for line_no, line in enumerate(fh, 1):
            line = line.strip()
            if not line:
                continue
            total += 1
            try:
                case = json.loads(line)
            except json.JSONDecodeError as exc:
                print(f"✗ line {line_no}: bad JSON — {exc}")
                failures += 1
                continue

            cid = case["id"]
            schema_name = case["violates"]
            reason = case["reason"]
            fm = case["frontmatter"]

            validator = validators.get(schema_name)
            if validator is None:
                print(f"✗ {cid}: unknown schema {schema_name!r}")
                failures += 1
                continue

            errors = list(validator.iter_errors(fm))
            if errors:
                print(f"✓ {cid}: rejected by {schema_name} ({reason}); {len(errors)} error(s)")
            else:
                print(f"✗ {cid}: SCHEMA ACCEPTED IT — expected rejection ({reason})")
                failures += 1

    print()
    if failures:
        print(f"FAIL: {failures}/{total} violations were accepted (schema gaps)")
        return 1
    print(f"OK: {total} violations correctly rejected")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
