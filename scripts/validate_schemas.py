#!/usr/bin/env python3
"""
validate_schemas.py — проверяет, что каждая JSON Schema в specs/schemas/v1/
самостоятельна well-formed (Draft 2020-12) и что все её $ref резолвятся.

Exit codes:
    0 — все схемы валидны
    1 — хотя бы одна схема некорректна
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
    print("run: pip install jsonschema referencing", file=sys.stderr)
    sys.exit(2)


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    schemas_dir = root / "specs" / "schemas" / "v1"
    if not schemas_dir.is_dir():
        print(f"not a directory: {schemas_dir}", file=sys.stderr)
        return 1

    schema_files = sorted(schemas_dir.glob("*.schema.json"))
    if not schema_files:
        print(f"no schemas found in {schemas_dir}", file=sys.stderr)
        return 1

    # Build registry: each schema reachable by its filename
    schemas: dict[str, dict] = {}
    for f in schema_files:
        with f.open() as fh:
            schemas[f.name] = json.load(fh)

    registry = Registry()
    for name, schema in schemas.items():
        registry = registry.with_resource(name, Resource.from_contents(schema))

    failures = 0
    for name, schema in schemas.items():
        # 1. Well-formedness (meta-schema check)
        try:
            Draft202012Validator.check_schema(schema)
        except Exception as exc:
            print(f"✗ {name}: meta-schema check failed → {exc}")
            failures += 1
            continue

        # 2. Validator can be instantiated
        try:
            Draft202012Validator(schema, registry=registry)
        except Exception as exc:
            print(f"✗ {name}: validator construction failed → {exc}")
            failures += 1
            continue

        # 3. $ref-полнота: пробуем дешёвую round-trip — сериализуем validation
        # пустого объекта; некорректные $ref всплывут.
        try:
            list(Draft202012Validator(schema, registry=registry).iter_errors({}))
        except Exception as exc:
            print(f"✗ {name}: $ref resolution failed → {exc}")
            failures += 1
            continue

        print(f"✓ {name}")

    print()
    if failures:
        print(f"FAIL: {failures}/{len(schemas)} schemas have problems")
        return 1
    print(f"OK: {len(schemas)} schemas valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
