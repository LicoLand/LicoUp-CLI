"""Reference carrier for the LicoUp extension Agent SDK, wire form `extension.v1`.

An extension is an ordinary program. This module is not required to write one:
the contract is one JSON object per line on `stdout` (diagnostics on bounded
`stderr`), and a program in any language that reads and writes those lines is a
complete extension. The carrier exists so an author does not re-implement the
parts that are easy to get subtly wrong — frame bounds, request id validation,
chunking, sequence numbers, admission receipts and terminal events.

What an author writes:

    from licoup_agent_sdk import Agent, Carrier

    class MyAgent(Agent):
        id = "vendor.example.my-agent"
        capabilities = ("vendor.example/stream",)

        def run(self, invocation, emit):
            emit.text("hello " + invocation.text)
            emit.terminal({"outcome": "succeeded"})

    raise SystemExit(Carrier(MyAgent()).serve())

Three rules this carrier enforces on the author's behalf, because a client that
breaks them lies to the user:

- **`agent.execute` reports admission, never completion.** The receipt says the
  work was taken, was already taken, was never started, or was not observed. The
  end of the work is an `agent.event` with kind `terminal`.
- **The event body is verbatim.** Text that looks like JSON, or like a broken
  envelope, is still text. Nothing here parses a reply.
- **A frame never exceeds the negotiated bound.** Oversize text is split; a
  request id that cannot be echoed inside the bound is refused before any work
  starts, so no receipt can be invented at a size the host never agreed to
  carry.

The optional methods are negotiated by telling the truth. A specialist that
cannot cancel says `cancel = "unsupported"` in `agent.describe`, does not
implement `agent.cancel`, and remains a complete Agent: basic execution and
events are never refused because an optional ability is absent.
"""
from __future__ import annotations

import json
import math
import re
import sys
import threading
import time
import traceback
from collections import OrderedDict

#: The wire major this carrier implements.
PROTOCOL_MAJOR = 1

#: The frame bound used when neither side asks for another.
DEFAULT_MAX_FRAME_BYTES = 64 * 1024

#: The accepted range for a negotiated frame bound.
MIN_MAX_FRAME_BYTES = 4 * 1024
MAX_MAX_FRAME_BYTES = 8 * 1024 * 1024

#: The bound on `agent.execute`'s invocation reference, in UTF-8 bytes.
MAX_INVOCATION_REFERENCE_BYTES = 160

#: How many recent invocation references the duplicate check remembers. The
#: check is a safety net against an accidental resubmission, not a session
#: store: a reference older than this window may start again, and a host that
#: resubmits work after a lost receipt must reconcile instead.
MAX_ADMITTED_REFERENCES = 4096

#: The most bytes one diagnostic line may carry.
MAX_DIAGNOSTIC_LINE_BYTES = 8 * 1024

JSONRPC_VERSION = "2.0"

#: The profiles this SDK can carry. `agent-execution` is always first.
AGENT_EXECUTION = "agent-execution"

# The JSON-RPC codes this carrier raises. They are protocol mechanics; the
# product's own refusals are ordinary values in results, not error codes.
_CODE_INVALID_REQUEST = -32600
_CODE_UNSUPPORTED = -32601
_CODE_INVALID_PARAMS = -32602
_CODE_UNANSWERABLE = -32001
_CODE_UNKNOWN_PRIOR = -32004

_NAMESPACED = re.compile(r"^[a-z0-9]+[.-][a-z0-9]+(?:[./-][A-Za-z0-9_-]+)*$")
_SEMVER = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?$")


def is_namespaced(name: str) -> bool:
    """Whether `name` is a namespaced identity, such as `vendor.example/render`."""
    return bool(name) and len(name) <= 160 and bool(_NAMESPACED.match(name))


def is_semver(version: str) -> bool:
    """Whether `version` is a three-part version, optionally with a prerelease."""
    return bool(version) and len(version) <= 64 and bool(_SEMVER.match(version))


