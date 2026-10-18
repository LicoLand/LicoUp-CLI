import { strictRoundCount } from "./constants.mjs";
import { digest, safeErrorCode } from "./errors.mjs";
import {
  failedProcessLocalFactCode,
  processLocalHostShutdownEvidence,
  processLocalBooleanFactKeys,
  processLocalRoundFactsReady,
} from "./round-facts.mjs";

export function aggregateProcessLocalResult(
  agentId,
  strict,
  packaged,
  rounds,
  selfTestEvidence,
  options = {},
) {
  const releaseUi = options.releaseUi === true;
  const expectedRounds = strict ? strictRoundCount : 1;
  const completed = rounds.filter((round) => round.ready).length;
  const facts = rounds.map((round) => round.facts).filter(Boolean);
  const every = (key) => facts.length === expectedRounds
    && facts.every((entry) => entry[key] === true);
  const oracleEvidenceComplete = selfTestEvidence !== null
    && typeof selfTestEvidence === "object"
    && Object.hasOwn(selfTestEvidence, "processLocalOraclePassed")
    && typeof selfTestEvidence.processLocalOraclePassed === "boolean";
  const oraclePassed = oracleEvidenceComplete
    && selfTestEvidence.processLocalOraclePassed === true;
  const processLocalFactsEvidenceComplete = facts.length === expectedRounds
    && facts.every((entry) => entry !== null
      && typeof entry === "object"
      && Object.hasOwn(entry, "continuityScope")
      && typeof entry.continuityScope === "string"
      && entry.continuityScope.length > 0
      && processLocalBooleanFactKeys.every((key) =>
        Object.hasOwn(entry, key) && typeof entry[key] === "boolean"));
  const processLocalFactsPassed = processLocalFactsEvidenceComplete
    && facts.every(processLocalRoundFactsReady);
  const roundsPassed = completed === expectedRounds
    && rounds.length === expectedRounds
    && processLocalFactsPassed;
  const hostShutdown = processLocalHostShutdownEvidence(options);
  const corePassed = roundsPassed && oraclePassed && hostShutdown.passed;
  const result = {
    status: corePassed ? (releaseUi ? "release-ui-passed" : "core-passed") : "failed",
    agent: agentId,
    strict,
    packaged,
    continuityScope: "process-local",
    processLocalOracleEvidenceComplete: oracleEvidenceComplete,
    processLocalOraclePassed: oraclePassed,
    processLocalFactsEvidenceComplete,
    processLocalFactsPassed,
    processLocalContinuation: every("processLocalContinuation"),
    persistentHost: every("persistentHost"),
    hostShutdownEvidenceComplete: hostShutdown.complete,
    hostShutdownPassed: hostShutdown.passed,
    cl06Ready: false,
    conversationGatePassed: releaseUi && corePassed,
    roundsRequired: expectedRounds,
    roundsCompleted: completed,
    conversationRoundsCompleted: rounds.filter((round) => round.conversationReady).length,
    conversationPassed: roundsPassed,
    roundsAttempted: rounds.length,
    testedSessions: rounds.reduce((total, round) => total + (round.testedSessions || 0), 0),
    requestCount: rounds.reduce((total, round) => total + (round.requestCount || 0), 0),
    successfulRequestCount: rounds.reduce(
      (total, round) => total + (round.successfulRequestCount || 0),
      0,
    ),
    roundResults: rounds.map((round) => ({
      round: round.roundIndex,
      continuityScope: "process-local",
      conversationPassed: round.conversationReady === true,
      cleanupPassed: round.cleanupVerified === true,
      testedSessions: round.testedSessions || 0,
      requestCount: round.requestCount || 0,
      successfulRequestCount: round.successfulRequestCount || 0,
      streamingPassed: round.facts?.orderedStreaming === true,
      errorCode: round.errorCode || null,
    })),
    consecutivePasses: releaseUi && corePassed ? completed : 0,
    officialNativeLane: every("persistentHost") && every("processLocalContinuation"),
    nativeToArc: false,
    arcToNative: false,
    realSessionIds: every("exactSessionId"),
    rawResponses: every("processLocalContinuation"),
    cwdParity: true,
    settingsParity: every("genericModelForwarded"),
    argvPromptAbsent: every("argvPromptAbsent"),
    historyReadback: every("historyReadback"),
    quiescenceOraclePassed: selfTestEvidence?.quiescenceOraclePassed === true,
    publicStreamChunkOracleEvidenceComplete:
      typeof selfTestEvidence?.publicStreamChunkOraclePassed === "boolean",
    publicStreamChunkOraclePassed: selfTestEvidence?.publicStreamChunkOraclePassed === true,
    permissionFailClosed: selfTestEvidence?.permissionFailClosed === true,
    errorFailClosed: selfTestEvidence?.errorFailClosed === true,
    boundedOutput: every("boundedOutput"),
    cleanupVerified: every("cleanupVerified"),
    cleanupPassed: every("cleanupVerified"),
    privacyPassed: every("argvPromptAbsent")
      && every("noResumeArgument")
      && every("noPersistenceArgument")
      && every("noPromptHistory")
      && every("noPersistedTranscript"),
    streamingEvidenceComplete: facts.length === expectedRounds
      && facts.every((entry) => typeof entry.orderedStreaming === "boolean"),
    streamingProven: every("orderedStreaming"),
    structuredProven: every("structuredSeen"),
    cleanupCount: rounds.reduce((total, round) => total + round.cleanupCount, 0),
    errorCode: rounds.find((round) => round.ready !== true)?.errorCode
      || (!oraclePassed ? "process_local_oracle_failed" : null)
      || (!hostShutdown.passed ? "process_local_host_shutdown_failed" : null)
      || (facts[0] ? failedProcessLocalFactCode(facts[0]) : "process_local_round_failed"),
    evidenceDigest: "",
  };
  if (corePassed) result.errorCode = null;
  result.evidenceDigest = digest({ ...result, evidenceDigest: undefined });
  return result;
}

