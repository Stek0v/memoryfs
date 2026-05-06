#!/usr/bin/env python3
"""
validate_fixtures.py — для каждого .md в fixtures/**/ парсит YAML-фронтматтер,
определяет тип по полю `type` (или по имени файла для policy.yaml) и валидирует
против соответствующей JSON Schema из specs/schemas/v1/.

Exit codes:
    0 — все фронтматтеры валидны
    1 — есть нарушения (печатаются)
    2 — отсутствуют зависимости
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


FRONTMATTER_RE = re.compile(r"^---\n(.*?)\n---\n", re.DOTALL)


# Map memoryfs type → schema filename
TYPE_TO_SCHEMA = {
    "memory":       "memory.schema.json",
    "conversation": "conversation.schema.json",
    "run":          "run.schema.json",
    "decision":     "decision.schema.json",
    "entity":       "entity.schema.json",
    "proposal":     "proposal.schema.json",
}


def parse_frontmatter(path: Path) -> dict | None:
    text = path.read_text(encoding="utf-8")
    m = FRONTMATTER_RE.match(text)
    if not m:
        return None
    return yaml.safe_load(m.group(1))


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    schemas_dir = root / "specs" / "schemas" / "v1"
    fixtures_dir = root / "fixtures"

    schemas: dict[str, dict] = {}
    for f in schemas_dir.glob("*.schema.json"):
        with f.open() as fh:
            schemas[f.name] = json.load(fh)

    registry = Registry()
    for name, schema in schemas.items():
        registry = registry.with_resource(name, Resource.from_contents(schema))

    targets: list[tuple[Path, str]] = []

    # Markdown frontmatters
    for md in sorted(fixtures_dir.rglob("*.md")):
        if md.name == "README.md":
            continue
        fm = parse_frontmatter(md)
        if fm is None:
            continue
        ftype = fm.get("type")
        schema_name = TYPE_TO_SCHEMA.get(ftype)
        if schema_name is None:
            print(f"⚠ skipping {md.relative_to(root)}: unknown type={ftype!r}")
            continue
        targets.append((md, schema_name))

    # policy.yaml файлы
    for yamlf in sorted(fixtures_dir.rglob("policy.yaml")):
        targets.append((yamlf, "policy.schema.json"))

    if not targets:
        print(f"no fixtures found in {fixtures_dir}", file=sys.stderr)
        return 1

    failures = 0
    for path, schema_name in targets:
        if path.suffix == ".yaml":
            try:
                with path.open() as fh:
                    data = yaml.safe_load(fh)
            except Exception as exc:
                print(f"✗ {path.relative_to(root)} — parse failed: {exc}")
                failures += 1
                continue
        else:
            data = parse_frontmatter(path)

        validator = Draft202012Validator(schemas[schema_name], registry=registry)
        errors = sorted(validator.iter_errors(data), key=lambda e: e.path)
        if errors:
            print(f"✗ {path.relative_to(root)} ({schema_name}): {len(errors)} error(s)")
            for e in errors[:10]:
                loc = "/".join(str(p) for p in e.absolute_path)
                print(f"    - {loc or '<root>'}: {e.message}")
            failures += 1
        else:
            print(f"✓ {path.relative_to(root)}")

    print()
    if failures:
        print(f"FAIL: {failures}/{len(targets)} fixtures invalid")
        return 1
    print(f"OK: {len(targets)} fixtures valid against schemas/v1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