class Refusal(Exception):
    """A request this carrier refuses, with the JSON-RPC code to answer it with."""

    code = _CODE_INVALID_PARAMS


class Invalid(Refusal):
    """The request itself was wrong."""

    code = _CODE_INVALID_PARAMS


class InvalidRequest(Refusal):
    """The request object is not valid JSON-RPC."""

    code = _CODE_INVALID_REQUEST


class Incompatible(Refusal):
    """This host cannot be served by this extension at all."""

    code = _CODE_INVALID_REQUEST


class Unanswerable(Refusal):
    """The request cannot be answered inside the negotiated frame bound."""

    code = _CODE_UNANSWERABLE


class Unsupported(Refusal):
    """A method this extension does not implement. Optional ones are expected here."""

    code = _CODE_UNSUPPORTED


class UnknownPrior(Refusal):
    """`agent.observe`/`agent.resume` named work this extension has no record of."""

    code = _CODE_UNKNOWN_PRIOR


def encode(frame: dict) -> str:
    """One frame as it appears on the wire: one JSON object and one newline."""
    return json.dumps(frame, ensure_ascii=False) + "\n"


def frame_bytes(frame: dict) -> int:
    return len(encode(frame).encode("utf-8"))


def log(line: str) -> None:
    """Diagnostics go to stderr, are bounded, and never become protocol."""
    try:
        sys.stderr.write(line[:MAX_DIAGNOSTIC_LINE_BYTES] + "\n")
        sys.stderr.flush()
    except Exception:  # pragma: no cover - stderr must never break the carrier
        pass


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


def event_frame(reference: str, sequence: int, kind: str, body) -> dict:
    """One `agent.event` notification."""
    return {
        "jsonrpc": JSONRPC_VERSION,
        "method": "agent.event",
        "params": {
            "invocationRef": reference,
            "sequence": sequence,
            "kind": kind,
            "body": body,
        },
    }


def usage_frame(observation: dict) -> dict:
    """One `usage.publish` notification, carrying a C11 observation verbatim."""
    return {
        "jsonrpc": JSONRPC_VERSION,
        "method": "usage.publish",
        "params": observation,
    }


def usage_observation(
    *,
    scope_ref: str,
    observation_id: str,
    metrics: dict,
    observed_at: str,
    source_epoch: str,
    revision: int = 1,
    interval_start: str | None = None,
) -> dict:
    """Build a C11 observation for a handler that has one.

    `observed_at` is the producer's own clock reading, passed in rather than
    invented here: a carrier that stamped a time on a handler's behalf would be
    reporting an observation the handler never made. `scope_ref` is the
    host-issued invocation reference; a payload may not assert its own source.
    """
    observation = {
        "schema": "licoup.usage-observation.v1",
        "observationId": observation_id,
        "revision": revision,
        "operation": "upsert",
        "sourceEpoch": source_epoch,
        "scopeRef": scope_ref,
        "observedAt": observed_at,
        "metrics": metrics,
    }
    if interval_start is not None:
        observation["intervalStart"] = interval_start
    return observation


class Invocation:
    """One admitted `agent.execute`, handed to the handler's `run`."""

    def __init__(self, reference: str, text: str) -> None:
        self.reference = reference
        self.text = text
        #: Set by the handler when it observes a cancel; the handler is still
        #: the one that must stop and report a terminal event.
        self.cancelled = threading.Event()
        #: Free-form handler bookkeeping (a child process, a session handle).
        self.state: dict = {}
        self._finished = threading.Event()

    def is_cancelled(self) -> bool:
        return self.cancelled.is_set()

    @property
    def finished(self) -> bool:
        """Whether the carrier has seen this invocation's work end."""
        return self._finished.is_set()

    def wait_finished(self, timeout: float) -> bool:
        """Wait for the work to end, so a cancel can answer the truth."""
        return self._finished.wait(timeout)

    def mark_finished(self) -> None:
        """Called by the carrier when `run` has returned, terminal or not."""
        self._finished.set()