export function aggregateResult(agentId, strict, packaged, rounds, selfTestEvidence, options = {}) {
  const releaseUi = options.releaseUi === true;
  const expectedRounds = strict ? strictRoundCount : 1;
  const completed = rounds.filter((round) => round.ready).length;
  const conversationCompleted = rounds.filter((round) => round.conversationReady).length;
  const allFacts = rounds.map((round) => round.facts).filter(Boolean);
  const every = (key) => allFacts.length === expectedRounds && allFacts.every((facts) => facts[key] === true);
  const quiescenceOraclePassed = selfTestEvidence?.quiescenceOraclePassed === true;
  const publicStreamChunkOracleEvidenceComplete = selfTestEvidence !== null
    && typeof selfTestEvidence === "object"
    && Object.hasOwn(selfTestEvidence, "publicStreamChunkOraclePassed")
    && typeof selfTestEvidence.publicStreamChunkOraclePassed === "boolean";
  const publicStreamChunkOraclePassed = publicStreamChunkOracleEvidenceComplete
    && selfTestEvidence.publicStreamChunkOraclePassed === true;
  const streamingEvidenceComplete = allFacts.length === expectedRounds
    && allFacts.every((facts) => Object.hasOwn(facts, "streamingSeen")
      && typeof facts.streamingSeen === "boolean");
  const streamingProven = streamingEvidenceComplete && every("streamingSeen");
  const corePassed = completed === expectedRounds
    && quiescenceOraclePassed
    && publicStreamChunkOraclePassed
    && streamingProven;
  // consecutivePasses / conversationGatePassed / P-10 only advance under --release-ui.
  // Core-only --strict proves lane semantics and must never look like CL-06 ready.
  const result = {
    status: corePassed ? (releaseUi ? "release-ui-passed" : "core-passed") : "failed",
    agent: agentId,
    strict,
    packaged,
    cl06Ready: false,
    conversationGatePassed: releaseUi && corePassed,
    roundsRequired: expectedRounds,
    roundsCompleted: completed,
    conversationRoundsCompleted: conversationCompleted,
    conversationPassed: conversationCompleted === expectedRounds,
    roundsAttempted: rounds.length,
    testedSessions: rounds.reduce((total, round) => total + (round.testedSessions || 0), 0),
    requestCount: rounds.reduce((total, round) => total + (round.requestCount || 0), 0),
    successfulRequestCount: rounds.reduce((total, round) => total + (round.successfulRequestCount || 0), 0),
    roundResults: rounds.map((round) => ({
      round: round.roundIndex,
      conversationPassed: round.conversationReady === true,
      cleanupPassed: round.cleanupVerified === true,
      testedSessions: round.testedSessions || 0,
      requestCount: round.requestCount || 0,
      successfulRequestCount: round.successfulRequestCount || 0,
      streamingPassed: round.facts?.streamingSeen === true,
      errorCode: round.errorCode || null,
    })),
    consecutivePasses: releaseUi && corePassed ? completed : 0,
    officialNativeLane: every("nativeToArc") && every("arcToNative"),
    openNew: every("openNew"),
    exactResume: every("exactResume"),
    nativeToArc: every("nativeToArc"),
    arcToNative: every("arcToNative"),
    realSessionIds: every("realSessionIds"),
    rawResponses: every("rawResponses"),
    cwdParity: every("cwdParity"),
    settingsParity: every("settingsParity"),
    argvPromptAbsent: every("argvPromptAbsent"),
    historyReadback: every("historyReadback"),
    quiescenceOraclePassed,
    publicStreamChunkOracleEvidenceComplete,
    publicStreamChunkOraclePassed,
    permissionFailClosed: selfTestEvidence.permissionFailClosed,
    errorFailClosed: selfTestEvidence.errorFailClosed,
    boundedOutput: every("boundedOutput") && selfTestEvidence.boundedOutputFailClosed,
    cleanupVerified: rounds.length === expectedRounds && rounds.every((round) => round.cleanupVerified),
    cleanupPassed: rounds.length === expectedRounds && rounds.every((round) => round.cleanupVerified),
    privacyPassed: every("argvPromptAbsent") && every("boundedOutput"),
    streamingEvidenceComplete,
    streamingProven,
    structuredProven: every("structuredSeen"),
    cleanupCount: rounds.reduce((total, round) => total + round.cleanupCount, 0),
    errorCode: rounds.find((round) => round.conversationReady !== true)?.errorCode
      || rounds.find((round) => !round.ready)?.errorCode
      || (!streamingProven ? "parity_streaming_failed" : null)
      || (!quiescenceOraclePassed ? "parity_quiescence_oracle_failed" : null)
      || (!publicStreamChunkOraclePassed ? "parity_stream_chunk_oracle_failed" : null)
      || null,
    evidenceDigest: "",
  };
  result.evidenceDigest = digest({ ...result, evidenceDigest: undefined });
  return result;
}

