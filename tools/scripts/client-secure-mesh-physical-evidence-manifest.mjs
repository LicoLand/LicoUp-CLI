#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { loadSecureClientContract } from "./lib/secure-client-contract.mjs";
import { createSecureClientMeshE2eeRefReportScope } from "./lib/secure-client-mesh-e2ee-ref-report.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import {
  androidPlatformCryptoCoverage as sharedAndroidPlatformCryptoCoverage,
  platformSecretStoreCustodyCoverage,
  stationAcceptanceCoverage as sharedStationAcceptanceCoverage
} from "./lib/secure-mesh-physical-report-coverage.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const reportPath = physicalEvidenceConfig.reportOutput;
const reportRefs = physicalEvidenceConfig.linkedReports;
const freshnessWindows = physicalEvidenceConfig.freshnessWindows;
const maxFutureSkewSeconds = 5 * 60;

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["device_identifier", /\b(?:UDID|ECID|Serial(?:Number)?|DeviceIdentifier)\s*[:=]\s*[A-Za-z0-9-]{8,}\b/u],
  ["raw_secret_value", /"(?:privateKey|sessionKey|rootKey|chainKey|messageKey|rawSecret|secretMaterial)"\s*:\s*"(?!redacted|\[redacted\])[^"]{8,}"/u]
]);

async function readJsonIfPresent(relativePath) {
  try {
    return JSON.parse(await fs.readFile(path.join(repoRoot, relativePath), "utf8"));
  } catch {
    return null;
  }
}

