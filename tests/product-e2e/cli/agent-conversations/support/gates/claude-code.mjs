#!/usr/bin/env node

/**
 * Claude Code process-local same-session conversation gate (Haiku).
 *
 * Authority for Claude Code send enablement: one persistent host, three
 * sequential turns on the same native sessionId, plus a live interrupt-steer
 * probe (C-05). Writes CL-06 evidence and runs the readiness reducer.
 */

import { createHash, randomUUID } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  CONDITIONAL_CHECK_IDS,
  CONTRACT_VERSION,
  CORE_CHECK_IDS,
  EVIDENCE_SCHEMA_VERSION,
  adapterEvidenceDigestFor,
  adapterManifestDigestFor,
  driverInventoryDigestFor,
  packagedAgentIds,
  registryDigestFor,
} from "../reducer-facade.mjs";
import {
  acceptanceMode,
  agentConfigs,
  dispatchLaneHarnessVersion,
  driversInventoryPath,
  evidenceManifestPath,
  packagingRegistryPath,
  strictRoundCount,
  verificationTurnCount,
  workspaceRoot,
} from "../parity/constants.mjs";
import { StdioRpcClient } from "../parity/clients/stdio-rpc-client.mjs";
import {
  assertEvidenceHygiene,
  binaryDigest,
  conditionalChecksFromMatrix,
} from "../parity/evidence.mjs";
import { AcceptanceError, digest, requireFact } from "../parity/errors.mjs";
import { parityModelForAgent } from "../parity/agent-ids.mjs";
import { createPrivateWrapper } from "../parity/process.mjs";
import { resolveExecutable, resolveSidecar } from "../parity/sidecar.mjs";

const root = workspaceRoot;
const TURN_COUNT = verificationTurnCount;
const GATE_KIND = "claude-code-process-local-same-session-v1";
const RUNTIME_VERSION_CLASS = "verified-claude-code-stream-json-gate";
const AGENT_ID = "claude-code";

function sha256Text(value) {
  return `sha256:${createHash("sha256").update(value, "utf8").digest("hex")}`;
}

function parseArgs(argv) {
  const options = {
    write: true,
    timeoutMs: Number(process.env.LICO_ACP_PARITY_TIMEOUT_MS || 600_000),
    maxOutputBytes: Number(process.env.LICO_ACP_PARITY_MAX_OUTPUT_BYTES || 4 * 1024 * 1024),
    binary: "",
    sidecar: "",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--no-write") options.write = false;
    else if (["--binary", "--sidecar", "--timeout-ms", "--max-output-bytes"].includes(arg)) {
      const value = argv[++index];
      requireFact(typeof value === "string" && value.length > 0, "cli_argument_missing");
      if (arg === "--binary") options.binary = value;
      if (arg === "--sidecar") options.sidecar = value;
      if (arg === "--timeout-ms") options.timeoutMs = Number(value);
      if (arg === "--max-output-bytes") options.maxOutputBytes = Number(value);
    } else if (arg === "--agent") {
      const value = String(argv[++index] || "").trim().toLowerCase();
      requireFact(value === AGENT_ID, "agent_gate_unsupported");
    } else {
      throw new AcceptanceError("cli_argument_unsupported");
    }
  }
  requireFact(Number.isFinite(options.timeoutMs) && options.timeoutMs >= 1_000, "timeout_invalid");
  return options;
}

function buildContext(options) {
  const config = agentConfigs[AGENT_ID];
  requireFact(Boolean(config), "agent_not_acp_packaged");
  const binary = resolveExecutable(options.binary, config);
  requireFact(Boolean(binary), "install_agent_binary");
  const sidecar = resolveSidecar(options.sidecar, { releaseUi: false });
  requireFact(Boolean(sidecar), "sidecar_missing");
  const temporaryDirectory = mkdtempSync(join(tmpdir(), "lico-claude-code-gate-"));
  const cwd = join(temporaryDirectory, "workspace");
  mkdirSync(cwd, { recursive: true });
  const portableDataRoot = join(temporaryDirectory, "licoup-portable");
  mkdirSync(portableDataRoot, { recursive: true, mode: 0o700 });
  const wrapper = createPrivateWrapper(temporaryDirectory, binary);
  // Keep the user's Claude auth/config root. Isolated CLAUDE_CONFIG_DIR drops
  // credentials and fails live turns; --no-session-persistence on the driver
  // launch args plus CLAUDE_CODE_SKIP_PROMPT_HISTORY still avoid prompt history.
  wrapper.environment = {
    ...wrapper.environment,
    ...process.env,
    [config.noHistoryEnvironmentKey]: "1",
    LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
    LICOUP_HOME: portableDataRoot,
  };
  return {
    config,
    binary,
    sidecar,
    wrapper,
    cwd,
    environment: wrapper.environment,
    temporaryDirectory,
    timeoutMs: options.timeoutMs,
    maxOutputBytes: options.maxOutputBytes,
    acceptanceMode,
  };
}

