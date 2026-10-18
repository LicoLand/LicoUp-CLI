import path from "node:path";
import {
  reduceCapabilityFacts,
  validateCapabilityReport,
} from "../lib/secure-mesh-capability-report.mjs";
import { atomicWriteReportJson } from "../lib/safe-report-io.mjs";
import { reportSchemaVersion, repoRoot, VERIFIER_REF } from "./constants.mjs";
import { assertNoLeak, sanitizeError } from "./privacy.mjs";

export function failureReport(error, configuredReportRef) {
  const capabilityFacts = [];
  const capabilityReport = reduceCapabilityFacts(capabilityFacts);
  validateCapabilityReport(capabilityReport);
  return {
    schemaVersion: reportSchemaVersion,
    verifier: VERIFIER_REF,
    generatedAt: new Date().toISOString(),
    report: configuredReportRef,
    platform: "macos",
    artifactKind: "macos-adaptive-custody-capability-proof",
    proofScope: "local_custody_only",
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawRuntimeOutputIncluded: false,
    interactionPolicy: {
      maximumInteractiveAuthorizationAttemptsPerProof: 1,
      backgroundInteractiveAuthorizationAttempts: 0,
      automaticRetryAllowed: false,
    },
    ok: false,
    capabilityFacts,
    capabilityReport,
    observed: {},
    summary: {
      exactCapabilitySetValid: true,
      safeOsStoreAvailable: false,
      standardKeychainAvailable: false,
      dataProtectionKeychainAvailable: false,
      strongestObservedKeychainConfiguration: "memory_only_ephemeral",
      promptBudgetSatisfied: false,
      adaptiveCustodyProofReady: false,
    },
    failure: {
      code: "macos_adaptive_custody_proof_failed",
      sanitized: sanitizeError(error),
    },
  };
}

export function writeReport(configuredReportRef, report) {
  assertNoLeak(report, "secure mesh macOS adaptive custody proof report");
  atomicWriteReportJson(repoRoot, configuredReportRef, report);
}

export function normalizeReportReference(value) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref.startsWith("build/") || path.isAbsolute(ref) || ref.includes("\0") ||
    ref.split("/").some((component) => !component || component === "." || component === "..")) {
    throw new Error("macos_adaptive_custody_report_ref_invalid");
  }
  return ref;
}
