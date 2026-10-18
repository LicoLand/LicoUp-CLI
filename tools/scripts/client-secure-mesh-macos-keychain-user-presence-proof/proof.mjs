import { writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import {
  reduceCapabilityFacts,
  validateCapabilityReport,
} from "../lib/secure-mesh-capability-report.mjs";
import { reportSchemaVersion, VERIFIER_REF } from "./constants.mjs";
import { createCapabilityFacts } from "./capability/facts.mjs";
import { summarize, observedProjection } from "./capability/summarize.mjs";
import { buildSignedSwiftHelper } from "./helper/codesign.mjs";
import { runSwiftProof } from "./helper/run-swift.mjs";
import { swiftSource } from "./helper/swift-source.mjs";
import { parseJsonOutput } from "./parse.mjs";

export function runProof({ tempDir, configuredReportRef, options }) {
  if (process.platform !== "darwin") {
    throw new Error("macOS adaptive custody proof requires a macOS host");
  }
  const swiftPath = path.join(tempDir, "MacosAdaptiveCustodyProof.swift");
  writeFileSync(swiftPath, swiftSource(), "utf8");
  const helper = buildSignedSwiftHelper(swiftPath, { tempDir, options });
  if (!helper.signatureValid || !helper.path) {
    throw new Error(`signed helper unavailable: ${helper.failureCode}`);
  }
  const payload = parseJsonOutput(runSwiftProof(helper, options).stdout);
  const facts = createCapabilityFacts(payload);
  const capabilityReport = reduceCapabilityFacts(facts);
  const capabilityValidation = validateCapabilityReport(capabilityReport);
  const summary = summarize(payload, helper, capabilityReport, capabilityValidation);

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
    ok: summary.adaptiveCustodyProofReady,
    capabilityFacts: facts,
    capabilityReport,
    observed: observedProjection(payload, helper),
    summary,
  };
}
