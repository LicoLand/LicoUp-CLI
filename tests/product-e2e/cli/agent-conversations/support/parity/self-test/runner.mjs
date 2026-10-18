import {
  chmodSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { printLiveGateChecklist, readyCandidateAgentIds } from "../live-gate.mjs";
import { acceptanceMode, agentConfigs, dispatchLaneHarnessVersion, driversInventoryPath, strictRoundCount } from "../constants.mjs";
import { AcceptanceError, digest } from "../errors.mjs";
import {
  conditionalChecksFromMatrix,
  coreChecksFromAggregate,
  createSanitizedSelfTestEvidenceReceipt,
  writeReleaseUiAdapterEvidence,
} from "../evidence.mjs";
import { nativeReadback, nativeTurn } from "../native/acp-turn.mjs";
import { nativePiReadback, nativePiTurn } from "../native/pi.mjs";
import { validateProductReceipt } from "../packaging.mjs";
import {
  createPrivateWrapper,
  scanBoundedNoFollow,
  seedDisposableProfile,
} from "../process.mjs";
import {
  failedParityFactCode,
  processLocalBooleanFactKeys,
  processLocalRoundFactsReady,
  roundFactsReady,
} from "../round-facts.mjs";
import { aggregateProcessLocalResult, aggregateResult, blockedResult } from "../results.mjs";
import { runRound } from "../run-round.mjs";
import { fakeRuntimeSource } from "./fake-runtime.mjs";
import { cleanupSession, preflightCleanup } from "../session-cleanup.mjs";
import { listSessions } from "../session-query.mjs";
import { probeDispatchLaneFamilies } from "../sidecar.mjs";
import { StdioRpcClient } from "../clients/stdio-rpc-client.mjs";
import {
  exerciseProcessLocalHostDrain,
  runProcessLocalRound,
  strictHistoryProjection,
} from "../process-local-round.mjs";
import { adapterEvidenceDigestFor } from "../../reducer-facade.mjs";
import {
  exerciseMalformedBeforePromptResponse,
  exerciseQuiescenceOracle,
  promptQuiescenceBudgetContract,
  publicStreamChunkOracleContract,
  sessionUpdateOracleContract,
} from "./acp-oracles.mjs";

function recomputedAggregateEvidenceDigest(aggregate) {
  return digest({ ...aggregate, evidenceDigest: undefined });
}

function processLocalSendParams(context, text, sessionId = "") {
  return {
    agent: "claude-code",
    text,
    ...(sessionId ? { sessionId } : {}),
    workingDirectory: context.cwd,
    binaryPath: context.wrapper.wrapperPath,
    model: context.parityModel,
    acceptanceMode: context.acceptanceMode,
    streamEvents: true,
    timeoutMs: context.timeoutMs,
    maxStdoutBytes: context.maxOutputBytes,
    maxStderrBytes: context.maxOutputBytes,
  };
}

async function processLocalStreamFailure(context, executable, prompt, expectedCode) {
  const client = new StdioRpcClient(executable, context);
  try {
    await client.connect();
    await client.streamConversation(processLocalSendParams(context, prompt));
    return false;
  } catch (error) {
    return error instanceof AcceptanceError && error.code === expectedCode;
  } finally {
    await client.abort();
  }
}

function releaseUiProcessLocalAggregate(aggregate) {
  const released = {
    ...aggregate,
    status: "release-ui-passed",
    conversationGatePassed: true,
    consecutivePasses: strictRoundCount,
    productArtifactDigest: `sha256:${digest("process-local-self-test-artifact")}`,
    productContinuityBindingDigest:
      `sha256:${digest("process-local-self-test-continuity")}`,
    evidenceDigest: "",
  };
  released.evidenceDigest = recomputedAggregateEvidenceDigest(released);
  return released;
}

function armProcessLocalCloseGate(directory, markerPath, gatePath) {
  return new Promise((resolveGate) => {
    let settled = false;
    let timeout;
    let interval;
    const finish = (released) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      clearInterval(interval);
      resolveGate(released);
    };
    const inspect = () => {
      const rows = existsSync(markerPath)
        ? readFileSync(markerPath, "utf8").split(/\r?\n/u)
        : [];
      if (!rows.includes("child_waiting_for_close_gate")
        || !rows.includes("descendant_pipe_open")) return;
      writeFileSync(gatePath, "release", { mode: 0o600 });
      finish(true);
    };
    interval = setInterval(inspect, 5);
    timeout = setTimeout(() => finish(false), 5000);
    inspect();
  });
}

export async function exerciseFailClosed(context, prompt, expectedCode) {
  let sessionId = "";
  let code = "";
  try {
    const result = await nativeTurn(context, "", prompt);
    sessionId = result.sessionId;
  } catch (error) {
    code = error instanceof AcceptanceError ? error.code : "unexpected_failure";
  }
  let records = await listSessions(context);
  if (!sessionId) {
    for (const [id, record] of records) {
      if (record.includes(prompt)) sessionId = id;
    }
  }
  const cleanup = sessionId
    ? await cleanupSession(context, sessionId, context.temporaryDirectory)
    : false;
  records = await listSessions(context);
  return code === expectedCode && cleanup && !records.has(sessionId);
}

