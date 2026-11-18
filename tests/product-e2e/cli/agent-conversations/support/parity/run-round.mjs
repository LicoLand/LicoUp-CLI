import { existsSync, readFileSync } from "node:fs";
import { parityEffortForAgent, parityModelForAgent } from "./agent-ids.mjs";
import { sidecarArgs, verificationTurnCount } from "./constants.mjs";
import { AcceptanceError, digest, requireFact, stableJson } from "./errors.mjs";
import { nativeReadback, runSidecar } from "./native/acp-turn.mjs";
import { arcSettings } from "./native/pi.mjs";
import { cleanupSession } from "./session-cleanup.mjs";
import { listSessions, officialHistory } from "./session-query.mjs";
import {
  failedParityFactCode,
  outputCategoryCode,
  roundConversationFactsReady,
  roundFactsReady,
} from "./round-facts.mjs";

function sidecarBinaryPath(context) {
  return context.sidecarBinaryPath
    || (context.config.promptInArguments ? context.binary : context.wrapper.wrapperPath);
}

function sidecarSessionId(result) {
  return result?.nativeSessionId || result?.sessionId || result?.threadId || "";
}

function verificationRequest(context, text, sessionId, model, reasoningEffort) {
  return {
    agent: context.config.id,
    text,
    ...(sessionId ? { sessionId } : {}),
    workingDirectory: context.cwd,
    binaryPath: sidecarBinaryPath(context),
    timeoutMs: context.timeoutMs,
    maxStdoutBytes: context.maxOutputBytes,
    maxStderrBytes: context.maxOutputBytes,
    streamEvents: true,
    ...(model ? { model } : {}),
    ...(reasoningEffort ? { reasoningEffort } : {}),
  };
}

