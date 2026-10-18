#!/usr/bin/env python3
"""Render a recorded frame array as the JSONL stream this sample consumes.

The carrier protocol is one JSON frame per line, so the recorded exchange is a
line stream. The repository privacy policy admits committed fixtures only as
JSON configuration or data files and denies committed JSONL exports, so the
frames are kept in `transcript.json`/`events.json` and rendered back to the
line format here:

    python3 -B replay.py transcript.json | python3 -B agent.py
"""
from __future__ import annotations

import json
import sys


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    for name in argv:
        with open(name, encoding="utf-8") as handle:
            for frame in json.load(handle):
                print(json.dumps(frame, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
