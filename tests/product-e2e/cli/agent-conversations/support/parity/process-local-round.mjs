import { existsSync, readFileSync } from "node:fs";
import { parityModelForAgent } from "./agent-ids.mjs";
import { AcceptanceError, digest, requireFact } from "./errors.mjs";
import { scanBoundedNoFollow } from "./process.mjs";
import {
  failedProcessLocalFactCode,
  processLocalRoundFactsReady,
} from "./round-facts.mjs";

function resultErrorCode(result) {
  const code = result?.error?.code;
  return typeof code === "string" ? code : "process_local_operation_failed";
}

function validOpaqueId(value) {
  return typeof value === "string"
    && value.trim().length > 0
    && Buffer.byteLength(value) <= 512
    && !/[\u0000-\u001f\u007f]/u.test(value);
}

function eventOwnership(turn, sessionId, otherTurnId = "") {
  if (!Array.isArray(turn?.events) || turn.events.length < 4) return false;
  let terminalSeen = false;
  let turnId = "";
  let completedOutput = null;
  const chunks = [];
  for (const event of turn.events) {
    const kind = event?.event;
    if (typeof kind !== "string" || kind.length === 0 || terminalSeen) return false;
    if (!validOpaqueId(event.sessionId) || event.sessionId !== sessionId) return false;
    if (!validOpaqueId(event.turnId) || (turnId && turnId !== event.turnId)) return false;
    turnId = event.turnId;
    if (kind === "agent.message.chunk") {
      if (typeof event?.payload?.text !== "string" || event.payload.text.length === 0) return false;
      chunks.push(event.payload.text);
    } else if (kind === "agent.message.completed") {
      if (completedOutput !== null || typeof event?.payload?.text !== "string") return false;
      completedOutput = event.payload.text;
    }
    terminalSeen = kind === "dispatch.turn.completed";
  }
  return validOpaqueId(turnId)
    && turnId !== otherTurnId
    && turnId === turn.result?.turnId
    && turn.result?.nativeSessionId === sessionId
    && turn.result?.sessionId === sessionId
    && turn.result?.threadId === sessionId
    && turn.events[0]?.event === "dispatch.turn.started"
    && chunks.length > 0
    && completedOutput !== null
    && chunks.join("") === completedOutput
    && completedOutput === turn.result?.output
    && turn.events.at(-1)?.event === "dispatch.turn.completed"
    && turn.eventTranscriptMatches === true;
}

function captureInvocations(context) {
  if (!existsSync(context.wrapper.capturePath)) return [];
  const rows = readFileSync(context.wrapper.capturePath, "utf8").split(/\r?\n/u);
  const invocations = [];
  let current = null;
  let noHistory = "";
  for (const row of rows) {
    if (row.startsWith("__NO_HISTORY__=")) {
      noHistory = row.slice("__NO_HISTORY__=".length);
      current = null;
    } else if (row === "__INVOCATION__") {
      current = { args: [], noHistory };
      invocations.push(current);
    } else if (current && row.length > 0) {
      current.args.push(row);
    }
  }
  return invocations;
}

function argvFacts(context, privateValues, model) {
  const captured = captureInvocations(context);
  const fixed = context.config.acpArgs;
  const matching = captured
    .filter((invocation) => fixed.every((argument) => invocation.args.includes(argument)));
  const forbidden = privateValues.filter((value) => typeof value === "string" && value.length > 0);
  return {
    invocationObserved: matching.length > 0,
    privateValuesAbsent: matching.length > 0
      && matching.every(({ args }) =>
        forbidden.every((value) => !args.some((arg) => arg.includes(value)))),
    noResumeArgument: matching.every(({ args }) =>
      !args.includes("--resume") && !args.includes("--continue")),
    noPersistenceArgument: matching.length > 0
      && matching.every(({ args }) => args.includes("--no-session-persistence")),
    genericModelForwarded: model.length > 0
      && matching.some(({ args }) =>
        args.some((value, index) => value === "--model" && args[index + 1] === model)),
    noPromptHistory: matching.length > 0
      && matching.every((invocation) => invocation.noHistory === "1"),
  };
}

export function strictHistoryProjection(history, sessionId, expectedTurns, maxBytes) {
  if (!history || typeof history !== "object" || Array.isArray(history)) return false;
  const expectedTopLevel = [
    "byteCount", "continuityScope", "nativeSessionId", "ok", "turnCount", "turns",
  ];
  if (Object.keys(history).sort().join("\n") !== expectedTopLevel.sort().join("\n")) return false;
  if (history.ok !== true
    || history.continuityScope !== "process-local"
    || history.nativeSessionId !== sessionId
    || !Array.isArray(history.turns)
    || history.turnCount !== history.turns.length
    || history.turnCount !== expectedTurns.length
    || history.turnCount > 64
    || !Number.isSafeInteger(history.byteCount)
    || history.byteCount < 0
    || history.byteCount > maxBytes) return false;
  const seen = new Set();
  let byteCount = 0;
  for (let index = 0; index < history.turns.length; index += 1) {
    const turn = history.turns[index];
    if (!turn || typeof turn !== "object" || Array.isArray(turn)
      || Object.keys(turn).sort().join("\n") !== "output\nturnId"
      || !validOpaqueId(turn.turnId)
      || seen.has(turn.turnId)
      || typeof turn.output !== "string"
      || turn.turnId !== expectedTurns[index]?.turnId
      || turn.output !== expectedTurns[index]?.output) return false;
    seen.add(turn.turnId);
    byteCount += Buffer.byteLength(turn.output);
  }
  return history.byteCount === byteCount;
}

