#!/usr/bin/env python3
"""Test-only SDK agent: an escaped child closes all stdio and keeps writing.

Fork happens before SDK worker threads exist. Each write and cleanup command is
bound to a per-run nonce in a unique synthetic cwd. No PID is ever signalled by
the test. Explicit fixture cleanup and a bounded fallback are NOT host release.
"""
import errno
import json
import os
from pathlib import Path
import signal
import sys
import time

sdk, route, nonce = sys.argv[1:4]
root = Path.cwd()


def record(name, value):
    temporary = root / (name + ".tmp")
    temporary.write_text(json.dumps(value), encoding="utf-8")
    temporary.replace(root / name)


try:
    child = os.fork()
except OSError as error:
    if error.errno != errno.EPERM:
        raise
    record("writer-attempt.json", {"nonce": nonce, "route": route, "outcome": "fork-denied", "errno": error.errno})
else:
    if child == 0:
        # Independent hard fallback: even an assertion failure cannot leave an
        # indefinitely live adversarial process. Normal cleanup uses a nonce.
        signal.alarm(18)
        if route == "setsid":
            os.setsid()
        elif route == "setpgid":
            os.setpgid(0, 0)
        else:
            os._exit(64)
        for fd in (0, 1, 2):
            os.close(fd)
        identity = {"nonce": nonce, "route": route, "pid": os.getpid(), "pgid": os.getpgrp(), "sid": os.getsid(0)}
        record("writer-identity.json", identity)
        end = time.monotonic() + 14
        reason = "fixture-fallback"
        sequence = 0
        while time.monotonic() < end:
            stop = root / "fixture-stop"
            if stop.exists() and stop.read_text(encoding="utf-8") == nonce:
                reason = "explicit-fixture-cleanup"
                break
            sequence += 1
            with (root / "writer-heartbeat").open("a", encoding="utf-8") as output:
                output.write(f"{nonce} {sequence}\n")
            time.sleep(0.05)
        record("writer-stopped.json", {**identity, "reason": reason, "writes": sequence})
        os._exit(0)
    record("writer-attempt.json", {"nonce": nonce, "route": route, "outcome": "forked", "pid": child})

sys.path.insert(0, sdk)
from licoup_agent_sdk import Agent, Carrier


class SyntheticWriterAgent(Agent):
    id = "dev.example.agent.silent-writer"
    capabilities = ("dev.example.agent/silent-writer",)

    def run(self, invocation, emit):
        emit.terminal({"outcome": "succeeded"})


raise SystemExit(Carrier(SyntheticWriterAgent()).serve())
