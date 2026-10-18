export function outputCategoryCode(value) {
  const output = String(value || "").toLowerCase();
  const categories = [
    ["a", /auth|login|credential|unauthorized|token/u],
    ["q", /quota|rate.?limit|usage.?limit/u],
    ["p", /permission|sandbox|denied|forbidden/u],
    ["s", /server|service|internal|unavailable|network|connect/u],
    ["r", /cannot|can't|unable|refus|policy/u],
  ];
  return categories.find(([, pattern]) => pattern.test(output))?.[0] || "o";
}

export function roundFactsReady(facts) {
  return roundConversationFactsReady(facts)
    && facts.cleanupVerified;
}

export const processLocalBooleanFactKeys = Object.freeze([
  "persistentHost",
  "openNew",
  "exactSessionId",
  "processLocalContinuation",
  "orderedStreaming",
  "historyReadback",
  "boundedHistory",
  "cleanupVerified",
  "cleanupSynchronized",
  "registryAbsent",
  "historyCleared",
  "hostLiveAfterCleanup",
  "argvPromptAbsent",
  "noResumeArgument",
  "noPersistenceArgument",
  "genericModelForwarded",
  "noPromptHistory",
  "noPersistedTranscript",
  "boundedOutput",
  "cancelNotAdvertised",
  "structuredSeen",
  "permissionFailClosed",
  "errorFailClosed",
]);

export function processLocalRoundFactsReady(facts) {
  return facts?.continuityScope === "process-local"
    && processLocalBooleanFactKeys.every((key) => facts[key] === true);
}

export function processLocalHostShutdownEvidence(options) {
  const complete = options !== null
    && typeof options === "object"
    && Object.hasOwn(options, "hostShutdownPassed")
    && typeof options.hostShutdownPassed === "boolean";
  return {
    complete,
    passed: complete && options.hostShutdownPassed === true,
  };
}

export function failedProcessLocalFactCode(facts) {
  const orderedFacts = [
    ["persistentHost", "persistent_host"],
    ["openNew", "open_new"],
    ["exactSessionId", "exact_session"],
    ["processLocalContinuation", "continuation"],
    ["orderedStreaming", "streaming"],
    ["historyReadback", "history"],
    ["boundedHistory", "history_bound"],
    ["cleanupVerified", "cleanup"],
    ["cleanupSynchronized", "cleanup_sync"],
    ["registryAbsent", "registry"],
    ["historyCleared", "history_clear"],
    ["hostLiveAfterCleanup", "host_liveness"],
    ["argvPromptAbsent", "argv_privacy"],
    ["noResumeArgument", "argv_resume"],
    ["noPersistenceArgument", "no_persistence"],
    ["genericModelForwarded", "model_forwarding"],
    ["noPromptHistory", "prompt_history"],
    ["noPersistedTranscript", "disk_persistence"],
    ["boundedOutput", "bounded_output"],
    ["cancelNotAdvertised", "cancel_capability"],
    ["structuredSeen", "structured_event"],
    ["permissionFailClosed", "permission"],
    ["errorFailClosed", "error"],
  ];
  const failed = orderedFacts.find(([key]) => facts?.[key] !== true);
  return failed ? `process_local_${failed[1]}_failed` : "process_local_fact_failed";
}

export function roundConversationFactsReady(facts) {
  return facts.openNew
    && facts.exactResume
    && facts.nativeToArc
    && facts.arcToNative
    && facts.realSessionIds
    && facts.rawResponses
    && facts.cwdParity
    && facts.settingsParity
    && facts.argvPromptAbsent
    && facts.historyReadback
    && facts.noPermissionRequests
    && facts.noUnsupportedRequests
    && facts.boundedOutput
    && facts.streamingSeen;
}

export function failedParityFactCode(facts) {
  if (facts.rawResponses !== true) return "parity_raw_responses_failed";
  if (facts.settingsParity !== true && /^[01]{6}$/u.test(facts.settingsParityMask || "")) {
    return `parity_settings_m${facts.settingsParityMask}_failed`;
  }
  const orderedFacts = [
    ["openNew", "open_new"],
    ["exactResume", "exact_resume"],
    ["nativeToArc", "native_to_arc"],
    ["arcToNative", "arc_to_native"],
    ["realSessionIds", "real_session_ids"],
    ["rawResponses", "raw_responses"],
    ["cwdParity", "cwd_parity"],
    ["settingsParity", "settings_parity"],
    ["argvPromptAbsent", "argv_privacy"],
    ["historyReadback", "history_readback"],
    ["noPermissionRequests", "permission_request"],
    ["noUnsupportedRequests", "unsupported_request"],
    ["boundedOutput", "bounded_output"],
    ["streamingSeen", "streaming"],
    ["cleanupVerified", "cleanup"],
  ];
  const failed = orderedFacts.find(([key]) => facts[key] !== true);
  return failed ? `parity_${failed[1]}_failed` : "parity_fact_failed";
}
