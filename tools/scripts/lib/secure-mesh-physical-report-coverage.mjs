/**
 * Shared physical-report coverage predicates for Secure Mesh blocker producers.
 * Owns the acceptance semantics reused by platform-secret-store-matrix,
 * physical-device-matrix, and physical-evidence-manifest so ready gates cannot
 * drift across those producers.
 */

import {
  licoArcBadTowerAcceptanceCoverage,
} from "./licoarc-badtower-acceptance-report.mjs";

export const ANDROID_PLATFORM_CRYPTO_ACCEPTANCE_SCHEMA =
  "licomesh.secure-mesh.android-platform-crypto-acceptance.v1";
export const ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT = 5;
export const UBUNTU_SECRET_SERVICE_BACKEND = "linux-secret-service-keyring";

export function redactedReportReady(report) {
  return report?.ok === true &&
    report?.redacted === true &&
    report?.rawPrivateMaterialIncluded !== true &&
    report?.rawPlaintextIncluded !== true &&
    report?.rawPublicWireBytesIncluded !== true &&
    report?.reportLeakScan === true;
}

export function androidPlatformCryptoCoverage(report, {
  reportRef = "",
  freshness = null
} = {}) {
  const summary = report?.summary || {};
  const contractReady = redactedReportReady(report) &&
    report?.schemaVersion === ANDROID_PLATFORM_CRYPTO_ACCEPTANCE_SCHEMA &&
    report?.platform === "android" &&
    summary.platformCryptoAcceptanceReady === true &&
    summary.platformCustodyContractReady === true &&
    summary.platformAuthorizationContractReady === true &&
    summary.rustFfiActionContractReady === true &&
    summary.mlsMemberRemoveReleaseActionReady === true &&
    summary.unknownReleaseActionsFailClosed === true &&
    Number(summary.nativeTestClassCount || 0) ===
      ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT;
  const ready = freshness == null
    ? contractReady
    : contractReady && freshness?.ready === true;
  return {
    report: reportRef,
    ready,
    contractReady,
    ...(freshness == null ? {} : { freshness }),
    platformCryptoAcceptanceReady:
      summary.platformCryptoAcceptanceReady === true,
    platformCustodyContractReady:
      summary.platformCustodyContractReady === true,
    platformAuthorizationContractReady:
      summary.platformAuthorizationContractReady === true,
    rustFfiActionContractReady: summary.rustFfiActionContractReady === true,
    mlsMemberRemoveReleaseActionReady:
      summary.mlsMemberRemoveReleaseActionReady === true,
    unknownReleaseActionsFailClosed:
      summary.unknownReleaseActionsFailClosed === true,
    nativeTestClassCount: Number(summary.nativeTestClassCount || 0)
  };
}

export function stationAcceptanceCoverage(report, { reportRef = "" } = {}) {
  const coverage = licoArcBadTowerAcceptanceCoverage(report);
  return {
    report: reportRef,
    ...coverage,
  };
}

export function windowsPersistentCustodyBoundaryValid(report) {
  const summary = report?.summary || {};
  const platform = report?.platform || {};
  return redactedReportReady(report) &&
    report?.schemaVersion ===
      "licomesh.secure-mesh.windows-implementation-report.v2" &&
    report?.diagnosticStatus === "persistent_custody_unverified" &&
    report?.evidenceKind === "redacted-windows-conservative-custody-boundary" &&
    report?.productionReady !== true &&
    report?.releaseReady !== true &&
    summary.windowsLocalBlockersCleared !== true &&
    summary.nativeHostEvidencePending === true &&
    summary.dpapiOrWindowsHelloProofReady !== true &&
    platform.platform === "windows" &&
    platform.status === "persistent-custody-unverified" &&
    platform.localImplementationReady !== true &&
    platform.dpapiOrWindowsHelloImplementationReady !== true &&
    platform.localSecretStore === "memory-only-ephemeral" &&
    platform.productionSupportClaimed !== true;
}