export async function runSelfTest() {
  const temporaryDirectory = mkdtempSync(join(tmpdir(), "lico-acp-parity-selftest-"));
  const fakeBinary = join(temporaryDirectory, "fake-runtime");
  const statePath = join(temporaryDirectory, "state.json");
  try {
    const isolatedWorkingDirectory = join(temporaryDirectory, "workspace");
    mkdirSync(isolatedWorkingDirectory, { recursive: true });
    writeFileSync(fakeBinary, fakeRuntimeSource, { mode: 0o700 });
    chmodSync(fakeBinary, 0o700);
    writeFileSync(statePath, JSON.stringify({ counter: 0, sessions: {} }));
    const fixtureProductReceiptPath = join(temporaryDirectory, "fixture-product-receipt.json");
    writeFileSync(fixtureProductReceiptPath, JSON.stringify({
      schemaVersion: "fixture",
      status: "passed",
      receiptKind: "fixture",
      fixtureBackend: true,
    }), { mode: 0o600 });
    let productFixtureReceiptRejected = false;
    try {
      validateProductReceipt(fixtureProductReceiptPath, "codex");
    } catch (error) {
      productFixtureReceiptRejected = error instanceof AcceptanceError;
    }
    const seedSource = join(temporaryDirectory, "disposable-seed-source");
    const seedTarget = join(temporaryDirectory, "disposable-seed-target");
    mkdirSync(join(seedSource, "credentials"), { recursive: true, mode: 0o700 });
    mkdirSync(join(seedSource, "sessions"), { recursive: true, mode: 0o700 });
    writeFileSync(join(seedSource, "config.toml"), "[self_test]\n", { mode: 0o600 });
    writeFileSync(join(seedSource, "credentials", "account"), "self-test", { mode: 0o600 });
    writeFileSync(join(seedSource, "sessions", "must-not-copy"), "private", { mode: 0o600 });
    const disposableProfileSeedSafe = seedDisposableProfile({
      temporaryDirectory,
      disposableDataRoot: seedTarget,
      disposableSeedSource: seedSource,
    })
      && existsSync(join(seedTarget, "config.toml"))
      && existsSync(join(seedTarget, "credentials", "account"))
      && !existsSync(join(seedTarget, "sessions"));
    rmSync(seedTarget, { recursive: true, force: true });
    const environment = {
      ...process.env,
      LICO_FAKE_ACP_STATE: statePath,
      LICO_FAKE_ACP_RESPONSE_FIRST: "1",
    };
    const wrapper = createPrivateWrapper(temporaryDirectory, fakeBinary);
    wrapper.environment = { ...wrapper.environment, ...environment };
    const context = {
      config: agentConfigs.opencode,
      binary: fakeBinary,
      sidecar: fakeBinary,
      wrapper,
      cwd: isolatedWorkingDirectory,
      environment,
      temporaryDirectory,
      timeoutMs: 60_000,
      maxOutputBytes: 32 * 1024,
      // The fixture lane is deliberately independent of the live model
      // authority; it must never invent a provider model when testing wiring.
      parityModel: "synthetic-verification-model",
      copilotSdkLaunchArgs: null,
    };
    const quiescenceOraclePassed = await exerciseQuiescenceOracle(context);
    const publicStreamChunkOraclePassed = publicStreamChunkOracleContract();
    const evidenceSeed = {
      permissionFailClosed: true,
      errorFailClosed: true,
      boundedOutputFailClosed: true,
      quiescenceOraclePassed,
      publicStreamChunkOraclePassed,
    };
    const rounds = [];
    for (let index = 0; index < strictRoundCount; index += 1) {
      rounds.push(await runRound(context, index + 1, evidenceSeed));
    }
    const strictReducer = aggregateResult("opencode", true, true, rounds, evidenceSeed);
    const fakeLiveStreamingGate = strictReducer.streamingEvidenceComplete === true
      && strictReducer.streamingProven === true
      && rounds.length === strictRoundCount
      && rounds.every((round) => round.facts?.streamingSeen === true);
    const liveRoundOrderingAgnostic = rounds.length === strictRoundCount
      && rounds.every((round) => round.ready === true
        && round.conversationReady === true);
    const verificationWorkflowContract = rounds.length === 1
      && rounds[0].requestCount === 2
      && rounds[0].successfulRequestCount === 2
      && rounds[0].testedSessions === 1
      && rounds[0].facts?.openNew === true
      && rounds[0].facts?.exactResume === true
      && rounds[0].facts?.turnOutputBytes?.length === 2;
    const driftWrapper = {
      ...wrapper,
      environment: {
        ...wrapper.environment,
        LICO_FAKE_RESUME_ID_DRIFT: "1",
      },
    };
    const identityDriftRound = await runRound({
      ...context,
      wrapper: driftWrapper,
      environment: driftWrapper.environment,
      observedSessions: new Set(),
      cleanedSessions: new Set(),
    }, 2, evidenceSeed);
    const exactResumeIdentityFailClosed = identityDriftRound.ready === false
      && identityDriftRound.requestCount === 2
      && identityDriftRound.errorCode === "exact_resume_session_mismatch";
    const sessionUpdateOracle = sessionUpdateOracleContract();
    const promptQuiescenceBudget = promptQuiescenceBudgetContract();
    const permissionFailClosed = await exerciseFailClosed(
      context,
      "SELFTEST_PERMISSION",
      "acp_permission_required",
    );
    const boundedOutputFailClosed = await exerciseFailClosed(
      { ...context, maxOutputBytes: 8 * 1024 },
      "SELFTEST_OVERFLOW",
      "acp_stdout_limit",
    );
    const malformedEnvelopeFailClosed = await exerciseFailClosed(
      context,
      "SELFTEST_MALFORMED_UPDATE_ENVELOPE",
      "acp_notification_envelope_invalid",
    );
    const malformedContentFailClosed = await exerciseFailClosed(
      context,
      "SELFTEST_MALFORMED_CONTENT",
      "acp_session_update_invalid",
    );
    const malformedBeforePromptResponseFailClosed =
      await exerciseMalformedBeforePromptResponse(context);
    const failedFacts = { ...rounds[0].facts, nativeToArc: false };
    const errorFailClosed = !roundFactsReady(failedFacts);
    const factFailureCode = failedParityFactCode(failedFacts) === "parity_native_to_arc_failed";
    const cleanupBlocked = blockedResult(
      "openclaw",
      false,
      true,
      "session_cleanup_interface_unavailable",
      evidenceSeed,
    ).status === "blocked";
    const copilotState = JSON.parse(readFileSync(statePath, "utf8"));
    copilotState.sessions["fake-copilot-cleanup"] = { cwd: isolatedWorkingDirectory, messages: [] };
    writeFileSync(statePath, JSON.stringify(copilotState));
    const copilotContext = {
      ...context,
      config: agentConfigs.copilot,
      copilotSdkLaunchArgs: null,
    };
    const copilotPreflight = await preflightCleanup(copilotContext);
    const copilotCleanupRpc = copilotPreflight.ready
      && await cleanupSession(copilotContext, "fake-copilot-cleanup", temporaryDirectory);
    const openclawState = JSON.parse(readFileSync(statePath, "utf8"));
    openclawState.sessions["fake-openclaw-cleanup"] = {
      cwd: isolatedWorkingDirectory,
      messages: [],
    };
    writeFileSync(statePath, JSON.stringify(openclawState));
    const openclawContext = {
      ...context,
      config: agentConfigs.openclaw,
    };
    const openclawPreflight = await preflightCleanup(openclawContext);
    const openclawCleanupRpc = openclawPreflight.ready
      && await cleanupSession(
        openclawContext,
        "fake-openclaw-cleanup",
        temporaryDirectory,
      );
    const kiloState = JSON.parse(readFileSync(statePath, "utf8"));
    kiloState.sessions["fake-kilo-cleanup"] = {
      cwd: isolatedWorkingDirectory,
      messages: [{ role: "user", text: "kilo" }],
    };
    writeFileSync(statePath, JSON.stringify(kiloState));
    const kiloContext = {
      ...context,
      config: agentConfigs["kilo-code"],
    };
    const kiloPreflight = await preflightCleanup(kiloContext);
    const kiloCleanupCli = kiloPreflight.ready
      && await cleanupSession(kiloContext, "fake-kilo-cleanup", temporaryDirectory);
    const hermesState = JSON.parse(readFileSync(statePath, "utf8"));
    hermesState.sessions["fake-session-hermes"] = {
      cwd: isolatedWorkingDirectory,
      messages: [{ role: "user", text: "hermes" }],
    };
    writeFileSync(statePath, JSON.stringify(hermesState));
    const hermesContext = {
      ...context,
      config: agentConfigs.hermes,
    };
    const hermesPreflight = await preflightCleanup(hermesContext);
    const hermesListed = hermesPreflight.ready ? await listSessions(hermesContext) : new Map();
    const hermesStamp = [...hermesListed.keys()][0] || "";
    const hermesCleanupCli = hermesPreflight.ready
      && hermesStamp.length > 0
      && await cleanupSession(hermesContext, hermesStamp, temporaryDirectory);
    const kimiDisposableRoot = join(temporaryDirectory, "kimi-isolated");
    const kimiContext = {
      ...context,
      config: agentConfigs["kimi-code"],
      disposableDataRoot: kimiDisposableRoot,
      disposableSeedSource: seedSource,
      cleanedSessions: new Set(),
      observedSessions: new Set(["fake-kimi-session"]),
    };
    const kimiPreflight = await preflightCleanup(kimiContext);
    const kimiCleanup = kimiPreflight.ready
      && await cleanupSession(kimiContext, "fake-kimi-session", temporaryDirectory);
    const piDisposableRoot = join(temporaryDirectory, "pi-isolated-sessions");
    const piEnvironment = {
      ...environment,
      PI_CODING_AGENT_SESSION_DIR: piDisposableRoot,
    };
    const piWrapper = {
      wrapperPath: fakeBinary,
      capturePath: join(temporaryDirectory, "pi-argv-capture"),
      environment: {
        ...environment,
        ...piEnvironment,
      },
    };
    const piContext = {
      ...context,
      config: agentConfigs.pi,
      wrapper: piWrapper,
      environment: piEnvironment,
      disposableDataRoot: piDisposableRoot,
      disposableSeedSource: "",
      cleanedSessions: new Set(),
      observedSessions: new Set(),
    };
    const piPreflight = await preflightCleanup(piContext);
    let piRpcExactResume = false;
    let piCleanup = false;
    if (piPreflight.ready) {
      try {
        const first = await nativePiTurn(
          piContext,
          "",
          "Hi",
        );
        const resumed = await nativePiTurn(
          piContext,
          first.sessionId,
          "Hi",
        );
        const readback = await nativePiReadback(piContext, first.sessionId);
        const firstOutput = String(first.output || "");
        const resumedOutput = String(resumed.output || "");
        piRpcExactResume = resumed.sessionId === first.sessionId
          && firstOutput.trim().length > 0
          && resumedOutput.trim().length > 0
          && readback.text.includes(firstOutput)
          && readback.text.includes(resumedOutput);
        piCleanup = await cleanupSession(piContext, first.sessionId, temporaryDirectory)
          && (await listSessions(piContext)).size === 0;
        // The fixture keeps its synthetic protocol state in the shared fake
        // runtime ledger in addition to the disposable Pi session files. Real
        // Pi has no such ledger; remove only fixture-tagged rows here.
        const piFixtureState = JSON.parse(readFileSync(statePath, "utf8"));
        for (const sessionId of Object.keys(piFixtureState.sessions)) {
          if (sessionId.startsWith("fake-pi-")) delete piFixtureState.sessions[sessionId];
        }
        writeFileSync(statePath, JSON.stringify(piFixtureState));
      } catch {
        piRpcExactResume = false;
        piCleanup = false;
      }
    }
    const codexContext = {
      ...context,
      config: agentConfigs.codex,
      cleanedSessions: new Set(),
      observedSessions: new Set(),
    };
    const codexPreflight = await preflightCleanup(codexContext);
    let codexRoundReady = false;
    let codexCleanupRpc = false;
    if (codexPreflight.ready) {
      const codexRound = await runRound(codexContext, 1, evidenceSeed);
      codexRoundReady = codexRound.ready === true
        && codexRound.requestCount === 2
        && codexRound.testedSessions === 1
        && codexRound.facts?.openNew === true
        && codexRound.facts?.exactResume === true
        && codexRound.facts?.nativeToArc === true
        && codexRound.facts?.arcToNative === true
        && codexRound.cleanupVerified === true;
      codexCleanupRpc = codexRound.cleanupVerified === true;
    }
    const claudeRoot = join(temporaryDirectory, "claude-process-local");
    const claudeConfigRoot = join(claudeRoot, "config");
    const claudeMarker = join(claudeRoot, "lifecycle.log");
    const claudeCloseGate = join(claudeRoot, "close.gate");
    mkdirSync(claudeConfigRoot, { recursive: true, mode: 0o700 });
    const claudeWrapper = createPrivateWrapper(claudeRoot, fakeBinary);
    const claudeEnvironment = {
      ...environment,
      ...claudeWrapper.environment,
      CLAUDE_CONFIG_DIR: claudeConfigRoot,
      CLAUDE_CODE_SKIP_PROMPT_HISTORY: "1",
      LICO_FAKE_REQUIRE_NO_HISTORY: "1",
      LICO_FAKE_CLAUDE_RETAIN_DESCENDANT: "1",
      LICO_FAKE_CLAUDE_CLOSE_GATE: claudeCloseGate,
      LICO_FAKE_PROCESS_LOCAL_MARKER: claudeMarker,
      LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
    };
    claudeWrapper.environment = claudeEnvironment;
    const claudeContext = {
      ...context,
      config: agentConfigs["claude-code"],
      sidecar: fakeBinary,
      wrapper: claudeWrapper,
      environment: claudeEnvironment,
      claudeConfigRoot,
      acceptanceMode,
      parityModel: "fixture-forwarded-model",
      armProcessLocalCleanupGate: () => armProcessLocalCloseGate(
        claudeRoot,
        claudeMarker,
        claudeCloseGate,
      ),
      processLocalCleanupProbe: () => {
        const rows = existsSync(claudeMarker)
          ? readFileSync(claudeMarker, "utf8").trim().split(/\r?\n/u)
          : [];
        const started = rows.indexOf("cleanup_started");
        const waiting = rows.indexOf("child_waiting_for_close_gate");
        const descendantOpen = rows.indexOf("descendant_pipe_open");
        const descendantClosed = rows.indexOf("descendant_pipe_closed");
        const closed = rows.indexOf("child_closed");
        const ioJoined = rows.indexOf("io_workers_joined");
        const acknowledged = rows.indexOf("cleanup_ack");
        return started >= 0
          && descendantOpen >= 0
          && waiting > started
          && descendantClosed > descendantOpen
          && closed > waiting
          && ioJoined > Math.max(descendantClosed, closed)
          && acknowledged > ioJoined;
      },
    };
    const processLocalEvidenceSeed = {
      ...evidenceSeed,
      processLocalOraclePassed: true,
    };
    const claudeClient = new StdioRpcClient(fakeBinary, claudeContext);
    let claudeProcessLocalRound = null;
    let claudeHostShutdown = false;
    let claudeHostShutdownErrorCode = null;
    try {
      await claudeClient.connect();
      claudeProcessLocalRound = await runProcessLocalRound(
        claudeContext,
        1,
        claudeClient,
        processLocalEvidenceSeed,
      );
      const shutdown = await claudeClient.shutdown();
      claudeHostShutdown = shutdown.acknowledged === true
        && shutdown.exited === true
        && shutdown.statusCode === 0;
    } catch (error) {
      await claudeClient.abort();
      claudeHostShutdownErrorCode = error instanceof AcceptanceError
        ? error.code
        : "process_local_host_shutdown_failed";
      claudeHostShutdown = false;
    }
    const lifecycleRows = existsSync(claudeMarker)
      ? readFileSync(claudeMarker, "utf8").trim().split(/\r?\n/u)
      : [];
    const cleanupStartedIndex = lifecycleRows.indexOf("cleanup_started");
    const childWaitingIndex = lifecycleRows.indexOf("child_waiting_for_close_gate");
    const descendantOpenIndex = lifecycleRows.indexOf("descendant_pipe_open");
    const descendantClosedIndex = lifecycleRows.indexOf("descendant_pipe_closed");
    const childClosedIndex = lifecycleRows.indexOf("child_closed");
    const ioJoinedIndex = lifecycleRows.indexOf("io_workers_joined");
    const cleanupAckIndex = lifecycleRows.indexOf("cleanup_ack");
    const claudeCleanupSynchronized = cleanupStartedIndex >= 0
      && descendantOpenIndex >= 0
      && childWaitingIndex > cleanupStartedIndex
      && descendantClosedIndex > descendantOpenIndex
      && childClosedIndex > childWaitingIndex
      && ioJoinedIndex > Math.max(descendantClosedIndex, childClosedIndex)
      && cleanupAckIndex > ioJoinedIndex;

    const exerciseDrain = async (termination) => {
      const root = join(temporaryDirectory, `claude-${termination}-drain`);
      const configRoot = join(root, "config");
      const markerPath = join(root, "lifecycle.log");
      const closeGate = join(root, "close.gate");
      mkdirSync(configRoot, { recursive: true, mode: 0o700 });
      const privateWrapper = createPrivateWrapper(root, fakeBinary);
      const drainEnvironment = {
        ...environment,
        ...privateWrapper.environment,
        CLAUDE_CONFIG_DIR: configRoot,
        CLAUDE_CODE_SKIP_PROMPT_HISTORY: "1",
        LICO_FAKE_CLAUDE_RETAIN_DESCENDANT: "1",
        LICO_FAKE_CLAUDE_CLOSE_GATE: closeGate,
        LICO_FAKE_PROCESS_LOCAL_MARKER: markerPath,
        LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
      };
      privateWrapper.environment = drainEnvironment;
      const drainContext = {
        ...claudeContext,
        wrapper: privateWrapper,
        environment: drainEnvironment,
        claudeConfigRoot: configRoot,
        armProcessLocalCleanupGate: () => armProcessLocalCloseGate(
          root,
          markerPath,
          closeGate,
        ),
      };
      const client = new StdioRpcClient(fakeBinary, drainContext);
      try {
        await client.connect();
        const drained = await exerciseProcessLocalHostDrain(
          drainContext,
          client,
          termination,
        );
        const rows = existsSync(markerPath)
          ? readFileSync(markerPath, "utf8").trim().split(/\r?\n/u)
          : [];
        const closedIndex = rows.indexOf("child_closed");
        const waitingIndex = rows.indexOf("child_waiting_for_close_gate");
        const descendantOpenIndex = rows.indexOf("descendant_pipe_open");
        const descendantClosedIndex = rows.indexOf("descendant_pipe_closed");
        const ioJoinedIndex = rows.indexOf("io_workers_joined");
        const terminalIndex = rows.indexOf(
          termination === "shutdown" ? "shutdown_ack" : "eof_drained",
        );
        return drained
          && waitingIndex >= 0
          && descendantOpenIndex >= 0
          && descendantClosedIndex > descendantOpenIndex
          && closedIndex > waitingIndex
          && ioJoinedIndex > Math.max(descendantClosedIndex, closedIndex)
          && terminalIndex > ioJoinedIndex;
      } catch {
        await client.abort();
        return false;
      }
    };
    const claudeShutdownDrain = await exerciseDrain("shutdown");
    const claudeEofDrain = await exerciseDrain("eof");
    const sequenceRoot = join(temporaryDirectory, "claude-sequence-negative");
    const sequenceConfigRoot = join(sequenceRoot, "config");
    mkdirSync(sequenceConfigRoot, { recursive: true, mode: 0o700 });
    const sequenceWrapper = createPrivateWrapper(sequenceRoot, fakeBinary);
    const sequenceEnvironment = {
      ...environment,
      ...sequenceWrapper.environment,
      CLAUDE_CONFIG_DIR: sequenceConfigRoot,
      CLAUDE_CODE_SKIP_PROMPT_HISTORY: "1",
      LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
    };
    sequenceWrapper.environment = sequenceEnvironment;
    const sequenceContext = {
      ...claudeContext,
      wrapper: sequenceWrapper,
      environment: sequenceEnvironment,
      claudeConfigRoot: sequenceConfigRoot,
    };
    const protocolMutationCases = [
      ["SELFTEST_PROCESS_LOCAL_SEQUENCE", "stdio_rpc_sequence_invalid"],
      ["SELFTEST_PROCESS_LOCAL_MISSING_SESSION", "stdio_rpc_event_session_id_invalid"],
      ["SELFTEST_PROCESS_LOCAL_MISSING_TURN", "stdio_rpc_event_turn_id_invalid"],
      ["SELFTEST_PROCESS_LOCAL_OVERSIZED_SESSION", "stdio_rpc_event_session_id_invalid"],
      ["SELFTEST_PROCESS_LOCAL_CROSS_SESSION", "stdio_rpc_event_identity_mismatch"],
      ["SELFTEST_PROCESS_LOCAL_CROSS_TURN", "stdio_rpc_event_identity_mismatch"],
      ["SELFTEST_PROCESS_LOCAL_DUPLICATE_EVENT", "stdio_rpc_event_duplicate"],
      ["SELFTEST_PROCESS_LOCAL_LATE_EVENT", "stdio_rpc_frame_after_terminal"],
      ["SELFTEST_PROCESS_LOCAL_EMPTY_CHUNK", "stdio_rpc_chunk_invalid"],
      ["SELFTEST_PROCESS_LOCAL_UNRELATED_CHUNK", "stdio_rpc_chunk_output_mismatch"],
      ["SELFTEST_PROCESS_LOCAL_OUTPUT_OVERFLOW", "stdio_rpc_output_limit"],
    ];
    const protocolMutationResults = [];
    for (const [prompt, expectedCode] of protocolMutationCases) {
      protocolMutationResults.push(await processLocalStreamFailure(
        {
          ...sequenceContext,
          maxOutputBytes: prompt === "SELFTEST_PROCESS_LOCAL_OUTPUT_OVERFLOW"
            ? 4096
            : sequenceContext.maxOutputBytes,
        },
        fakeBinary,
        prompt,
        expectedCode,
      ));
    }
    let reusedTurnIdRejected = false;
    const reusedClient = new StdioRpcClient(fakeBinary, sequenceContext);
    try {
      await reusedClient.connect();
      const firstReused = await reusedClient.streamConversation(processLocalSendParams(
        sequenceContext,
        "SELFTEST_PROCESS_LOCAL_REUSED_TURN",
      ));
      await reusedClient.streamConversation(processLocalSendParams(
        sequenceContext,
        "SELFTEST_PROCESS_LOCAL_REUSED_TURN",
        firstReused.result.nativeSessionId,
      ));
    } catch (error) {
      reusedTurnIdRejected = error instanceof AcceptanceError
        && error.code === "stdio_rpc_turn_id_reused";
    } finally {
      await reusedClient.abort();
    }
    const claudeSequenceFailClosed = protocolMutationResults.every(Boolean)
      && reusedTurnIdRejected;
    const exerciseFaultRound = async (
      name,
      environmentPatch,
      expectedCode,
      requireRedactedBlocker = false,
    ) => {
      const root = join(temporaryDirectory, `claude-fault-${name}`);
      const configRoot = join(root, "config");
      const markerPath = join(root, "lifecycle.log");
      const closeGate = join(root, "close.gate");
      mkdirSync(configRoot, { recursive: true, mode: 0o700 });
      const privateWrapper = createPrivateWrapper(root, fakeBinary);
      const faultEnvironment = {
        ...environment,
        ...privateWrapper.environment,
        CLAUDE_CONFIG_DIR: configRoot,
        CLAUDE_CODE_SKIP_PROMPT_HISTORY: "1",
        LICO_FAKE_REQUIRE_NO_HISTORY: "1",
        LICO_FAKE_CLAUDE_RETAIN_DESCENDANT: "1",
        LICO_FAKE_CLAUDE_CLOSE_GATE: closeGate,
        LICO_FAKE_PROCESS_LOCAL_MARKER: markerPath,
        LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
        ...environmentPatch,
      };
      privateWrapper.environment = faultEnvironment;
      const faultContext = {
        ...claudeContext,
        wrapper: privateWrapper,
        environment: faultEnvironment,
        claudeConfigRoot: configRoot,
        armProcessLocalCleanupGate: () => armProcessLocalCloseGate(
          root,
          markerPath,
          closeGate,
        ),
        processLocalCleanupProbe: () => {
          const rows = existsSync(markerPath)
            ? readFileSync(markerPath, "utf8").trim().split(/\r?\n/u)
            : [];
          return rows.indexOf("cleanup_started") >= 0
            && rows.indexOf("descendant_pipe_open") >= 0
            && rows.indexOf("child_waiting_for_close_gate")
              > rows.indexOf("cleanup_started")
            && rows.indexOf("descendant_pipe_closed")
              > rows.indexOf("descendant_pipe_open")
            && rows.indexOf("child_closed")
              > rows.indexOf("child_waiting_for_close_gate")
            && rows.indexOf("io_workers_joined")
              > Math.max(
                rows.indexOf("descendant_pipe_closed"),
                rows.indexOf("child_closed"),
              )
            && rows.indexOf("cleanup_ack") > rows.indexOf("io_workers_joined");
        },
      };
      const client = new StdioRpcClient(fakeBinary, faultContext);
      try {
        await client.connect();
        const round = await runProcessLocalRound(
          faultContext,
          1,
          client,
          processLocalEvidenceSeed,
        );
        const shutdown = await client.shutdown();
        const serializedRound = JSON.stringify(round);
        return round.ready === false
          && round.errorCode === expectedCode
          && (!requireRedactedBlocker
            || (round.facts === null
              && !serializedRound.includes(root)
              && !serializedRound.includes(configRoot)
              && !Object.hasOwn(round, "stderr")
              && !Object.hasOwn(round, "message")))
          && shutdown.acknowledged === true
          && shutdown.exited === true
          && shutdown.statusCode === 0
          && client.closed === true;
      } catch {
        await client.abort();
        return false;
      }
    };
    const historyNegativeCases = [
      ["identity", { LICO_FAKE_PROCESS_LOCAL_FAULT: "history_forged_identity" }],
      ["turn-count", { LICO_FAKE_PROCESS_LOCAL_FAULT: "history_turn_count" }],
      ["byte-count", { LICO_FAKE_PROCESS_LOCAL_FAULT: "history_byte_count" }],
      ["shape", { LICO_FAKE_PROCESS_LOCAL_FAULT: "history_shape" }],
    ];
    const historyNegativeResults = [];
    for (const [name, patch] of historyNegativeCases) {
      historyNegativeResults.push(await exerciseFaultRound(
        name,
        patch,
        "process_local_history_failed",
      ));
    }
    const cleanupEarlyAckRejected = await exerciseFaultRound(
      "cleanup-early-ack",
      { LICO_FAKE_PROCESS_LOCAL_FAULT: "cleanup_early_ack" },
      "process_local_cleanup_sync_failed",
    );
    const utf8HistoryClient = new StdioRpcClient(fakeBinary, sequenceContext);
    let historyUtf8FifoPassed = false;
    try {
      await utf8HistoryClient.connect();
      const utf8First = await utf8HistoryClient.streamConversation(
        processLocalSendParams(sequenceContext, "SELFTEST_PROCESS_LOCAL_UTF8_A"),
      );
      const utf8SessionId = utf8First.result.nativeSessionId;
      const utf8Second = await utf8HistoryClient.streamConversation(
        processLocalSendParams(
          sequenceContext,
          "SELFTEST_PROCESS_LOCAL_UTF8_B",
          utf8SessionId,
        ),
      );
      const utf8Third = await utf8HistoryClient.streamConversation(
        processLocalSendParams(
          sequenceContext,
          "SELFTEST_PROCESS_LOCAL_UTF8_C",
          utf8SessionId,
        ),
      );
      const utf8Fourth = await utf8HistoryClient.streamConversation(
        processLocalSendParams(
          sequenceContext,
          "SELFTEST_PROCESS_LOCAL_UTF8_D",
          utf8SessionId,
        ),
      );
      const utf8History = await utf8HistoryClient.request(
        "agent.conversation.history",
        { agent: "claude-code", sessionId: utf8SessionId },
      );
      const secondOutput = "🙂".repeat(2250);
      const thirdOutput = "é".repeat(4000);
      const fourthOutput = "ß".repeat(4000);
      const expectedByteCount = Buffer.byteLength(secondOutput)
        + Buffer.byteLength(thirdOutput)
        + Buffer.byteLength(fourthOutput);
      const cleanup = await utf8HistoryClient.request(
        "agent.conversation.cleanup",
        { agent: "claude-code", sessionId: utf8SessionId },
      );
      const cleared = await utf8HistoryClient.request(
        "agent.conversation.history",
        { agent: "claude-code", sessionId: utf8SessionId },
      );
      const shutdown = await utf8HistoryClient.shutdown();
      historyUtf8FifoPassed = strictHistoryProjection(
        utf8History,
        utf8SessionId,
        [
          { turnId: utf8Second.result.turnId, output: secondOutput },
          { turnId: utf8Third.result.turnId, output: thirdOutput },
          { turnId: utf8Fourth.result.turnId, output: fourthOutput },
        ],
        32768,
      )
        && utf8History.turns.every((turn) => turn.turnId !== utf8First.result.turnId)
        && utf8History.byteCount === expectedByteCount
        && expectedByteCount
          !== secondOutput.length + thirdOutput.length + fourthOutput.length
        && cleanup?.ok === true
        && cleanup?.status === "cleaned"
        && cleared?.ok === false
        && shutdown.acknowledged === true
        && shutdown.exited === true
        && shutdown.statusCode === 0;
    } catch {
      await utf8HistoryClient.abort();
      historyUtf8FifoPassed = false;
    }
    const historyProjectionEvictionRejected = strictHistoryProjection({
      ok: true,
      continuityScope: "process-local",
      nativeSessionId: "fixture-session",
      turns: Array.from({ length: 65 }, (_, index) => ({
        turnId: `turn-${index}`,
        output: "x",
      })),
      turnCount: 65,
      byteCount: 65,
    }, "fixture-session", Array.from({ length: 65 }, (_, index) => ({
      turnId: `turn-${index}`,
      output: "x",
    })), 4096) === false;
    const persistedTranscriptRejected = await exerciseFaultRound(
      "persisted-transcript",
      { LICO_FAKE_CLAUDE_PERSIST: "1" },
      "process_local_disk_persistence_failed",
    );
    const scanRoot = join(temporaryDirectory, "claude-scan-negatives");
    const scanTarget = join(scanRoot, "target");
    const scanLink = join(scanRoot, "linked");
    mkdirSync(scanRoot, { recursive: true, mode: 0o700 });
    writeFileSync(scanTarget, "synthetic");
    symlinkSync(scanTarget, scanLink);
    let persistenceSymlinkRejected = false;
    try {
      scanBoundedNoFollow(scanRoot, ["synthetic"]);
    } catch (error) {
      persistenceSymlinkRejected = error instanceof AcceptanceError
        && error.code === "persistence_scan_symlink";
    }
    const deepScanRoot = join(temporaryDirectory, "claude-scan-limit");
    let deepScanCursor = deepScanRoot;
    for (let depth = 0; depth < 5; depth += 1) {
      mkdirSync(deepScanCursor, { recursive: true, mode: 0o700 });
      deepScanCursor = join(deepScanCursor, `depth-${depth}`);
    }
    mkdirSync(deepScanCursor, { recursive: true, mode: 0o700 });
    writeFileSync(join(deepScanCursor, "marker"), "synthetic");
    let persistenceScanLimitRejected = false;
    try {
      scanBoundedNoFollow(deepScanRoot, ["synthetic"], { maxDepth: 2 });
    } catch (error) {
      persistenceScanLimitRejected = error instanceof AcceptanceError
        && error.code === "persistence_scan_limit";
    }
    const authenticationBlockedAndClosed = await exerciseFaultRound(
      "authentication",
      { LICO_FAKE_PROCESS_LOCAL_FAULT: "authentication" },
      "claude_code_authentication_required",
      true,
    );
    const shutdownFailureRoot = join(temporaryDirectory, "claude-shutdown-failure");
    const shutdownFailureConfig = join(shutdownFailureRoot, "config");
    mkdirSync(shutdownFailureConfig, { recursive: true, mode: 0o700 });
    const shutdownFailureWrapper = createPrivateWrapper(shutdownFailureRoot, fakeBinary);
    const shutdownFailureEnvironment = {
      ...environment,
      ...shutdownFailureWrapper.environment,
      CLAUDE_CONFIG_DIR: shutdownFailureConfig,
      CLAUDE_CODE_SKIP_PROMPT_HISTORY: "1",
      LICO_FAKE_REQUIRE_NO_HISTORY: "1",
      LICO_FAKE_PROCESS_LOCAL_FAULT: "shutdown_failure",
      LICO_AGENT_CONVERSATION_ACCEPTANCE: acceptanceMode,
    };
    shutdownFailureWrapper.environment = shutdownFailureEnvironment;
    const shutdownFailureContext = {
      ...claudeContext,
      wrapper: shutdownFailureWrapper,
      environment: shutdownFailureEnvironment,
      claudeConfigRoot: shutdownFailureConfig,
    };
    const shutdownFailureClient = new StdioRpcClient(fakeBinary, shutdownFailureContext);
    let shutdownFailureFailClosed = false;
    try {
      await shutdownFailureClient.connect();
      await shutdownFailureClient.streamConversation(processLocalSendParams(
        shutdownFailureContext,
        "Hi",
      ));
      await shutdownFailureClient.shutdown();
    } catch (error) {
      await shutdownFailureClient.abort();
      shutdownFailureFailClosed = error instanceof AcceptanceError
        && error.code === "process_local_shutdown_failed"
        && shutdownFailureClient.closed === true;
    }
    const processLocalFixtureOraclePassed = claudeProcessLocalRound?.ready === true
      && claudeHostShutdown
      && claudeCleanupSynchronized
      && claudeShutdownDrain
      && claudeEofDrain
      && claudeSequenceFailClosed
      && historyNegativeResults.every(Boolean)
      && cleanupEarlyAckRejected
      && historyUtf8FifoPassed
      && historyProjectionEvictionRejected
      && persistedTranscriptRejected
      && persistenceSymlinkRejected
      && persistenceScanLimitRejected
      && authenticationBlockedAndClosed
      && shutdownFailureFailClosed;
    const processLocalAggregate = aggregateProcessLocalResult(
      "claude-code",
      false,
      true,
      claudeProcessLocalRound ? [claudeProcessLocalRound] : [],
      { ...evidenceSeed, processLocalOraclePassed: processLocalFixtureOraclePassed },
      { hostShutdownPassed: claudeHostShutdown },
    );
    const falseProcessLocalOracleAggregate = aggregateProcessLocalResult(
      "claude-code",
      false,
      true,
      claudeProcessLocalRound ? [claudeProcessLocalRound] : [],
      { ...evidenceSeed, processLocalOraclePassed: false },
      { hostShutdownPassed: claudeHostShutdown },
    );
    const missingProcessLocalOracleAggregate = aggregateProcessLocalResult(
      "claude-code",
      false,
      true,
      claudeProcessLocalRound ? [claudeProcessLocalRound] : [],
      evidenceSeed,
      { hostShutdownPassed: claudeHostShutdown },
    );
    const processLocalReleaseAggregate = aggregateProcessLocalResult(
      "claude-code",
      false,
      true,
      claudeProcessLocalRound ? [claudeProcessLocalRound] : [],
      { ...evidenceSeed, processLocalOraclePassed: true },
      { hostShutdownPassed: true, releaseUi: true },
    );
    const falseHostShutdownAggregate = aggregateProcessLocalResult(
      "claude-code",
      false,
      true,
      claudeProcessLocalRound ? [claudeProcessLocalRound] : [],
      { ...evidenceSeed, processLocalOraclePassed: true },
      { hostShutdownPassed: false, releaseUi: true },
    );
    const missingHostShutdownAggregate = aggregateProcessLocalResult(
      "claude-code",
      false,
      true,
      claudeProcessLocalRound ? [claudeProcessLocalRound] : [],
      { ...evidenceSeed, processLocalOraclePassed: true },
      { releaseUi: true },
    );
    const processLocalTruthKeys = ["continuityScope", ...processLocalBooleanFactKeys];
    const processLocalFactTruthRows = [];
    for (const key of processLocalTruthKeys) {
      const falseRound = {
        ...claudeProcessLocalRound,
        ready: false,
        conversationReady: false,
        facts: {
          ...claudeProcessLocalRound?.facts,
          [key]: key === "continuityScope" ? "cross-process" : false,
        },
      };
      const missingFacts = { ...claudeProcessLocalRound?.facts };
      delete missingFacts[key];
      const missingRound = {
        ...claudeProcessLocalRound,
        ready: false,
        conversationReady: false,
        facts: missingFacts,
      };
      const falseAggregate = aggregateProcessLocalResult(
        "claude-code",
        false,
        true,
        [falseRound],
        { ...evidenceSeed, processLocalOraclePassed: true },
        { hostShutdownPassed: true, releaseUi: true },
      );
      const missingAggregate = aggregateProcessLocalResult(
        "claude-code",
        false,
        true,
        [missingRound],
        { ...evidenceSeed, processLocalOraclePassed: true },
        { hostShutdownPassed: true, releaseUi: true },
      );
      const falseWrite = writeReleaseUiAdapterEvidence(
        releaseUiProcessLocalAggregate(falseAggregate),
        {
          ...context,
          evidenceManifestPath: join(
            temporaryDirectory,
            `process-local-${key}-false-evidence.json`,
          ),
        },
      );
      const missingWrite = writeReleaseUiAdapterEvidence(
        releaseUiProcessLocalAggregate(missingAggregate),
        {
          ...context,
          evidenceManifestPath: join(
            temporaryDirectory,
            `process-local-${key}-missing-evidence.json`,
          ),
        },
      );
      processLocalFactTruthRows.push(
        processLocalRoundFactsReady(claudeProcessLocalRound?.facts) === true
          && processLocalRoundFactsReady(falseRound.facts) === false
          && processLocalRoundFactsReady(missingRound.facts) === false
          && processLocalReleaseAggregate.status === "release-ui-passed"
          && processLocalReleaseAggregate.processLocalFactsEvidenceComplete === true
          && processLocalReleaseAggregate.processLocalFactsPassed === true
          && falseAggregate.status === "failed"
          && falseAggregate.processLocalFactsEvidenceComplete === true
          && falseAggregate.processLocalFactsPassed === false
          && missingAggregate.status === "failed"
          && missingAggregate.processLocalFactsEvidenceComplete === false
          && missingAggregate.processLocalFactsPassed === false
          && coreChecksFromAggregate(falseAggregate)["P-03"] === "fail"
          && coreChecksFromAggregate(falseAggregate)["P-04"] === "fail"
          && coreChecksFromAggregate(missingAggregate)["P-03"] === "fail"
          && coreChecksFromAggregate(missingAggregate)["P-04"] === "fail"
          && falseWrite.written === false
          && falseWrite.reason === "process_local_facts_unproven"
          && missingWrite.written === false
          && missingWrite.reason === "process_local_facts_unproven"
          && new Set([
            processLocalReleaseAggregate.evidenceDigest,
            falseAggregate.evidenceDigest,
            missingAggregate.evidenceDigest,
          ]).size === 3,
      );
    }
    const processLocalEvidencePath = join(
      temporaryDirectory,
      "process-local-release-ui-evidence.json",
    );
    const processLocalPositiveWrite = writeReleaseUiAdapterEvidence(
      releaseUiProcessLocalAggregate(processLocalReleaseAggregate),
      { ...context, evidenceManifestPath: processLocalEvidencePath },
    );
    const processLocalPersistedEvidence = processLocalPositiveWrite.written === true
        && existsSync(processLocalEvidencePath)
      ? JSON.parse(readFileSync(processLocalEvidencePath, "utf8"))
      : {};
    const processLocalPersistedAdapter = processLocalPersistedEvidence.adapters?.find(
      (adapter) => adapter.agentId === "claude-code",
    );
    const processLocalMutatedAdapter = processLocalPersistedAdapter
      ? {
        ...processLocalPersistedAdapter,
        coreChecks: {
          ...processLocalPersistedAdapter.coreChecks,
          "P-03": "fail",
          "P-04": "fail",
        },
      }
      : null;
    const processLocalFactTruthTable = processLocalFactTruthRows.length
        === processLocalTruthKeys.length
      && processLocalFactTruthRows.every(Boolean)
      && coreChecksFromAggregate(processLocalReleaseAggregate)["P-03"] === "pass"
      && coreChecksFromAggregate(processLocalReleaseAggregate)["P-04"] === "pass"
      && processLocalPositiveWrite.written === true
      && processLocalPersistedAdapter?.coreChecks?.["P-03"] === "pass"
      && processLocalPersistedAdapter?.coreChecks?.["P-04"] === "pass"
      && processLocalPersistedAdapter?.evidenceDigest
        === processLocalPositiveWrite.evidenceDigest
      && adapterEvidenceDigestFor(processLocalPersistedAdapter)
        === processLocalPersistedAdapter?.evidenceDigest
      && adapterEvidenceDigestFor(processLocalMutatedAdapter)
        !== processLocalPersistedAdapter?.evidenceDigest;
    const falseHostShutdownEvidencePath = join(
      temporaryDirectory,
      "process-local-host-shutdown-false-evidence.json",
    );
    const missingHostShutdownEvidencePath = join(
      temporaryDirectory,
      "process-local-host-shutdown-missing-evidence.json",
    );
    const falseHostShutdownWrite = writeReleaseUiAdapterEvidence(
      releaseUiProcessLocalAggregate(falseHostShutdownAggregate),
      {
        ...context,
        evidenceManifestPath: falseHostShutdownEvidencePath,
      },
    );
    const missingHostShutdownWrite = writeReleaseUiAdapterEvidence(
      releaseUiProcessLocalAggregate(missingHostShutdownAggregate),
      {
        ...context,
        evidenceManifestPath: missingHostShutdownEvidencePath,
      },
    );
    const processLocalHostShutdownTruthTable = processLocalReleaseAggregate.status
        === "release-ui-passed"
      && processLocalReleaseAggregate.hostShutdownEvidenceComplete === true
      && processLocalReleaseAggregate.hostShutdownPassed === true
      && falseHostShutdownAggregate.status === "failed"
      && falseHostShutdownAggregate.conversationGatePassed === false
      && falseHostShutdownAggregate.hostShutdownEvidenceComplete === true
      && falseHostShutdownAggregate.hostShutdownPassed === false
      && falseHostShutdownAggregate.errorCode === "process_local_host_shutdown_failed"
      && missingHostShutdownAggregate.status === "failed"
      && missingHostShutdownAggregate.conversationGatePassed === false
      && missingHostShutdownAggregate.hostShutdownEvidenceComplete === false
      && missingHostShutdownAggregate.hostShutdownPassed === false
      && missingHostShutdownAggregate.errorCode === "process_local_host_shutdown_failed"
      && coreChecksFromAggregate(processLocalReleaseAggregate)["P-03"] === "pass"
      && coreChecksFromAggregate(processLocalReleaseAggregate)["P-04"] === "pass"
      && coreChecksFromAggregate(falseHostShutdownAggregate)["P-03"] === "fail"
      && coreChecksFromAggregate(falseHostShutdownAggregate)["P-04"] === "fail"
      && coreChecksFromAggregate(missingHostShutdownAggregate)["P-03"] === "fail"
      && coreChecksFromAggregate(missingHostShutdownAggregate)["P-04"] === "fail"
      && falseHostShutdownWrite.written === false
      && falseHostShutdownWrite.reason === "process_local_host_shutdown_unproven"
      && missingHostShutdownWrite.written === false
      && missingHostShutdownWrite.reason === "process_local_host_shutdown_unproven"
      && !existsSync(falseHostShutdownEvidencePath)
      && !existsSync(missingHostShutdownEvidencePath)
      && new Set([
        processLocalReleaseAggregate.evidenceDigest,
        falseHostShutdownAggregate.evidenceDigest,
        missingHostShutdownAggregate.evidenceDigest,
      ]).size === 3;
    const falseProcessLocalOracleWrite = writeReleaseUiAdapterEvidence(
      releaseUiProcessLocalAggregate(falseProcessLocalOracleAggregate),
      {
        ...context,
        evidenceManifestPath: join(
          temporaryDirectory,
          "process-local-oracle-false-evidence.json",
        ),
      },
    );
    const missingProcessLocalOracleWrite = writeReleaseUiAdapterEvidence(
      releaseUiProcessLocalAggregate(missingProcessLocalOracleAggregate),
      {
        ...context,
        evidenceManifestPath: join(
          temporaryDirectory,
          "process-local-oracle-missing-evidence.json",
        ),
      },
    );
    const processLocalOracleFailClosed = processLocalAggregate.status === "core-passed"
      && processLocalAggregate.continuityScope === "process-local"
      && processLocalAggregate.nativeToArc === false
      && processLocalAggregate.arcToNative === false
      && falseProcessLocalOracleAggregate.status === "failed"
      && missingProcessLocalOracleAggregate.status === "failed"
      && falseProcessLocalOracleAggregate.errorCode === "process_local_oracle_failed"
      && missingProcessLocalOracleAggregate.errorCode === "process_local_oracle_failed"
      && coreChecksFromAggregate(falseProcessLocalOracleAggregate)["P-04"] === "fail"
      && coreChecksFromAggregate(missingProcessLocalOracleAggregate)["P-04"] === "fail"
      && falseProcessLocalOracleWrite.written === false
      && falseProcessLocalOracleWrite.reason === "process_local_oracle_unproven"
      && missingProcessLocalOracleWrite.written === false
      && missingProcessLocalOracleWrite.reason === "process_local_oracle_unproven"
      && new Set([
        processLocalAggregate.evidenceDigest,
        falseProcessLocalOracleAggregate.evidenceDigest,
        missingProcessLocalOracleAggregate.evidenceDigest,
      ]).size === 3;
    const processLocalOraclePassed = processLocalFixtureOraclePassed
      && processLocalFactTruthTable
      && processLocalHostShutdownTruthTable
      && processLocalOracleFailClosed;
    const coreOnlyAggregate = aggregateResult(
      "opencode",
      true,
      true,
      rounds,
      evidenceSeed,
      { releaseUi: false },
    );
    const releaseUiAggregate = aggregateResult(
      "opencode",
      true,
      true,
      rounds,
      evidenceSeed,
      { releaseUi: true },
    );
    const falseStreamingRounds = rounds.map((round, index) => ({
      ...round,
      facts: index === 0
        ? { ...round.facts, streamingSeen: false }
        : { ...round.facts },
    }));
    const missingStreamingRounds = rounds.map((round, index) => {
      const facts = { ...round.facts };
      if (index === 0) delete facts.streamingSeen;
      return { ...round, facts };
    });
    const falseStreamingAggregate = aggregateResult(
      "opencode",
      true,
      true,
      falseStreamingRounds,
      evidenceSeed,
      { releaseUi: true },
    );
    const missingStreamingAggregate = aggregateResult(
      "opencode",
      true,
      true,
      missingStreamingRounds,
      evidenceSeed,
      { releaseUi: true },
    );
    const falseOracleSeed = { ...evidenceSeed, quiescenceOraclePassed: false };
    const missingOracleSeed = Object.fromEntries(
      Object.entries(evidenceSeed).filter(([key]) => key !== "quiescenceOraclePassed"),
    );
    const falseOracleAggregate = aggregateResult(
      "opencode",
      true,
      true,
      rounds,
      falseOracleSeed,
      { releaseUi: true },
    );
    const missingOracleAggregate = aggregateResult(
      "opencode",
      true,
      true,
      rounds,
      missingOracleSeed,
      { releaseUi: true },
    );
    const falseStreamChunkOracleAggregate = aggregateResult(
      "opencode",
      true,
      true,
      rounds,
      { ...evidenceSeed, publicStreamChunkOraclePassed: false },
      { releaseUi: true },
    );
    const missingStreamChunkOracleSeed = Object.fromEntries(
      Object.entries(evidenceSeed)
        .filter(([key]) => key !== "publicStreamChunkOraclePassed"),
    );
    const missingStreamChunkOracleAggregate = aggregateResult(
      "opencode",
      true,
      true,
      rounds,
      missingStreamChunkOracleSeed,
      { releaseUi: true },
    );
    const falseOracleEvidenceAggregate = {
      ...releaseUiAggregate,
      quiescenceOraclePassed: false,
      evidenceDigest: "",
    };
    falseOracleEvidenceAggregate.evidenceDigest = recomputedAggregateEvidenceDigest(
      falseOracleEvidenceAggregate,
    );
    const missingOracleEvidenceAggregate = Object.fromEntries(
      Object.entries(releaseUiAggregate)
        .filter(([key]) => key !== "quiescenceOraclePassed"),
    );
    missingOracleEvidenceAggregate.evidenceDigest = recomputedAggregateEvidenceDigest(
      missingOracleEvidenceAggregate,
    );
    const trueAggregateEvidenceDigest = recomputedAggregateEvidenceDigest(releaseUiAggregate);
    const aggregateEvidenceDigestBinding = releaseUiAggregate.evidenceDigest
        === trueAggregateEvidenceDigest
      && falseOracleEvidenceAggregate.evidenceDigest
        === recomputedAggregateEvidenceDigest(falseOracleEvidenceAggregate)
      && missingOracleEvidenceAggregate.evidenceDigest
        === recomputedAggregateEvidenceDigest(missingOracleEvidenceAggregate)
      && new Set([
        releaseUiAggregate.evidenceDigest,
        falseOracleEvidenceAggregate.evidenceDigest,
        missingOracleEvidenceAggregate.evidenceDigest,
      ]).size === 3;
    const falseOracleEvidenceWrite = writeReleaseUiAdapterEvidence(
      falseOracleEvidenceAggregate,
      {},
    );
    const missingOracleEvidenceWrite = writeReleaseUiAdapterEvidence(
      missingOracleEvidenceAggregate,
      {},
    );
    const falseStreamingEvidenceWrite = writeReleaseUiAdapterEvidence(
      falseStreamingAggregate,
      {},
    );
    const missingStreamingEvidenceWrite = writeReleaseUiAdapterEvidence(
      missingStreamingAggregate,
      {},
    );
    const falseStreamChunkOracleEvidenceWrite = writeReleaseUiAdapterEvidence(
      falseStreamChunkOracleAggregate,
      {},
    );
    const missingStreamChunkOracleEvidenceWrite = writeReleaseUiAdapterEvidence(
      missingStreamChunkOracleAggregate,
      {},
    );
    const streamingAggregateDigestBinding = releaseUiAggregate.evidenceDigest
        === recomputedAggregateEvidenceDigest(releaseUiAggregate)
      && falseStreamingAggregate.evidenceDigest
        === recomputedAggregateEvidenceDigest(falseStreamingAggregate)
      && missingStreamingAggregate.evidenceDigest
        === recomputedAggregateEvidenceDigest(missingStreamingAggregate)
      && new Set([
        releaseUiAggregate.evidenceDigest,
        falseStreamingAggregate.evidenceDigest,
        missingStreamingAggregate.evidenceDigest,
      ]).size === 3;
    const trueStreamChunkOracleDigest = recomputedAggregateEvidenceDigest(releaseUiAggregate);
    const falseStreamChunkOracleDigest = recomputedAggregateEvidenceDigest(falseStreamChunkOracleAggregate);
    const missingStreamChunkOracleDigest = recomputedAggregateEvidenceDigest(missingStreamChunkOracleAggregate);
    const publicStreamChunkAggregateDigestBinding = releaseUiAggregate.evidenceDigest
        === trueStreamChunkOracleDigest
      && falseStreamChunkOracleAggregate.evidenceDigest
        === falseStreamChunkOracleDigest
      && missingStreamChunkOracleAggregate.evidenceDigest
        === missingStreamChunkOracleDigest
      && new Set([
        trueStreamChunkOracleDigest,
        falseStreamChunkOracleDigest,
        missingStreamChunkOracleDigest,
      ]).size === 3;
    const isolatedEvidenceManifestPath = join(
      temporaryDirectory,
      "release-ui-adapter-evidence.json",
    );
    const persistedAggregate = {
      ...releaseUiAggregate,
      productArtifactDigest: `sha256:${digest("self-test-product-artifact")}`,
      productContinuityBindingDigest: `sha256:${digest("self-test-continuity-binding")}`,
      evidenceDigest: "",
    };
    persistedAggregate.evidenceDigest = recomputedAggregateEvidenceDigest(persistedAggregate);
    const persistedEvidenceWrite = writeReleaseUiAdapterEvidence(persistedAggregate, {
      ...context,
      evidenceManifestPath: isolatedEvidenceManifestPath,
    });
    if (!persistedEvidenceWrite.written) {
      throw new AcceptanceError(`self_test_${persistedEvidenceWrite.reason}`);
    }
    const persistedEvidence = JSON.parse(readFileSync(isolatedEvidenceManifestPath, "utf8"));
    const persistedAdapter = persistedEvidence.adapters?.find(
      (adapter) => adapter.agentId === releaseUiAggregate.agent,
    );
    const persistedAdapterWithFailedP04 = persistedAdapter
      ? {
        ...persistedAdapter,
        coreChecks: { ...persistedAdapter.coreChecks, "P-04": "fail" },
      }
      : null;
    const persistedAdapterWithFailedStreaming = persistedAdapter
      ? {
        ...persistedAdapter,
        conditionalChecks: {
          ...persistedAdapter.conditionalChecks,
          "C-01": { nativeSupport: "supported", result: "fail" },
        },
      }
      : null;
    const persistedAdapterEvidenceBindsP04 = persistedEvidenceWrite.written === true
      && persistedAdapter?.coreChecks?.["P-04"] === "pass"
      && persistedAdapter.evidenceDigest === persistedEvidenceWrite.evidenceDigest
      && adapterEvidenceDigestFor(persistedAdapter) === persistedAdapter.evidenceDigest
      && adapterEvidenceDigestFor(persistedAdapterWithFailedP04)
        !== persistedAdapter.evidenceDigest;
    const persistedAdapterEvidenceBindsStreaming = persistedEvidenceWrite.written === true
      && persistedAdapter?.conditionalChecks?.["C-01"]?.result === "pass"
      && adapterEvidenceDigestFor(persistedAdapterWithFailedStreaming)
        !== persistedAdapter.evidenceDigest;
    const quiescenceEvidenceFailClosed = releaseUiAggregate.quiescenceOraclePassed === true
      && coreChecksFromAggregate(releaseUiAggregate)["P-04"] === "pass"
      && falseOracleAggregate.status === "failed"
      && missingOracleAggregate.status === "failed"
      && falseOracleAggregate.roundsCompleted === strictRoundCount
      && missingOracleAggregate.conversationRoundsCompleted === strictRoundCount
      && falseOracleAggregate.conversationPassed === true
      && missingOracleAggregate.conversationPassed === true
      && falseOracleAggregate.quiescenceOraclePassed === false
      && missingOracleAggregate.quiescenceOraclePassed === false
      && falseOracleAggregate.errorCode === "parity_quiescence_oracle_failed"
      && missingOracleAggregate.errorCode === "parity_quiescence_oracle_failed"
      && coreChecksFromAggregate(falseOracleEvidenceAggregate)["P-04"] === "fail"
      && coreChecksFromAggregate(missingOracleEvidenceAggregate)["P-04"] === "fail"
      && falseOracleEvidenceWrite.written === false
      && falseOracleEvidenceWrite.reason === "quiescence_oracle_unproven"
      && missingOracleEvidenceWrite.written === false
      && missingOracleEvidenceWrite.reason === "quiescence_oracle_unproven"
      && aggregateEvidenceDigestBinding
      && persistedAdapterEvidenceBindsP04;
    const streamingEvidenceFailClosed = fakeLiveStreamingGate
      && releaseUiAggregate.streamingEvidenceComplete === true
      && releaseUiAggregate.streamingProven === true
      && coreChecksFromAggregate(releaseUiAggregate)["P-04"] === "pass"
      && falseStreamingAggregate.status === "failed"
      && missingStreamingAggregate.status === "failed"
      && falseStreamingAggregate.conversationGatePassed === false
      && missingStreamingAggregate.conversationGatePassed === false
      && falseStreamingAggregate.roundsCompleted === strictRoundCount
      && missingStreamingAggregate.roundsCompleted === strictRoundCount
      && falseStreamingAggregate.conversationRoundsCompleted === strictRoundCount
      && missingStreamingAggregate.conversationRoundsCompleted === strictRoundCount
      && falseStreamingAggregate.streamingEvidenceComplete === true
      && missingStreamingAggregate.streamingEvidenceComplete === false
      && falseStreamingAggregate.streamingProven === false
      && missingStreamingAggregate.streamingProven === false
      && falseStreamingAggregate.errorCode === "parity_streaming_failed"
      && missingStreamingAggregate.errorCode === "parity_streaming_failed"
      && coreChecksFromAggregate(falseStreamingAggregate)["P-04"] === "fail"
      && coreChecksFromAggregate(missingStreamingAggregate)["P-04"] === "fail"
      && falseStreamingEvidenceWrite.written === false
      && falseStreamingEvidenceWrite.reason === "streaming_unproven"
      && missingStreamingEvidenceWrite.written === false
      && missingStreamingEvidenceWrite.reason === "streaming_unproven"
      && streamingAggregateDigestBinding
      && persistedAdapterEvidenceBindsStreaming;
    const publicStreamChunkEvidenceFailClosed = releaseUiAggregate
      .publicStreamChunkOraclePassed === true
      && releaseUiAggregate.publicStreamChunkOracleEvidenceComplete === true
      && falseStreamChunkOracleAggregate.publicStreamChunkOracleEvidenceComplete === true
      && missingStreamChunkOracleAggregate.publicStreamChunkOracleEvidenceComplete === false
      && falseStreamChunkOracleAggregate.status === "failed"
      && missingStreamChunkOracleAggregate.status === "failed"
      && falseStreamChunkOracleAggregate.conversationGatePassed === false
      && missingStreamChunkOracleAggregate.conversationGatePassed === false
      && falseStreamChunkOracleAggregate.evidenceDigest === falseStreamChunkOracleDigest
      && missingStreamChunkOracleAggregate.evidenceDigest === missingStreamChunkOracleDigest
      && releaseUiAggregate.evidenceDigest === trueStreamChunkOracleDigest
      && falseStreamChunkOracleAggregate.errorCode
        === "parity_stream_chunk_oracle_failed"
      && missingStreamChunkOracleAggregate.errorCode
        === "parity_stream_chunk_oracle_failed"
      && coreChecksFromAggregate(falseStreamChunkOracleAggregate)["P-04"] === "fail"
      && coreChecksFromAggregate(missingStreamChunkOracleAggregate)["P-04"] === "fail"
      && falseStreamChunkOracleEvidenceWrite.written === false
      && falseStreamChunkOracleEvidenceWrite.reason === "stream_chunk_oracle_unproven"
      && missingStreamChunkOracleEvidenceWrite.written === false
      && missingStreamChunkOracleEvidenceWrite.reason === "stream_chunk_oracle_unproven"
      && publicStreamChunkAggregateDigestBinding;
    const consecutivePassesFailClosed = coreOnlyAggregate.consecutivePasses === 0
      && coreOnlyAggregate.conversationGatePassed === false
      && coreOnlyAggregate.status === "core-passed"
      && releaseUiAggregate.consecutivePasses === strictRoundCount
      && releaseUiAggregate.conversationGatePassed === true
      && releaseUiAggregate.status === "release-ui-passed"
      && releaseUiAggregate.cl06Ready === false;
    const cursorDriver = JSON.parse(readFileSync(driversInventoryPath, "utf8"))
      .drivers
      .find((row) => row.agentId === "cursor");
    const cursorInventoryAligned = cursorDriver?.blockerCodes?.length === 0
      && cursorDriver?.capabilityMatrix?.exactResume === true
      && cursorDriver?.capabilityMatrix?.officialLane === true
      && cursorDriver?.driverMode === "conversation"
      && agentConfigs.cursor.cleanupKind === "cursor-cli-chat-leaf"
      && agentConfigs.cursor.laneFamily === "cli";
    const cursorContext = {
      ...context,
      config: agentConfigs.cursor,
      cleanedSessions: new Set(),
      observedSessions: new Set(),
    };
    let cursorSessionLoadOk = false;
    try {
      const cursorOpened = await nativeTurn(
        cursorContext,
        "",
        "Hi",
      );
      const cursorResumed = await nativeTurn(
        cursorContext,
        cursorOpened.sessionId,
        "Hi",
      );
      const cursorRead = await nativeReadback(cursorContext, cursorOpened.sessionId);
      const cursorOpenedOutput = String(cursorOpened.output || "");
      const cursorResumedOutput = String(cursorResumed.output || "");
      cursorSessionLoadOk = cursorOpened.sessionId.length > 0
        && cursorResumed.sessionId === cursorOpened.sessionId
        && cursorOpenedOutput.trim().length > 0
        && cursorResumedOutput.trim().length > 0
        && cursorRead.text.includes(cursorOpenedOutput)
        && cursorRead.text.includes(cursorResumedOutput);
      // Self-test owns the temporary fake-runtime state file, so reclaim its
      // synthetic Cursor rows directly. This is not a production cleanup path
      // and never touches Cursor's real storage.
      const cursorFixtureState = JSON.parse(readFileSync(statePath, "utf8"));
      for (const sessionId of cursorContext.observedSessions || []) {
        delete cursorFixtureState.sessions[sessionId];
      }
      writeFileSync(statePath, JSON.stringify(cursorFixtureState));
    } catch {
      cursorSessionLoadOk = false;
    }
    const liveGate = printLiveGateChecklist();
    const cursorLiveGate = liveGate.adapters?.find((row) => row.agentId === "cursor");
    const liveGateReadyCandidates = Array.isArray(liveGate.adapters)
      && liveGate.adapters.length === readyCandidateAgentIds.length
      && liveGate.adapters.every((row) => row.remainingLiveGate.includes("authorize_side_effects"))
      && cursorLiveGate?.cleanupReady === true
      && !cursorLiveGate?.remainingLiveGate?.includes("implement_safe_cleanup")
      && liveGate.cl06Ready === false;
    const interruptSteerSupportedPass = conditionalChecksFromMatrix(
      { interruptSteer: true },
      { interruptSteer: true },
    )["C-05"];
    const interruptSteerSupportedMissing = conditionalChecksFromMatrix(
      { interruptSteer: true },
    )["C-05"];
    const interruptSteerUnsupported = conditionalChecksFromMatrix(
      { interruptSteer: false },
      { interruptSteer: true },
    )["C-05"];
    const interruptSteerEvidenceFailClosed =
      interruptSteerSupportedPass.nativeSupport === "supported"
      && interruptSteerSupportedPass.result === "pass"
      && interruptSteerSupportedMissing.nativeSupport === "supported"
      && interruptSteerSupportedMissing.result === "unverified"
      && interruptSteerUnsupported.nativeSupport === "unsupported"
      && interruptSteerUnsupported.result === "unsupported-by-native";
    const remaining = await listSessions(context);
    const dispatchLaneProbe = probeDispatchLaneFamilies(fakeBinary);
    let evidenceWrite = null;
    if (dispatchLaneProbe.ok) {
      evidenceWrite = createSanitizedSelfTestEvidenceReceipt(dispatchLaneProbe);
    }
    const result = {
      status: strictReducer.status === "core-passed"
        && streamingEvidenceFailClosed
        && quiescenceOraclePassed
        && publicStreamChunkOraclePassed
        && publicStreamChunkEvidenceFailClosed
        && liveRoundOrderingAgnostic
        && verificationWorkflowContract
        && exactResumeIdentityFailClosed
        && sessionUpdateOracle
        && promptQuiescenceBudget
        && permissionFailClosed
        && boundedOutputFailClosed
        && malformedEnvelopeFailClosed
        && malformedContentFailClosed
        && malformedBeforePromptResponseFailClosed
        && errorFailClosed
        && factFailureCode
        && quiescenceEvidenceFailClosed
        && cleanupBlocked
        && copilotCleanupRpc
        && openclawCleanupRpc
        && kiloCleanupCli
        && hermesCleanupCli
        && kimiCleanup
        && piRpcExactResume
        && piCleanup
        && codexRoundReady
        && codexCleanupRpc
        && processLocalOraclePassed
        && processLocalOracleFailClosed
        && processLocalFactTruthTable
        && processLocalHostShutdownTruthTable
        && claudeCleanupSynchronized
        && claudeShutdownDrain
        && claudeEofDrain
        && claudeSequenceFailClosed
        && historyNegativeResults.every(Boolean)
        && cleanupEarlyAckRejected
        && historyUtf8FifoPassed
        && historyProjectionEvictionRejected
        && persistedTranscriptRejected
        && persistenceSymlinkRejected
        && persistenceScanLimitRejected
        && authenticationBlockedAndClosed
        && shutdownFailureFailClosed
        && consecutivePassesFailClosed
        && cursorInventoryAligned
        && cursorSessionLoadOk
        && liveGateReadyCandidates
        && interruptSteerEvidenceFailClosed
        && productFixtureReceiptRejected
        && disposableProfileSeedSafe
        && remaining.size === 0
        && dispatchLaneProbe.ok
        ? "passed"
        : "failed",
      cl06Ready: false,
      conversationGatePassed: false,
      strictRounds: strictReducer.roundsCompleted,
      verificationTurns: rounds[0]?.requestCount || 0,
      verificationSessions: rounds[0]?.testedSessions || 0,
      openNew: strictReducer.openNew,
      exactResume: strictReducer.exactResume,
      nativeToArc: strictReducer.nativeToArc,
      arcToNative: strictReducer.arcToNative,
      realSessionIds: strictReducer.realSessionIds,
      rawResponses: strictReducer.rawResponses,
      cwdParity: strictReducer.cwdParity,
      settingsParity: strictReducer.settingsParity,
      argvPromptAbsent: strictReducer.argvPromptAbsent,
      historyReadback: strictReducer.historyReadback,
      fakeLiveStreamingGate,
      streamingEvidenceFailClosed,
      streamingAggregateDigestBinding,
      persistedAdapterEvidenceBindsStreaming,
      quiescenceOraclePassed,
      publicStreamChunkOraclePassed,
      publicStreamChunkEvidenceFailClosed,
      liveRoundOrderingAgnostic,
      verificationWorkflowContract,
      exactResumeIdentityFailClosed,
      sessionUpdateOracle,
      promptQuiescenceBudget,
      permissionFailClosed,
      errorFailClosed,
      factFailureCode,
      quiescenceEvidenceFailClosed,
      aggregateEvidenceDigestBinding,
      persistedAdapterEvidenceBindsP04,
      boundedOutputFailClosed,
      malformedEnvelopeFailClosed,
      malformedContentFailClosed,
      malformedBeforePromptResponseFailClosed,
      cleanupBlocked,
      copilotCleanupRpc,
      openclawCleanupRpc,
      kiloCleanupCli,
      hermesCleanupCli,
      kimiCleanup,
      piRpcExactResume,
      piCleanup,
      codexRoundReady,
      codexCleanupRpc,
      processLocalOraclePassed,
      processLocalOracleFailClosed,
      processLocalFactTruthTable,
      processLocalFactTruthMask: processLocalFactTruthRows
        .map((value) => value === true ? "1" : "0")
        .join(""),
      processLocalHostShutdownTruthTable,
      processLocalHostShutdownPassed: claudeHostShutdown,
      processLocalHostShutdownErrorCode: claudeHostShutdownErrorCode,
      processLocalCleanupSynchronized: claudeCleanupSynchronized,
      claudeProcessLocalRound: claudeProcessLocalRound?.ready === true,
      claudeProcessLocalErrorCode: claudeProcessLocalRound?.errorCode || null,
      claudeCleanupSynchronized,
      claudeShutdownDrain,
      claudeEofDrain,
      claudeSequenceFailClosed,
      claudeSequenceMask: [...protocolMutationResults, reusedTurnIdRejected]
        .map((value) => value === true ? "1" : "0")
        .join(""),
      claudeHistoryNegatives: historyNegativeResults.every(Boolean),
      claudeCleanupEarlyAckRejected: cleanupEarlyAckRejected,
      claudeHistoryUtf8Fifo: historyUtf8FifoPassed,
      claudeHistoryEvictionBounded: historyProjectionEvictionRejected,
      claudePersistedTranscriptRejected: persistedTranscriptRejected,
      claudePersistenceSymlinkRejected: persistenceSymlinkRejected,
      claudePersistenceScanLimitRejected: persistenceScanLimitRejected,
      claudeAuthenticationBlockedAndClosed: authenticationBlockedAndClosed,
      claudeShutdownFailureFailClosed: shutdownFailureFailClosed,
      claudeContinuityScope: processLocalAggregate.continuityScope,
      consecutivePassesFailClosed,
      cursorInventoryAligned,
      cursorSessionLoadOk,
      liveGateReadyCandidates,
      interruptSteerEvidenceFailClosed,
      productFixtureReceiptRejected,
      disposableProfileSeedSafe,
      dispatchLaneContract: dispatchLaneProbe.ok,
      laneFamiliesCovered: dispatchLaneProbe.laneFamiliesCovered,
      harnessVersion: dispatchLaneHarnessVersion,
      evidenceWrite,
      cleanupVerified: strictReducer.cleanupVerified && remaining.size === 0,
      cleanupCount: strictReducer.cleanupCount + 14,
      errorCode: null,
      evidenceDigest: "",
    };
    if (result.status !== "passed") result.errorCode = "self_test_failed";
    result.evidenceDigest = digest({ ...result, evidenceDigest: undefined });
    return result;
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}
