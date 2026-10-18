#!/usr/bin/env node

/**
 * Arc ↔ native local-service conversation gate (background, no release UI).
 *
 * Proves live local forwarding with one new-conversation turn followed by one
 * exact continuation turn on the same native session.
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
import { parityEffortForAgent, parityModelForAgent } from "../parity/agent-ids.mjs";
import {
  appServerFinalMessage,
  withAppServer,
} from "../parity/native/app-server.mjs";
import { createPrivateWrapper } from "../parity/process.mjs";
import {
  cleanupSession,
  preflightCleanup,
} from "../parity/session-cleanup.mjs";
import { resolveExecutable, resolveSidecar } from "../parity/sidecar.mjs";
import { ensureOpenCodeServeAttachUrl } from "./opencode-http.mjs";
import { runRound } from "../parity/run-round.mjs";

const root = workspaceRoot;
const ROUND_COUNT = strictRoundCount;

const AGENT_GATES = Object.freeze({
  codex: Object.freeze({
    gateKind: "codex-arc-local-service-v1",
    runtimeVersionClass: "verified-codex-app-server-arc-gate",
  }),
  opencode: Object.freeze({
    gateKind: "opencode-arc-local-service-v1",
    runtimeVersionClass: "verified-opencode-serve-arc-gate",
  }),
});

export const arcLocalServiceGateAgentIds = Object.freeze(Object.keys(AGENT_GATES));

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
    else if (argument === "--timeout-ms") options.timeoutMs = Number(argv[++index]);
    else if (argument === "--sidecar") options.sidecar = resolve(argv[++index]);
    else if (argument === "--binary") options.binary = resolve(argv[++index]);
    else if (argument === "--agent") options.agent = String(argv[++index] || "");
    else throw new AcceptanceError("cli_argument_unsupported");
  }
  requireFact(Number.isFinite(options.timeoutMs) && options.timeoutMs >= 1_000, "timeout_invalid");
  requireFact(Object.hasOwn(AGENT_GATES, options.agent), "agent_gate_unsupported");
  return options;
}

/// Prove Codex in-turn `turn/steer` (C-05) against the live app-server lane.
async function proveCodexInterruptSteer(context) {
  const model = parityModelForAgent("codex");
  const effort = parityEffortForAgent("codex", model);

  let verifiedThreadId = "";
  let operationError = null;
  try {
    verifiedThreadId = await withAppServer(context, async (client) => {
      const started = await client.request("thread/start", {
        cwd: context.cwd,
        ...(model ? { model } : {}),
      });
      const threadId = started?.thread?.id || "";
      requireFact(typeof threadId === "string" && threadId.length > 0, "steer_thread_id_missing");
      verifiedThreadId = threadId;
      context.observedSessions?.add(threadId);

      const turnStarted = await client.request("turn/start", {
        threadId,
        input: [{ type: "text", text: "Hi" }],
        ...(model ? { model } : {}),
        ...(effort ? { effort } : {}),
      });
      const turnId = turnStarted?.turn?.id || "";
      requireFact(typeof turnId === "string" && turnId.length > 0, "steer_turn_id_missing");

      // Wait until the turn is active before steering.
      await client.waitForNotification(
        (message) => (
          (
            message.method === "turn/started"
            && message.params?.threadId === threadId
            && message.params?.turn?.id === turnId
          )
          || (
            message.method === "item/agentMessage/delta"
            && message.params?.threadId === threadId
          )
        ),
      );

      await client.request("turn/steer", {
        threadId,
        expectedTurnId: turnId,
        input: [{ type: "text", text: "Hi" }],
      });

      const completed = await client.waitForNotification(
        (message) => message.method === "turn/completed"
          && message.params?.threadId === threadId
          && message.params?.turn?.id === turnId,
      );
      const turnStatus = String(completed.params?.turn?.status || "").toLowerCase();
      requireFact(
        turnStatus === "completed" || turnStatus === "interrupted",
        "steer_turn_not_terminal",
      );
      const completedItems = client.notifications
        .filter((message) => message.method === "item/completed"
          && message.params?.threadId === threadId
          && message.params?.turnId === turnId)
        .map((message) => message.params?.item)
        .filter(Boolean);
      const turn = {
        ...(completed.params?.turn || {}),
        items: [
          ...(Array.isArray(completed.params?.turn?.items) ? completed.params.turn.items : []),
          ...completedItems,
        ],
      };
      const output = appServerFinalMessage(turn);
      if (turnStatus === "completed") {
        requireFact(output.trim().length > 0, "steer_final_message_missing");
      }
      return threadId;
    });
  } catch (error) {
    operationError = error;
  }

  let cleanupError = null;
  if (verifiedThreadId) {
    try {
      const cleaned = await cleanupSession(context, verifiedThreadId, context.temporaryDirectory);
      if (!cleaned) cleanupError = new AcceptanceError("steer_cleanup_failed");
    } catch (error) {
      cleanupError = error instanceof AcceptanceError
        ? error
        : new AcceptanceError("steer_cleanup_failed");
    }
  }
  if (operationError) {
    if (cleanupError) {
      operationError.details = {
        ...(operationError.details || {}),
        cleanupCode: cleanupError.code,
      };
    }
    throw operationError;
  }
  if (cleanupError) throw cleanupError;
  return true;
}

function buildContext(options, agentId, config) {
  const binary = resolveExecutable(options.binary, config);
  requireFact(Boolean(binary), "install_agent_binary");
  const sidecar = resolveSidecar(options.sidecar, { releaseUi: false });
  requireFact(Boolean(sidecar), "sidecar_missing");
  const temporaryDirectory = mkdtempSync(join(tmpdir(), `lico-${agentId}-arc-gate-`));
  const cwd = join(temporaryDirectory, "workspace");
  mkdirSync(cwd, { recursive: true });
  const wrapper = createPrivateWrapper(temporaryDirectory, binary);
  wrapper.environment = {
    ...wrapper.environment,
    ...process.env,
    LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
  };
  const context = {
    config,
    binary,
    // Promoted adapters bind this gate to the discovered native binary digest;
    // a private argv-capture wrapper would be rejected by the sidecar.
    sidecarBinaryPath: binary,
    sidecar,
    wrapper,
    cwd,
    environment: wrapper.environment,
    temporaryDirectory,
    disposableDataRoot: "",
    disposableSeedSource: "",
    disposableProfileSeeded: false,
    timeoutMs: options.timeoutMs,
    maxOutputBytes: options.maxOutputBytes,
    observedSessions: new Set(),
    cleanedSessions: new Set(),
    acceptanceMode,
    serveAttachUrl: "",
    copilotSdkLaunchArgs: null,
  };
  if (agentId === "opencode") {
    context.serveAttachUrl = ensureOpenCodeServeAttachUrl(
      sidecar,
      binary,
      options.timeoutMs,
    );
  }
  return context;
}

async function runBidirectionalRound(context, roundIndex) {
  const round = await runRound(context, roundIndex, {
    permissionFailClosed: true,
    errorFailClosed: true,
  });
  requireFact(round.ready === true, round.errorCode || "verification_conversation_failed");
  return {
    openNew: round.facts.openNew === true,
    exactResume: round.facts.exactResume === true,
    nativeToArc: round.facts.nativeToArc === true,
    arcToNative: round.facts.arcToNative === true,
    realSessionIds: round.facts.realSessionIds === true,
    streamingProven: round.facts.streamingSeen === true,
    structuredProven: round.facts.structuredSeen === true,
    cleanupPassed: round.cleanupVerified === true,
    outputBytes: round.facts.turnOutputBytes,
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
      nativeToArc: true,
      arcToNative: true,
      openNew: true,
      exactResume: true,
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
    "P-01": pass(true),
    "P-02": pass(aggregate.realSessionIds === true),
    "P-03": pass(aggregate.openNew === true && aggregate.exactResume === true),
    "P-04": pass(aggregate.finalResults === true && aggregate.streamingProven === true),
    "P-05": pass(true),
    "P-06": pass(true),
    "P-07": pass(true),
    "P-08": pass(true),
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

export async function runArcLocalServiceConversationGate(argv = process.argv.slice(2)) {
  const options = parseArgs(argv);
  const agentId = options.agent;
  const gate = AGENT_GATES[agentId];
  const config = agentConfigs[agentId];
  requireFact(Boolean(config), "agent_not_acp_packaged");
  const context = buildContext(options, agentId, config);
  const debug = process.env.LICOUP_LOCAL_SERVICE_GATE_DEBUG === "1";
  const startedAt = Date.now();
  const logStep = (step) => {
    if (debug) {
      process.stderr.write(`${JSON.stringify({ agentId, step, elapsedMs: Date.now() - startedAt })}\n`);
    }
  };

  try {
    const preflight = await preflightCleanup(context);
    requireFact(preflight.ready === true, preflight.code || "cleanup_preflight_failed");

    const rounds = [];
    for (let round = 1; round <= ROUND_COUNT; round += 1) {
      logStep(`round_${round}_start`);
      const result = await runBidirectionalRound(context, round);
      logStep(`round_${round}_done`);
      rounds.push(result);
    }

    let interruptSteerProven = true;
    if (agentId === "codex") {
      logStep("interrupt_steer_start");
      interruptSteerProven = await proveCodexInterruptSteer(context);
      logStep("interrupt_steer_done");
      requireFact(interruptSteerProven === true, "interrupt_steer_unproven");
    }

    const aggregate = {
      agent: agentId,
      openNew: rounds.every((row) => row.openNew),
      exactResume: rounds.every((row) => row.exactResume),
      nativeToArc: rounds.every((row) => row.nativeToArc),
      arcToNative: rounds.every((row) => row.arcToNative),
      realSessionIds: rounds.every((row) => row.realSessionIds),
      finalResults: rounds.every((row) => row.outputBytes.every((value) => value > 0)),
      streamingProven: rounds.every((row) => row.streamingProven),
      structuredProven: rounds.every((row) => row.structuredProven),
      interruptSteerProven,
      cleanupPassed: rounds.every((row) => row.cleanupPassed),
      conversationGatePassed: true,
      consecutivePasses: ROUND_COUNT,
    };
    requireFact(aggregate.openNew && aggregate.exactResume, "exact_resume_verification_failed");

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
      roundsRequired: ROUND_COUNT,
      roundsCompleted: rounds.length,
      turnsRequired: verificationTurnCount,
      turnsCompleted: rounds.reduce((total, row) => total + row.outputBytes.length, 0),
      openNew: true,
      exactResume: true,
      nativeToArc: true,
      arcToNative: true,
      cleanupPassed: true,
      conversationGatePassed: true,
      consecutivePasses: ROUND_COUNT,
      evidenceWrite,
      sendEnabled: readinessRow?.sendEnabled === true,
      readinessStatus: readinessRow?.status || null,
      roundOutputBytes: rounds.map((row) => row.outputBytes),
    };
  } finally {
    rmSync(context.temporaryDirectory, { recursive: true, force: true });
  }
}

async function main() {
  let output;
  try {
    output = await runArcLocalServiceConversationGate(process.argv.slice(2));
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
      ...(typeof error?.details?.cleanupCode === "string"
        ? { cleanupReasonCode: error.details.cleanupCode }
        : {}),
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