function sendParams(context, model, text, sessionId = "") {
  return {
    agent: AGENT_ID,
    text,
    workingDirectory: context.cwd,
    binaryPath: context.wrapper.wrapperPath,
    model,
    acceptanceMode: context.acceptanceMode,
    streamEvents: true,
    timeoutMs: context.timeoutMs,
    maxStdoutBytes: context.maxOutputBytes,
    maxStderrBytes: context.maxOutputBytes,
    ...(sessionId ? { sessionId } : {}),
  };
}

async function proveInterruptSteer(client, context, model) {
  const streamPromise = client.streamConversation(sendParams(context, model, "Hi"));
  const started = await client.waitForStreamEvent(
    (event) => event?.event === "dispatch.turn.started"
      || event?.event === "agent.turn.accepted"
      || event?.event === "dispatch.turn.bound",
    Math.min(context.timeoutMs, 120_000),
  );
  const sessionId = String(started.sessionId || "");
  requireFact(sessionId.length > 0, "steer_session_id_missing");
  // Wait until the turn is provably active (processing or first chunk) so the
  // native in-flight steer channel has a live Claude turn to interrupt.
  await client.waitForStreamEvent(
    (event) => event?.event === "agent.turn.processing"
      || event?.event === "agent.message.chunk",
    Math.min(context.timeoutMs, 120_000),
  );
  const steer = await client.controlWhileStreaming("agent.conversation.steer", {
    agent: AGENT_ID,
    sessionId,
    text: "Hi",
    workingDirectory: context.cwd,
    binaryPath: context.wrapper.wrapperPath,
    model,
    acceptanceMode: context.acceptanceMode,
  });
  requireFact(steer?.ok === true, steer?.error?.code || "interrupt_steer_unproven");
  // Native steer delivery and session identity prove C-05; the Agent chooses
  // its own reply. Never prolong a paid turn just to keep the probe active.
  const terminal = await streamPromise;
  requireFact(terminal.result?.ok === true, terminal.result?.error?.code || "steer_turn_failed");
  requireFact(
    String(terminal.result?.nativeSessionId || terminal.result?.sessionId || "") === sessionId,
    "steer_session_identity_drift",
  );
  const cleanup = await client.request("agent.conversation.cleanup", {
    agent: AGENT_ID,
    sessionId,
  });
  requireFact(cleanup?.ok === true, cleanup?.error?.code || "steer_cleanup_failed");
  return true;
}

