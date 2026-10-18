#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCargoTestFilter } from "./lib/cargo-test-filter-runner.mjs";
import { loadDigestBoundJsonInput } from "./lib/secure-client-contract.mjs";
import { loadSecureMeshAcpRelayGovernedBaselineConfig } from "./lib/secure-mesh-acp-relay-governed-baseline-config.mjs";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";
import { readSourceCheckBundle } from "./lib/source-check-bundle.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const config = await loadSecureMeshAcpRelayGovernedBaselineConfig();
const reportPath = config.reportOutput;
const args = new Set(process.argv.slice(2));
const strict = args.has("--strict");
const contractBindingCheck = args.has("--contract-binding-check");

const leakPatterns = Object.freeze([
  ["local_path", /(?:^|["'\s])\/(?:Users|home|private|tmp|var\/folders)\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u]
]);

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/(?:Users|home)\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/(?:private|tmp)\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

async function readText(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function evaluateSourceCheck(check) {
  const { files, source } = await readSourceCheckBundle(check, readText);
  const missingTokens = check.tokens.filter((token) => !source.includes(token));
  return {
    id: check.id,
    file: check.file,
    files,
    ok: missingTokens.length === 0,
    missingTokens
  };
}

function runNativeTest(filter) {
  return runCargoTestFilter({
    repoRoot,
    manifestPath: "crates/licoup-native/Cargo.toml",
    filter,
    sanitizeError
  });
}

function envValue(keys = []) {
  for (const key of keys) {
    const value = String(process.env[key] || "").trim();
    if (value) return value;
  }
  return "";
}

async function loadExternalGatewayEvidence() {
  const inputConfig = config.externalGatewayEvidence;
  const reportPath = envValue(inputConfig.pathEnvKeys);
  const expectedDigest = envValue(inputConfig.digestEnvKeys);
  if (!reportPath && !expectedDigest) {
    return {
      provided: false,
      digestBound: false,
      accepted: null,
      schemaVersion: "",
      sourceOfTruth: ""
    };
  }
  const input = await loadDigestBoundJsonInput({
    filePath: reportPath,
    expectedDigest,
    label: "ACP gateway support report"
  });
  const report = input.value;
  assertNoLeak(report, "ACP gateway support report");
  const summary = report?.summary || {};
  const rawMaterialExcluded = report?.rawPrivateMaterialIncluded === false &&
    report?.rawPlaintextIncluded === false &&
    report?.rawPublicWireBytesIncluded === false;
  const reportLeakScan = report?.reportLeakScan === true || summary.reportLeakScan === true;
  const accepted = (report?.ok === true || report?.verificationOk === true) &&
    report?.redacted === true &&
    rawMaterialExcluded &&
    reportLeakScan;
  return {
    provided: true,
    digestBound: true,
    digest: input.digest,
    accepted,
    schemaVersion: String(report?.schemaVersion || ""),
    sourceOfTruth: String(report?.sourceOfTruth || "")
  };
}

if (contractBindingCheck) {
  console.log(JSON.stringify({
    ok: true,
    configRef: config.configRef,
    clientEnvelopeOwnedLocally: config.ownership.secureMeshAcpEnvelope === "client",
    gatewayEvidenceIsExternal: config.ownership.acpRelayGovernance === "core",
    explicitGatewayReportPathRequired: config.externalGatewayEvidence.pathEnvKeys.length > 0,
    explicitGatewayReportDigestRequired: config.externalGatewayEvidence.digestEnvKeys.length > 0,
    adjacentServerCheckoutRequired: false
  }, null, 2));
  process.exit(0);
}

const sourceResults = [];
for (const check of config.sourceChecks) {
  sourceResults.push(await evaluateSourceCheck(check));
}
const nativeResults = config.nativeTestFilters.map(runNativeTest);
const externalGatewayEvidence = await loadExternalGatewayEvidence();

const clientOk = sourceResults.every((check) => check.ok) && nativeResults.every((check) => check.ok);
// Client releases prove their own sealed-envelope behavior against a neutral
// relay. Gateway-owned governance remains an independently reported support
// capability and must never turn an unsupported gateway into a client failure.
const clientInteroperabilityReady = clientOk;
const ok = clientInteroperabilityReady;
const productionReady = false;
const checkedAt = new Date().toISOString();

const report = {
  ok,
  schemaVersion: "licomesh.secure-mesh.acp-relay-governed-baseline-report.v1",
  verifier: "tools/scripts/client-secure-mesh-acp-relay-governed-baseline.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-acp-relay-governed-baseline.mjs",
  generatedAt: checkedAt,
  ...optionalReleaseInvocationBinding(),
  checkedAt,
  blocker: "acp agent conversation",
  diagnosticStatus: ok ? "client-envelope-interoperability-accepted" : "incomplete",
  clientInteroperabilityReady,
  productionReady,
  releaseReady: false,
  evidenceKind: "redacted-acp-relay-governed-baseline-and-client-envelope-evidence",
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  reportLeakScan: true,
  ownership: config.ownership,
  configRef: config.configRef,
  sourceResults,
  nativeResults,
  externalGatewayEvidence,
  summary: {
    verificationPassed: ok,
    clientEnvelopeReady: clientOk,
    gatewaySupportEvidenceProvided: externalGatewayEvidence.provided,
    gatewaySupportEvidenceDigestBound: externalGatewayEvidence.digestBound,
    gatewaySupportEvidenceReady: externalGatewayEvidence.accepted,
    sourceCheckCount: sourceResults.length,
    nativeTestCount: nativeResults.length,
    productionReady,
    releaseReady: false,
    remainingGates: [
      ...(clientOk ? [] : ["client Secure Mesh ACP envelope taxonomy and AAD fail-closed tests"]),
      "selected-platform physical client interoperability"
    ]
  }
};

assertNoLeak(report, "secure mesh ACP relay governed baseline report");
atomicWriteReportJson(
  path.join(repoRoot, "build"),
  reportPath.replace(/^build\//u, ""),
  report,
);

console.log(JSON.stringify({
  ok,
  report: reportPath,
  ownership: config.ownership,
  diagnosticStatus: report.diagnosticStatus,
  productionReady,
  clientEnvelopeReady: clientOk,
  clientInteroperabilityReady,
  gatewaySupportEvidenceProvided: externalGatewayEvidence.provided,
  gatewaySupportEvidenceReady: externalGatewayEvidence.accepted,
  remainingGateCount: report.summary.remainingGates.length
}, null, 2));

if (!ok || (strict && productionReady !== true)) {
  process.exitCode = 1;
}