class Emitter:
    """The only way a handler writes events for one invocation."""

    # Worst-case JSON escape cost per character: `\uXXXX` is six bytes.
    _ESCAPE_COST = {ord('"'): 2, ord("\\"): 2}
    _CHUNK_RESERVE_BYTES = 512

    def __init__(self, carrier: "Carrier", reference: str, bound: int) -> None:
        self._carrier = carrier
        self.reference = reference
        self._bound = bound
        self._sequence = 0
        self._terminal = False
        self._lock = carrier.write_lock

    @property
    def sequence(self) -> int:
        return self._sequence

    @property
    def max_frame_bytes(self) -> int:
        """The negotiated frame bound this invocation's events must fit."""
        return self._bound

    def _cost(self, character: str) -> int:
        code = ord(character)
        if code in self._ESCAPE_COST:
            return self._ESCAPE_COST[code]
        if code < 0x20 or code > 0x7E:
            return 6
        return 1

    def _chunks(self, text: str) -> list[str]:
        """Split text so every emitted event frame stays inside the bound."""
        if not text:
            return [""]
        overhead = frame_bytes(
            event_frame(self.reference, self._sequence + 1, "text", "")
        ) + self._CHUNK_RESERVE_BYTES
        budget = self._bound - overhead
        if budget < 1:
            # The bound cannot carry an empty event: nothing can be said inside it.
            raise Incompatible("frame_bound_below_minimum")
        chunks: list[str] = []
        current: list[str] = []
        cost = 0
        for character in text:
            character_cost = self._cost(character)
            if current and cost + character_cost > budget:
                chunks.append("".join(current))
                current = []
                cost = 0
            current.append(character)
            cost += character_cost
        chunks.append("".join(current))
        return chunks

    def _emit(self, kind: str, body) -> None:
        with self._lock:
            if self._terminal:
                log("event_after_terminal_ignored")
                return
            sequence = self._sequence + 1
            self._sequence = sequence
            if kind == "terminal":
                self._terminal = True
        self._carrier.write(event_frame(self.reference, sequence, kind, body))

    def text(self, text: str) -> None:
        """Ordinary output. The body is carried verbatim, in bounds."""
        for chunk in self._chunks(text):
            self._emit("text", chunk)

    def artifact(self, body) -> None:
        self._emit("artifact", body)

    def progress(self, body) -> None:
        self._emit("progress", body)

    def state(self, body) -> None:
        self._emit("state", body)

    def terminal(self, body) -> None:
        """The end of the work. The only event that reports one."""
        self._emit("terminal", body)

    def usage(self, observation: dict) -> None:
        """Publish one C11 observation for this invocation's effect."""
        self._carrier.write(usage_frame(observation))

    @property
    def is_terminal(self) -> bool:
        return self._terminal