export async function runProcessLocalRound(context, roundIndex, client, selfTestEvidence) {
  const firstPrompt = "Hi";
  const secondPrompt = "Hi";
  const model = context.parityModel || parityModelForAgent(context.config.id);
  let cleanupCount = 0;
  let requestCount = 0;
  let successfulRequestCount = 0;
  let sessionId = "";
  let facts = null;
  let errorCode = null;
  try {
    requireFact(model.length > 0, "process_local_model_required");
    requestCount += 1;
    const capabilities = await client.request("agent.conversation.capabilities", {
      agent: context.config.id,
    });
    successfulRequestCount += 1;
    requireFact(capabilities?.ok === true, resultErrorCode(capabilities));

    requestCount += 1;
    const opened = await client.request("agent.conversation.open", {
      agent: context.config.id,
      workingDirectory: context.cwd,
      binaryPath: context.wrapper.wrapperPath,
      model,
      acceptanceMode: context.acceptanceMode,
    });
    successfulRequestCount += 1;
    requireFact(opened?.ok === true && opened?.openMode === "new", resultErrorCode(opened));

    const sendParams = {
      agent: context.config.id,
      workingDirectory: context.cwd,
      binaryPath: context.wrapper.wrapperPath,
      model,
      acceptanceMode: context.acceptanceMode,
      streamEvents: true,
      timeoutMs: context.timeoutMs,
      maxStdoutBytes: context.maxOutputBytes,
      maxStderrBytes: context.maxOutputBytes,
    };
    requestCount += 1;
    const first = await client.streamConversation({ ...sendParams, text: firstPrompt });
    requireFact(first.result?.ok === true, resultErrorCode(first.result));
    successfulRequestCount += 1;
    sessionId = String(first.result?.nativeSessionId || first.result?.sessionId || "");
    requireFact(sessionId.length > 0 && sessionId.length <= 512, "process_local_session_id_invalid");
    const firstOutput = String(first.result?.output || "");
    requireFact(firstOutput.trim().length > 0, "process_local_final_message_missing");

    requestCount += 1;
    const resumed = await client.request("agent.conversation.open", {
      ...sendParams,
      sessionId,
    });
    successfulRequestCount += 1;
    requireFact(
      resumed?.ok === true
        && resumed?.openMode === "resume"
        && (resumed.nativeSessionId || resumed.sessionId) === sessionId,
      "process_local_resume_identity_mismatch",
    );

    requestCount += 1;
    const second = await client.streamConversation({
      ...sendParams,
      text: secondPrompt,
      sessionId,
    });
    requireFact(second.result?.ok === true, resultErrorCode(second.result));
    successfulRequestCount += 1;
    requireFact(
      (second.result?.nativeSessionId || second.result?.sessionId) === sessionId,
      "process_local_resume_identity_mismatch",
    );
    const secondOutput = String(second.result?.output || "");
    requireFact(secondOutput.trim().length > 0, "process_local_final_message_missing");

    requestCount += 1;
    const history = await client.request("agent.conversation.history", {
      agent: context.config.id,
      sessionId,
    });
    successfulRequestCount += 1;
    const historyBounded = strictHistoryProjection(history, sessionId, [
      { turnId: first.result?.turnId, output: firstOutput },
      { turnId: second.result?.turnId, output: secondOutput },
    ], context.maxOutputBytes);

    const argv = argvFacts(context, [
      sessionId,
      firstPrompt,
      secondPrompt,
      firstOutput,
      secondOutput,
      context.cwd,
      context.claudeConfigRoot,
      context.wrapper.wrapperPath,
      context.wrapper.capturePath,
    ], model);
    const cleanupBarrier = typeof context.armProcessLocalCleanupGate === "function"
      ? context.armProcessLocalCleanupGate()
      : null;
    requestCount += 1;
    const cleanup = await client.request("agent.conversation.cleanup", {
      agent: context.config.id,
      sessionId,
    });
    successfulRequestCount += 1;
    const cleanupBarrierReleased = cleanupBarrier ? await cleanupBarrier : null;
    const cleaned = cleanup?.ok === true && cleanup?.status === "cleaned";
    if (cleaned) cleanupCount += 1;

    requestCount += 1;
    const postCleanupResume = await client.request("agent.conversation.open", {
      ...sendParams,
      sessionId,
    });
    successfulRequestCount += 1;
    requestCount += 1;
    const postCleanupHistory = await client.request("agent.conversation.history", {
      agent: context.config.id,
      sessionId,
    });
    successfulRequestCount += 1;
    const resumeRejected = postCleanupResume?.ok === false
      && resultErrorCode(postCleanupResume) === "claude_code_live_session_unavailable";
    const historyCleared = postCleanupHistory?.ok === false
      && ["claude_code_session_unavailable", "claude_code_live_session_unavailable"]
        .includes(resultErrorCode(postCleanupHistory));
    const persistenceScan = scanBoundedNoFollow(
      context.claudeConfigRoot,
      [sessionId, firstPrompt, secondPrompt],
    );

    facts = {
      continuityScope: "process-local",
      persistentHost: true,
      openNew: true,
      exactSessionId: sessionId.length > 0,
      processLocalContinuation: firstOutput.trim().length > 0
        && secondOutput.trim().length > 0
        && first.result?.nativeSessionId === sessionId
        && second.result?.nativeSessionId === sessionId,
      orderedStreaming: eventOwnership(first, sessionId)
        && eventOwnership(second, sessionId, first.result?.turnId)
        && first.streamingSeen === true
        && second.streamingSeen === true,
      historyReadback: historyBounded,
      boundedHistory: historyBounded,
      cleanupVerified: cleaned && resumeRejected && historyCleared,
      cleanupSynchronized: cleaned
        && (typeof context.processLocalCleanupProbe === "function"
          ? cleanupBarrierReleased === true && context.processLocalCleanupProbe() === true
          : selfTestEvidence.processLocalCleanupSynchronized === true),
      registryAbsent: resumeRejected,
      historyCleared,
      hostLiveAfterCleanup: client.closed === false,
      argvPromptAbsent: argv.privateValuesAbsent,
      noResumeArgument: argv.noResumeArgument,
      noPersistenceArgument: argv.noPersistenceArgument,
      genericModelForwarded: argv.genericModelForwarded,
      noPromptHistory: context.environment.CLAUDE_CODE_SKIP_PROMPT_HISTORY === "1"
        && argv.noPromptHistory,
      noPersistedTranscript: persistenceScan.complete && !persistenceScan.found,
      boundedOutput: first.boundedOutput === true && second.boundedOutput === true,
      cancelNotAdvertised: capabilities?.capabilities?.cancel === false,
      structuredSeen: first.structuredSeen === true && second.structuredSeen === true,
      permissionFailClosed: selfTestEvidence.permissionFailClosed,
      errorFailClosed: selfTestEvidence.errorFailClosed,
      settingsDigest: digest({
        modelForwarded: argv.genericModelForwarded,
        continuityScope: history?.continuityScope,
      }),
    };
  } catch (error) {
    errorCode = error instanceof AcceptanceError ? error.code : "process_local_round_failed";
    if (sessionId) {
      try {
        const cleanup = await client.request("agent.conversation.cleanup", {
          agent: context.config.id,
          sessionId,
        });
        if (cleanup?.ok === true) cleanupCount += 1;
      } catch {
        // The stable error code above remains the only public failure detail.
      }
    }
  }
  const ready = processLocalRoundFactsReady(facts);
  return {
    ready,
    conversationReady: ready,
    roundIndex,
    continuityScope: "process-local",
    cleanupCount,
    cleanupVerified: facts?.cleanupVerified === true,
    testedSessions: sessionId ? 1 : 0,
    requestCount,
    successfulRequestCount,
    errorCode: ready ? null : (errorCode || failedProcessLocalFactCode(facts)),
    facts,
  };
}

