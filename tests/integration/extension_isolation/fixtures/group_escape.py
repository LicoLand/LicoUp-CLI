#!/usr/bin/env python3
"""An adversarial descendant probe for the isolation suite.

It is a real interpreter process that forks once. The child either stays in the
supervised process group (`grouped`) or tries to leave it with `setsid()`
(`escape`), records what happened in `descendant.outcome`, then keeps the
stdout pipe open for a bounded window and always exits by itself. The parent
records its own pid and exits immediately, so only the descendant holds the
pipe when the host tears the instance down.

The bounded self-termination is deliberate: a failing test can then leave no
process behind even when reclamation cannot reach the descendant.

Usage: group_escape.py <escape|grouped> [hold_seconds] [outside_write_path]

With an outside path the child also attempts one write outside its roots and
records whether confinement refused it, so a test can show that leaving the
process group does not leave the sandbox.
"""

from __future__ import annotations

import os
import sys
import time


def main() -> int:
    mode = sys.argv[1] if len(sys.argv) > 1 else "escape"
    hold = float(sys.argv[2]) if len(sys.argv) > 2 else 4.0
    outside = sys.argv[3] if len(sys.argv) > 3 else None

    # The root probes its own confinement first, so a host can see that leaving
    # the group (where that is possible) does not leave the sandbox.
    confinement = "no-probe"
    if outside is not None:
        try:
            with open(outside, "w", encoding="utf-8") as handle:
                handle.write("escaped\n")
            confinement = "write-ok"
        except OSError as error:
            confinement = f"write-denied:{error.errno}"

    try:
        child = os.fork()
    except OSError as error:
        # A profile that denies process creation ends the probe here; there is
        # no descendant at all, which is the point of the restricted mode.
        with open("root.outcome", "w", encoding="utf-8") as handle:
            handle.write(f"{os.getpid()} {confinement} fork-denied:{error.errno}\n")
        os._exit(0)

    if child == 0:
        outcome = "grouped"
        if mode == "escape":
            try:
                os.setsid()
                outcome = "setsid-ok"
            except OSError as error:
                outcome = f"setsid-denied:{error.errno}"
        with open("descendant.outcome", "w", encoding="utf-8") as handle:
            handle.write(f"{os.getpid()} {outcome}\n")
        sys.stdout.write(f"descendant {outcome}\n")
        sys.stdout.flush()
        # Hold the inherited pipe open for a bounded window, then always exit.
        time.sleep(hold)
        os._exit(0)

    with open("root.outcome", "w", encoding="utf-8") as handle:
        handle.write(f"{os.getpid()} {confinement} forked\n")
    os._exit(0)


if __name__ == "__main__":
    raise SystemExit(main())
