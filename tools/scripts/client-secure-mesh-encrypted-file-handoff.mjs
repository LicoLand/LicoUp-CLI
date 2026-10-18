#!/usr/bin/env node
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCargoTestFilter } from "./lib/cargo-test-filter-runner.mjs";
import { loadSecureClientContract } from "./lib/secure-client-contract.mjs";
import { createSecureClientMeshE2eeRefReportScope } from "./lib/secure-client-mesh-e2ee-ref-report.mjs";
import { loadSecureMeshEncryptedFileHandoffConfig } from "./lib/secure-mesh-encrypted-file-handoff-config.mjs";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";
import { readSourceCheckBundle } from "./lib/source-check-bundle.mjs";
import {
  licoArcBadTowerAcceptanceCoverage,
} from "./lib/licoarc-badtower-acceptance-report.mjs";
import {
  ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT,
  windowsImplementationReady,
  windowsPersistentCustodyBoundaryValid,
} from "./lib/secure-mesh-physical-report-coverage.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const encryptedFileHandoffConfig = await loadSecureMeshEncryptedFileHandoffConfig();
const sourceChecks = Object.freeze(encryptedFileHandoffConfig.sourceChecks);
const nativeTestFilters = Object.freeze(encryptedFileHandoffConfig.nativeTestFilters);
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;
const reportPath = physicalReportRefs.encryptedFileHandoff;
const args = new Set(process.argv.slice(2));
const strict = args.has("--strict");

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u],
  ["plaintext_file_canary", /(?:private-file-canary|private-relative-canary|file-body-plaintext-secret-canary-content|settlement-private-file-canary|mobile-ffi-private-file-canary|private-cli-file-canary)/u]
]);

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
  const forbiddenTokensPresent = (check.forbiddenTokens || [])
    .filter((token) => source.includes(token));
  return {
    id: check.id,
    file: check.file,
    files,
    ok: missingTokens.length === 0 && forbiddenTokensPresent.length === 0,
    missingTokens,
    forbiddenTokensPresent
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

async function loadAndroidPlatformCryptoEvidence() {
  const report = physicalReportRefs.androidPlatformCrypto;
  const payload = await readJsonIfPresent(report);
  const present = Boolean(payload && Object.keys(payload).length > 0);
  const summary = payload?.summary || {};
  const ready = present &&
    payload?.ok === true &&
    payload?.schemaVersion === "licomesh.secure-mesh.android-platform-crypto-acceptance.v1" &&
    payload?.verifier === "tools/scripts/client-android-native-tests.mjs" &&
    payload?.platform === "android" &&
    summary.platformCryptoAcceptanceReady === true &&
    summary.platformCustodyContractReady === true &&
    summary.platformAuthorizationContractReady === true &&
    summary.rustFfiActionContractReady === true &&
    summary.mlsMemberRemoveReleaseActionReady === true &&
    summary.unknownReleaseActionsFailClosed === true &&
    summary.nativeTestClassCount === ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT &&
    payload?.redacted === true &&
    payload?.rawPrivateMaterialIncluded === false &&
    payload?.rawPlaintextIncluded === false &&
    payload?.rawPublicWireBytesIncluded === false;
  return {
    targetId: "android-arm64",
    report,
    present,
    ok: ready,
    platform: "android",
    platformCryptoAcceptanceReady: summary.platformCryptoAcceptanceReady === true,
    platformCustodyContractReady: summary.platformCustodyContractReady === true,
    platformAuthorizationContractReady: summary.platformAuthorizationContractReady === true,
    rustFfiActionContractReady: summary.rustFfiActionContractReady === true,
    mlsMemberRemoveReleaseActionReady: summary.mlsMemberRemoveReleaseActionReady === true,
    unknownReleaseActionsFailClosed: summary.unknownReleaseActionsFailClosed === true,
    rawPrivateMaterialIncluded: payload?.rawPrivateMaterialIncluded === true,
    rawPlaintextIncluded: payload?.rawPlaintextIncluded === true,
    rawPublicWireBytesIncluded: payload?.rawPublicWireBytesIncluded === true,
    status: ready ? "android-platform-crypto-verified" : (present ? "incomplete" : "missing")
  };
}

async function loadStationAcceptanceEvidence() {
  const report = physicalReportRefs.stationAcceptance;
  const payload = await readJsonIfPresent(report);
  const present = Boolean(payload && Object.keys(payload).length > 0);
  const coverage = licoArcBadTowerAcceptanceCoverage(payload);
  return {
    report,
    present,
    ...coverage,
  };
}

async function loadReleaseBuiltDesktopFilePolicyEvidence() {
  const reports = [
    {
      targetId: "macos-arm64",
      platform: "macos",
      osFamily: "macos",
      arch: "arm64",
      report: physicalReportRefs.macosReleaseCliProof
    },
    {
      targetId: "linux-glibc-arm64",
      platform: "ubuntu-linux",
      osFamily: "linux-glibc",
      arch: "arm64",
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
      payload?.redacted === true &&
      payload?.rawPrivateMaterialIncluded !== true &&
      payload?.rawPlaintextIncluded !== true &&
      payload?.rawPublicWireBytesIncluded !== true;
    entries.push({
      targetId: candidate.targetId,
      platform: candidate.platform,
      osFamily: candidate.osFamily,
      arch: candidate.arch,
      report: candidate.report,
      present,
      ready,
      releaseCliProofReady: summary.releaseCliProofReady === true,
      filePolicyReady: summary.filePolicyReady === true,
      fileRouteReady: summary.fileRouteReady === true,
      fileReceiveDestinationReady: summary.fileReceiveDestinationReady === true,
      fileReceiveConfirmationReady: summary.fileReceiveConfirmationReady === true,
      trustPolicyReady: summary.trustPolicyReady === true,
      commandReplayRejected: summary.commandReplayRejected === true
    });
  }
  const windowsImplementation = await loadWindowsFileImplementationEvidence();
  const requiredReadyPlatforms = reports.map((entry) => entry.platform);
  const allRequiredPlatformsReady = requiredReadyPlatforms.every((platform) =>
    entries.some((entry) => entry.platform === platform && entry.ready)
  );
  const matrixSatisfied = allRequiredPlatformsReady && windowsImplementation.localReady === true;
  return {
    entries,
    readyPlatforms: entries.filter((entry) => entry.ready).map((entry) => entry.platform),
    requiredReadyPlatforms,
    allRequiredPlatformsReady,
    windowsImplementation,
    windowsLocalBlockersCleared: windowsImplementation.localReady === true,
    matrixSatisfied,
    ready: entries.some((entry) => entry.ready)
  };
}

async function loadWindowsFileImplementationEvidence() {
  const report = physicalReportRefs.windowsImplementation;
  const payload = await readJsonIfPresent(report);
  const summary = payload?.summary || {};
  const platform = payload?.platform || {};
  const present = Boolean(payload && Object.keys(payload).length > 0);
  const conservativeBoundaryValid =
    windowsPersistentCustodyBoundaryValid(payload);
  const localReady = windowsImplementationReady(payload);
  return {
    report,
    present,
    conservativeBoundaryValid,
    localReady,
    windowsLocalBlockersCleared: summary.windowsLocalBlockersCleared === true,
    nativeHostEvidencePending: summary.nativeHostEvidencePending === true,
    dpapiOrWindowsHelloProofReady: summary.dpapiOrWindowsHelloProofReady === true,
    windowsSignedInstallerProofReady: summary.windowsSignedInstallerProofReady === true,
    windowsTrustCommandFileMatrixReady: summary.windowsTrustCommandFileMatrixReady === true,
    localImplementationReady: platform.localImplementationReady === true,
    productionSupportClaimed: platform.productionSupportClaimed === true,
    status: localReady
      ? "persistent-custody-verified"
      : (conservativeBoundaryValid
        ? "persistent-custody-unverified"
        : (present ? "invalid" : "missing"))
  };
}

function physicalHandoffMatrix(androidPlatformCrypto, releaseBuiltDesktopFilePolicy, stationAcceptance) {
  const androidPlatformReady = androidPlatformCrypto.ok === true;
  const stationInteroperabilityReady = stationAcceptance.ready === true;
  const releaseBuiltReady = releaseBuiltDesktopFilePolicy.ready === true;
  const releaseBuiltMatrixSatisfied = releaseBuiltDesktopFilePolicy.matrixSatisfied === true;
  const releaseBuiltReadyPlatforms = releaseBuiltDesktopFilePolicy.readyPlatforms || [];
  return [
    {
      scenario: "Android-to-desktop-to-iPhone",
      status: androidPlatformReady && stationInteroperabilityReady
        ? "android-platform-and-station-interoperability-verified-partial"
        : "missing",
      evidence: androidPlatformReady && stationInteroperabilityReady
        ? [
            "Android platform custody, authorization, and Rust FFI action contracts passed.",
            "Two fresh endpoints completed the strict Lico Arc BadTower round trip.",
            "The station remained non-authoritative, exposed no endpoint plaintext, and rejected non-conformant envelopes."
          ]
        : [],
      evidenceReports: androidPlatformReady && stationInteroperabilityReady
        ? [androidPlatformCrypto.report, stationAcceptance.report]
        : [],
      remainingGates: [
        "Run the Android sender, desktop reseal, and physical iPhone recipient flow.",
        "Prove endpoint-specific ciphertext opens only at the intended iPhone endpoint.",
        "Capture endpoint-authenticated receive confirmation and resume receipts for the full client flow."
      ]
    },
    {
      scenario: "iPhone-to-desktop-to-Android",
      status: androidPlatformReady && stationInteroperabilityReady
        ? "android-platform-and-station-interoperability-verified-partial"
        : "missing",
      evidence: androidPlatformReady && stationInteroperabilityReady
        ? [
            "Android platform custody, authorization, and Rust FFI action contracts passed.",
            "The strict Lico Arc BadTower acceptance passed with exact five-field envelopes and non-authoritative transport hints."
          ]
        : [],
      evidenceReports: androidPlatformReady && stationInteroperabilityReady
        ? [androidPlatformCrypto.report, stationAcceptance.report]
        : [],
      remainingGates: [
        "Run the physical iPhone sender, desktop reseal, and Android recipient flow.",
        "Prove endpoint-specific ciphertext opens only at the intended Android endpoint.",
        "Capture endpoint-authenticated receive confirmation and resume receipts for the full client flow."
      ]
    },
    {
      scenario: "desktop-to-desktop-release-build",
      status: releaseBuiltMatrixSatisfied
        ? "release-built-desktop-file-policy-matrix-satisfied-with-windows-fail-closed"
        : (releaseBuiltReady ? "release-built-desktop-file-policy-partial" : "missing"),
      evidence: releaseBuiltReady
        ? [
            `Release-built desktop CLI file route, receive-destination, and receive-confirmation policy passed on ${releaseBuiltReadyPlatforms.join(" and ")}.`,
            "Receive confirmation remains user-visible, write-deferred, and disables auto-preview plus auto-ingestion by default.",
            "Release-built CLI report redaction checks keep file names, approved roots, and body material out of evidence reports.",
            ...(releaseBuiltDesktopFilePolicy.windowsLocalBlockersCleared
              ? ["Windows local implementation blockers are cleared; native-host custody and signed-artifact receipts remain external evidence gates."]
              : [])
          ]
        : [],
      evidenceReports: releaseBuiltDesktopFilePolicy.entries
        .filter((entry) => entry.ready)
        .map((entry) => entry.report)
        .concat(releaseBuiltDesktopFilePolicy.windowsLocalBlockersCleared
          ? [releaseBuiltDesktopFilePolicy.windowsImplementation.report]
          : [])
        .concat(stationInteroperabilityReady ? [stationAcceptance.report] : []),
      remainingGates: releaseBuiltMatrixSatisfied
        ? (stationInteroperabilityReady
          ? []
          : ["Run the strict Lico Arc BadTower interoperability acceptance."])
        : releaseBuiltReady
        ? [
            "Collect Windows-native custody and signed-artifact receipts for the implementation-ready client.",
            "Complete endpoint-specific reseal/open proof on selected release clients."
          ]
        : [
            "Run macOS and Ubuntu/Linux release-built clients with approved receive roots.",
            "Collect Windows-native custody and signed-artifact receipts for the implementation-ready client."
          ]
    }
  ];
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
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}

const contract = await loadSecureClientContract();
const {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} = contract;
const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find((item) => item === "encrypted file handoff");
if (!blocker) {
  throw new Error("Client-pinned Secure Client Mesh contract does not define encrypted file handoff blocker");
}

const sourceResults = [];
for (const check of sourceChecks) {
  sourceResults.push(await evaluateSourceCheck(check));
}
const nativeResults = nativeTestFilters.map(runNativeTest);
const androidPlatformCrypto = await loadAndroidPlatformCryptoEvidence();
const stationAcceptance = await loadStationAcceptanceEvidence();
const releaseBuiltDesktopFilePolicy = await loadReleaseBuiltDesktopFilePolicyEvidence();
const ok = sourceResults.every((check) => check.ok) &&
  nativeResults.every((check) => check.ok) &&
  stationAcceptance.ready === true;
const productionReady = false;
const checkedAt = new Date().toISOString();
const scopeEvidence = await createSecureClientMeshE2eeRefReportScope({
  contract,
  reportRef: reportPath,
  blocker,
  checkedAt
});
const handoffMatrix = physicalHandoffMatrix(
  androidPlatformCrypto,
  releaseBuiltDesktopFilePolicy,
  stationAcceptance
);
const receiveConfirmationReady =
  sourceResults.every((check) => check.ok) &&
  nativeResults.some((item) => item.id === "secure_mesh_file_receive_confirmation_requires_user_action_and_disables_auto_open" && item.ok) &&
  nativeResults.some((item) => item.id === "secure_mesh_file_receive_confirmation_cli_requires_user_confirmation_without_auto_open" && item.ok) &&
  nativeResults.some((item) => item.id === "mobile_ffi_exposes_shared_file_route_and_receive_destination_policy" && item.ok);
const multiRecipientEndpointSpecificResealProofReady =
  nativeResults.some((item) => item.id === "secure_mesh_file_handoff_proof_reseals_distinct_ciphertext_for_multiple_recipients" && item.ok) &&
  nativeResults.some((item) => item.id === "mobile_ffi_exposes_shared_file_handoff_reseal_proof_without_plaintext" && item.ok);
const boundedTransferQueueReady =
  nativeResults.some((item) => item.id === "secure_mesh_file_transfer_queue_is_bounded_confirmed_and_purged" && item.ok) &&
  nativeResults.some((item) => item.id === "secure_mesh_file_transfer_queue_rejects_ciphertext_byte_overflow_without_mutation" && item.ok);
const report = {
  ok,
  schemaVersion: "licomesh.secure-mesh.encrypted-file-handoff-report.v1",
  evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  verifier: "tools/scripts/client-secure-mesh-encrypted-file-handoff.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-encrypted-file-handoff.mjs",
  generatedAt: checkedAt,
  ...optionalReleaseInvocationBinding(),
  checkedAt,
  sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  blocker,
  diagnosticStatus: "incomplete",
  productionReady,
  releaseReady: false,
  evidenceKind: "redacted-native-file-codec-cli-ffi-and-policy-evidence",
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  reportLeakScan: true,
  ...scopeEvidence,
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
  sourceResults,
  nativeResults,
  androidPlatformCrypto,
  stationAcceptance,
  releaseBuiltDesktopFilePolicy,
  handoffEvidence: {
    encryptedManifestAndChunks: nativeResults.some((item) => item.id === "secure_mesh_file_manifest_and_chunk_round_trip_without_outer_metadata_leak" && item.ok),
    deliveryJsonRedacted: nativeResults.some((item) => item.id === "secure_mesh_file_delivery_json_hides_manifest_and_chunk_plaintext" && item.ok),
    ciphertextTamperRejected: nativeResults.some((item) => item.id === "secure_mesh_file_chunk_rejects_corrupted_ciphertext_hash" && item.ok),
    pathTraversalRejected: nativeResults.some((item) => item.id === "secure_mesh_file_manifest_rejects_path_traversal" && item.ok),
    boundedTransferQueueReady,
    duplicateChunkConflictRejected: nativeResults.some((item) => item.id === "secure_mesh_file_transfer_rejects_conflicting_duplicate_chunk" && item.ok),
    endpointSpecificResealProofReady: nativeResults.some((item) => item.id === "secure_mesh_file_handoff_proof_reseals_endpoint_specific_ciphertext" && item.ok),
    multiRecipientEndpointSpecificResealProofReady,
    boundedTransferQueueReady,
    receiveDestinationApprovedRootRequired: nativeResults.some((item) => item.id === "secure_mesh_file_receive_destination_rejects_unapproved_paths" && item.ok),
    receiveConfirmationRequiresUserAction: nativeResults.some((item) => item.id === "secure_mesh_file_receive_confirmation_requires_user_action_and_disables_auto_open" && item.ok),
    receiveConfirmationCliRedacted: nativeResults.some((item) => item.id === "secure_mesh_file_receive_confirmation_cli_requires_user_confirmation_without_auto_open" && item.ok),
    cliRouteAndReceivePolicyRedacted: nativeResults.some((item) => item.id === "secure_mesh_file_receive_destination_cli_redacts_destination_paths" && item.ok),
    mobileFfiUsesSharedRustPolicy: nativeResults.some((item) => item.id === "mobile_ffi_exposes_shared_file_route_and_receive_destination_policy" && item.ok),
    mobileFfiHandoffResealProofReady: nativeResults.some((item) => item.id === "mobile_ffi_exposes_shared_file_handoff_reseal_proof_without_plaintext" && item.ok),
    androidPlatformCryptoReady: androidPlatformCrypto.ok === true,
    stationAcceptanceReady: stationAcceptance.ready === true,
    stationAcceptanceFreshEndpointCount: stationAcceptance.freshEndpointCount,
    stationAcceptancePositiveExchange: stationAcceptance.positiveExchange,
    stationAcceptanceRoundTrip: stationAcceptance.roundTrip,
    stationAcceptancePlaintextAbsent: stationAcceptance.stationPlaintextAbsent,
    stationAcceptanceNonConformantEnvelopeRejected:
      stationAcceptance.nonConformantEnvelopeRejected,
    stationAcceptanceTransportHintsNonAuthoritative:
      stationAcceptance.transportHintsNonAuthoritative,
    stationAcceptanceExactFiveOuterFields:
      stationAcceptance.exactFiveOuterFields,
    releaseBuiltDesktopFilePolicyReady: releaseBuiltDesktopFilePolicy.ready === true,
    releaseBuiltDesktopMatrixSatisfied: releaseBuiltDesktopFilePolicy.matrixSatisfied === true,
    releaseBuiltDesktopWindowsLocalBlockersCleared:
      releaseBuiltDesktopFilePolicy.windowsLocalBlockersCleared === true,
    releaseBuiltDesktopReadyPlatforms: releaseBuiltDesktopFilePolicy.readyPlatforms
  },
  physicalHandoffMatrix: handoffMatrix,
  summary: {
    verificationPassed: ok,
    sourceCheckCount: sourceResults.length,
    nativeTestCount: nativeResults.length,
    androidPlatformCryptoReady: androidPlatformCrypto.ok === true,
    stationAcceptanceReady: stationAcceptance.ready === true,
    stationAcceptanceFreshEndpointCount: stationAcceptance.freshEndpointCount,
    stationAcceptancePositiveExchange: stationAcceptance.positiveExchange,
    stationAcceptanceRoundTrip: stationAcceptance.roundTrip,
    stationAcceptancePlaintextAbsent: stationAcceptance.stationPlaintextAbsent,
    stationAcceptanceNonConformantEnvelopeRejected:
      stationAcceptance.nonConformantEnvelopeRejected,
    stationAcceptanceTransportHintsNonAuthoritative:
      stationAcceptance.transportHintsNonAuthoritative,
    stationAcceptanceExactFiveOuterFields:
      stationAcceptance.exactFiveOuterFields,
    releaseBuiltDesktopFilePolicyReady: releaseBuiltDesktopFilePolicy.ready === true,
    releaseBuiltDesktopMatrixSatisfied: releaseBuiltDesktopFilePolicy.matrixSatisfied === true,
    releaseBuiltDesktopWindowsLocalBlockersCleared:
      releaseBuiltDesktopFilePolicy.windowsLocalBlockersCleared === true,
    releaseBuiltDesktopReadyPlatformCount: releaseBuiltDesktopFilePolicy.readyPlatforms.length,
    releaseBuiltDesktopReadyPlatforms: releaseBuiltDesktopFilePolicy.readyPlatforms,
    physicalHandoffScenarioCount: handoffMatrix.length,
    physicalHandoffPartialScenarioCount: handoffMatrix.filter((item) => item.status !== "missing").length,
    multiRecipientEndpointSpecificResealProofReady,
    productionReady,
    releaseReady: false,
    reportLeakScan: true,
    remainingGates: [
      "physical Android-to-desktop-to-iPhone encrypted file handoff",
      "physical iPhone-to-desktop-to-Android encrypted file handoff",
      releaseBuiltDesktopFilePolicy.matrixSatisfied === true
        ? null
        : "desktop macOS, Windows, and Ubuntu/Linux release-built file handoff matrix",
      ...(multiRecipientEndpointSpecificResealProofReady
        ? []
        : ["shared Rust endpoint-specific reseal proof for every recipient"]),
      ...(receiveConfirmationReady
        ? []
        : ["client receive confirmation with auto-preview and auto-ingestion disabled by default"]),
      ...(stationAcceptance.ready === true
        ? []
        : ["strict Lico Arc BadTower interoperability acceptance"])
    ].filter(Boolean)
  }
};

assertNoLeak(report, "secure mesh encrypted file handoff report");
atomicWriteReportJson(
  path.join(repoRoot, "build"),
  reportPath.replace(/^build\//u, ""),
  report,
);

console.log(JSON.stringify({
  ok,
  report: reportPath,
  sourceOfTruth: report.sourceOfTruth,
  blocker: report.blocker,
  diagnosticStatus: report.diagnosticStatus,
  productionReady,
  sourceCheckCount: sourceResults.length,
  nativeTestCount: nativeResults.length,
  androidPlatformCryptoReady: androidPlatformCrypto.ok === true,
  stationAcceptanceReady: stationAcceptance.ready === true,
  releaseBuiltDesktopFilePolicyReady: releaseBuiltDesktopFilePolicy.ready === true,
  releaseBuiltDesktopMatrixSatisfied: releaseBuiltDesktopFilePolicy.matrixSatisfied === true,
  releaseBuiltDesktopWindowsLocalBlockersCleared:
    releaseBuiltDesktopFilePolicy.windowsLocalBlockersCleared === true,
  releaseBuiltDesktopReadyPlatformCount: releaseBuiltDesktopFilePolicy.readyPlatforms.length,
  physicalHandoffPartialScenarioCount: report.summary.physicalHandoffPartialScenarioCount,
  remainingGateCount: report.summary.remainingGates.length
}, null, 2));

if (!ok || (strict && productionReady !== true)) {
  process.exitCode = 1;
}
