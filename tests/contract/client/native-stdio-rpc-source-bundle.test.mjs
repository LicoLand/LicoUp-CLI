import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const facadePath =
  "apps/desktop/lib/src/platform/native_client/agent_service_stdio_rpc.dart";
const sourceRoot =
  "apps/desktop/lib/src/platform/native_client/agent_service_stdio_rpc";

const leafNames = Object.freeze([
  "client.dart",
  "command_exchange.dart",
  "command_round_trip.dart",
  "conversation_exchange.dart",
  "in_flight_control.dart",
  "line_framer.dart",
  "method_policy.dart",
  "operation_pending_queue.dart",
  "operation_queue.dart",
  "protocol.dart",
  "read_policy.dart",
  "read_pool.dart",
  "request_writer.dart",
  "response_codec.dart",
  "session.dart",
  "session_expectation.dart",
  "session_manager.dart",
  "shutdown.dart",
  "stream_observation.dart",
]);

const allowedDependencies = Object.freeze({
  "client.dart": [
    "command_exchange.dart",
    "conversation_exchange.dart",
    "in_flight_control.dart",
    "method_policy.dart",
    "operation_queue.dart",
    "protocol.dart",
    "read_policy.dart",
    "read_pool.dart",
    "session_manager.dart",
    "shutdown.dart",
    "stream_observation.dart",
  ],
  "command_exchange.dart": [
    "command_round_trip.dart",
    "session_manager.dart",
  ],
  "command_round_trip.dart": [
    "request_writer.dart",
    "response_codec.dart",
    "session.dart",
    "session_manager.dart",
  ],
  "conversation_exchange.dart": [
    "request_writer.dart",
    "response_codec.dart",
    "session.dart",
    "session_manager.dart",
  ],
  "in_flight_control.dart": ["command_exchange.dart", "session_manager.dart"],
  "line_framer.dart": [],
  "method_policy.dart": [],
  "operation_pending_queue.dart": [],
  "operation_queue.dart": ["operation_pending_queue.dart"],
  "protocol.dart": [],
  "read_policy.dart": [],
  "read_pool.dart": [
    "operation_pending_queue.dart",
    "session_manager.dart",
    "stream_observation.dart",
  ],
  "request_writer.dart": ["session.dart"],
  "response_codec.dart": ["protocol.dart"],
  "session.dart": [
    "line_framer.dart",
    "protocol.dart",
    "response_codec.dart",
    "session_expectation.dart",
    "stream_observation.dart",
  ],
  "session_expectation.dart": ["response_codec.dart"],
  "session_manager.dart": [
    "protocol.dart",
    "session.dart",
    "stream_observation.dart",
  ],
  "shutdown.dart": [
    "protocol.dart",
    "request_writer.dart",
    "response_codec.dart",
    "session.dart",
    "session_manager.dart",
  ],
  "stream_observation.dart": [],
});

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(leafNames.map(async (leaf) => [
    leaf,
    await read(`${sourceRoot}/${leaf}`),
  ])));
}

function localDependencies(source) {
  return [...source.matchAll(
    /agent_service_stdio_rpc\/([a-z_]+\.dart)'/gu,
  )].map((match) => match[1]).sort();
}

test("stdio RPC facade exports one stable client from ordinary libraries", async () => {
  const facade = await read(facadePath);
  assert.ok(facade.includes("show NativeStdioRpcClient"));
  for (const forbidden of ["part ", "part of", "class NativeStdioRpcClient", "#[path"])
    assert.equal(facade.includes(forbidden), false);
});

test("stdio RPC leaves remain ordinary and acyclic", async () => {
  const source = await sources();
  for (const [leaf, body] of Object.entries(source)) {
    assert.equal(body.includes("part "), false);
    assert.equal(body.includes("part of"), false);
    assert.equal(body.includes("/agent_service_stdio_rpc.dart"), false);
    assert.deepEqual(localDependencies(body), allowedDependencies[leaf]);
  }
});

