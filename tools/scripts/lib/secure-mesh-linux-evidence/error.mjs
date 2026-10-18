import {
  LINUX_EVIDENCE_FAILURE_CATEGORIES,
  LINUX_EVIDENCE_RULE_ID,
} from "./constants.mjs";

export class LinuxEvidenceValidationError extends Error {
  constructor(ruleId, category) {
    super("Linux evidence validation failed");
    this.name = "LinuxEvidenceValidationError";
    this.ruleId = ruleId;
    this.category = category;
  }
}

export function assertRule(condition, ruleId, category) {
  if (!condition) throw new LinuxEvidenceValidationError(ruleId, category);
}

export function classifyLinuxEvidenceValidationFailure(
  error,
  fallbackRuleId = "linux_evidence_validation_unclassified",
) {
  const ruleId = error instanceof LinuxEvidenceValidationError &&
    LINUX_EVIDENCE_RULE_ID.test(String(error.ruleId || ""))
    ? error.ruleId
    : fallbackRuleId;
  const category = error instanceof LinuxEvidenceValidationError &&
    LINUX_EVIDENCE_FAILURE_CATEGORIES.has(error.category)
    ? error.category
    : "schema";
  return Object.freeze({ ruleId, category });
}
