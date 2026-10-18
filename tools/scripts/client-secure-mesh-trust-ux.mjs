#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { runCargoTestFilter } from "./lib/cargo-test-filter-runner.mjs";
import { acquireTestArtifactLease } from "./lib/test-artifact-lifecycle.mjs";
import { loadSecureClientContract } from "./lib/secure-client-contract.mjs";
import { createSecureClientMeshE2eeRefReportScope } from "./lib/secure-client-mesh-e2ee-ref-report.mjs";
import { loadSecureMeshPhysicalEvidenceConfig } from "./lib/secure-mesh-physical-evidence-config.mjs";
import { loadSecureMeshTrustUxConfig } from "./lib/secure-mesh-trust-ux-config.mjs";
import { optionalReleaseInvocationBinding } from "./lib/release-closure-challenge.mjs";
import { atomicWriteReportJson } from "./lib/safe-report-io.mjs";
import { readSourceCheckBundle } from "./lib/source-check-bundle.mjs";
import {
  SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
  SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID,
  SECURE_MESH_TRUST_UX_REPORT_SCHEMA_VERSION,
  SECURE_MESH_TRUST_UX_SELECTED_TARGETS,
  reduceSecureMeshTrustUxReadiness,
  runSecureMeshTrustUxReducerSelfTest
} from "./lib/secure-mesh-trust-ux-reducer.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const args = new Set(process.argv.slice(2));
if (args.has("--self-test")) {
  console.log(JSON.stringify(runSecureMeshTrustUxReducerSelfTest()));
  process.exit(0);
}
const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
const physicalReportRefs = physicalEvidenceConfig.linkedReports;
const trustUxConfig = await loadSecureMeshTrustUxConfig();
const {
  sourceChecks,
  nativeTestFilters,
  productTestTargets,
  expectedMobileNativeTrustActions
} = trustUxConfig;
const reportPath = physicalReportRefs.trustUx;
const strict = args.has("--strict");
const selectedTargetStrict = args.has("--selected-target-strict");
const macosAdaptiveReceiptRef = "build/reports/secure-mesh-macos-capabilities.json";

const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u]
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
  return {
    id: check.id,
    file: check.file,
    files,
    ok: missingTokens.length === 0,
    missingTokens
  };
}