export function blockedResult(agentId, strict, packaged, code, selfTestEvidence) {
  const publicStreamChunkOracleEvidenceComplete = selfTestEvidence !== null
    && typeof selfTestEvidence === "object"
    && Object.hasOwn(selfTestEvidence, "publicStreamChunkOraclePassed")
    && typeof selfTestEvidence.publicStreamChunkOraclePassed === "boolean";
  const result = {
    status: "blocked",
    agent: agentId,
    strict,
    packaged,
    cl06Ready: false,
    conversationGatePassed: false,
    roundsRequired: strict ? strictRoundCount : 1,
    roundsCompleted: 0,
    conversationRoundsCompleted: 0,
    conversationPassed: false,
    roundsAttempted: 0,
    testedSessions: 0,
    requestCount: 0,
    successfulRequestCount: 0,
    roundResults: [],
    consecutivePasses: 0,
    officialNativeLane: false,
    nativeToArc: false,
    arcToNative: false,
    realSessionIds: false,
    rawResponses: false,
    cwdParity: false,
    settingsParity: false,
    argvPromptAbsent: false,
    historyReadback: false,
    quiescenceOraclePassed: selfTestEvidence?.quiescenceOraclePassed === true,
    publicStreamChunkOracleEvidenceComplete,
    publicStreamChunkOraclePassed: publicStreamChunkOracleEvidenceComplete
      && selfTestEvidence.publicStreamChunkOraclePassed === true,
    permissionFailClosed: selfTestEvidence.permissionFailClosed,
    errorFailClosed: selfTestEvidence.errorFailClosed,
    boundedOutput: selfTestEvidence.boundedOutputFailClosed,
    cleanupVerified: false,
    cleanupPassed: false,
    privacyPassed: selfTestEvidence.boundedOutputFailClosed,
    streamingEvidenceComplete: false,
    streamingProven: false,
    structuredProven: false,
    cleanupCount: 0,
    errorCode: safeErrorCode(code),
    evidenceDigest: "",
  };
  result.evidenceDigest = digest({ ...result, evidenceDigest: undefined });
  return result;
}