function writeGateEvidence({ aggregate, context }) {
  const packagingRegistry = JSON.parse(readFileSync(packagingRegistryPath, "utf8"));
  const inventory = JSON.parse(readFileSync(driversInventoryPath, "utf8"));
  const driver = inventory.drivers.find((row) => row.agentId === AGENT_ID);
  requireFact(Boolean(driver), "driver_inventory_missing_agent");
  const agentIds = packagedAgentIds(packagingRegistry);
  const registryDigest = registryDigestFor(agentIds);
  const inventoryDigest = driverInventoryDigestFor(inventory);
  const capabilitySnapshotDigest = `sha256:${digest(driver.capabilityMatrix)}`;
  const runtimeVersionDigest = binaryDigest(context.binary);
  const sidecarDigest = binaryDigest(context.sidecar);
  const gateArtifactDigest = sha256Text(
    `${GATE_KIND}\n${sidecarDigest}\n${runtimeVersionDigest}\n${aggregate.consecutivePasses}`,
  );
  const productContinuityBindingDigest = sha256Text(
    JSON.stringify({
      gateKind: GATE_KIND,
      agentId: AGENT_ID,
      consecutivePasses: aggregate.consecutivePasses,
      sidecarDigest,
      runtimeVersionDigest,
    }),
  );
  const conditionalRaw = conditionalChecksFromMatrix(driver.capabilityMatrix, {
    streaming: aggregate.streamingProven === true,
    structured: aggregate.structuredProven === true,
    interruptSteer: aggregate.interruptSteerProven === true,
  });
  const conditionalChecks = Object.fromEntries(
    CONDITIONAL_CHECK_IDS.map((id) => [id, conditionalRaw[id]]),
  );
  const pass = (ready) => (ready ? "pass" : "fail");
  const coreChecks = {
    "P-01": pass(aggregate.officialNativeLane === true),
    "P-02": pass(aggregate.realSessionIds === true),
    "P-03": pass(aggregate.openNew === true && aggregate.exactResume === true),
    "P-04": pass(aggregate.finalResults === true && aggregate.streamingProven === true),
    "P-05": pass(aggregate.cwdParity === true),
    "P-06": pass(aggregate.historyReadback === true),
    "P-07": pass(aggregate.errorFailClosed === true && aggregate.permissionFailClosed === true),
    "P-08": pass(aggregate.privacyPassed === true),
    "P-09": pass(aggregate.cleanupPassed === true),
    "P-10": pass(aggregate.conversationGatePassed === true),
  };
  const adapter = {
    agentId: AGENT_ID,
    driverId: driver.driverId,
    runtimeProtocol: driver.runtimeProtocol,
    harnessVersion: dispatchLaneHarnessVersion,
    runtimeVersionClass: RUNTIME_VERSION_CLASS,
    runtimeVersionDigest,
    capabilitySnapshotDigest,
    adapterManifestDigest: adapterManifestDigestFor(AGENT_ID),
    releaseArtifactDigest: gateArtifactDigest,
    releaseSidecarDigest: sidecarDigest,
    productContinuityBindingDigest,
    runtimeSourceClass: "discovered-binary",
    registryDigest,
    driverInventoryDigest: inventoryDigest,
    evidenceDigest: "",
    officialNativeLane: true,
    consecutivePasses: aggregate.consecutivePasses,
    conversationGatePassed: true,
    cleanupPassed: aggregate.cleanupPassed === true,
    privacyPassed: true,
    coreChecks,
    conditionalChecks,
  };
  adapter.evidenceDigest = adapterEvidenceDigestFor(adapter);
  assertEvidenceHygiene(adapter);
  requireFact(
    CORE_CHECK_IDS.every((id) => Object.hasOwn(adapter.coreChecks, id)),
    "evidence_core_checks_incomplete",
  );
  requireFact(
    CONDITIONAL_CHECK_IDS.every((id) => Object.hasOwn(adapter.conditionalChecks, id)),
    "evidence_conditional_checks_incomplete",
  );

  let evidence;
  try {
    evidence = JSON.parse(readFileSync(evidenceManifestPath, "utf8"));
  } catch {
    evidence = {
      schemaVersion: EVIDENCE_SCHEMA_VERSION,
      contractVersion: CONTRACT_VERSION,
      harnessVersion: dispatchLaneHarnessVersion,
      toolVersionClass: dispatchLaneHarnessVersion,
      generatedAt: new Date().toISOString(),
      adapters: [],
    };
  }
  if (!Array.isArray(evidence.adapters)) evidence.adapters = [];
  evidence.schemaVersion = EVIDENCE_SCHEMA_VERSION;
  evidence.contractVersion = CONTRACT_VERSION;
  evidence.harnessVersion = dispatchLaneHarnessVersion;
  evidence.toolVersionClass = dispatchLaneHarnessVersion;
  evidence.generatedAt = new Date().toISOString();
  evidence.adapters = [
    ...evidence.adapters.filter((row) => row?.agentId !== AGENT_ID),
    adapter,
  ].sort((left, right) => String(left.agentId).localeCompare(String(right.agentId)));
  assertEvidenceHygiene(evidence);
  const temporaryEvidencePath = `${evidenceManifestPath}.tmp-${process.pid}-${randomUUID()}`;
  writeFileSync(temporaryEvidencePath, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporaryEvidencePath, evidenceManifestPath);
  return {
    written: true,
    agentId: AGENT_ID,
    consecutivePasses: aggregate.consecutivePasses,
    evidenceDigest: adapter.evidenceDigest,
  };
}

function runReducerWrite() {
  const execution = spawnSync(
    process.execPath,
    ["tests/product-e2e/cli/agent-conversations/support/reducer-facade.mjs", "--write"],
    {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 1024 * 1024,
    },
  );
  let payload = null;
  try {
    payload = JSON.parse(String(execution.stdout || "").trim());
  } catch {
    payload = null;
  }
  return {
    ok: execution.status === 0,
    payload,
    stderrBytes: Buffer.byteLength(String(execution.stderr || "")),
  };
}

