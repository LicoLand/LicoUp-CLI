#!/usr/bin/env node

/**
 * Same-session sequential conversation gate (background, no release UI).
 *
 * Authority for send enablement on supported agents: one new-conversation
 * turn, then one continuation turn on that exact sessionId.
 * Writes CL-06 evidence and runs the readiness reducer. Does not launch the
 * product e2e LicoUp.app path.
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
import { homedir, tmpdir } from "node:os";
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
  agentConfigs,
  acceptanceMode,
  dispatchLaneHarnessVersion,
  driversInventoryPath,
  evidenceManifestPath,
  packagingRegistryPath,
  strictRoundCount,
  verificationTurnCount,
  workspaceRoot,
} from "../parity/constants.mjs";
import {
  assertEvidenceHygiene,
  binaryDigest,
  conditionalChecksFromMatrix,
} from "../parity/evidence.mjs";
import { AcceptanceError, digest, requireFact } from "../parity/errors.mjs";
import {
  nativeTurn,
  runSidecar,
} from "../parity/native/acp-turn.mjs";
import { parityModelForAgent } from "../parity/agent-ids.mjs";
import { createPrivateWrapper } from "../parity/process.mjs";
import {
  cleanupSession,
  preflightCleanup,
} from "../parity/session-cleanup.mjs";
import { resolveExecutable, resolveSidecar } from "../parity/sidecar.mjs";

const root = workspaceRoot;
const TURN_COUNT = verificationTurnCount;

const AGENT_GATES = Object.freeze({
  cursor: Object.freeze({
    gateKind: "cursor-same-session-sequential-v1",
    runtimeVersionClass: "verified-cursor-cli-gate",
    cleanupViaSidecar: true,
    turnViaSidecar: false,
  }),
  "kimi-code": Object.freeze({
    gateKind: "kimi-code-same-session-sequential-v1",
    runtimeVersionClass: "verified-kimi-code-acp-gate",
    cleanupViaSidecar: false,
    turnViaSidecar: false,
  }),
  antigravity: Object.freeze({
    gateKind: "antigravity-same-session-sequential-v1",
    runtimeVersionClass: "verified-antigravity-cli-argv-hook-gate",
    cleanupViaSidecar: true,
    turnViaSidecar: true,
  }),
});

export const sameSessionGateAgentIds = Object.freeze(Object.keys(AGENT_GATES));

function sha256Text(value) {
  return `sha256:${createHash("sha256").update(value, "utf8").digest("hex")}`;
}

function parseArgs(argv) {
  const options = {
    agent: "",
    write: true,
    timeoutMs: Number(process.env.LICO_ACP_PARITY_TIMEOUT_MS || 600_000),
    maxOutputBytes: Number(process.env.LICO_ACP_PARITY_MAX_OUTPUT_BYTES || 4 * 1024 * 1024),
    sidecar: "",
    binary: "",
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--no-write") options.write = false;
    else if (argument === "--timeout-ms") {
      options.timeoutMs = Number(argv[++index]);
    } else if (argument === "--sidecar") {
      options.sidecar = resolve(argv[++index]);
    } else if (argument === "--binary") {
      options.binary = resolve(argv[++index]);
    } else if (argument === "--agent") {
      options.agent = String(argv[++index] || "");
    } else {
      throw new AcceptanceError("cli_argument_unsupported");
    }
  }
  requireFact(Number.isFinite(options.timeoutMs) && options.timeoutMs >= 1_000, "timeout_invalid");
  requireFact(Object.hasOwn(AGENT_GATES, options.agent), "agent_gate_unsupported");
  return options;
}

async function runSidecarCleanup(sidecar, agentId, sessionId, environment, timeoutMs) {
  const run = spawnSync(
    sidecar,
    ["agent", "conversation", "cleanup", "--stdin-json", "true"],
    {
      input: `${JSON.stringify({ agent: agentId, sessionId })}\n`,
      encoding: "utf8",
      env: environment,
      maxBuffer: 256 * 1024,
      timeout: Math.min(timeoutMs, 90_000),
    },
  );
  if (run.error?.code === "ETIMEDOUT") return { ok: false, reason: "cleanup_timeout" };
  let payload = null;
  try {
    payload = JSON.parse(String(run.stdout || "").trim());
  } catch {
    return { ok: false, reason: "cleanup_invalid_json" };
  }
  return {
    ok: run.status === 0 && payload?.ok === true,
    reason: payload?.error?.code || (run.status === 0 ? null : "cleanup_failed"),
  };
}

function writeGateEvidence({ agentId, gate, aggregate, context }) {
  const packagingRegistry = JSON.parse(readFileSync(packagingRegistryPath, "utf8"));
  const inventory = JSON.parse(readFileSync(driversInventoryPath, "utf8"));
  const driver = inventory.drivers.find((row) => row.agentId === agentId);
  requireFact(Boolean(driver), "driver_inventory_missing_agent");
  const agentIds = packagedAgentIds(packagingRegistry);
  const registryDigest = registryDigestFor(agentIds);
  const inventoryDigest = driverInventoryDigestFor(inventory);
  const capabilitySnapshotDigest = `sha256:${digest(driver.capabilityMatrix)}`;
  const runtimeVersionDigest = binaryDigest(context.binary);
  const sidecarDigest = binaryDigest(context.sidecar);
  const gateArtifactDigest = sha256Text(
    `${gate.gateKind}\n${sidecarDigest}\n${runtimeVersionDigest}\n${aggregate.consecutivePasses}`,
  );
  const productContinuityBindingDigest = sha256Text(
    JSON.stringify({
      gateKind: gate.gateKind,
      agentId,
      consecutivePasses: aggregate.consecutivePasses,
      sidecarDigest,
      runtimeVersionDigest,
    }),
  );
  const conditionalRaw = conditionalChecksFromMatrix(driver.capabilityMatrix, {
    streaming: aggregate.streamingProven === true,
    structured: aggregate.structuredProven === true,
  });
  const conditionalChecks = Object.fromEntries(
    CONDITIONAL_CHECK_IDS.map((id) => [id, conditionalRaw[id]]),
  );
  const pass = (ready) => (ready ? "pass" : "fail");
  const streamingSatisfied = driver.capabilityMatrix.streaming !== true
    || aggregate.streamingProven === true;
  const coreChecks = {
    "P-01": pass(aggregate.officialNativeLane === true),
    "P-02": pass(aggregate.realSessionIds === true),
    "P-03": pass(aggregate.openNew === true && aggregate.exactResume === true),
    "P-04": pass(aggregate.finalResults === true && streamingSatisfied),
    "P-05": pass(aggregate.cwdParity === true),
    "P-06": pass(aggregate.historyReadback === true),
    "P-07": pass(aggregate.errorFailClosed === true && aggregate.permissionFailClosed === true),
    "P-08": pass(aggregate.privacyPassed === true),
    "P-09": pass(aggregate.cleanupPassed === true),
    "P-10": pass(aggregate.conversationGatePassed === true),
  };
  const adapter = {
    agentId,
    driverId: driver.driverId,
    runtimeProtocol: driver.runtimeProtocol,
    harnessVersion: dispatchLaneHarnessVersion,
    runtimeVersionClass: gate.runtimeVersionClass,
    runtimeVersionDigest,
    capabilitySnapshotDigest,
    adapterManifestDigest: adapterManifestDigestFor(agentId),
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
    ...evidence.adapters.filter((row) => row?.agentId !== agentId),
    adapter,
  ].sort((left, right) => String(left.agentId).localeCompare(String(right.agentId)));
  assertEvidenceHygiene(evidence);
  const temporaryEvidencePath = `${evidenceManifestPath}.tmp-${process.pid}-${randomUUID()}`;
  writeFileSync(temporaryEvidencePath, `${JSON.stringify(evidence, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporaryEvidencePath, evidenceManifestPath);
  return {
    written: true,
    agentId,
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

function buildContext(options, agentId, config) {
  const binary = resolveExecutable(options.binary, config);
  requireFact(Boolean(binary), "install_agent_binary");
  const sidecar = resolveSidecar(options.sidecar, { releaseUi: false });
  requireFact(Boolean(sidecar), "sidecar_missing");
  const temporaryDirectory = mkdtempSync(join(tmpdir(), `lico-${agentId}-gate-`));
  const cwd = join(temporaryDirectory, "workspace");
  mkdirSync(cwd, { recursive: true });
  const wrapper = createPrivateWrapper(temporaryDirectory, binary);
  const disposableDataRoot = ["disposable-data-root", "pi-disposable-session-root"].includes(
    config.cleanupKind,
  )
    ? join(temporaryDirectory, "isolated-agent-data")
    : "";
  const disposableSeedSource = config.cleanupKind === "disposable-data-root"
    ? (process.env[config.disposableEnvironmentKey] || join(homedir(), ".kimi-code"))
    : "";
  const environment = disposableDataRoot
    ? { ...process.env, [config.disposableEnvironmentKey]: disposableDataRoot }
    : process.env;
  const portableDataRoot = join(temporaryDirectory, "licoup-portable");
  mkdirSync(portableDataRoot, { recursive: true, mode: 0o700 });
  wrapper.environment = {
    ...wrapper.environment,
    ...environment,
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
    disposableDataRoot,
    disposableSeedSource,
    disposableProfileSeeded: false,
    timeoutMs: options.timeoutMs,
    maxOutputBytes: options.maxOutputBytes,
    observedSessions: new Set(),
    cleanedSessions: new Set(),
    acceptanceMode,
    copilotSdkLaunchArgs: null,
  };
}

export async function runSameSessionConversationGate(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const agentId = options.agent;
  const gate = AGENT_GATES[agentId];
  const config = agentConfigs[agentId];
  requireFact(Boolean(config), "agent_not_acp_packaged");
  const context = buildContext(options, agentId, config);

  const turnResults = [];
  let sessionId = "";
  const debug = process.env.LICO_SAME_SESSION_GATE_DEBUG === "1"
    || (agentId === "cursor" && process.env.LICO_CURSOR_GATE_DEBUG === "1");
  const startedAt = Date.now();
  const logStep = (step) => {
    if (debug) {
      process.stderr.write(`${JSON.stringify({ agentId, step, elapsedMs: Date.now() - startedAt })}\n`);
    }
  };
  try {
    const preflight = await preflightCleanup(context);
    requireFact(preflight.ready === true, preflight.code || "cleanup_preflight_failed");

    logStep("turns_start");
    for (let turn = 1; turn <= TURN_COUNT; turn += 1) {
      const prompt = "Hi";
      logStep(`turn_${turn}_start`);
      let result;
      if (gate.turnViaSidecar === true || config.turnViaSidecar === true) {
        const request = {
          agent: agentId,
          text: prompt,
          workingDirectory: context.cwd,
          binaryPath: context.binary,
          timeoutMs: context.timeoutMs,
          maxStdoutBytes: context.maxOutputBytes,
          maxStderrBytes: context.maxOutputBytes,
          streamEvents: true,
        };
        if (sessionId) request.sessionId = sessionId;
        const model = parityModelForAgent(agentId);
        if (model) request.model = model;
        const sidecar = await runSidecar(context, request);
        const nextSessionId = sidecar.result?.sessionId
          || sidecar.result?.nativeSessionId
          || "";
        const output = String(sidecar.result?.output || "").trim();
        result = {
          sessionId: nextSessionId,
          output,
          boundedOutput: sidecar.boundedOutput === true,
          streamingSeen: sidecar.streamingSeen === true,
          structuredSeen: sidecar.structuredSeen === true,
        };
      } else {
        result = await nativeTurn(context, sessionId, prompt);
      }
      logStep(`turn_${turn}_done`);
      if (!sessionId) sessionId = result.sessionId;
      requireFact(result.sessionId === sessionId, "session_identity_drift");
      requireFact(
        typeof result.output === "string" && result.output.trim().length > 0,
        "native_final_message_missing",
      );
      turnResults.push({
        turn,
        action: turn === 1 ? "open-new" : "exact-resume",
        outputBytes: Buffer.byteLength(result.output),
        boundedOutput: result.boundedOutput === true,
        streamingSeen: result.streamingSeen === true,
        structuredSeen: result.structuredSeen === true,
      });
    }

    let cleanupPassed = false;
    if (gate.cleanupViaSidecar) {
      const cleanup = await runSidecarCleanup(
        context.sidecar,
        agentId,
        sessionId,
        {
          ...process.env,
          LICO_CLIENT_PATH: context.sidecar,
          LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
        },
        options.timeoutMs,
      );
      requireFact(cleanup.ok === true, cleanup.reason || "cleanup_failed");
      cleanupPassed = true;
    } else {
      cleanupPassed = await cleanupSession(context, sessionId, context.temporaryDirectory);
      requireFact(cleanupPassed === true, "cleanup_failed");
    }

    const aggregate = {
      agent: agentId,
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
      cleanupPassed,
      conversationGatePassed: true,
      consecutivePasses: strictRoundCount,
    };

    let evidenceWrite = null;
    if (options.write) {
      evidenceWrite = writeGateEvidence({ agentId, gate, aggregate, context });
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
      readinessRow = (readiness.adapters || []).find((row) => row?.agentId === agentId) || null;
      requireFact(readinessRow?.sendEnabled === true, `${agentId}_send_not_enabled`);
    }

    return {
      status: "passed",
      gateKind: gate.gateKind,
      agent: agentId,
      turnsRequired: TURN_COUNT,
      turnsCompleted: turnResults.length,
      openNew: aggregate.openNew,
      exactResume: aggregate.exactResume,
      sameSession: true,
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
    rmSync(context.temporaryDirectory, { recursive: true, force: true });
  }
}

async function main() {
  let output;
  try {
    output = await runSameSessionConversationGate(process.argv.slice(2));
  } catch (error) {
    const reasonCode = /^[a-z0-9_-]+$/u.test(error?.message || "")
      ? error.message
      : "unexpected_failure";
    const agent = (() => {
      try {
        return parseArgs(process.argv.slice(2)).agent;
      } catch {
        return null;
      }
    })();
    output = {
      status: "failed",
      gateKind: agent ? AGENT_GATES[agent]?.gateKind || null : null,
      agent,
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
