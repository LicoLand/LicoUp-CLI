#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCargoTestFilter } from "./lib/cargo-test-filter-runner.mjs";
import { loadSecureMeshAcpArchiveReleaseProofConfig } from "./lib/secure-mesh-acp-archive-release-proof-config.mjs";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";
import { readSourceCheckBundle } from "./lib/source-check-bundle.mjs";
import { windowsImplementationReady } from "./lib/secure-mesh-physical-report-coverage.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const config = await loadSecureMeshAcpArchiveReleaseProofConfig();
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;
const reportPath = config.reportOutput;
const args = new Set(process.argv.slice(2));
const strict = args.has("--strict");
const selectedClosureMode = String(
  process.env.LICO_CLIENT_RELEASE_SELECTED_TARGETS || "",
).trim().length > 0;

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u]
]);

function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/private\/var\/folders\/[^\s"]+/gu, "<local-temp>")
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

async function readJsonIfPresent(relativePath) {
  try {
    return JSON.parse(await readText(relativePath));
  } catch {
    return null;
  }
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

async function loadReleaseBuiltDesktopEvidence() {
  const reports = [
    {
      targetId: "macos-arm64",
      platform: "macos",
      report: physicalReportRefs.macosReleaseCliProof
    },
    {
      targetId: "linux-glibc-arm64",
      platform: "ubuntu-linux",
      report: physicalReportRefs.ubuntuReleaseCliProof
    }
  ];
  const entries = [];
  for (const candidate of reports) {
    const payload = await readJsonIfPresent(candidate.report);
    const summary = payload?.summary || {};
    const present = Boolean(payload && Object.keys(payload).length > 0);
    const ready = present &&
      payload?.ok === true &&
      payload?.artifactKind === "release-cli-binary" &&
      summary.filePolicyReady === true &&
      summary.fileRouteReady === true &&
      summary.fileReceiveDestinationReady === true &&
      summary.fileReceiveConfirmationReady === true &&
      payload?.redacted === true;
    entries.push({
      targetId: candidate.targetId,
      platform: candidate.platform,
      report: candidate.report,
      present,
      ready,
      fileRouteReady: summary.fileRouteReady === true,
      fileReceiveDestinationReady: summary.fileReceiveDestinationReady === true,
      fileReceiveConfirmationReady: summary.fileReceiveConfirmationReady === true,
      autoPreviewDisabled: summary.fileReceiveConfirmationReady === true,
      autoIngestionDisabled: summary.fileReceiveConfirmationReady === true
    });
  }
  const windowsPayload = await readJsonIfPresent(physicalReportRefs.windowsImplementation);
  const windowsLocalImplementationReady =
    windowsImplementationReady(windowsPayload);
  return {
    entries,
    readyPlatforms: entries.filter((entry) => entry.ready).map((entry) => entry.platform),
    allRequiredPlatformsReady: entries.every((entry) => entry.ready),
    windowsLocalImplementationReady,
    matrixSatisfied: entries.every((entry) => entry.ready) && windowsLocalImplementationReady
  };
}

const sourceResults = [];
for (const check of config.sourceChecks) {
  sourceResults.push(await evaluateSourceCheck(check));
}
const nativeResults = config.nativeTestFilters.map(runNativeTest);
const releaseBuiltDesktop = await loadReleaseBuiltDesktopEvidence();
const archiveLayerReady = sourceResults.every((check) => check.ok) &&
  nativeResults.every((check) => check.ok);
const releaseFilePolicyReady = releaseBuiltDesktop.matrixSatisfied === true;
// A selected client train proves each selected artifact's file policy directly
// in the final reducer. The historical all-desktop/Windows matrix remains a
// diagnostic for the product-line reducer and must not block an unselected OS.
// This verifier owns the local archive/privacy implementation. Exact built
// artifact policy is reduced later for the selected release target and remains
// diagnostic here when no target closure is being evaluated.
const ok = archiveLayerReady;
const productionReady = false;
const checkedAt = new Date().toISOString();

const report = {
  ok,
  schemaVersion: "licomesh.secure-mesh.acp-archive-release-proof-report.v1",
  verifier: "tools/scripts/client-secure-mesh-acp-archive-release-proof.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-acp-archive-release-proof.mjs",
  generatedAt: checkedAt,
  ...optionalReleaseInvocationBinding(),
  checkedAt,
  blocker: "acp agent conversation",
  diagnosticStatus: ok ? "release-archive-layer-accepted-production-blocked" : "incomplete",
  productionReady,
  releaseReady: false,
  evidenceKind: "redacted-release-cli-and-acp-archive-layer-evidence",
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  reportLeakScan: true,
  selectedClosureMode,
  configRef: config.configRef,
  sourceResults,
  nativeResults,
  releaseBuiltDesktop,
  archiveDefaults: {
    defaultViewThread: sourceResults.some((check) =>
      check.id === "semantic-archive-privacy-defaults-hide-raw" && check.ok),
    hideRawInDefaultView: sourceResults.some((check) =>
      check.id === "semantic-archive-privacy-defaults-hide-raw" && check.ok),
    hideAuditInDefaultView: sourceResults.some((check) =>
      check.id === "semantic-archive-privacy-defaults-hide-raw" && check.ok),
    diagnosticsPanelRequired: sourceResults.some((check) =>
      check.id === "workspace-hides-raw-behind-diagnostics" && check.ok),
    acpArchiveLayerProtected: sourceResults.some((check) =>
      check.id === "acp-archive-layer-is-protected-payload-class" && check.ok),
    noRawAcpJsonInDefaultView: archiveLayerReady,
    noProtectedPayloadArchiveExposureByDefault: archiveLayerReady
  },
  summary: {
    verificationPassed: ok,
    archiveLayerReady,
    releaseFilePolicyReady,
    selectedTargetFilePolicyDeferred: selectedClosureMode,
    releaseBuiltDesktopMatrixSatisfied: releaseBuiltDesktop.matrixSatisfied === true,
    releaseBuiltDesktopReadyPlatforms: releaseBuiltDesktop.readyPlatforms,
    windowsLocalImplementationReady:
      releaseBuiltDesktop.windowsLocalImplementationReady === true,
    autoPreviewDisabledByDefault: archiveLayerReady,
    autoIngestionDisabledByDefault: archiveLayerReady,
    productionReady,
    releaseReady: false,
    remainingGates: [
      ...(archiveLayerReady ? [] : ["ACP archive layer separation and default-view privacy"]),
      ...((selectedClosureMode || releaseFilePolicyReady)
        ? []
        : ["desktop release-built CLI file route / receive destination / confirmation matrix"]),
      "physical Android/iPhone/desktop/real-target ACP archive lifecycle",
      "A8 independent audit"
    ]
  }
};

assertNoLeak(report, "secure mesh ACP archive release proof report");
atomicWriteReportJson(
  path.join(repoRoot, "build"),
  reportPath.replace(/^build\//u, ""),
  report,
);

console.log(JSON.stringify({
  ok,
  report: reportPath,
  diagnosticStatus: report.diagnosticStatus,
  productionReady,
  archiveLayerReady,
  releaseFilePolicyReady,
  releaseBuiltDesktopReadyPlatformCount: releaseBuiltDesktop.readyPlatforms.length,
  remainingGateCount: report.summary.remainingGates.length
}, null, 2));

if (!ok || (strict && productionReady !== true)) {
  process.exitCode = 1;
}