async function digestReportRef(relativePath) {
  try {
    const bytes = await fs.readFile(path.join(repoRoot, relativePath));
    return {
      report: relativePath,
      present: true,
      sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
      byteLength: bytes.byteLength
    };
  } catch {
    return { report: relativePath, present: false, sha256: "", byteLength: 0 };
  }
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function dedupe(values) {
  return [...new Set((Array.isArray(values) ? values : [])
    .map((value) => String(value || "").trim())
    .filter(Boolean))];
}

function reportSummary(id, ref, report) {
  const present = Boolean(report && Object.keys(report).length > 0);
  const privacyReady = id === "stationAcceptance"
    ? sharedStationAcceptanceCoverage(report).ready === true
    : report?.redacted === true &&
      report?.rawPrivateMaterialIncluded === false &&
      report?.rawPlaintextIncluded === false &&
      report?.rawPublicWireBytesIncluded === false &&
      report?.reportLeakScan === true;
  return {
    id,
    report: ref,
    present,
    ok: report?.ok === true,
    schemaVersion: String(report?.schemaVersion || ""),
    privacyReady,
  };
}

function evaluateFreshness(report, checkedAt, maxAgeSeconds) {
  const observedAt = Date.parse(report?.checkedAt || report?.generatedAt || "");
  const now = Date.parse(checkedAt);
  if (!Number.isFinite(observedAt)) {
    return { ready: false, status: "missing_timestamp", ageSeconds: null };
  }
  const ageSeconds = Math.floor((now - observedAt) / 1000);
  if (ageSeconds < -maxFutureSkewSeconds) {
    return { ready: false, status: "future_timestamp", ageSeconds };
  }
  if (ageSeconds > maxAgeSeconds) {
    return { ready: false, status: "stale", ageSeconds };
  }
  return { ready: true, status: "fresh", ageSeconds };
}

function stationAcceptanceCoverage(report) {
  return sharedStationAcceptanceCoverage(report, {
    reportRef: reportRefs.stationAcceptance
  });
}

function androidPlatformCryptoCoverage(report, freshness) {
  return sharedAndroidPlatformCryptoCoverage(report, {
    reportRef: reportRefs.androidPlatformCrypto,
    freshness
  });
}

function platformCoverage(platformReport, androidCrypto, installLaunch, physicalMatrix) {
  const custody = platformSecretStoreCustodyCoverage(platformReport);
  const matrixSummary = physicalMatrix?.summary || {};
  const androidCustodyReady = custody.androidBindingReady;
  const iosCustodyReady = custody.iosBindingReady;
  const macosCustodyReady = custody.macosReady;
  const ubuntuCustodyReady = custody.ubuntuReady;
  const androidPhysicalDeviceProofReady =
    installLaunch?.summary?.androidPhysicalDeviceProofReady === true ||
    installLaunch?.androidPhysicalDeviceProofReady === true ||
    installLaunch?.physicalDevice === true;
  return [
    {
      platform: "android",
      status: androidCrypto.ready && androidCustodyReady ? "partial" : "missing",
      platformCryptoAcceptanceReady: androidCrypto.ready,
      platformCustodyReady: androidCustodyReady,
      physicalDeviceProofReady: androidPhysicalDeviceProofReady,
      commandResultPhysicalProofReady: false,
      remainingGates: [
        ...(androidPhysicalDeviceProofReady ? [] : ["physical Android install/launch custody proof"]),
        "physical Android command/result, restart, replay, key rotation, revocation, and file proof"
      ]
    },
    {
      platform: "ios",
      status: iosCustodyReady ? "partial" : "missing",
      platformCustodyReady: iosCustodyReady,
      physicalDeviceProofReady: false,
      commandResultPhysicalProofReady: false,
      remainingGates: [
        "physical iPhone LocalAuthentication, Keychain custody, command/result, restart, replay, key rotation, revocation, and file proof"
      ]
    },
    {
      platform: "macos",
      status: macosCustodyReady ? "partial" : "missing",
      platformCustodyReady: macosCustodyReady,
      releaseCliProofReady: custody.macosReleaseCliProofReady,
      remainingGates: ["signed local install, launch, update, and publication receipts"]
    },
    {
      platform: "ubuntu-linux",
      status: ubuntuCustodyReady ? "partial" : "missing",
      platformCustodyReady: ubuntuCustodyReady,
      releaseCliProofReady: custody.ubuntuReleaseCliProofReady,
      adaptiveCustodyReady: custody.ubuntuLinuxAdaptiveCustodyReady,
      packageUpdateReady: custody.ubuntuLinuxPackageUpdateReady,
      remainingGates: ["release package and publication receipts"]
    },
    {
      platform: "windows",
      status: matrixSummary.windowsLocalImplementationReady === true
        ? "persistent-custody-verified"
        : (matrixSummary.windowsConservativeCustodyBoundaryValid === true
          ? "persistent-custody-unverified"
          : "missing"),
      platformCustodyReady: false,
      localImplementationReady:
        matrixSummary.windowsLocalImplementationReady === true,
      nativeHostEvidenceReady: false,
      remainingGates: ["Windows-native Credential Manager lifecycle receipt"]
    }
  ];
}

const contract = await loadSecureClientContract();
const {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} = contract;
const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find((item) =>
  item === "physical device matrix"
);
if (!blocker) {
  throw new Error("Client-pinned Secure Client Mesh contract does not define physical device matrix blocker");
}

const reports = Object.fromEntries(await Promise.all(
  Object.entries(reportRefs).map(async ([key, ref]) => [key, await readJsonIfPresent(ref)])
));
const checkedAt = new Date().toISOString();
const linkedReports = Object.entries(reportRefs).map(([key, ref]) =>
  reportSummary(key, ref, reports[key])
);
const artifactDigests = await Promise.all(
  Object.values(reportRefs).map(digestReportRef)
);
const androidPlatformCryptoFreshness = evaluateFreshness(
  reports.androidPlatformCrypto,
  checkedAt,
  freshnessWindows.androidPlatformCryptoSeconds
);
const stationAcceptance = stationAcceptanceCoverage(reports.stationAcceptance);
const androidPlatformCrypto = androidPlatformCryptoCoverage(
  reports.androidPlatformCrypto,
  androidPlatformCryptoFreshness
);
const platforms = platformCoverage(
  reports.platformSecretStore,
  androidPlatformCrypto,
  reports.androidInstallLaunch,
  reports.physicalDeviceMatrix
);
const platformSummary = reports.platformSecretStore?.summary || {};
const physicalMatrixSummary = reports.physicalDeviceMatrix?.summary || {};
const configuredReportCount = linkedReports.length;
const linkedReportCount = linkedReports.filter((entry) => entry.present).length;
const missingConfiguredReportIds = linkedReports
  .filter((entry) => !entry.present)
  .map((entry) => entry.id);
const allConfiguredReportsPresent = missingConfiguredReportIds.length === 0;
const allPresentReportsRedacted = linkedReports
  .filter((entry) => entry.present)
  .every((entry) => entry.privacyReady);
const localIntegrityReportIds = new Set([
  "androidPlatformCrypto",
  "stationAcceptance",
  "platformSecretStore",
  "physicalDeviceMatrix",
  "encryptedFileHandoff",
  "trustUx",
  "windowsImplementation",
  "updateReleaseChannel",
]);
const localReportsIntegrityReady = linkedReports
  .filter((entry) => localIntegrityReportIds.has(entry.id))
  .every((entry) => entry.present && entry.privacyReady);
const physicalEvidenceChainReady =
  physicalMatrixSummary.physicalEvidenceChainReady === true &&
  stationAcceptance.ready &&
  androidPlatformCrypto.ready &&
  platforms.find((entry) => entry.platform === "android")?.platformCustodyReady === true &&
  platforms.find((entry) => entry.platform === "ios")?.platformCustodyReady === true &&
  platforms.find((entry) => entry.platform === "macos")?.platformCustodyReady === true;
const remainingGates = dedupe([
  ...platforms.flatMap((entry) => entry.remainingGates),
  ...(stationAcceptance.ready
    ? []
    : ["strict Lico Arc BadTower interoperability acceptance"]),
  ...(androidPlatformCrypto.ready
    ? []
    : ["fresh Android platform cryptography acceptance"]),
  ...(reports.physicalDeviceMatrix?.ok === true
    ? []
    : ["current physical device matrix"]),
  ...(reports.encryptedFileHandoff?.ok === true
    ? []
    : ["client encrypted file handoff acceptance"]),
  ...(reports.trustUx?.ok === true ? [] : ["client trust UX acceptance"])
]);
// Missing physical-host receipts are an explicit release dependency, not a
// malformed local manifest. Local verification therefore proves the schema,
// producer and every present report while `evidenceChainComplete` remains
// fail-closed until every configured receipt exists.
const diagnosticOk = localReportsIntegrityReady &&
  stationAcceptance.ready &&
  androidPlatformCrypto.ready &&
  reports.platformSecretStore?.ok === true &&
  reports.physicalDeviceMatrix?.ok === true;
const manifestIntegrityReady = diagnosticOk;
const evidenceChainComplete = manifestIntegrityReady &&
  physicalEvidenceChainReady &&
  remainingGates.length === 0;
const scopeEvidence = await createSecureClientMeshE2eeRefReportScope({
  contract,
  reportRef: reportPath,
  blocker,
  checkedAt
});

const summary = {
  diagnosticOk,
  okMeaning: "manifest_integrity_not_production_evidence",
  configuredReportCount,
  linkedReportCount,
  missingConfiguredReportCount: missingConfiguredReportIds.length,
  missingConfiguredReportIds,
  allConfiguredReportsPresent,
  allReportsRedacted: allPresentReportsRedacted,
  localReportsIntegrityReady,
  redactionReady: allPresentReportsRedacted,
  linkedReportFreshnessReady: androidPlatformCryptoFreshness.ready,
  linkedReportFreshnessStaleOrInvalidCount:
    androidPlatformCryptoFreshness.ready ? 0 : 1,
  androidPlatformCryptoFreshnessReady:
    androidPlatformCryptoFreshness.ready,
  androidPlatformCryptoFreshnessStatus:
    androidPlatformCryptoFreshness.status,
  stationAcceptanceReady: stationAcceptance.ready,
  androidPlatformCryptoAcceptanceReady: androidPlatformCrypto.ready,
  androidPlatformCustodyContractReady:
    androidPlatformCrypto.platformCustodyContractReady,
  androidPlatformAuthorizationContractReady:
    androidPlatformCrypto.platformAuthorizationContractReady,
  androidRustFfiActionContractReady:
    androidPlatformCrypto.rustFfiActionContractReady,
  androidMlsMemberRemoveReleaseActionReady:
    androidPlatformCrypto.mlsMemberRemoveReleaseActionReady,
  androidUnknownReleaseActionsFailClosed:
    androidPlatformCrypto.unknownReleaseActionsFailClosed,
  androidNativeTestClassCount: androidPlatformCrypto.nativeTestClassCount,
  androidPhysicalSecretStoreBindingReady:
    platformSummary.androidPhysicalSecretStoreBindingReady === true,
  androidPhysicalSystemCredentialAuthReady:
    platformSummary.androidPhysicalSystemCredentialAuthReady === true,
  androidPhysicalKeyStoreHardwareAuthReady:
    platformSummary.androidPhysicalKeyStoreHardwareAuthReady === true,
  androidPhysicalCallbackContractReady:
    platformSummary.androidPhysicalCallbackContractReady === true,
  iosPhysicalSecretStoreBindingReady:
    platformSummary.iosPhysicalSecretStoreBindingReady === true,
  iosUserPresencePolicyReady:
    platformSummary.iosUserPresencePolicyReady === true,
  iosProductionCallbackAuthReady:
    platformSummary.iosProductionCallbackAuthReady === true,
  iosPhysicalCallbackContractReady:
    platformSummary.iosPhysicalCallbackContractReady === true,
  iosPhysicalDeviceDiscovered: false,
  macosKeyringReleaseEvidenceReady:
    platformSummary.macosSafeOsStoreAvailable === true &&
    platformSummary.macosReleaseCliProofReady === true,
  macosUserPresencePolicyReady:
    platformSummary.macosUserPresencePolicyReady === true ||
    platformSummary.macosSafeOsStoreAvailable === true,
  macosSingleSystemAuthorizationContextVerified:
    platformSummary.macosSingleSystemAuthorizationContextVerified === true,
  macosInteractiveAuthorizationPromptBudgetReady:
    platformSummary.macosPromptBudgetSatisfied === true,
  ubuntuLinuxReleaseEvidenceReady:
    platformSummary.ubuntuVmSecretStoreReady === true &&
    platformSummary.ubuntuReleaseCliProofReady === true &&
    platformSummary.ubuntuLinuxAdaptiveCustodyReady === true &&
    platformSummary.ubuntuLinuxPackageUpdateReady === true,
  windowsLocalImplementationReady:
    physicalMatrixSummary.windowsLocalImplementationReady === true,
  windowsConservativeCustodyBoundaryValid:
    physicalMatrixSummary.windowsConservativeCustodyBoundaryValid === true,
  windowsNativeHostEvidenceReady: false,
  partialPlatformCount: platforms.filter((entry) => entry.status === "partial").length,
  blockedPlatformCount: platforms.filter((entry) => entry.status.startsWith("blocked")).length,
  missingPlatformCount: platforms.filter((entry) => entry.status === "missing").length,
  manifestIntegrityReady,
  physicalEvidenceChainReady,
  evidenceChainComplete,
  releaseEvidenceReady: evidenceChainComplete,
  productionReady: false,
  releaseReady: false,
  remainingGates
};

const manifest = {
  schemaVersion: "licomesh.secure-mesh.physical-evidence-manifest-report.v2",
  evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  verifier: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs",
  generatedAt: checkedAt,
  checkedAt,
  sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  report: reportPath,
  blocker,
  diagnosticStatus: diagnosticOk ? "passed" : "incomplete",
  ok: diagnosticOk,
  diagnosticOk,
  okMeaning: summary.okMeaning,
  redactionReady: summary.redactionReady,
  manifestIntegrityReady,
  physicalEvidenceChainReady,
  evidenceChainComplete,
  releaseEvidenceReady: evidenceChainComplete,
  ready: evidenceChainComplete,
  productionReady: false,
  releaseReady: false,
  evidenceKind: "redacted-client-platform-cryptography-physical-evidence-manifest",
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  reportLeakScan: true,
  physicalEvidenceConfig: {
    ref: physicalEvidenceConfig.configRef,
    schemaVersion: physicalEvidenceConfig.schemaVersion,
    linkedReportCount: Object.keys(reportRefs).length,
    freshnessWindowCount: Object.keys(freshnessWindows).length
  },
  ...scopeEvidence,
  contractBinding: {
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    canonicalBlocker: blocker,
    canonicalBlockerCount: SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.length
  },
  linkedReports,
  linkedReportFreshness: {
    androidPlatformCrypto: androidPlatformCryptoFreshness
  },
  stationAcceptance,
  androidPlatformCrypto,
  platformCoverage: platforms,
  physicalProofClasses: [
    "physical-device proof classes",
    "platform secret-store proof classes",
    "client Rust and platform cryptography proof classes",
    "Lico Arc BadTower interoperability proof classes"
  ],
  releaseProofClasses: [
    "signing/deployment proof classes",
    "release bundle shape proof classes",
    "update channel proof classes"
  ],
  artifactDigests,
  summary
};

assertNoLeak(manifest, "secure mesh physical evidence manifest");
atomicWriteReportJson(
  path.join(repoRoot, "build"),
  reportPath.replace(/^build\//u, ""),
  manifest
);

console.log(JSON.stringify({
  ok: manifest.ok,
  report: reportPath,
  diagnosticStatus: manifest.diagnosticStatus,
  stationAcceptanceReady: summary.stationAcceptanceReady,
  androidPlatformCryptoAcceptanceReady:
    summary.androidPlatformCryptoAcceptanceReady,
  manifestIntegrityReady,
  physicalEvidenceChainReady,
  releaseEvidenceReady: manifest.releaseEvidenceReady,
  missingConfiguredReportCount: summary.missingConfiguredReportCount,
  remainingGateCount: remainingGates.length
}, null, 2));

if (!manifest.ok) {
  process.exitCode = 1;
}