export async function runRound(context, roundIndex, selfTestEvidence) {
  const knownSessions = new Set();
  const previousObservedSessions = context.observedSessions;
  context.observedSessions = knownSessions;
  let cleanupCount = 0;
  let cleanupVerified = false;
  let testedSessions = 0;
  let requestCount = 0;
  let successfulRequestCount = 0;
  let facts;
  let roundError = null;
  try {
    requireFact(verificationTurnCount === 2, "verification_turn_contract_invalid");
    const forcedModel = context.parityModel || parityModelForAgent(context.config.id);
    const forcedCodexModel = context.config.id === "codex" ? forcedModel : "";
    const forcedCodexEffort = context.config.id === "codex"
      ? parityEffortForAgent("codex", forcedCodexModel)
      : "";

    requestCount += 1;
    const created = await runSidecar(context, verificationRequest(
      context,
      "Hi",
      "",
      forcedModel,
      forcedCodexEffort,
    ));
    successfulRequestCount += 1;
    const sessionId = sidecarSessionId(created.result);
    requireFact(typeof sessionId === "string" && sessionId.length > 0, "arc_session_id_missing");
    knownSessions.add(sessionId);
    testedSessions = 1;

    requestCount += 1;
    const resumed = await runSidecar(context, verificationRequest(
      context,
      "Hi",
      sessionId,
      forcedModel,
      forcedCodexEffort,
    ));
    successfulRequestCount += 1;
    requireFact(sidecarSessionId(resumed.result) === sessionId, "exact_resume_session_mismatch");

    const readback = await nativeReadback(context, sessionId);
    const official = await officialHistory(context, sessionId, context.temporaryDirectory);
    const history = `${readback.text}\n${official}`;
    const capture = existsSync(context.wrapper.capturePath)
      ? readFileSync(context.wrapper.capturePath, "utf8")
      : "";
    const fixedPromptAbsent = !context.config.acpArgs.some((argument) => argument === "Hi")
      && !sidecarArgs.some((argument) => argument === "Hi");
    const capturedPrompt = capture.split(/\r?\n/u).some((line) => line === "Hi");
    const argvPromptAbsent = fixedPromptAbsent
      && (context.config.promptInArguments || !capturedPrompt);

    const createdSettings = arcSettings(created.result);
    const resumedSettings = arcSettings(resumed.result);
    const readbackAvailable = readback.readbackAvailable !== false;
    const readbackSettings = readback.settings && typeof readback.settings === "object"
      ? readback.settings
      : {};
    const settingsKeys = ["cwd", "model", "reasoningEffort", "mode", "runtimeAgent", "allowAll"];
    const settingsParityMask = settingsKeys
      .map((key) => [createdSettings, resumedSettings]
        .every((entry) => readbackAvailable
          && stableJson(entry[key]) === stableJson(readbackSettings[key])) ? "1" : "0")
      .join("");
    const settingsParity = settingsParityMask === "111111";
    const createdOutput = String(created.result.output || "");
    const resumedOutput = String(resumed.result.output || "");
    requireFact(createdOutput.trim().length > 0, "native_final_message_missing");
    requireFact(resumedOutput.trim().length > 0, "native_final_message_missing");
    const nativeHistoryContainsOutput = readbackAvailable
      && history.includes(createdOutput)
      && history.includes(resumedOutput);

    facts = {
      openNew: sessionId.length > 0,
      exactResume: sidecarSessionId(resumed.result) === sessionId,
      nativeToArc: created.result.sessionId === sessionId
        && created.result.threadId === sessionId,
      arcToNative: nativeHistoryContainsOutput,
      realSessionIds: sessionId.length > 0,
      nativeFirstOutputCategory: outputCategoryCode(createdOutput),
      arcResumeOutputCategory: outputCategoryCode(resumedOutput),
      rawResponses: createdOutput.trim().length > 0 && resumedOutput.trim().length > 0,
      cwdParity: createdSettings.cwd === context.cwd
        && resumedSettings.cwd === context.cwd
        && readbackAvailable
        && readbackSettings.cwd === context.cwd,
      settingsParity,
      settingsParityMask,
      argvPromptAbsent,
      historyReadback: nativeHistoryContainsOutput,
      noPermissionRequests: true,
      noUnsupportedRequests: true,
      boundedOutput: created.boundedOutput && resumed.boundedOutput && readback.boundedOutput,
      streamingSeen: created.streamingSeen === true && resumed.streamingSeen === true,
      structuredSeen: created.structuredSeen === true && resumed.structuredSeen === true,
      cleanupVerified: false,
      permissionFailClosed: selfTestEvidence.permissionFailClosed,
      errorFailClosed: selfTestEvidence.errorFailClosed,
      settingsDigest: digest([createdSettings, resumedSettings, readbackSettings]),
      turnOutputBytes: [
        Buffer.byteLength(createdOutput),
        Buffer.byteLength(resumedOutput),
      ],
    };
  } catch (error) {
    roundError = error instanceof AcceptanceError ? error : new AcceptanceError("unexpected_failure");
  } finally {
    let allDeleted = true;
    for (const sessionId of knownSessions) {
      const deleted = await cleanupSession(context, sessionId, context.temporaryDirectory);
      if (deleted) cleanupCount += 1;
      else allDeleted = false;
    }
    try {
      const verified = await listSessions(context);
      cleanupVerified = allDeleted
        && [...knownSessions].every((sessionId) => !verified.has(sessionId));
    } catch {
      cleanupVerified = false;
    }
    context.observedSessions = previousObservedSessions;
  }
  if (!facts) {
    return {
      ready: false,
      roundIndex,
      cleanupCount,
      cleanupVerified,
      testedSessions,
      requestCount,
      successfulRequestCount,
      errorCode: roundError?.code || "round_failed",
      facts: null,
    };
  }
  facts.cleanupVerified = cleanupVerified;
  const ready = roundFactsReady(facts)
    && facts.permissionFailClosed
    && facts.errorFailClosed;
  const conversationReady = roundConversationFactsReady(facts)
    && facts.permissionFailClosed
    && facts.errorFailClosed;
  return {
    ready,
    conversationReady,
    roundIndex,
    cleanupCount,
    cleanupVerified,
    testedSessions,
    requestCount,
    successfulRequestCount,
    errorCode: ready ? null : (roundError?.code || failedParityFactCode(facts)),
    facts,
  };
}