class Agent:
    """What a third-party adapter author subclasses.

    Only `run` must be implemented. Every optional ability is declared by
    setting the matching attribute and implementing the matching hook; the
    defaults are the honest absences, and they are complete answers.
    """

    #: The namespaced Agent identity.
    id = ""
    #: What kind of instance this is, in the extension's own vocabulary.
    instance_kind = "executable"
    #: Input kinds the Agent accepts.
    input_kinds = ("text",)
    #: Namespaced capabilities the Agent offers.
    capabilities: tuple = ()
    #: The interface version this Agent implements.
    interface_version = "1.0.0"
    #: `reported` or `unavailable`. Unavailable is a complete answer.
    usage = "unavailable"
    #: `supported` or `unsupported`.
    cancel = "unsupported"
    #: `supported` or `unsupported`.
    resume = "unsupported"
    #: The profiles this process serves.
    profiles = (AGENT_EXECUTION,)

    def describe(self) -> dict:
        """Answer `agent.describe`, from the class attributes and nothing paid."""
        if not is_namespaced(self.id):
            raise Invalid("agent_description_invalid: id")
        if not is_semver(self.interface_version):
            raise Invalid("agent_description_invalid: interfaceVersion")
        if not self.instance_kind:
            raise Invalid("agent_description_invalid: instanceKind")
        for capability in self.capabilities:
            if not is_namespaced(capability):
                raise Invalid("agent_description_invalid: capabilities")
        if self.usage not in ("reported", "unavailable"):
            raise Invalid("agent_description_invalid: usage")
        if self.cancel not in ("supported", "unsupported"):
            raise Invalid("agent_description_invalid: cancel")
        if self.resume not in ("supported", "unsupported"):
            raise Invalid("agent_description_invalid: resume")
        if self.cancel == "supported" and type(self).cancel_request is Agent.cancel_request:
            log("agent_declares_cancel_without_a_handler")
        if self.resume == "supported" and type(self).replay is Agent.replay:
            log("agent_declares_resume_without_a_handler")
        return {
            "id": self.id,
            "instanceKind": self.instance_kind,
            "inputKinds": list(self.input_kinds),
            "capabilities": list(self.capabilities),
            "interfaceVersion": self.interface_version,
            "usage": self.usage,
            "cancel": self.cancel,
            "resume": self.resume,
        }

    def run(self, invocation: Invocation, emit: Emitter) -> None:
        """Execute one admitted invocation.

        Must call `emit.terminal(...)` exactly once. A handler that returns
        without one is reported as an unknown outcome rather than silence.
        """
        raise NotImplementedError

    # --- optional abilities -------------------------------------------------
    # A handler implements a hook and sets the matching attribute. None of
    # these are required, and an unimplemented one is refused only for its own
    # method.

    def cancel_request(self, invocation: Invocation | None) -> str:
        """Answer `agent.cancel`. Return `acknowledged`, `requested`,
        `unsupported` or `unknown`; only `acknowledged` claims work stopped."""
        return "unknown"

    def replay(self, prior_ref: str, cursor: str) -> tuple[dict, list[dict]]:
        """Answer `agent.observe`/`agent.resume` with a receipt and replayed events."""
        raise Unsupported("unsupported_method")

    def history(self, cursor: str, limit: int) -> dict:
        raise Unsupported("unsupported_method")

    def models(self) -> dict:
        raise Unsupported("unsupported_method")

    def reconcile(self, invocation_ref: str) -> dict:
        raise Unsupported("unsupported_method")

    def shutdown(self) -> None:
        """Called once when `extension.shutdown` is answered and before exit."""
        return None