async function productControllerDirectSealGap() {
  const controllerRoot = path.join(
    repoRoot,
    "apps/desktop/lib/src/application/features/mobile_relay/controller"
  );
  const forbiddenTokens = [
    "seal_payload_envelope",
    "seal_acp_protected_payload",
    "seal_file_manifest",
    "seal_file_chunk",
    "seal_mobile_relay_payload",
    "authorize_protected_send("
  ];
  const entries = await fs.readdir(controllerRoot, { withFileTypes: true });
  const dartFiles = entries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".dart"))
    .map((entry) => path.join("apps/desktop/lib/src/application/features/mobile_relay/controller", entry.name));
  const hits = [];
  for (const relativePath of dartFiles) {
    const source = await readText(relativePath);
    for (const token of forbiddenTokens) {
      if (source.includes(token)) {
        hits.push({ file: relativePath, token });
      }
    }
  }
  return {
    id: "product-controllers-forbid-direct-seal-path-calls",
    file: "apps/desktop/lib/src/application/features/mobile_relay/controller",
    ok: hits.length === 0,
    missingTokens: [],
    forbiddenHits: hits
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

function runProductTests(targets) {
  const appRoot = path.join(repoRoot, "apps", "desktop");
  const appPrefix = "apps/desktop/";
  const testTargets = targets.map((target) => target.slice(appPrefix.length));
  const lease = acquireTestArtifactLease({
    repoRoot,
    scope: "secure-mesh-trust-ux",
    targetPath: "apps/desktop/build"
  });
  let command;
  try {
    command = spawnSync("flutter", ["test", ...testTargets], {
      cwd: appRoot,
      env: process.env,
      encoding: "utf8",
      stdio: "pipe",
      maxBuffer: 8 * 1024 * 1024,
      timeout: 300_000
    });
  } finally {
    lease.release();
  }
  const exitCode = Number.isInteger(command.status) ? command.status : -1;
  return {
    id: SECURE_MESH_TRUST_UX_PRODUCT_TEST_ID,
    ok: exitCode === 0,
    targetCount: targets.length,
    exitCode,
    failureCategory: exitCode === 0
      ? ""
      : command.error?.code === "ENOENT"
        ? "flutter-unavailable"
        : command.error?.code === "ETIMEDOUT"
          ? "test-timeout"
          : "test-failed"
  };
}

async function mobileNativeTrustActionGap() {
  const source = await readText(
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/action_catalog.rs"
  );
  const presentActions = expectedMobileNativeTrustActions.filter((action) => source.includes(action));
  return {
    expectedActions: expectedMobileNativeTrustActions,
    presentActions,
    missingActions: expectedMobileNativeTrustActions.filter((action) => !presentActions.includes(action)),
    status: presentActions.length === expectedMobileNativeTrustActions.length ? "complete" : "missing"
  };
}

async function physicalTrustEvidence() {
  const androidReportRef = physicalReportRefs.androidPlatformCrypto;
  const android = await readJsonIfPresent(androidReportRef);
  return {
    android: androidPlatformTrustEvidence(android, androidReportRef),
    ios: {
      report: "",
      present: false,
      ok: false,
      platform: "ios",
      releaseGate: false,
      supportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
      status: "unsupported-not-claimed"
    }
  };
}

function androidPlatformTrustEvidence(report, reportRef) {
  const summary = report?.summary || {};
  const present = Boolean(report && Object.keys(report).length > 0);
  const platformContractReady = report?.ok === true &&
    report?.schemaVersion === "licomesh.secure-mesh.android-platform-crypto-acceptance.v1" &&
    report?.platform === "android" &&
    report?.redacted === true &&
    report?.reportLeakScan === true &&
    summary.platformCryptoAcceptanceReady === true &&
    summary.platformCustodyContractReady === true &&
    summary.platformAuthorizationContractReady === true &&
    summary.rustFfiActionContractReady === true &&
    summary.mlsMemberRemoveReleaseActionReady === true &&
    summary.unknownReleaseActionsFailClosed === true;
  return {
    report: reportRef,
    present,
    ok: false,
    platform: "android",
    physicalDevice: false,
    peerVerified: false,
    capabilityReportValid: platformContractReady,
    mandatoryFoundationComplete: platformContractReady,
    custodyStrategy: "",
    safeCustodyReady: false,
    portableConfigPrivateMaterialAbsent:
      report?.rawPrivateMaterialIncluded === false &&
      summary.privatePathsIncluded === false,
    restartReplayReady: false,
    lifecycleFfiReady: summary.rustFfiActionContractReady === true,
    sasVerificationReady: false,
    qrVerificationReady: false,
    keyChangeBlocksSensitive: false,
    rotateLifecycleReady: false,
    revokeBlocksSensitive: false,
    recoveryRequiresConfirmation: false,
    trustLifecycleReady: false,
    failureCategory: present && platformContractReady
      ? "physical-peer-trust-not-proven"
      : "android-platform-crypto-report-missing-or-invalid",
    status: present && platformContractReady
      ? "android-platform-crypto-ready-physical-trust-missing"
      : "missing"
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
const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find((item) => item === "trust UX");
if (!blocker) {
  throw new Error("Client-pinned Secure Client Mesh contract does not define trust UX blocker");
}

const sourceResults = [];
for (const check of sourceChecks) {
  sourceResults.push(await evaluateSourceCheck(check));
}
const productControllerSealGap = await productControllerDirectSealGap();
sourceResults.push(productControllerSealGap);
const nativeResults = nativeTestFilters.map(runNativeTest);
const productTestResults = [runProductTests(productTestTargets)];
const mobileNativeTrustActions = await mobileNativeTrustActionGap();
const physicalTrust = await physicalTrustEvidence();
const macosAdaptiveReceipt = await readJsonIfPresent(macosAdaptiveReceiptRef);
const ok = sourceResults.every((check) => check.ok) &&
  nativeResults.every((check) => check.ok) &&
  productTestResults.every((check) => check.ok);
const productionReady = false;
const checkedAt = new Date().toISOString();
const scopeEvidence = await createSecureClientMeshE2eeRefReportScope({
  contract,
  reportRef: reportPath,
  blocker,
  checkedAt
});
const mobileNativeActionsComplete = mobileNativeTrustActions.missingActions.length === 0;
const selectedTargetReadiness = reduceSecureMeshTrustUxReadiness({
  verificationPassed: ok,
  mobileNativeActionsComplete,
  sourceResults,
  productTestResults,
  physicalTrust,
  macosAdaptiveReceipt
});
const {
  productTrustUxReady,
  androidPhysicalTrustReady,
  macosTrustReceiptReady,
  selectedTargetReleaseReady
} = selectedTargetReadiness;
const remainingGates = [
  ...(mobileNativeActionsComplete
    ? []
    : ["mobile native trust verification actions for Android and iPhone"]),
  ...(productTrustUxReady
    ? []
    : ["desktop and mobile QR/60-digit-safety-number/fingerprint product UX"]),
  ...(androidPhysicalTrustReady
    ? []
    : ["physical Android QR/60-digit-safety-number/fingerprint trust lifecycle with key-change/revoke fail-closed"]),
  ...(macosTrustReceiptReady
    ? []
    : ["signed current-source macOS trust UX install, launch, and smoke receipt"]),
  "release proof bundle linking trust UX evidence to command/result/file/prekey/group fail-closed behavior"
];
const report = {
  ok,
  schemaVersion: SECURE_MESH_TRUST_UX_REPORT_SCHEMA_VERSION,
  evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  verifier: "tools/scripts/client-secure-mesh-trust-ux.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-trust-ux.mjs",
  generatedAt: checkedAt,
  ...optionalReleaseInvocationBinding(),
  checkedAt,
  sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  blocker,
  diagnosticStatus: selectedTargetReleaseReady ? "selected-target-ready" : "incomplete",
  productionReady,
  releaseReady: selectedTargetReleaseReady,
  evidenceKind: "redacted-selected-target-trust-ux-report",
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
  productTestResults,
  trustEvidence: {
    crossSigningTamperRejected: nativeResults.some((item) => item.id === "secure_mesh_device_cross_signature_verifies_and_rejects_tamper" && item.ok),
    sasSymmetric: nativeResults.some((item) => item.id === "secure_mesh_device_sas_is_symmetric" && item.ok),
    qrPayloadUsesFingerprintsOnly: nativeResults.some((item) => item.id === "secure_mesh_device_qr_payload_uses_fingerprints_not_raw_keys" && item.ok),
    identityKeyChangeDetected: nativeResults.some((item) => item.id === "secure_mesh_device_key_change_is_detected" && item.ok),
    policyBlocksKeyChangedDevice: nativeResults.some((item) => item.id === "secure_mesh_device_trust_policy_json_treats_caller_verified_as_advisory_and_blocks_key_change" && item.ok),
    authorizeProtectedSendBlocksUntrustedPeers: nativeResults.some((item) => item.id === "secure_mesh_authorize_protected_send_blocks_unverified_key_changed_and_revoked_for_all_kinds" && item.ok),
    observationAloneNeverAuthorizes: nativeResults.some((item) => item.id === "secure_mesh_authorize_protected_send_from_trust_record_and_rejects_observation_alone" && item.ok),
    productSealBlocksUntrustedPeers: nativeResults.some((item) => item.id === "mobile_relay_protected_send_blocks_unverified_key_changed_and_revoked_peers" && item.ok),
    relayPeerSubstitutionFailClosed: nativeResults.some((item) => item.id === "out_of_band_pairing_response_rejects_substituted_peer_without_claim_proof" && item.ok),
    signedTrustRecordRequiredBeforeProtectedUse: nativeResults.some((item) => item.id === "mobile_relay_secure_command_requires_signed_peer_trust_record" && item.ok),
    tamperedTrustRecordRejected: nativeResults.some((item) => item.id === "mobile_relay_secure_command_rejects_tampered_peer_trust_record" && item.ok),
    cliPolicyEvaluatorAvailable: nativeResults.some((item) => item.id === "secure_mesh_device_trust_evaluate_cli_reports_policy_decision" && item.ok),
    deviceVerifyRequiresUserConfirmation: nativeResults.some((item) => item.id === "secure_mesh_command_policy_allows_only_registered_commands" && item.ok)
  },
  productUxSurface: {
    desktopPolicyEvaluation: sourceResults.some((item) => item.id === "desktop-controller-exposes-trust-policy-for-relay-panel" && item.ok),
    ordinaryUiDiagnosticsHidden: sourceResults.some((item) => item.id === "desktop-tests-keep-diagnostics-hidden-from-ordinary-ui" && item.ok),
    mobileNativeTrustActions
  },
  physicalTrustEvidence: physicalTrust,
  selectedTargetAcceptance: {
    selectedTargets: [...SECURE_MESH_TRUST_UX_SELECTED_TARGETS],
    productTrustUxReady,
    androidPhysicalTrustReady,
    macosTrustReceiptReady,
    selectedTargetReleaseReady,
    iosSupportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
    iosReleaseGate: false,
    macosReceipt: macosAdaptiveReceiptRef
  },
  physicalTrustMatrix: [
    {
      scenario: "desktop-to-Android QR/60-digit-safety-number/fingerprint verification",
      status: androidPhysicalTrustReady ? "android-physical-verified-partial" : "missing",
      evidence: androidPhysicalTrustReady
        ? [
            "Physical Android client executed shared Rust QR and SAS verification actions through the app process.",
            "Physical Android key-change policy blocked sensitive operations until reverification.",
            "Physical Android rotate, revoke, and recovery lifecycle checks fail closed for sensitive command/result/file/prekey/group use."
          ]
        : [],
      evidenceReports: androidPhysicalTrustReady ? [physicalTrust.android.report] : [],
      remainingGates: androidPhysicalTrustReady
        ? []
        : [
            "Physical Android client exposes shared Rust trust verification actions.",
            "Desktop and Android clients confirm the same QR/60-digit-safety-number/fingerprint evidence.",
            "Sensitive command/result/file/prekey/group use remains blocked until verification succeeds."
          ]
    },
    {
      scenario: "desktop-to-iPhone QR/60-digit-safety-number/fingerprint verification",
      status: "unsupported-not-claimed",
      releaseGate: false,
      evidence: [],
      evidenceReports: [],
      remainingGates: []
    },
    {
      scenario: "recovery-rotation-revoke lifecycle",
      status: androidPhysicalTrustReady ? "selected-target-verified" : "missing",
      evidence: [
        ...(androidPhysicalTrustReady ? ["Physical Android lifecycle proof covers rotate, revoke, and recovery confirmation."] : [])
      ],
      evidenceReports: [
        ...(androidPhysicalTrustReady ? [physicalTrust.android.report] : [])
      ],
      remainingGates: [
        ...(productTrustUxReady ? [] : ["Promote lifecycle warnings and recovery confirmation into product UX evidence."]),
        ...(androidPhysicalTrustReady ? [] : ["Link Android revoke/rotate/recover evidence to the selected artifact."])
      ]
    }
  ],
  summary: {
    verificationPassed: ok,
    sourceCheckCount: sourceResults.length,
    nativeTestCount: nativeResults.length,
    productTestCount: productTestResults.length,
    productTrustUxTestsReady: selectedTargetReadiness.productTrustUxTestsReady,
    mobileNativeTrustActionsReady: mobileNativeActionsComplete,
    productTrustUxReady,
    macosTrustReceiptReady,
    androidPhysicalTrustLifecycleReady: androidPhysicalTrustReady,
    iosSupportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
    iosReleaseGate: false,
    selectedTargetReleaseReady,
    productionReady,
    releaseReady: selectedTargetReleaseReady,
    reportLeakScan: true,
    remainingGates
  }
};

assertNoLeak(report, "secure mesh trust UX report");
atomicWriteReportJson(repoRoot, reportPath, report);

console.log(JSON.stringify({
  ok,
  report: reportPath,
  sourceOfTruth: report.sourceOfTruth,
  blocker: report.blocker,
  diagnosticStatus: report.diagnosticStatus,
  productionReady,
  sourceCheckCount: sourceResults.length,
  nativeTestCount: nativeResults.length,
  productTestCount: productTestResults.length,
  missingMobileNativeTrustActionCount: mobileNativeTrustActions.missingActions.length,
  androidPhysicalTrustLifecycleReady: androidPhysicalTrustReady,
  iosSupportStatus: SECURE_MESH_TRUST_UX_IOS_SUPPORT_STATUS,
  selectedTargetReleaseReady,
  remainingGateCount: report.summary.remainingGates.length
}, null, 2));

if (
  !ok ||
  (strict && productionReady !== true) ||
  (selectedTargetStrict && selectedTargetReleaseReady !== true)
) {
  process.exitCode = 1;
}
