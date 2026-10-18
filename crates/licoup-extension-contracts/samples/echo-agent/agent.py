#!/usr/bin/env python3
"""A minimal Agent for the LicoUp extension protocol.

This is a contract sample, not a product adapter. It opens no socket, reads no
file, calls no model and keeps no state beyond the session it is in, so running
it proves the protocol carrier and nothing else.

It is deliberately not written in Rust: the point of the contract is that an
Agent is an ordinary program in any language, framed as one JSON-RPC 2.0 message
per line. It implements the three methods a specialist Agent is required to
implement — `agent.describe`, `agent.execute`, `agent.event` — plus the
handshake. It implements none of the optional ones, and that is a complete
extension, not a degraded one.

The adjacent wire_fixture.json contains authored synthetic wire vectors, not
runtime logs. Actual subprocess checks live in tests/test_echo_agent.py.
"""
from __future__ import annotations

import json
import math
import sys

PROTOCOL_MAJOR = 1
MAX_FRAME_BYTES = 64 * 1024
AGENT_ID = "org.licoland.example.echo"

# Every method this Agent implements. The optional ones are absent on purpose.
METHODS = (
    "extension.initialize",
    "extension.ready",
    "extension.shutdown",
    "agent.describe",
    "agent.execute",
    "agent.event",
)


# The room a response envelope must keep for its result: the id echo plus the
# largest result any method here produces. A request whose id cannot leave that
# room is refused before anything runs, because a response that cannot fit the
# negotiated bound must not be invented at a size the host never agreed to carry.
RESPONSE_RESERVE_BYTES = 512


def encode(frame: dict) -> str:
    """One frame as it appears on the wire: one JSON object and one newline."""
    return json.dumps(frame, ensure_ascii=False) + "\n"


def emit(frame: dict, max_bytes: int = MAX_FRAME_BYTES) -> None:
    """Write one frame inside the negotiated bound.

    stdout carries the protocol and nothing else, and it never carries a frame
    larger than both sides negotiated. A frame that does not fit — an over-long
    request id about to be echoed, or a result larger than the bound — is
    replaced by a bounded refusal that names the bound and carries neither the
    request identity nor its content, because reflecting either at a size the
    host never agreed to carry is the failure this bound exists to prevent.
    """
    wire = encode(frame)
    if len(wire.encode("utf-8")) > max_bytes:
        log("transport_frame_oversize")
        wire = encode(
            {
                "jsonrpc": "2.0",
                "id": None,
                "error": {
                    "code": -32001,
                    "message": "transport_frame_oversize",
                    "data": {"maxFrameBytes": max_bytes},
                },
            }
        )
        if len(wire.encode("utf-8")) > max_bytes:
            # The bound is smaller than a refusal: nothing can be said inside it.
            log("frame_bound_below_minimum")
            raise SystemExit(2)
    sys.stdout.write(wire)
    sys.stdout.flush()


def request_id_is_valid(value) -> bool:
    """Whether a value may be a JSON-RPC id: a string, a number, or null."""
    if value is None or isinstance(value, str):
        return True
    if isinstance(value, bool):
        return False
    if isinstance(value, int):
        return True
    # A non-finite float has no JSON spelling: echoing it would put `Infinity`
    # or `NaN` on the wire, which no reader can parse.
    return isinstance(value, float) and math.isfinite(value)


def request_id_is_answerable(value, max_bytes: int) -> bool:
    """Whether a response can echo this id and still fit the negotiated bound."""
    if not request_id_is_valid(value):
        return False
    probe = {"jsonrpc": "2.0", "id": value, "result": {}}
    return len(encode(probe).encode("utf-8")) + RESPONSE_RESERVE_BYTES <= max_bytes


def log(line: str) -> None:
    """Diagnostics go to stderr and never become protocol."""
    sys.stderr.write(line[:4096] + "\n")


class Invalid(Exception):
    """The request itself was wrong."""

    code = -32602


class Incompatible(Invalid):
    """This host cannot be served by this extension at all."""

    code = -32600


class InvalidRequest(Invalid):
    """The request object is not valid JSON-RPC."""

    code = -32600


class Unanswerable(Invalid):
    """The request cannot be answered inside the negotiated frame bound."""

    code = -32001


class Unsupported(Invalid):
    """A method this Agent does not implement. Optional ones are expected here."""

    code = -32601