export async function exerciseProcessLocalHostDrain(context, client, termination) {
  const model = context.parityModel || parityModelForAgent(context.config.id);
  requireFact(model.length > 0, "process_local_model_required");
  const turn = await client.streamConversation({
    agent: context.config.id,
    text: "Hi",
    workingDirectory: context.cwd,
    binaryPath: context.wrapper.wrapperPath,
    model,
    acceptanceMode: context.acceptanceMode,
    streamEvents: true,
    timeoutMs: context.timeoutMs,
    maxStdoutBytes: context.maxOutputBytes,
    maxStderrBytes: context.maxOutputBytes,
  });
  requireFact(String(turn.result?.output || "").trim().length > 0, "process_local_drain_setup_failed");
  const closeBarrier = typeof context.armProcessLocalCleanupGate === "function"
    ? context.armProcessLocalCleanupGate()
    : null;
  const closed = termination === "shutdown"
    ? await client.shutdown()
    : await client.closeInputAndWait();
  const closeBarrierReleased = closeBarrier ? await closeBarrier : true;
  const scan = scanBoundedNoFollow(
    context.claudeConfigRoot,
    [turn.result?.nativeSessionId],
  );
  return closed.exited === true
    && closed.statusCode === 0
    && closeBarrierReleased === true
    && scan.complete === true
    && scan.found === false;
}
