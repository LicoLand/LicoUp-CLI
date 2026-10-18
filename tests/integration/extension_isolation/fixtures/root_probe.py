#!/usr/bin/env python3
"""A fork-free filesystem confinement probe for the isolation suite.

It attempts one write outside its roots, one read outside its roots and one write
inside its own root, catches each refusal as an error (so nothing aborts), and
records the three outcomes in its writable working directory. Python creation
probes live in `group_escape.py`; this one is about paths.

Usage: root_probe.py <outside_write> <outside_read>
"""

from __future__ import annotations

import sys
from pathlib import Path


def main() -> int:
    outside_write = sys.argv[1]
    outside_read = sys.argv[2]
    results: list[str] = []

    try:
        Path(outside_write).write_text("escaped\n", encoding="utf-8")
        results.append("write-allowed")
    except OSError as error:
        results.append(f"write-denied:{error.errno}")

    try:
        reserved = Path(outside_read).read_text(encoding="utf-8")
        results.append("read-allowed" if reserved else "read-empty")
    except OSError as error:
        results.append(f"read-denied:{error.errno}")

    try:
        Path("inside.txt").write_text("inside", encoding="utf-8")
        results.append("inside-wrote")
    except OSError as error:
        results.append(f"inside-denied:{error.errno}")

    Path("probe-result.txt").write_text(" ".join(results) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
