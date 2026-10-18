#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { loadSecureClientContract } from "./lib/secure-client-contract.mjs";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import { loadSecureMeshWindowsImplementationConfig } from "./lib/secure-mesh-windows-implementation-config.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;
const windowsImplementationConfig = await loadSecureMeshWindowsImplementationConfig();
const reportPath = physicalReportRefs.windowsImplementation;
const args = new Set(process.argv.slice(2));

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u],
  ["file_url", /file:\/\/\//u]
]);

const sourceChecks = Object.freeze(windowsImplementationConfig.sourceChecks);

async function readText(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function evaluateSourceCheck(check) {
  const source = await readText(check.file);
  const missingTokens = check.tokens.filter((token) => !source.includes(token));
  const forbiddenTokensPresent = (check.forbiddenTokens || [])
    .filter((token) => source.includes(token));
  return {
    id: check.id,
    file: check.file,
    ok: missingTokens.length === 0 && forbiddenTokensPresent.length === 0,
    missingTokens,
    forbiddenTokensPresent
  };
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/private\/var\/folders\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/file:\/\/\/[^\s"]+/gu, "file:///<redacted>")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}

function assertWindowsImplementationInvariant(report) {
  const platform = report?.platform || {};
  const summary = report?.summary || {};
  const forbiddenTrue = [
    ["report.productionReady", report?.productionReady],
    ["report.releaseReady", report?.releaseReady],
    ["platform.productionSupportClaimed", platform.productionSupportClaimed],
    ["platform.localImplementationReady", platform.localImplementationReady],
    ["platform.dpapiOrWindowsHelloImplementationReady", platform.dpapiOrWindowsHelloImplementationReady],
    ["platform.dpapiOrWindowsHelloProofReady", platform.dpapiOrWindowsHelloProofReady],
    ["platform.signedInstallerExecutionProofReady", platform.signedInstallerExecutionProofReady],
    ["platform.trustCommandFileMatrixReady", platform.trustCommandFileMatrixReady],
    ["platform.releaseChannelPublicationReady", platform.releaseChannelPublicationReady],
    ["summary.dpapiOrWindowsHelloProofReady", summary.dpapiOrWindowsHelloProofReady],
    ["summary.windowsSignedInstallerProofReady", summary.windowsSignedInstallerProofReady],
    ["summary.windowsTrustCommandFileMatrixReady", summary.windowsTrustCommandFileMatrixReady],
    ["summary.windowsLocalBlockersCleared", summary.windowsLocalBlockersCleared],
    ["summary.productionReady", summary.productionReady],
    ["summary.releaseReady", summary.releaseReady]
  ];
  for (const [field, value] of forbiddenTrue) {
    if (value === true) {
      throw new Error(`Windows implementation cannot set ${field}=true`);
    }
  }
  if (report?.diagnosticStatus !== "persistent_custody_unverified" ||
    platform.localImplementationReady !== false ||
    platform.x64BuilderVerifierReady !== true ||
    platform.dpapiOrWindowsHelloImplementationReady !== false ||
    platform.arm64UpstreamUnavailable !== true ||
    platform.localSecretStore !== "memory-only-ephemeral" ||
    summary.windowsLocalBlockersCleared !== false ||
    summary.nativeHostEvidencePending !== true) {
    throw new Error("Windows implementation report must keep unverified persistent custody disabled");
  }
}

function runSelfTest() {
  const base = {
    diagnosticStatus: "persistent_custody_unverified",
    productionReady: false,
    releaseReady: false,
    platform: {
      localImplementationReady: false,
      x64BuilderVerifierReady: true,
      dpapiOrWindowsHelloImplementationReady: false,
      arm64UpstreamUnavailable: true,
      productionSupportClaimed: false,
      localSecretStore: "memory-only-ephemeral",
      dpapiOrWindowsHelloProofReady: false,
      signedInstallerExecutionProofReady: false,
      trustCommandFileMatrixReady: false,
      releaseChannelPublicationReady: false
    },
    summary: {
      windowsLocalBlockersCleared: false,
      nativeHostEvidencePending: true,
      dpapiOrWindowsHelloProofReady: false,
      windowsSignedInstallerProofReady: false,
      windowsTrustCommandFileMatrixReady: false,
      productionReady: false,
      releaseReady: false
    }
  };
  assertWindowsImplementationInvariant(base);
  for (const [section, field] of [
    ["report", "productionReady"],
    ["report", "releaseReady"],
    ["platform", "productionSupportClaimed"],
    ["platform", "localImplementationReady"],
    ["platform", "dpapiOrWindowsHelloImplementationReady"],
    ["platform", "dpapiOrWindowsHelloProofReady"],
    ["platform", "signedInstallerExecutionProofReady"],
    ["platform", "trustCommandFileMatrixReady"],
    ["summary", "dpapiOrWindowsHelloProofReady"],
    ["summary", "windowsSignedInstallerProofReady"],
    ["summary", "windowsTrustCommandFileMatrixReady"],
    ["summary", "windowsLocalBlockersCleared"]
  ]) {
    const mutated = JSON.parse(JSON.stringify(base));
    if (section === "report") {
      mutated[field] = true;
    } else {
      mutated[section][field] = true;
    }
    let rejected = false;
    try {
      assertWindowsImplementationInvariant(mutated);
    } catch {
      rejected = true;
    }
    if (!rejected) {
      throw new Error(`Windows implementation invariant self-test accepted ${section}.${field}=true`);
    }
  }
  console.log(JSON.stringify({
    ok: true,
    selfTest: "windows-implementation-invariant",
    negativeFixtureRejected: true
  }, null, 2));
}

if (args.has("--self-test")) {
  runSelfTest();
  process.exit(0);
}

const contract = await loadSecureClientContract();
const {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} = contract;
const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find((item) => item === "physical device matrix");
if (!blocker) {
  throw new Error("Client-pinned Secure Client Mesh contract does not define physical device matrix blocker");
}

const sourceResults = [];
for (const check of sourceChecks) {
  sourceResults.push(await evaluateSourceCheck(check));
}
const ok = sourceResults.every((check) => check.ok);
const productionReady = false;
const releaseReady = false;
const report = {
  ok,
  schemaVersion: "licomesh.secure-mesh.windows-implementation-report.v2",
  evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  verifier: "tools/scripts/client-secure-mesh-windows-implementation.mjs",
  generatedAt: new Date().toISOString(),
  sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  report: reportPath,
  blocker,
  diagnosticStatus: "persistent_custody_unverified",
  productionReady,
  releaseReady,
  evidenceKind: "redacted-windows-conservative-custody-boundary",
  redacted: true,
  reportLeakScan: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  contractBinding: {
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    canonicalBlocker: blocker,
    canonicalBlockerCount: SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.length
  },
  physicalEvidenceConfig: {
    ref: physicalEvidenceConfig.configRef,
    schemaVersion: physicalEvidenceConfig.schemaVersion,
    linkedReportCount: Object.keys(physicalReportRefs).length
  },
  windowsImplementationConfig: {
    ref: windowsImplementationConfig.configRef,
    schemaVersion: windowsImplementationConfig.schemaVersion,
    sourceCheckCount: sourceChecks.length
  },
  sourceResults,
  platform: {
    platform: "windows",
    status: "persistent-custody-unverified",
    localImplementationReady: false,
    x64BuilderVerifierReady: ok,
    dpapiOrWindowsHelloImplementationReady: false,
    arm64UpstreamUnavailable: true,
    productionSupportClaimed: false,
    localSecretStore: "memory-only-ephemeral",
    dpapiOrWindowsHelloProofReady: false,
    ownerOnlyAclBoundaryReady: true,
    signedInstallerExecutionProofReady: false,
    trustCommandFileMatrixReady: false,
    releaseChannelPublicationReady: false
  },
  failClosedAssurances: [
    "Windows persistent custody remains disabled until measured platform-native user authorization and create-read-delete evidence exist.",
    "Windows arm64 stays unsupported until the pinned Flutter toolchain provides an official arm64 desktop target.",
    "Owner-only ACL evidence is tracked separately and does not substitute for platform-bound E2EE secret storage.",
    "Signed installer or portable replacement execution proof remains required before Windows can be declared supported.",
    "No raw keys, plaintext payloads, local paths, tokens, or backend runtime data are included in this report."
  ],
  summary: {
    verificationPassed: ok,
    sourceCheckCount: sourceResults.length,
    windowsLocalBlockersCleared: false,
    nativeHostEvidencePending: true,
    dpapiOrWindowsHelloProofReady: false,
    windowsSignedInstallerProofReady: false,
    windowsTrustCommandFileMatrixReady: false,
    productionReady,
    releaseReady,
    remainingGates: [
      "Windows-native user-presence authorization and Credential Manager create-read-delete proof",
      "Windows signed installer or portable replacement execution proof",
      "Windows trust, command/result, file handoff, restart, replay, and no-plaintext matrix",
      "Official Flutter Windows arm64 target support"
    ]
  }
};

assertWindowsImplementationInvariant(report);
assertNoLeak(report, "secure mesh Windows implementation report");
atomicWriteReportJson(repoRoot, reportPath, report);

console.log(JSON.stringify({
  ok,
  report: reportPath,
  sourceOfTruth: report.sourceOfTruth,
  blocker: report.blocker,
  diagnosticStatus: report.diagnosticStatus,
  windowsLocalBlockersCleared: false,
  productionReady,
  releaseReady,
  remainingGateCount: report.summary.remainingGates.length
}, null, 2));

if (!ok) {
  process.exitCode = 1;
}