export function windowsImplementationReady(report) {
  const summary = report?.summary || {};
  const platform = report?.platform || {};
  return redactedReportReady(report) &&
    report?.diagnosticStatus === "persistent_custody_verified" &&
    report?.productionReady !== true &&
    report?.releaseReady !== true &&
    summary.windowsLocalBlockersCleared === true &&
    summary.nativeHostEvidencePending !== true &&
    summary.dpapiOrWindowsHelloProofReady === true &&
    platform.platform === "windows" &&
    platform.localImplementationReady === true &&
    platform.dpapiOrWindowsHelloImplementationReady === true &&
    platform.localSecretStore !== "memory-only-ephemeral" &&
    platform.productionSupportClaimed !== true;
}

export function windowsImplementationCoverage(report, { reportRef = "" } = {}) {
  return {
    report: reportRef,
    present: Boolean(report && Object.keys(report).length > 0),
    conservativeBoundaryValid: windowsPersistentCustodyBoundaryValid(report),
    ready: windowsImplementationReady(report),
    windowsLocalBlockersCleared:
      report?.summary?.windowsLocalBlockersCleared === true,
    dpapiOrWindowsHelloProofReady:
      report?.summary?.dpapiOrWindowsHelloProofReady === true
  };
}

/**
 * Interpret platform-secret-store-matrix summary custody flags.
 * `*BindingReady` is custody-binding only; `*Ready` also requires raw-JSON
 * override absence where that proof is defined (android/ios).
 */
export function platformSecretStoreCustodyCoverage(report) {
  const summary = report?.summary || {};
  const androidBindingReady = report?.ok === true &&
    summary.androidPhysicalSecretStoreBindingReady === true &&
    summary.androidPhysicalSystemCredentialAuthReady === true &&
    summary.androidPhysicalKeyStoreHardwareAuthReady === true &&
    summary.androidPhysicalCallbackContractReady === true;
  const iosBindingReady = report?.ok === true &&
    summary.iosPhysicalSecretStoreBindingReady === true &&
    summary.iosUserPresencePolicyReady === true &&
    summary.iosProductionCallbackAuthReady === true &&
    summary.iosSystemLocalAuthPromptReady === true &&
    summary.iosKeychainAccessControlNotDowngraded === true &&
    summary.iosNonInteractiveFailClosedReady === true &&
    summary.iosCancelLockFailClosedReady === true &&
    summary.iosPhysicalCallbackContractReady === true;
  const androidReady = androidBindingReady &&
    summary.androidPhysicalRawJsonSecretOverridesProvenAbsent === true;
  const iosReady = iosBindingReady &&
    summary.iosPhysicalRawJsonSecretOverridesProvenAbsent === true;
  const macosReady = report?.ok === true &&
    summary.macosSafeOsStoreAvailable === true &&
    summary.macosExactCapabilitySetValid === true &&
    summary.macosSingleSystemAuthorizationContextVerified === true &&
    summary.macosPromptBudgetSatisfied === true &&
    summary.macosAppCredentialPromptUsed !== true;
  const ubuntuReady = summary.ubuntuVmSecretStoreReady === true &&
    summary.ubuntuVmSecretStoreBackend === UBUNTU_SECRET_SERVICE_BACKEND;
  return {
    ok: report?.ok === true,
    androidBindingReady,
    androidReady,
    iosBindingReady,
    iosReady,
    macosReady,
    ubuntuReady,
    androidPhysicalSecretStoreBindingReady:
      summary.androidPhysicalSecretStoreBindingReady === true,
    androidPhysicalSystemCredentialAuthReady:
      summary.androidPhysicalSystemCredentialAuthReady === true,
    androidPhysicalKeyStoreHardwareAuthReady:
      summary.androidPhysicalKeyStoreHardwareAuthReady === true,
    androidPhysicalCallbackContractReady:
      summary.androidPhysicalCallbackContractReady === true,
    androidPhysicalRawJsonSecretOverridesProvenAbsent:
      summary.androidPhysicalRawJsonSecretOverridesProvenAbsent === true,
    iosPhysicalSecretStoreBindingReady:
      summary.iosPhysicalSecretStoreBindingReady === true,
    iosUserPresencePolicyReady: summary.iosUserPresencePolicyReady === true,
    iosProductionCallbackAuthReady:
      summary.iosProductionCallbackAuthReady === true,
    iosPhysicalCallbackContractReady:
      summary.iosPhysicalCallbackContractReady === true,
    iosPhysicalRawJsonSecretOverridesProvenAbsent:
      summary.iosPhysicalRawJsonSecretOverridesProvenAbsent === true,
    macosKeychainReady: summary.macosSafeOsStoreAvailable === true,
    macosSafeOsStoreAvailable: summary.macosSafeOsStoreAvailable === true,
    macosExactCapabilitySetValid:
      summary.macosExactCapabilitySetValid === true,
    macosReleaseCliProofReady: summary.macosReleaseCliProofReady === true,
    macosUserPresencePolicyReady:
      summary.macosUserPresencePolicyReady === true ||
      summary.macosSafeOsStoreAvailable === true,
    macosSingleSystemAuthorizationContextVerified:
      summary.macosSingleSystemAuthorizationContextVerified === true,
    macosInteractiveAuthorizationPromptBudgetReady:
      summary.macosPromptBudgetSatisfied === true,
    macosPromptBudgetSatisfied: summary.macosPromptBudgetSatisfied === true,
    macosAppCredentialPromptUsed:
      summary.macosAppCredentialPromptUsed === true,
    ubuntuSecretServiceReady: ubuntuReady,
    ubuntuVmSecretStoreReady: summary.ubuntuVmSecretStoreReady === true,
    ubuntuVmSecretStoreBackend: String(summary.ubuntuVmSecretStoreBackend || ""),
    ubuntuReleaseCliProofReady: summary.ubuntuReleaseCliProofReady === true,
    ubuntuLinuxAdaptiveCustodyReady:
      summary.ubuntuLinuxAdaptiveCustodyReady === true,
    ubuntuLinuxPackageUpdateReady:
      summary.ubuntuLinuxPackageUpdateReady === true,
    remainingGates: Array.isArray(summary.remainingGates)
      ? summary.remainingGates.map((gate) => String(gate || "")).filter(Boolean)
      : []
  };
}

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function redactedFixture(extra = {}) {
  return {
    ok: true,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    ...extra
  };
}

