import {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} from "./config.mjs";
import { reportRecord } from "./lists.mjs";

export function releaseInputIntegrity(report = {}, {
  schemaVersion,
  verifier,
  generatedBy = verifier,
  blocker: expectedBlocker = "physical device matrix"
} = {}) {
  report = reportRecord(report);
  const present = Boolean(report && Object.keys(report).length > 0);
  const failures = [];
  if (!present) {
    failures.push("report_present");
  }
  if (schemaVersion && report?.schemaVersion !== schemaVersion) {
    failures.push("schemaVersion");
  }
  if (report?.evidenceRefSchemaVersion !== SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION) {
    failures.push("evidenceRefSchemaVersion");
  }
  if (report?.sourceOfTruth !== SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH) {
    failures.push("sourceOfTruth");
  }
  if (verifier && report?.verifier !== verifier) {
    failures.push("verifier");
  }
  if (generatedBy && report?.generatedBy !== generatedBy) {
    failures.push("generatedBy");
  }
  if (expectedBlocker && report?.blocker !== expectedBlocker) {
    failures.push("blocker");
  }
  if (report?.redacted !== true || report?.reportLeakScan !== true) {
    failures.push("redaction");
  }
  if (
    report?.rawPrivateMaterialIncluded === true ||
    report?.rawPlaintextIncluded === true ||
    report?.rawPublicWireBytesIncluded === true
  ) {
    failures.push("rawMaterialFlags");
  }
  return {
    ok: failures.length === 0,
    present,
    failures,
    failureCount: failures.length,
    status: failures.length === 0 ? "current" : "schema_or_source_mismatch"
  };
}