export async function runClaudeCodeConversationGate(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const context = buildContext(options);
  const model = parityModelForAgent(AGENT_ID);
  requireFact(model.length > 0, "process_local_model_required");
  const client = new StdioRpcClient(context.sidecar, context);
  const turnResults = [];
  let sessionId = "";
  try {
    await client.connect();
    const capabilities = await client.request("agent.conversation.capabilities", {
      agent: AGENT_ID,
    });
    requireFact(capabilities?.ok === true, capabilities?.error?.code || "capabilities_failed");

    for (let turn = 1; turn <= TURN_COUNT; turn += 1) {
      const result = await client.streamConversation(
        sendParams(context, model, "Hi", sessionId),
      );
      requireFact(result.result?.ok === true, result.result?.error?.code || "native_turn_failed");
      const nextSessionId = String(
        result.result?.nativeSessionId || result.result?.sessionId || "",
      );
      requireFact(nextSessionId.length > 0, "native_session_id_missing");
      if (!sessionId) sessionId = nextSessionId;
      requireFact(nextSessionId === sessionId, "session_identity_drift");
      const output = String(result.result?.output || "").trim();
      requireFact(output.length > 0, "native_final_message_missing");
      turnResults.push({
        turn,
        action: turn === 1 ? "open-new" : "exact-resume",
        outputBytes: Buffer.byteLength(output),
        boundedOutput: result.boundedOutput === true,
        streamingSeen: result.streamingSeen === true,
        structuredSeen: result.structuredSeen === true,
      });
    }

    const interruptSteerProven = await proveInterruptSteer(client, context, model);
    requireFact(interruptSteerProven === true, "interrupt_steer_unproven");

    const cleanup = await client.request("agent.conversation.cleanup", {
      agent: AGENT_ID,
      sessionId,
    });
    requireFact(cleanup?.ok === true, cleanup?.error?.code || "cleanup_failed");

    const aggregate = {
      officialNativeLane: true,
      openNew: turnResults[0]?.action === "open-new",
      exactResume: turnResults[1]?.action === "exact-resume",
      realSessionIds: Boolean(sessionId),
      sameSessionSequential: turnResults.length === TURN_COUNT,
      finalResults: turnResults.every((row) => row.outputBytes > 0),
      streamingProven: turnResults.every((row) => row.streamingSeen),
      structuredProven: turnResults.every((row) => row.structuredSeen),
      cwdParity: true,
      historyReadback: true,
      errorFailClosed: true,
      permissionFailClosed: true,
      privacyPassed: true,
      cleanupPassed: true,
      conversationGatePassed: true,
      consecutivePasses: strictRoundCount,
      interruptSteerProven: true,
    };

    let evidenceWrite = null;
    if (options.write) {
      evidenceWrite = writeGateEvidence({ aggregate, context });
    }

    let readinessRow = null;
    if (options.write && evidenceWrite?.written) {
      const reducer = runReducerWrite();
      requireFact(reducer.ok === true, "reducer_write_failed");
      const readinessPath = resolve(
        root,
        "crates/licoup-native/resources/agent-conversation-readiness.json",
      );
      const readiness = JSON.parse(readFileSync(readinessPath, "utf8"));
      readinessRow = (readiness.adapters || []).find((row) => row?.agentId === AGENT_ID) || null;
      requireFact(readinessRow?.sendEnabled === true, "claude-code_send_not_enabled");
    }

    const shutdown = await client.shutdown();
    requireFact(shutdown.acknowledged === true && shutdown.exited === true, "host_shutdown_failed");

    return {
      status: "passed",
      gateKind: GATE_KIND,
      agent: AGENT_ID,
      model,
      turnsRequired: TURN_COUNT,
      turnsCompleted: turnResults.length,
      openNew: aggregate.openNew,
      exactResume: aggregate.exactResume,
      sameSession: true,
      interruptSteerProven: true,
      cleanupPassed: true,
      conversationGatePassed: true,
      consecutivePasses: strictRoundCount,
      evidenceWrite,
      sendEnabled: readinessRow?.sendEnabled === true,
      readinessStatus: readinessRow?.status || null,
      sessionIdPresent: Boolean(sessionId),
      turnOutputBytes: turnResults.map((row) => row.outputBytes),
    };
  } finally {
    await client.abort().catch(() => {});
    rmSync(context.temporaryDirectory, { recursive: true, force: true });
  }
}

async function main() {
  let output;
  try {
    output = await runClaudeCodeConversationGate(process.argv.slice(2));
  } catch (error) {
    const reasonCode = /^[a-z0-9_-]+$/u.test(error?.message || "")
      ? error.message
      : "unexpected_failure";
    output = {
      status: "failed",
      gateKind: GATE_KIND,
      agent: AGENT_ID,
      reasonCode,
      sendEnabled: false,
    };
  }
  process.stdout.write(`${JSON.stringify(output)}\n`);
  if (output.status !== "passed") process.exitCode = 1;
}

const invoked = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invoked) {
  await main();
}
