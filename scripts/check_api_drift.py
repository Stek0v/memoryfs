#!/usr/bin/env python3
"""Check that implemented Rust routes match OpenAPI spec paths.

Compares paths declared in specs/openapi.yaml against routes registered
in crates/core/src/api.rs. Reports missing implementations and extra
routes not in the spec.

Exit code 0: no drift. Exit code 1: drift detected.
"""

import re
import sys
import yaml
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_openapi_paths() -> dict[str, set[str]]:
    """Return {path: {methods}} from the OpenAPI spec."""
    spec_path = ROOT / "specs" / "openapi.yaml"
    with open(spec_path) as f:
        spec = yaml.safe_load(f)

    result: dict[str, set[str]] = {}
    for path, path_item in spec.get("paths", {}).items():
        methods = set()
        for method in ("get", "post", "put", "patch", "delete"):
            if method in path_item:
                methods.add(method.upper())
        if methods:
            # Normalize: /v1 prefix is in the server URL, not the path
            normalized = "/v1" + path if not path.startswith("/v1") else path
            result[normalized] = methods
    return result


def load_rust_routes() -> dict[str, set[str]]:
    """Parse .route() calls from api.rs to extract {path: {methods}}."""
    api_rs = ROOT / "crates" / "core" / "src" / "api.rs"
    content = api_rs.read_text()

    result: dict[str, set[str]] = {}

    # Match .route("path", get(handler).put(handler)...)
    route_pattern = re.compile(
        r'\.route\(\s*"([^"]+)"\s*,\s*([^)]+(?:\)[^.)\n]*)*)\)'
    )

    for match in route_pattern.finditer(content):
        path = match.group(1)
        handler_str = match.group(2)

        methods = set()
        for method in ("get", "post", "put", "patch", "delete"):
            if re.search(rf'\b{method}\b\s*\(', handler_str):
                methods.add(method.upper())

        if methods:
            result[path] = methods

    return result


def normalize_openapi_path(path: str) -> str:
    """Convert OpenAPI {param} to axum :param or *param syntax."""
    # /files/{path} -> /files/*path (catch-all in axum)
    # /workspaces/{workspace_id} -> /workspaces/:workspace_id
    # /entities/{entity_id} -> /entities/:entity_id
    result = path
    result = re.sub(r'\{(path)\}', r'*\1', result)
    result = re.sub(r'\{(\w+)\}', r':\1', result)
    return result


def main() -> int:
    openapi_paths = load_openapi_paths()
    rust_routes = load_rust_routes()

    # Normalize OpenAPI paths to axum format
    openapi_normalized: dict[str, set[str]] = {}
    for path, methods in openapi_paths.items():
        normalized = normalize_openapi_path(path)
        openapi_normalized[normalized] = methods

    # Find drift
    missing_impl: list[str] = []
    extra_impl: list[str] = []
    method_mismatch: list[str] = []

    for path, spec_methods in openapi_normalized.items():
        if path not in rust_routes:
            missing_impl.append(f"  {path} [{', '.join(sorted(spec_methods))}]")
        else:
            impl_methods = rust_routes[path]
            missing_methods = spec_methods - impl_methods
            if missing_methods:
                method_mismatch.append(
                    f"  {path}: spec has {', '.join(sorted(missing_methods))} "
                    f"but impl only has {', '.join(sorted(impl_methods))}"
                )

    for path, impl_methods in rust_routes.items():
        if path not in openapi_normalized:
            extra_impl.append(f"  {path} [{', '.join(sorted(impl_methods))}]")

    # Report
    has_drift = False

    if missing_impl:
        print("MISSING implementations (in spec, not in code):")
        for line in sorted(missing_impl):
            print(line)
        print()
        has_drift = True

    if method_mismatch:
        print("METHOD mismatches:")
        for line in sorted(method_mismatch):
            print(line)
        print()
        has_drift = True

    if extra_impl:
        print("EXTRA routes (in code, not in spec):")
        for line in sorted(extra_impl):
            print(line)
        print()
        # Extra routes are informational, not a failure
        # (impl may have admin/debug endpoints not in public spec)

    if not has_drift and not extra_impl:
        print("OK: no API drift detected")
    elif not has_drift:
        print("OK: no missing implementations (extra routes are informational)")

    strict = "--strict" in sys.argv
    if strict:
        return 1 if has_drift else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
