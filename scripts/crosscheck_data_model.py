#!/usr/bin/env python3
"""
crosscheck_data_model.py — извлекает YAML-блоки из 02-data-model.md и валидирует
их против соответствующих JSON Schemas. Ловит дрейф между прозой и контрактами.

Стратегия:
  1. Найти все ```yaml...``` блоки в 02-data-model.md.
  2. Если блок содержит поле `type:` со значением, входящим в TYPE_TO_SCHEMA —
     считать его примером фронтматтера и валидировать.
  3. Блоки без `type:` пропускать с пометкой (это могут быть фрагменты политики
     или примеры выражений).

Exit codes:
    0  все блоки прошли
    1  есть нарушения (дрейф)
    2  отсутствуют зависимости
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

try:
    import yaml
    from jsonschema import Draft202012Validator
    from referencing import Registry, Resource
except ImportError as exc:
    print(f"missing dependency: {exc}", file=sys.stderr)
    print("run: pip install jsonschema referencing pyyaml", file=sys.stderr)
    sys.exit(2)


YAML_BLOCK_RE = re.compile(r"```yaml\n(.*?)\n```", re.DOTALL)
TYPE_TO_SCHEMA = {
    "memory":       "memory.schema.json",
    "conversation": "conversation.schema.json",
    "run":          "run.schema.json",
    "decision":     "decision.schema.json",
    "entity":       "entity.schema.json",
    "proposal":     "proposal.schema.json",
}


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    md_path = root / "02-data-model.md"
    schemas_dir = root / "specs" / "schemas" / "v1"

    if not md_path.is_file():
        print(f"not found: {md_path}", file=sys.stderr)
        return 1

    schemas = {}
    for f in schemas_dir.glob("*.schema.json"):
        with f.open() as fh:
            schemas[f.name] = json.load(fh)

    registry = Registry()
    for name, s in schemas.items():
        registry = registry.with_resource(name, Resource.from_contents(s))

    text = md_path.read_text(encoding="utf-8")
    blocks = YAML_BLOCK_RE.findall(text)
    if not blocks:
        print("no YAML blocks found", file=sys.stderr)
        return 1

    skipped = 0
    typed_blocks = 0
    failures = 0
    drift_kinds: dict[str, int] = {}

    for i, block in enumerate(blocks, 1):
        try:
            data = yaml.safe_load(block)
        except yaml.YAMLError as exc:
            print(f"✗ block #{i}: YAML parse error: {exc}")
            failures += 1
            continue

        if not isinstance(data, dict) or "type" not in data:
            skipped += 1
            continue

        ftype = data.get("type")
        schema_name = TYPE_TO_SCHEMA.get(ftype)
        if schema_name is None:
            skipped += 1
            continue

        typed_blocks += 1
        validator = Draft202012Validator(schemas[schema_name], registry=registry)
        errors = sorted(validator.iter_errors(data), key=lambda e: tuple(e.path))
        if errors:
            print(f"✗ block #{i} (type={ftype}): {len(errors)} drift(s) vs {schema_name}")
            for e in errors[:8]:
                loc = "/".join(str(p) for p in e.absolute_path)
                msg = e.message
                # Категоризируем дрейф
                key = "field_missing" if "is a required property" in msg else \
                      "type_mismatch" if "is not of type" in msg else \
                      "pattern_mismatch" if "does not match" in msg else \
                      "additional_props" if "Additional properties" in msg else \
                      "other"
                drift_kinds[key] = drift_kinds.get(key, 0) + 1
                print(f"    [{key:18s}] {loc or '<root>'}: {msg}")
            failures += 1
        else:
            print(f"✓ block #{i} (type={ftype}) — matches {schema_name}")

    print()
    print(f"summary: {typed_blocks} typed YAML blocks, {skipped} skipped (untyped)")
    if failures:
        print(f"FAIL: {failures} block(s) drift from schemas")
        print()
        print("Drift breakdown:")
        for k, n in sorted(drift_kinds.items(), key=lambda kv: -kv[1]):
            print(f"  {k}: {n}")
        return 1
    print("OK: prose and schemas are consistent")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
