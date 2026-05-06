"""Extractor CLI entry point."""
from __future__ import annotations

import argparse
import sys


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="memoryfs-extractor",
        description="Pull memories from conversations via LLM extraction",
    )
    parser.add_argument(
        "--endpoint",
        default="http://127.0.0.1:7777",
        help="MemoryFS API endpoint",
    )
    parser.add_argument(
        "--workspace",
        required=True,
        help="Workspace ID (ws_…)",
    )
    parser.add_argument(
        "--token",
        required=False,
        help="Bearer token (atk_…). If omitted, MEMORYFS_TOKEN env is used.",
    )
    parser.add_argument(
        "--mode",
        choices=["once", "watch", "backfill"],
        default="watch",
        help="once: process pending, exit. watch: long-running. backfill: process all conversations again.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Compute proposed memories but do NOT post them to the server.",
    )
    args = parser.parse_args(argv)

    print("memoryfs-extractor: skeleton, no work performed")
    print(f"  endpoint={args.endpoint}")
    print(f"  workspace={args.workspace}")
    print(f"  mode={args.mode}")
    print(f"  dry_run={args.dry_run}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