export function runPhysicalReportCoverageSelfTest() {
  requireValue(redactedReportReady(redactedFixture()), "redacted_ready_rejected");
  requireValue(
    !redactedReportReady(redactedFixture({ reportLeakScan: false })),
    "leak_scan_gap_accepted"
  );

  const androidSummary = {
    platformCryptoAcceptanceReady: true,
    platformCustodyContractReady: true,
    platformAuthorizationContractReady: true,
    rustFfiActionContractReady: true,
    mlsMemberRemoveReleaseActionReady: true,
    unknownReleaseActionsFailClosed: true,
    nativeTestClassCount: ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT
  };
  const android = redactedFixture({
    schemaVersion: ANDROID_PLATFORM_CRYPTO_ACCEPTANCE_SCHEMA,
    platform: "android",
    summary: androidSummary
  });
  requireValue(
    androidPlatformCryptoCoverage(android).ready === true,
    "android_crypto_ready_rejected"
  );
  requireValue(
    androidPlatformCryptoCoverage(android, {
      freshness: { ready: false, status: "stale" }
    }).ready !== true,
    "android_crypto_stale_accepted"
  );
  requireValue(
    androidPlatformCryptoCoverage({
      ...android,
      summary: {
        ...androidSummary,
        nativeTestClassCount: ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT - 1
      }
    }).contractReady !== true,
    "android_crypto_class_count_gap_accepted"
  );

  const station = {
    schemaVersion: "licoup.licoarc-badtower.acceptance.v1",
    ok: true,
    protocolCandidateDigest: `sha256:${"a".repeat(64)}`,
    stationCandidateDigest: `sha256:${"b".repeat(64)}`,
    clientCandidateDigest: `sha256:${"c".repeat(64)}`,
    scenario: {
      freshEndpointCount: 2,
      positiveExchange: true,
      roundTrip: true,
      stationPlaintextAbsent: true,
      nonConformantEnvelopeRejected: true,
      transportHintsNonAuthoritative: true,
      exactFiveOuterFields: true,
    },
    privacy: {
      redacted: true,
      endpointContentIncluded: false,
      ciphertextIncluded: false,
      keyMaterialIncluded: false,
      machineIdentityIncluded: false,
      rawRuntimeDataIncluded: false,
    },
    claims: {
      clientRelease: false,
      protocolPublication: false,
      stationRelease: false,
      hostedOperation: false,
    },
  };
  requireValue(
    stationAcceptanceCoverage(station).ready === true,
    "station_acceptance_ready_rejected",
  );
  requireValue(
    stationAcceptanceCoverage({
      ...station,
      scenario: { ...station.scenario, roundTrip: false },
    }).ready !== true,
    "station_round_trip_gap_accepted",
  );

  const windows = redactedFixture({
    schemaVersion: "licomesh.secure-mesh.windows-implementation-report.v2",
    diagnosticStatus: "persistent_custody_unverified",
    evidenceKind: "redacted-windows-conservative-custody-boundary",
    productionReady: false,
    releaseReady: false,
    summary: {
      windowsLocalBlockersCleared: false,
      nativeHostEvidencePending: true,
      dpapiOrWindowsHelloProofReady: false
    },
    platform: {
      platform: "windows",
      status: "persistent-custody-unverified",
      localImplementationReady: false,
      dpapiOrWindowsHelloImplementationReady: false,
      localSecretStore: "memory-only-ephemeral",
      productionSupportClaimed: false
    }
  });
  requireValue(
    windowsPersistentCustodyBoundaryValid(windows) === true,
    "windows_conservative_boundary_rejected"
  );
  requireValue(
    windowsImplementationReady(windows) !== true,
    "windows_unverified_custody_accepted"
  );
  requireValue(
    windowsPersistentCustodyBoundaryValid({
      ...windows,
      platform: { ...windows.platform, localSecretStore: "unknown" }
    }) !== true,
    "windows_unsafe_custody_boundary_accepted"
  );

  const custody = platformSecretStoreCustodyCoverage({
    ok: true,
    summary: {
      androidPhysicalSecretStoreBindingReady: true,
      androidPhysicalSystemCredentialAuthReady: true,
      androidPhysicalKeyStoreHardwareAuthReady: true,
      androidPhysicalCallbackContractReady: true,
      androidPhysicalRawJsonSecretOverridesProvenAbsent: true,
      iosPhysicalSecretStoreBindingReady: true,
      iosUserPresencePolicyReady: true,
      iosProductionCallbackAuthReady: true,
      iosSystemLocalAuthPromptReady: true,
      iosKeychainAccessControlNotDowngraded: true,
      iosNonInteractiveFailClosedReady: true,
      iosCancelLockFailClosedReady: true,
      iosPhysicalCallbackContractReady: true,
      iosPhysicalRawJsonSecretOverridesProvenAbsent: true,
      macosSafeOsStoreAvailable: true,
      macosExactCapabilitySetValid: true,
      macosSingleSystemAuthorizationContextVerified: true,
      macosPromptBudgetSatisfied: true,
      macosAppCredentialPromptUsed: false,
      ubuntuVmSecretStoreReady: true,
      ubuntuVmSecretStoreBackend: UBUNTU_SECRET_SERVICE_BACKEND
    }
  });
  requireValue(custody.androidBindingReady === true, "android_binding_rejected");
  requireValue(custody.androidReady === true, "android_ready_rejected");
  requireValue(custody.iosReady === true, "ios_ready_rejected");
  requireValue(custody.macosReady === true, "macos_ready_rejected");
  requireValue(custody.ubuntuReady === true, "ubuntu_ready_rejected");
  requireValue(
    platformSecretStoreCustodyCoverage({
      ok: true,
      summary: {
        androidPhysicalSecretStoreBindingReady: true,
        androidPhysicalSystemCredentialAuthReady: true,
        androidPhysicalKeyStoreHardwareAuthReady: true,
        androidPhysicalCallbackContractReady: true,
        androidPhysicalRawJsonSecretOverridesProvenAbsent: false
      }
    }).androidReady !== true,
    "android_raw_json_gap_accepted_as_ready"
  );
  requireValue(
    platformSecretStoreCustodyCoverage({
      ok: true,
      summary: {
        androidPhysicalSecretStoreBindingReady: true,
        androidPhysicalSystemCredentialAuthReady: true,
        androidPhysicalKeyStoreHardwareAuthReady: true,
        androidPhysicalCallbackContractReady: true,
        androidPhysicalRawJsonSecretOverridesProvenAbsent: false
      }
    }).androidBindingReady === true,
    "android_binding_requires_raw_json"
  );

  return Object.freeze({ ok: true, caseCount: 16 });
}
