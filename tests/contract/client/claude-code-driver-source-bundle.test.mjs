import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
// The whole Claude Code driver is the adapter package's: the vendor protocol,
// its parser, the replay arm and the process that speaks them. The host keeps
// no Claude Code driver module of its own, so this contract reads the package's
// leaves from its own crate and the host's composition from the kernel.
const kernelRoot = "crates/licoup-native/src/platform";
const driverRoot = "crates/licoup-agent-claude-code/src/driver";
const packageRoot = "crates/licoup-agent-claude-code";
const parserRoot = `${packageRoot}/src/protocol/parser`;
const compositionPath = `${kernelRoot}/runtime_adapters/drivers.rs`;
// The fake streaming CLI the driver's own suite and the host's process-local
// history suite both compile. The package cannot share a file with the host's
// test tree without a path dependency its own contract forbids, so the two
// copies are one fixture: they must stay byte-identical rather than drift.
const fixtureMirrors = Object.freeze([
  "fake_claude_code.rs",
  "claude_process_local_test_lock.rs",
]);

const productionLeaves = Object.freeze([
  "approval.rs",
  "control.rs",
  "execution.rs",
  "failure.rs",
  "io.rs",
  "launch.rs",
  "model.rs",
  "probe.rs",
  "reset.rs",
  "supervision.rs",
  "transport.rs",
]);
const parserLeaves = Object.freeze([
  "../parser.rs",
  "../params.rs",
  "../launch.rs",
  "../failure.rs",
  "../control.rs",
  "adapter.rs",
  "events.rs",
  "state.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function exists(relativePath) {
  try {
    await fs.access(path.join(repoRoot, relativePath));
    return true;
  } catch {
    return false;
  }
}

async function sources() {
  const driver = Object.fromEntries(await Promise.all(
    productionLeaves.map(async (leaf) => [leaf, await read(`${driverRoot}/${leaf}`)]),
  ));
  // The package's protocol lives under its own `protocol/` root, so a leaf the
  // driver re-exports keeps its package name and a leaf the parser owns is read
  // from the parser root.
  const packageLeaves = Object.fromEntries(await Promise.all(
    parserLeaves.map(async (leaf) => {
      const relative = leaf.startsWith("../")
        ? `${packageRoot}/src/protocol/${leaf.slice(3)}`
        : `${parserRoot}/${leaf}`;
      return [leaf.startsWith("../") ? leaf.slice(3) : leaf, await read(relative)];
    }),
  ));
  return { ...driver, ...packageLeaves };
}

test("the host declares no Claude Code driver and the composition reads the package", async () => {
  // The kernel has no Claude Code driver module or tree at all: the host reads
  // the Agent's protocol and process through the package and cannot hold a
  // second owner of either.
  assert.equal(
    await exists("crates/licoup-native/src/platform/claude_code_driver.rs"),
    false,
    "the host still declares a Claude Code driver module",
  );
  assert.equal(
    await exists("crates/licoup-native/src/platform/claude_code_driver"),
    false,
    "the host still declares a Claude Code driver tree",
  );
  const platform = await read(`${kernelRoot}/mod.rs`);
  assert.doesNotMatch(platform, /mod claude_code_driver;/u,
    "the host module tree still declares a Claude Code driver module");

  const composition = await read(compositionPath);
  assert.match(composition, /use licoup_agent_claude_code::driver as claude_code_driver;/u,
    "the composition does not read the package's driver");
  // The composition keeps no copy of the vendor fact: the launch declaration,
  // the framing, the argv and the turn phases are all the package's.
  for (const forbidden of [
    "FIXED_STREAM_ARGS",
    '"--input-format"',
    '"stream-json"',
    "MAX_PROTOCOL_LINE_BYTES",
    "struct PersistentTransport",
    "struct TurnState",
  ]) {
    assert.equal(composition.includes(forbidden), false,
      `the host composition keeps a copy of the package's protocol: ${forbidden}`);
  }

  // The package's driver root is the composition of the leaves below it, and
  // holds no implementation and no hidden include of its own.
  const facade = await read(`${driverRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    [...productionLeaves].sort(),
  );
  assert.match(facade, /^pub use execution::execute;$/mu);
  assert.match(facade, /^pub use probe::probe;$/mu);
  assert.match(facade, /^pub use supervision::\{cancel, cleanup_session, history, steer\};$/mu);
  assert.match(facade, /^pub use model::\{RUNTIME_PROTOCOL, RunResult\};$/mu);
  for (const implementationToken of ["include!(", "#[path"]) {
    assert.equal(facade.includes(implementationToken), false);
  }

  // One fixture, two crates that cannot share a test file: the copies are the
  // same bytes, so the driver and the host's fold cannot drift apart.
  for (const fixture of fixtureMirrors) {
    assert.equal(
      await read(`crates/licoup-native/tests/fixtures/${fixture}`),
      await read(`${packageRoot}/tests/fixtures/${fixture}`),
      `the mirrored Claude Code fixture drifted: ${fixture}`,
    );
  }
});

test("Claude Code keeps the fixed streaming-input lane with native resume and no shell fallback", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  // The runtime protocol is the protocol leaf's declaration, read by the
  // driver at its own package path rather than through a second name.
  assert.ok(source["model.rs"].includes("crate::protocol::RUNTIME_PROTOCOL"));
  assert.ok(source["parser.rs"].includes('"claude-code.stream-json.v1"') === false);
  assert.ok(
    (await read(`${packageRoot}/src/protocol/mod.rs`)).includes(
      'RUNTIME_PROTOCOL: &str = "claude-code-cli-stream-json"',
    ),
  );
  for (const token of [
    '"--input-format"',
    '"stream-json"',
    '"--output-format"',
    '"--include-partial-messages"',
  ]) {
    assert.ok(source["launch.rs"].includes(token), `missing fixed command token: ${token}`);
  }
  assert.ok(source["params.rs"].includes("stdin_message"));
  assert.ok(source["launch.rs"].includes('"--append-system-prompt"'));
  assert.equal(source["params.rs"].includes("claude_code_private_instructions_unsupported"), false);
  for (const forbidden of [
    '"--continue"',
    'Command::new("sh")',
    'Command::new("bash")',
    'Command::new("cmd")',
    'Command::new("powershell")',
  ]) {
    assert.equal(joined.includes(forbidden), false);
  }
  assert.ok(source["launch.rs"].includes('"--resume"'));
});

test("Claude Code public lifecycle contract is bounded and exact-session scoped", async () => {
  const manifest = JSON.parse(await read(
    "packages/contracts/client/fixtures/agent-conversation-adapter/manifests/claude-code.json",
  ));
  assert.equal(manifest.transport.sessionScope, "persistent");
  assert.equal(manifest.transport.continuityChannel, "launch-argument");
  assert.ok(Number.isSafeInteger(manifest.lifecycle.maxConcurrentTransports));
  assert.ok(manifest.lifecycle.maxConcurrentTransports > 0);
  assert.ok(manifest.lifecycle.maxConcurrentTransports <= 64);
  assert.ok(Number.isSafeInteger(manifest.lifecycle.maxTrackedSessions));
  assert.ok(manifest.lifecycle.maxTrackedSessions >= manifest.lifecycle.maxConcurrentTransports);
  assert.ok(manifest.lifecycle.maxTrackedSessions <= 4096);
  assert.equal(manifest.lifecycle.cleanupScope, "process-session");
  assert.equal(manifest.operations.exactResume.status, "supported");
  assert.equal(manifest.operations.cleanup.status, "supported");
  assert.equal(manifest.operations.history.status, "supported");
  assert.equal(manifest.privacy.safeCleanup, true);
  assert.equal(manifest.privacy.continuityIdInArguments, true);
});

test("Claude Code IO, events, controls, probe, and failures stay bounded and redacted", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  for (const token of [
    "MAX_PROTOCOL_LINE_BYTES",
    "LineLimitExceeded",
    "BoundedStdinWriter",
    "max_stdout",
    "max_stderr",
    "CONTROL_QUEUE_CAPACITY",
    "IO_THREAD_EXIT_GRACE",
    "claude_code_timeout",
    "PROCESS_POLL_INTERVAL",
  ]) {
    assert.ok(joined.includes(token), `missing bounded lifecycle token: ${token}`);
  }
  assert.ok(source["events.rs"].includes("processing_evidence_kind"));
  assert.ok(source["adapter.rs"].includes("fn parse_line"));
  assert.ok(source["io.rs"].includes("Line(Vec<u8>)"));
  assert.equal(source["io.rs"].includes("serde_json::from"), false);
  assert.ok(source["failure.rs"].includes("message: &'static str"));
  for (const rawProjection of [
    "stderr: String",
    "stderr: Vec",
    "String::from_utf8_lossy(&stderr",
    '"tool_input": message',
    '"message": message',
    '"session_id": message',
  ]) {
    assert.equal(joined.includes(rawProjection), false);
  }
});

test("Claude Code split contains no production unsafe or hidden compatibility include", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  assert.equal(joined.includes("unsafe {"), false);
  assert.equal(joined.includes("include!("), false);
  assert.equal(joined.includes("#[path"), false);
});

test("Claude Code process-local lifecycle and transcript have one complete supervisor authority", async () => {
  const source = await sources();
  for (const token of [
    "TransportLifecycle",
    "Live",
    "Closing",
    "Closed",
    "CompleteTranscript",
    "project_backward_page",
  ]) {
    assert.ok(
      source["model.rs"].includes(token),
      `missing process-local model token: ${token}`,
    );
  }
  assert.equal(source["model.rs"].includes("BoundedTranscript"), false);
  assert.equal(source["model.rs"].includes("VecDeque"), false);
  for (const symbol of ["cleanup_session", "clear_all_for_test"]) {
    assert.ok(
      source["supervision.rs"].includes(symbol),
      `missing frozen lifecycle symbol: ${symbol}`,
    );
  }
  assert.ok(source["state.rs"].includes("claude_code_authentication_required"));
});

test("Claude Code product controls and parity use one persistent stdio RPC owner", async () => {
  const [
    request,
    server,
    processIo,
    rpcClient,
    processLocalRound,
    results,
    evidence,
  ] = await Promise.all([
    read("crates/licoup-native/src/bin/licoup/stdio_rpc/request.rs"),
    read("crates/licoup-native/src/bin/licoup/stdio_rpc/server.rs"),
    read("apps/desktop/lib/src/platform/native_client/agent_service_process_io.dart"),
    read("tests/product-e2e/cli/agent-conversations/support/parity/clients/stdio-rpc-client.mjs"),
    read("tests/product-e2e/cli/agent-conversations/support/parity/process-local-round.mjs"),
    read("tests/product-e2e/cli/agent-conversations/support/parity/results.mjs"),
    read("tests/product-e2e/cli/agent-conversations/support/parity/evidence.mjs"),
  ]);
  const operationVariants = {
    open: "AgentConversationOpen",
    send: "AgentConversationSend",
    history: "AgentConversationHistory",
    cleanup: "AgentConversationCleanup",
    capabilities: "AgentConversationCapabilities",
    cancel: "AgentConversationCancel",
  };
  assert.ok(request.includes('strip_prefix("agent.conversation.")'));
  for (const [operation, variant] of Object.entries(operationVariants)) {
    assert.ok(
      request.includes(`ConversationProtocolMethod::${variant}`),
      `missing persistent conversation operation: ${operation}`,
    );
  }
  assert.ok(server.includes("PersistentConversationRuntime"));
  assert.ok(processIo.includes("executeStructured"));
  assert.ok(rpcClient.includes('["rpc", "stdio"]'));
  assert.ok(processLocalRound.includes('continuityScope: "process-local"'));
  assert.equal(processLocalRound.includes("nativeTurn("), false);
  assert.equal(processLocalRound.includes("AcpClient"), false);
  assert.equal(processLocalRound.includes("runSidecar("), false);
  assert.equal(processLocalRound.includes("runBoundedProcess("), false);
  assert.equal(processLocalRound.includes(["cleanup", "DurationMs"].join("")), false);
  assert.ok(processLocalRound.includes("strictHistoryProjection"));
  assert.ok(processLocalRound.includes("eventTranscriptMatches"));
  assert.ok(rpcClient.includes("stdio_rpc_frame_after_terminal"));
  assert.ok(rpcClient.includes("stdio_rpc_turn_id_reused"));
  assert.ok(rpcClient.includes("stdio_rpc_chunk_output_mismatch"));
  assert.ok(results.includes("processLocalFactsEvidenceComplete"));
  assert.ok(results.includes("processLocalFactsPassed"));
  assert.ok(evidence.includes("process_local_facts_unproven"));
});

test("Claude Code capabilities expose supervised active-turn cancel", async () => {
  const [manifestText, inventoryText] = await Promise.all([
    read("packages/contracts/client/fixtures/agent-conversation-adapter/manifests/claude-code.json"),
    read("crates/licoup-native/resources/agent-conversation-drivers.json"),
  ]);
  const manifest = JSON.parse(manifestText);
  const inventory = JSON.parse(inventoryText);
  const driver = inventory.drivers.find((row) => row.agentId === "claude-code");
  assert.equal(manifest.transport.sessionScope, "persistent");
  assert.equal(manifest.operations.history.status, "supported");
  assert.equal(manifest.operations.cancel.status, "supported");
  assert.equal(manifest.acceptance.continuityScope, "process-local");
  assert.equal(manifest.acceptance.nativeToArcRequired, false);
  assert.equal(manifest.acceptance.arcToNativeRequired, false);
  assert.equal(driver.capabilityMatrix.processLocalContinuation, true);
  assert.equal(driver.capabilityMatrix.cancel, true);
});

test("Claude Code routing remains model-data driven and native resume stays explicit", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n").toLowerCase();
  for (const forbidden of [
    "deepseek",
    "kimi k3",
    "gpt-5.6",
    '"--continue"',
  ]) {
    assert.equal(joined.includes(forbidden), false);
  }
  assert.ok(source["launch.rs"].includes('"--model"'));
  assert.ok(source["launch.rs"].includes('"--resume"'));
  assert.equal(source["launch.rs"].includes('"--no-session-persistence"'), false);
});
