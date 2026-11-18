#!/usr/bin/env python3
"""A real outbound-connection probe for the isolation suite.

It does not speak the extension protocol: it attempts one TCP connection to a
loopback port named by its first argument and records the observable result in
its own (writable) working directory. The test owns the listener, so a refusal
is a policy decision, not a missing service.
"""

from __future__ import annotations

import socket
import sys
from pathlib import Path


def main() -> int:
    port = int(sys.argv[1])
    result = "denied"
    try:
        connection = socket.create_connection(("127.0.0.1", port), timeout=2.0)
        connection.close()
        result = "connected"
    except OSError as error:
        result = f"denied:{error.__class__.__name__}"
    Path("net-result.txt").write_text(result, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
