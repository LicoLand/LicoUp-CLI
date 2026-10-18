import path from "node:path";
import process from "node:process";
import {
  linuxEvidencePrivacyRecord,
  linuxEvidenceSchemaVersion,
  linuxNodeMatrixSchema,
} from "../lib/secure-mesh-linux-evidence.mjs";
import { atomicWriteReportJson } from "../lib/safe-report-io.mjs";
import { assert } from "./assert.mjs";
import { requiredFile } from "./util.mjs";

export function requiredOption(options, name) {
  const value = String(options[name] || "").trim();
  assert(value, "Linux node matrix option is missing");
  return value;
}

export function safeReportDestination(options) {
  const rootValue = String(process.env.LICO_LINUX_VM_REPORT_ROOT || "").trim();
  assert(rootValue, "Linux node matrix report root is missing");
  const root = path.resolve(rootValue);
  const target = path.resolve(requiredOption(options, "report"));
  const relative = path.relative(root, target);
  assert(relative && !relative.startsWith("..") && !path.isAbsolute(relative),
    "Linux node matrix report path escapes its allowed root");
  return { root, ref: relative };
}

export function writeReport(options, report) {
  const { root, ref } = safeReportDestination(options);
  atomicWriteReportJson(root, ref, report);
}

export function writeFailureReport(options, verificationPhase, reason) {
  if (!options.report) return;
  const { root, ref } = safeReportDestination(options);
  atomicWriteReportJson(root, ref, {
    schema: linuxNodeMatrixSchema,
    schemaVersion: linuxEvidenceSchemaVersion,
    ok: false,
    artifactKind: "linux-current-client-node-matrix",
    reason,
    phase: verificationPhase,
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    privacy: linuxEvidencePrivacyRecord()
  });
}
