const REQUIRED_AUTHORITY = ["target", "content", "scope"];

function present(value) {
  return typeof value === "string" && value.trim().length > 0;
}

export function admitLiveEvaluation(request = {}) {
  const mode = request.mode === "live" ? "live" : "offline";
  if (mode !== "live") {
    return {
      admitted: true,
      mode: "offline",
      code: null,
      stage: "continuity/qualification",
      recovery: null,
      effectClass: "none",
      externalCalls: 0,
      effectsStarted: false,
      passingCandidateReceipt: false,
      candidateReceipt: null,
    };
  }

  const authority = request.authority ?? {};
  const missingAuthority = REQUIRED_AUTHORITY.filter((field) => !present(authority[field]));
  if (missingAuthority.length > 0) {
    return {
      admitted: false,
      mode: "live",
      code: "approval_required",
      stage: "continuity/qualification",
      recovery: "obtain_approval",
      effectClass: "none",
      missing: missingAuthority,
      externalCalls: 0,
      effectsStarted: false,
      passingCandidateReceipt: false,
      candidateReceipt: null,
    };
  }

  const spendCeiling = request.budget?.spendCeiling;
  if (typeof spendCeiling !== "number" || !(spendCeiling > 0)) {
    return {
      admitted: false,
      mode: "live",
      code: "budget_exhausted",
      stage: "continuity/qualification",
      recovery: "obtain_approval",
      effectClass: "none",
      missing: ["spendCeiling"],
      externalCalls: 0,
      effectsStarted: false,
      passingCandidateReceipt: false,
      candidateReceipt: null,
    };
  }

  return {
    admitted: true,
    mode: "live",
    code: null,
    stage: "continuity/qualification",
    recovery: null,
    effectClass: "none",
    reason: "trusted-admission-required",
    realModelQualification: "unknown",
    rustRequired: true,
    externalCalls: 0,
    effectsStarted: false,
    passingCandidateReceipt: false,
    candidateReceipt: null,
  };
}