class Carrier:
    """Frames one Agent over stdin/stdout and answers the handshake."""

    def __init__(
        self,
        agent: Agent,
        *,
        max_frame_bytes: int = DEFAULT_MAX_FRAME_BYTES,
        argv: list[str] | None = None,
    ) -> None:
        self.agent = agent
        self._own_max = self._checked_own_bound(max_frame_bytes)
        self._bound = self._own_max
        self._initialized = False
        self._stopping = threading.Event()
        self._admitted: OrderedDict[str, None] = OrderedDict()
        self._invocations: dict[str, Invocation] = {}
        self._emitters: dict[str, Emitter] = {}
        self._workers: dict[str, threading.Thread] = {}
        self._state_lock = threading.Lock()
        #: Held while a frame is written, so events from worker threads and
        #: responses from the read loop cannot interleave inside one frame.
        self.write_lock = threading.RLock()
        self.argv = list(argv if argv is not None else sys.argv[1:])

    @staticmethod
    def _checked_own_bound(bound: int) -> int:
        if not isinstance(bound, int) or isinstance(bound, bool):
            raise ValueError("max_frame_bytes must be an int")
        return min(max(bound, MIN_MAX_FRAME_BYTES), MAX_MAX_FRAME_BYTES)

    # --- wire output --------------------------------------------------------

    def write(self, frame: dict) -> None:
        wire = encode(frame)
        if len(wire.encode("utf-8")) > self._bound:
            # Reflecting a frame larger than the bound is exactly what the
            # bound exists to prevent; say so inside the bound instead.
            log("transport_frame_oversize")
            wire = encode(
                {
                    "jsonrpc": JSONRPC_VERSION,
                    "id": None,
                    "error": {
                        "code": _CODE_UNANSWERABLE,
                        "message": "transport_frame_oversize",
                        "data": {"maxFrameBytes": self._bound},
                    },
                }
            )
            if len(wire.encode("utf-8")) > self._bound:
                log("frame_bound_below_minimum")
                raise SystemExit(2)
        with self.write_lock:
            sys.stdout.write(wire)
            sys.stdout.flush()

    def _response(self, request_id, result: dict) -> dict:
        return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": result}

    def _error(self, request_id, code: int, message: str) -> dict:
        return {
            "jsonrpc": JSONRPC_VERSION,
            "id": request_id,
            "error": {"code": code, "message": message},
        }

    # --- lifecycle ----------------------------------------------------------

    def _initialize(self, params: dict) -> dict:
        if self._initialized:
            raise Invalid("already_initialized")
        protocol = params.get("protocol") or {}
        if not isinstance(protocol, dict) or protocol.get("major") != PROTOCOL_MAJOR:
            raise Incompatible("incompatible_protocol")
        bound = params.get("maxFrameBytes", self._own_max)
        if type(bound) is not int or not MIN_MAX_FRAME_BYTES <= bound <= MAX_MAX_FRAME_BYTES:
            raise Invalid("invalid_frame_bound")
        self._bound = min(bound, self._own_max)
        self._initialized = True
        profiles = list(self.agent.profiles)
        # The extension states what it is prepared to serve, before the
        # response, exactly as the accepted baseline sample does.
        self.write(
            {
                "jsonrpc": JSONRPC_VERSION,
                "method": "extension.ready",
                "params": {"profiles": profiles},
            }
        )
        return {
            "protocol": {"major": PROTOCOL_MAJOR, "minimumMinor": 0},
            "maxFrameBytes": self._bound,
            "profiles": profiles,
        }

    # --- dispatch -----------------------------------------------------------

    def _handle(self, request: dict, notification: bool):
        """Answer one request.

        Returns `(response, stop, post_action)`. The post action runs after the
        response is written, so a receipt always precedes the events of the work
        it admitted. `notification` is true when the frame carried no id: work
        whose receipt could never be delivered is not admitted.
        """
        request_id = request.get("id")
        method = request.get("method")
        params = request.get("params") or {}
        if not isinstance(params, dict):
            raise Invalid("invalid_params")

        if method == "extension.initialize":
            return self._response(request_id, self._initialize(params)), False, None
        if not self._initialized:
            # A call before the handshake is not a business call.
            raise Invalid("not_initialized")
        if method == "agent.describe":
            return self._response(request_id, self.agent.describe()), False, None
        if method == "agent.execute":
            return self._execute(request_id, params, notification)
        if method == "agent.cancel":
            return self._cancel(request_id, params)
        if method in ("agent.observe", "agent.resume"):
            return self._replay(request_id, params)
        if method == "agent.history":
            return self._history(request_id, params)
        if method == "agent.models":
            return self._response(request_id, self.agent.models()), False, None
        if method == "agent.reconcile":
            reference = params.get("invocationRef")
            if not isinstance(reference, str) or not reference:
                raise Invalid("invalid_invocation_ref")
            return (
                self._response(request_id, self.agent.reconcile(reference)),
                False,
                None,
            )
        if method == "agent.steer" or method == "agent.fork":
            raise Unsupported("unsupported_method")
        if method == "extension.shutdown":
            # Ordered exit: stop taking work, let admitted work reach its
            # terminal event inside the grace window, and only then answer that
            # the extension has stopped. Answering first would tell the host
            # work had stopped while it was still running.
            self._drain_workers()
            self._stopping.set()
            return self._response(request_id, {"outcome": "stopped"}), True, None
        raise Unsupported("unsupported_method")

    def _validated_reference(self, params: dict) -> str:
        reference = params.get("invocationRef")
        if (
            not isinstance(reference, str)
            or not reference
            or len(reference.encode("utf-8")) > MAX_INVOCATION_REFERENCE_BYTES
        ):
            raise Invalid("invalid_invocation_ref")
        return reference

    def _execute(self, request_id, params: dict, notification: bool):
        reference = self._validated_reference(params)
        text = params.get("input")
        if not isinstance(text, str):
            raise Invalid("invalid_input")
        if notification:
            # An execute with no id has no receipt to report admission with.
            # Admitting work the host could never see accepted is the failure
            # the admission rule exists to prevent.
            log("agent_execute_notification_not_admitted")
            return None, False, None
        with self._state_lock:
            duplicate = reference in self._admitted
            if not duplicate:
                self._admitted[reference] = None
                while len(self._admitted) > MAX_ADMITTED_REFERENCES:
                    self._admitted.popitem(last=False)
        if duplicate:
            # Nothing is started twice, and no event is replayed as new work.
            return (
                self._response(
                    request_id, {"invocationRef": reference, "outcome": "duplicate"}
                ),
                False,
                None,
            )

        def start() -> None:
            self._start_worker(reference, text)

        return (
            self._response(
                request_id, {"invocationRef": reference, "outcome": "accepted"}
            ),
            False,
            start,
        )

    def _start_worker(self, reference: str, text: str) -> None:
        invocation = Invocation(reference, text)
        emitter = Emitter(self, reference, self._bound)
        with self._state_lock:
            self._invocations[reference] = invocation
            self._emitters[reference] = emitter

        def work() -> None:
            try:
                self.agent.run(invocation, emitter)
            except SystemExit:
                raise
            except Unsupported as refusal:
                log(f"agent_run_unsupported {refusal}")
                emitter.terminal({"outcome": "failed", "reason": "unsupported_method"})
            except Refusal as refusal:
                log(f"agent_run_refused {refusal}")
                emitter.terminal({"outcome": "failed", "reason": "invalid_request"})
            except Exception:
                log("agent_run_error\n" + traceback.format_exc())
                emitter.terminal({"outcome": "failed", "reason": "agent_error"})
            finally:
                if not emitter.is_terminal:
                    # The handler returned without ending the work. Silence
                    # would read as success; an unknown outcome is the honest
                    # report.
                    log("agent_returned_without_terminal")
                    emitter.terminal({"outcome": "unknown"})
                invocation.mark_finished()
                self._forget_invocation(reference, invocation)

        thread = threading.Thread(
            target=work, name=f"licoup-invocation-{reference}", daemon=True
        )
        with self._state_lock:
            self._workers[reference] = thread
        thread.start()

    def _forget_invocation(self, reference: str, invocation: Invocation) -> None:
        """Release a finished invocation's bookkeeping.

        The worker has ended, so no cancel can need it any more; keeping every
        invocation would make a long session grow without bound.
        """
        with self._state_lock:
            if self._invocations.get(reference) is invocation:
                self._invocations.pop(reference, None)
                self._emitters.pop(reference, None)
                self._workers.pop(reference, None)

    def _cancel(self, request_id, params: dict):
        if self.agent.cancel != "supported":
            raise Unsupported("unsupported_method")
        reference = self._validated_reference(params)
        with self._state_lock:
            invocation = self._invocations.get(reference)
        outcome = self.agent.cancel_request(invocation)
        allowed = {"requested", "acknowledged", "unsupported", "unknown"}
        if outcome not in allowed:
            raise Invalid("invalid_cancel_outcome")
        return (
            self._response(
                request_id, {"invocationRef": reference, "outcome": outcome}
            ),
            False,
            None,
        )

    def _replay(self, request_id, params: dict):
        if self.agent.resume != "supported":
            raise Unsupported("unsupported_method")
        prior = params.get("priorInvocationRef")
        if (
            not isinstance(prior, str)
            or not prior
            or len(prior.encode("utf-8")) > MAX_INVOCATION_REFERENCE_BYTES
        ):
            raise Invalid("agent_resume_requires_prior_invocation")
        cursor = params.get("cursor") or ""
        if not isinstance(cursor, str):
            raise Invalid("invalid_cursor")
        receipt, events = self.agent.replay(prior, cursor)
        reference = receipt.get("priorInvocationRef", prior)

        def emit_replayed() -> None:
            # Replayed events are notifications of the recorded stream, emitted
            # after the receipt. They are not new work.
            with self.write_lock:
                for sequence, kind, body in events:
                    self.write(event_frame(reference, sequence, kind, body))

        return self._response(request_id, receipt), False, emit_replayed

    def _history(self, request_id, params: dict):
        cursor = params.get("cursor") or ""
        limit = params.get("limit", 20)
        if not isinstance(cursor, str) or type(limit) is not int or limit < 1:
            raise Invalid("invalid_history_request")
        return self._response(request_id, self.agent.history(cursor, limit)), False, None

    # --- serve --------------------------------------------------------------

    def serve(self) -> int:
        """Read frames until shutdown or EOF. Returns the process exit code."""
        try:
            while not self._stopping.is_set():
                line = sys.stdin.buffer.readline(self._bound + 1)
                if not line:
                    break
                if len(line) > self._bound:
                    # A frame larger than the negotiated bound is a framing
                    # fault, never an unbounded buffer.
                    log("frame_too_large")
                    self.write(
                        self._error(None, _CODE_INVALID_REQUEST, "frame_too_large")
                    )
                    self._drain_workers()
                    return 2
                if not self._handle_line(line):
                    break
        except SystemExit as exit_request:
            self._drain_workers()
            return int(exit_request.code or 0)
        self._drain_workers()
        return 0

    def _handle_line(self, line: bytes) -> bool:
        """Handle one wire line. Returns False when the process should stop."""
        response = None
        response_id = None
        notification = True
        post_action = None
        stop = False
        try:
            request = json.loads(line)
            if not isinstance(request, dict) or request.get("jsonrpc") != JSONRPC_VERSION:
                raise InvalidRequest("invalid_request")
            notification = "id" not in request
            if not notification:
                candidate = request.get("id")
                if not request_id_is_valid(candidate):
                    raise InvalidRequest("invalid_request")
                probe = {"jsonrpc": JSONRPC_VERSION, "id": candidate, "result": {}}
                if frame_bytes(probe) + Emitter._CHUNK_RESERVE_BYTES > self._bound:
                    # The id cannot be echoed inside the bound, so no response to
                    # this request exists; refusing now keeps an unanswerable
                    # request from starting work whose receipt could never be
                    # delivered.
                    raise Unanswerable("request_id_exceeds_negotiated_frame_bound")
                response_id = candidate
            response, stop, post_action = self._handle(request, notification)
        except Refusal as refusal:
            response = self._error(response_id, refusal.code, str(refusal))
        except (ValueError, TypeError, AttributeError):
            response = self._error(response_id, _CODE_INVALID_PARAMS, "invalid_request")
        if response is not None and not notification:
            self.write(response)
        if post_action is not None:
            post_action()
        return not stop

    def _drain_workers(self, grace: float = 2.0) -> None:
        """Ask the handler to wind down, then wait for admitted work to end.

        The wait is one shared deadline, not one per worker, so a chatty
        extension cannot hold the process open past the grace window. A worker
        that outlives it is reported on `stderr`; its invocation simply has no
        terminal event, which the host records as an unknown outcome.
        """
        self.agent.shutdown()
        deadline = time.monotonic() + grace
        with self._state_lock:
            workers = list(self._workers.items())
        for reference, thread in workers:
            thread.join(timeout=max(0.0, deadline - time.monotonic()))
            if thread.is_alive():
                log(f"worker_still_running {reference}")
