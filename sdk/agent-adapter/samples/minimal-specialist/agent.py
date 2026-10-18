#!/usr/bin/env python3
"""The smallest complete Agent: the handshake, describe, execute and events.

It streams its input back verbatim, calls no model and keeps no session. It
implements none of the optional methods, and that is a complete extension, not
a degraded one: `agent.describe` truthfully reports `usage: unavailable`,
`cancel: unsupported` and `resume: unsupported`, and basic execution and events
are served exactly as usual.

Replay the recorded session with:

    python3 -B replay.py transcript.json | python3 -B agent.py

`transcript.json` holds the host frames of one exchange and `events.json` the
answer frames, both as recorded frame arrays that `replay.py` renders back to
the one-frame-per-line carrier format.
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "python"))

from licoup_agent_sdk import Agent, Carrier


class MinimalSpecialist(Agent):
    """A specialist with no optional abilities and nothing to negotiate away."""

    id = "dev.example.agent.sdk.minimal"
    capabilities = ("dev.example.agent/stream",)

    def run(self, invocation, emit):
        if invocation.text:
            # Plain text. Text that looks like JSON is still text, and a reply
            # is never invalid because it has no format.
            emit.text(invocation.text)
        emit.terminal({"outcome": "succeeded"})


if __name__ == "__main__":
    raise SystemExit(Carrier(MinimalSpecialist()).serve())
