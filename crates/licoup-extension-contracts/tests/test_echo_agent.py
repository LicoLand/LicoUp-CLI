"""Execute the offline sample; recorded frames alone do not prove a carrier."""
import json
from pathlib import Path
import subprocess
import sys
import unittest


SAMPLE = Path(__file__).resolve().parents[1] / "samples" / "echo-agent" / "agent.py"


def request(method, params=None, request_id=1):
    return {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}}


def initialize(major=1, bound=65536):
    return request("extension.initialize", {"protocol": {"major": major}, "maxFrameBytes": bound})


def run_session(messages):
    wire = b"".join((json.dumps(message, ensure_ascii=False) + "\n").encode() for message in messages)
    process = subprocess.run([sys.executable, "-B", str(SAMPLE)], input=wire,
                             capture_output=True, timeout=5, check=False)
    lines = process.stdout.splitlines(keepends=True)
    return process, lines, [json.loads(line) for line in lines]


class EchoAgentContractTests(unittest.TestCase):
    def test_plain_and_json_looking_replies_are_verbatim_and_duplicate_is_not_reexecuted(self):
        for text in ["A plain reply with no required marker.", '{"principal":"not authority"', ""]:
            execute = request("agent.execute", {"invocationRef": "synthetic-1", "input": text}, 3)
            process, _, frames = run_session([
                initialize(), request("agent.describe", request_id=2), execute, execute,
                request("extension.shutdown", request_id=4),
            ])
            self.assertEqual(process.returncode, 0)
            self.assertEqual(process.stderr, b"")
            events = [frame["params"] for frame in frames if frame.get("method") == "agent.event"]
            self.assertEqual("".join(event["body"] for event in events if event["kind"] == "text"), text)
            self.assertEqual(sum(event["kind"] == "terminal" for event in events), 1)
            self.assertEqual([event["sequence"] for event in events], list(range(1, len(events) + 1)))
            receipts = [frame["result"]["outcome"] for frame in frames if frame.get("id") == 3]
            self.assertEqual(receipts, ["accepted", "duplicate"])
            description = next(frame["result"] for frame in frames if frame.get("id") == 2)
            self.assertEqual(description["usage"], "unavailable")
            self.assertEqual(description["cancel"], "unsupported")

    def test_incompatible_handshake_refuses_without_crashing_or_starting_work(self):
        process, _, frames = run_session([
            initialize(major=2), request("agent.execute", {"invocationRef": "x", "input": "plain"}, 2),
            initialize(), request("extension.shutdown", request_id=4),
        ])
        self.assertEqual(process.returncode, 0)
        self.assertEqual(process.stderr, b"")
        self.assertEqual(frames[0]["error"]["message"], "incompatible_protocol")
        self.assertEqual(frames[1]["error"]["message"], "not_initialized")
        self.assertFalse(any(frame.get("method") == "agent.event" for frame in frames))

    def test_negotiated_bound_applies_to_utf8_and_escaped_output(self):
        text = "line\n雪" * 240
        process, lines, frames = run_session([
            initialize(bound=4096), request("agent.execute", {"invocationRef": "synthetic-2", "input": text}, 2),
            request("extension.shutdown", request_id=3),
        ])
        self.assertEqual(process.returncode, 0)
        self.assertTrue(all(len(line) <= 4096 for line in lines))
        self.assertEqual(next(frame["result"]["maxFrameBytes"] for frame in frames if frame.get("id") == 1), 4096)
        self.assertEqual("".join(frame["params"]["body"] for frame in frames
                                 if frame.get("method") == "agent.event" and frame["params"]["kind"] == "text"), text)

    def test_oversize_input_is_refused_without_echoing_content(self):
        process, _, frames = run_session([
            initialize(bound=4096), request("agent.execute", {"invocationRef": "x", "input": "x" * 5000}),
        ])
        self.assertEqual(process.returncode, 2)
        self.assertEqual(frames[-1]["error"]["message"], "frame_too_large")
        self.assertFalse(any(frame.get("method") == "agent.event" for frame in frames))

    def test_a_malformed_id_is_refused_without_reflection(self):
        for malformed in [{"nested": "id"}, ["id"], True, float("inf"), float("nan")]:
            process, lines, frames = run_session([
                initialize(),
                request("agent.describe", request_id=malformed),
                request("extension.shutdown", request_id=2),
            ])
            self.assertEqual(process.returncode, 0)
            self.assertTrue(all(len(line) <= 65536 for line in lines))
            refusal = frames[2]
            self.assertEqual(refusal["error"]["code"], -32600, f"id {malformed!r} is not an id")
            self.assertIsNone(refusal["id"], "an invalid id is not echoed")

    def test_an_unanswerable_id_is_refused_inside_the_negotiated_bound(self):
        oversized = "x" * 4000
        process, lines, frames = run_session([
            initialize(bound=4096),
            request("agent.describe", request_id=oversized),
            request("agent.describe", request_id="kept"),
            request("extension.shutdown", request_id=4),
        ])
        self.assertEqual(process.returncode, 0)
        self.assertTrue(all(len(line) <= 4096 for line in lines), "every frame stays inside the bound")
        refusal = frames[2]
        self.assertEqual(refusal["error"]["code"], -32001)
        self.assertEqual(refusal["error"]["message"], "request_id_exceeds_negotiated_frame_bound")
        self.assertIsNone(refusal["id"])
        self.assertFalse(
            any(oversized in line.decode() for line in lines),
            "the over-long id is not reflected at any size",
        )
        self.assertEqual(frames[3]["id"], "kept", "answerable requests keep being served")

    def test_an_unanswerable_id_never_starts_work(self):
        # 3800 characters keep this request frame inside the 4096-byte bound and
        # still leave no room for a response that echoes the id.
        process, _, frames = run_session([
            initialize(bound=4096),
            request("agent.execute", {"invocationRef": "synthetic-3", "input": "must not run"},
                    request_id="y" * 3800),
            request("extension.shutdown", request_id=2),
        ])
        self.assertEqual(process.returncode, 0)
        self.assertFalse(any(frame.get("method") == "agent.event" for frame in frames))
        self.assertEqual(frames[2]["error"]["message"], "request_id_exceeds_negotiated_frame_bound")
        self.assertIsNone(frames[2]["id"])

    def test_a_long_but_answerable_id_is_echoed_inside_the_bound(self):
        identity = "z" * 3000
        process, lines, frames = run_session([
            initialize(bound=4096),
            request("agent.describe", request_id=identity),
            request("extension.shutdown", request_id=2),
        ])
        self.assertEqual(process.returncode, 0)
        self.assertTrue(all(len(line) <= 4096 for line in lines))
        self.assertEqual(frames[2]["id"], identity)
        self.assertEqual(frames[2]["result"]["usage"], "unavailable")

    def test_notifications_and_unsupported_methods_preserve_jsonrpc_semantics(self):
        notification = {"jsonrpc": "2.0", "method": "agent.cancel", "params": {}}
        process, _, frames = run_session([
            initialize(), notification, request("agent.cancel", request_id=2),
            request("extension.shutdown", request_id=3),
        ])
        self.assertEqual(process.returncode, 0)
        self.assertFalse(any("id" in frame and frame["id"] is None for frame in frames))
        self.assertEqual(next(frame["error"]["code"] for frame in frames if frame.get("id") == 2), -32601)


if __name__ == "__main__":
    unittest.main()