class Session:
    """One extension session: initialized, then admitted invocations."""

    def __init__(self) -> None:
        self.initialized = False
        self.admitted: set[str] = set()
        self.max_frame_bytes = MAX_FRAME_BYTES

    def initialize(self, params: dict) -> dict:
        if self.initialized:
            raise Invalid("already_initialized")
        protocol = params.get("protocol") or {}
        if protocol.get("major") != PROTOCOL_MAJOR:
            raise Incompatible("incompatible_protocol")
        bound = params.get("maxFrameBytes", MAX_FRAME_BYTES)
        if type(bound) is not int or not 4096 <= bound <= 8 * 1024 * 1024:
            raise Invalid("invalid_frame_bound")
        self.max_frame_bytes = min(bound, MAX_FRAME_BYTES)
        self.initialized = True
        # The extension states what it is prepared to serve and nothing more.
        emit(
            {
                "jsonrpc": "2.0",
                "method": "extension.ready",
                "params": {"profiles": ["agent-execution"]},
            },
            self.max_frame_bytes,
        )
        return {
            "protocol": {"major": PROTOCOL_MAJOR, "minimumMinor": 0},
            "maxFrameBytes": self.max_frame_bytes,
            "profiles": ["agent-execution"],
        }

    def describe(self) -> dict:
        # Answered from the manifest: no inference, no credential, no cost.
        return {
            "id": AGENT_ID,
            "instanceKind": "executable",
            "inputKinds": ["text"],
            "capabilities": ["org.licoland.example/stream"],
            "interfaceVersion": "1.0.0",
            "usage": "unavailable",
            "cancel": "unsupported",
            "resume": "unsupported",
        }

    def execute(self, params: dict) -> dict:
        reference = params.get("invocationRef")
        text = params.get("input")
        if not isinstance(reference, str) or not reference or len(reference.encode("utf-8")) > 160:
            raise Invalid("invalid_invocation_ref")
        if not isinstance(text, str):
            raise Invalid("invalid_input")
        duplicate = reference in self.admitted
        self.admitted.add(reference)
        if duplicate:
            # Nothing is started twice, and no event is replayed as new work.
            return {"invocationRef": reference, "outcome": "duplicate"}
        # The receipt reports admission. The end of the work is an event.
        # JSON escaping takes at most six bytes per character. Reserve room for
        # the envelope and the bounded invocation reference before slicing, so
        # every emitted event stays inside the negotiated bound.
        chunk_chars = max(1, (self.max_frame_bytes - 2048) // 6)
        chunks = [text[start:start + chunk_chars] for start in range(0, len(text), chunk_chars)] or [""]
        for sequence, chunk in enumerate(chunks, 1):
            emit(event(reference, sequence, "text", chunk), self.max_frame_bytes)
        emit(
            event(reference, len(chunks) + 1, "terminal", {"outcome": "succeeded"}),
            self.max_frame_bytes,
        )
        return {"invocationRef": reference, "outcome": "accepted"}


def event(reference: str, sequence: int, kind: str, body) -> dict:
    return {
        "jsonrpc": "2.0",
        "method": "agent.event",
        "params": {
            "invocationRef": reference,
            "sequence": sequence,
            "kind": kind,
            "body": body,
        },
    }


def handle(session: Session, request: dict) -> tuple[dict | None, bool]:
    """Answer one request. Returns the response, if any, and whether to stop."""
    request_id = request.get("id")
    method = request.get("method")
    params = request.get("params") or {}
    if not isinstance(params, dict):
        raise Invalid("invalid_params")
    if method == "extension.initialize":
        result = session.initialize(params)
    elif not session.initialized:
        # A call before the handshake is not a business call.
        raise Invalid("not_initialized")
    elif method == "agent.describe":
        result = session.describe()
    elif method == "agent.execute":
        result = session.execute(params)
    elif method == "extension.shutdown":
        return {"jsonrpc": "2.0", "id": request_id, "result": {"outcome": "stopped"}}, True
    else:
        raise Unsupported("unsupported_method")
    return {"jsonrpc": "2.0", "id": request_id, "result": result}, False


def main() -> int:
    session = Session()
    while True:
        # Bounded read: a frame larger than the negotiated bound is a framing
        # fault, never an unbounded buffer.
        line = sys.stdin.buffer.readline(session.max_frame_bytes + 1)
        if not line:
            return 0
        if len(line) > session.max_frame_bytes:
            log("frame_too_large")
            emit(
                {
                    "jsonrpc": "2.0",
                    "id": None,
                    "error": {"code": -32600, "message": "frame_too_large"},
                },
                session.max_frame_bytes,
            )
            return 2
        response_id = None
        notification = False
        try:
            request = json.loads(line)
            if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
                raise InvalidRequest("invalid_request")
            notification = "id" not in request
            if not notification:
                candidate = request.get("id")
                if not request_id_is_valid(candidate):
                    raise InvalidRequest("invalid_request")
                if not request_id_is_answerable(candidate, session.max_frame_bytes):
                    # The id cannot be echoed inside the bound, so no response to
                    # this request exists; refusing now keeps an unanswerable
                    # request from starting work whose receipt could never be
                    # delivered.
                    raise Unanswerable("request_id_exceeds_negotiated_frame_bound")
                response_id = candidate
            response, stop = handle(session, request)
        except Invalid as refusal:
            response, stop = (
                {
                    "jsonrpc": "2.0",
                    "id": response_id,
                    "error": {"code": refusal.code, "message": str(refusal)},
                },
                False,
            )
        except (ValueError, TypeError, AttributeError):
            response, stop = (
                {
                    "jsonrpc": "2.0",
                    "id": response_id,
                    "error": {"code": -32602, "message": "invalid_request"},
                },
                False,
            )
        if response is not None and not notification:
            emit(response, session.max_frame_bytes)
        if stop:
            return 0


if __name__ == "__main__":
    raise SystemExit(main())