test("stdio RPC protocol and response codecs bind bounded identities", async () => {
  const source = await sources();
  const protocol = source["protocol.dart"];
  const response = source["response_codec.dart"];
  for (const token of [
    "stdioRpcMaxFrameBytes",
    "stdioRpcMaxErrorCodeBytes",
    "stdioRpcMaxArgs",
    "stdioRpcMaxArgumentCodeUnits",
    "validStdioRpcErrorCode",
    "validStdioRpcArgs",
    "request_too_large",
  ]) {
    assert.ok(protocol.includes(token), `missing protocol bound: ${token}`);
  }
  for (const token of [
    "decoded['protocol'] != stdioRpcProtocol",
    "decoded['id'] != requestId",
    "decoded['workflowId'] != workflowId",
    "ConversationDeltaDecoder",
    "StdioRpcProtocolViolation",
  ]) {
    assert.ok(response.includes(token), `missing response binding: ${token}`);
  }
});

test("stdio RPC preserves mutation ordering, cursor replay, and private projections", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  assert.ok(source["client.dart"].includes("StdioRpcOperationQueue _operations"));
  assert.ok(source["client.dart"].includes("_operations.serialize"));
  assert.ok(source["operation_queue.dart"].includes("class StdioRpcOperationQueue"));
  assert.ok(source["command_round_trip.dart"].includes("replayed"));
  assert.ok(source["session.dart"].includes("StdioRpcLineFramer"));
  assert.ok(source["session.dart"].includes("_expectedFrames.containsKey(requestId)"));
  assert.ok(source["session.dart"].includes("_expectedConversations.containsKey(requestId)"));
  assert.ok(source["method_policy.dart"].includes("stdioRpcMethodIsInFlightControl"));
  assert.ok(source["in_flight_control.dart"].includes("executeStdioRpcStructuredCommand"));
  assert.ok(source["in_flight_control.dart"].includes("invalidateAndDiscard"));
  assert.ok(source["client.dart"].includes("arguments: const ['rpc', 'conversation']"));
  assert.ok(source["conversation_exchange.dart"].includes("agent.conversation.attach"));
  assert.ok(source["conversation_exchange.dart"].includes("afterCursor"));
  assert.equal(source["in_flight_control.dart"].includes("StdioRpcOperationQueue"), false);
  assert.ok(source["session.dart"].includes("stderrBytes"));
  assert.ok(source["session.dart"].includes("stderrTruncated"));
  for (const projection of [
    "stderrText",
    "StringBuffer stderr",
    "utf8.decode(process.stderr",
    "error.toString()",
    "StackTrace.current",
  ]) {
    assert.equal(joined.includes(projection), false);
  }
});

test("stdio RPC owns fast protocol, framer, and public-client regressions", async () => {
  for (const testPath of [
    "apps/desktop/test/native_conversation_port_test.dart",
    "apps/desktop/test/native_stdio_rpc_client_test.dart",
    "apps/desktop/test/native_stdio_rpc_read_pool_test.dart",
    "apps/desktop/test/native_stdio_rpc_line_framer_test.dart",
    "apps/desktop/test/native_stdio_rpc_protocol_test.dart",
    "apps/desktop/test/native_persistent_runtime_test.dart",
    "apps/desktop/test/agent_conversation_reattachment_test.dart",
  ]) {
    const source = await read(testPath);
    assert.ok(source.includes("void main()"));
    assert.equal(source.includes("part "), false);
  }
});

test("native conversation RPC uses a client-local persistent owner", async () => {
  const host = await read("crates/licoup-native/src/bin/licoup/conversation_host.rs");
  const server = await read("crates/licoup-native/src/bin/licoup/stdio_rpc/server/conversation.rs");
  for (const token of [
    "rpc\", \"conversation-host",
    "serve_stdio_rpc_with_persistent_conversation",
    "client_disconnected",
    "runtime.request_host_stop()",
    "runtime.wait_for_admitted_turns()",
    "runtime.wait_for_clients_to_disconnect()",
  ]) assert.ok(host.includes(token), `missing persistent host contract: ${token}`);
  assert.equal(host.includes("runtime.idle()"), false);
  for (const token of [
    "PersistentConversationRuntime",
    "turnHandle",
    "afterCursor",
    "record_event",
  ]) assert.ok(server.includes(token), `missing persistent replay contract: ${token}`);
});
