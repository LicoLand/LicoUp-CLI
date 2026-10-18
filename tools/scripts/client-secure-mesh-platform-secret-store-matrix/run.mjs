import process from "node:process";
import { optionalReleaseInvocationBinding } from "../lib/release-closure-challenge.mjs";
import { atomicWriteReportJson } from "../lib/safe-report-io.mjs";
import { loadSecureClientContract } from "../lib/secure-client-contract.mjs";
import { createSecureClientMeshE2eeRefReportScope } from "../lib/secure-client-mesh-e2ee-ref-report.mjs";
import { loadSecureMeshPhysicalEvidenceConfig } from "../lib/secure-mesh-physical-evidence-config.mjs";
import { loadSecureMeshPlatformSecretStoreMatrixConfig } from "../lib/secure-mesh-platform-secret-store-matrix-config.mjs";
import { createCoverage } from "./coverage.mjs";
import { readJsonIfPresent, repoRoot } from "./io.mjs";
import { runNativeTest } from "./native-test.mjs";
import { assertNoLeak, dedupeRemainingGates } from "./privacy.mjs";
import { evaluateSourceCheck, selectedSourceChecksReady } from "./source-check.mjs";
import path from "node:path";

export async function main(argv = process.argv.slice(2)) {
  const args = new Set(argv);
  const strict = args.has("--strict");
  const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
  const platformSecretStoreMatrixConfig = await loadSecureMeshPlatformSecretStoreMatrixConfig();
  const physicalReportRefs = physicalEvidenceConfig.linkedReports;
  const reportPath = physicalReportRefs.platformSecretStore;
  const {
    androidPlatformCryptoCoverage,
    androidInstallLaunchCoverage,
    macosPlatformCryptoCoverage,
    ubuntuPlatformCryptoCoverage,
    genericProofCoverage,
    windowsImplementationCoverage,
  } = createCoverage(physicalReportRefs);

const contract = await loadSecureClientContract();
const {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} = contract;
const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find((item) =>
  item === "platform secret-store binding"
);
if (!blocker) {
  throw new Error("Client-pinned Secure Client Mesh contract does not define platform secret-store binding blocker");
}

const sourceResults = await Promise.all(
  platformSecretStoreMatrixConfig.sourceChecks.map(evaluateSourceCheck)
);
const nativeResults = platformSecretStoreMatrixConfig.nativeTestFilters.map(runNativeTest);
const [androidPlatformCryptoReport, androidInstallLaunchReport,
  macosUserPresenceProofReport, macosReleaseCliProofReport,
  ubuntuVmSecretStoreReport, ubuntuReleaseCliProofReport,
  ubuntuLinuxAdaptiveCustodyProofReport, ubuntuLinuxPackageUpdateProofReport,
  windowsImplementationReport] = await Promise.all([
  readJsonIfPresent(physicalReportRefs.androidPlatformCrypto),
  readJsonIfPresent(physicalReportRefs.androidInstallLaunch),
  readJsonIfPresent(physicalReportRefs.macosUserPresenceProof),
  readJsonIfPresent(physicalReportRefs.macosReleaseCliProof),
  readJsonIfPresent(physicalReportRefs.ubuntuVmSecretStore),
  readJsonIfPresent(physicalReportRefs.ubuntuReleaseCliProof),
  readJsonIfPresent(physicalReportRefs.ubuntuLinuxAdaptiveCustodyProof),
  readJsonIfPresent(physicalReportRefs.ubuntuLinuxPackageUpdateProof),
  readJsonIfPresent(physicalReportRefs.windowsImplementation)
]);

const rustCryptographyAcceptanceReady = nativeResults.every((result) => result.ok);
const androidPlatformCrypto = androidPlatformCryptoCoverage(androidPlatformCryptoReport);
const androidInstallLaunch = androidInstallLaunchCoverage(androidInstallLaunchReport);
const iosPlatformContractReady = selectedSourceChecksReady(sourceResults, [
  "rust-ios-callback-secret-store-ffi-exists",
  "ios-callback-abi-keychain-handle-and-raw-json-ban-exists",
  "ios-bridge-rust-secret-store-callback-wiring-exists",
  "ios-secret-store-callback-uses-single-system-authorization-context",
  "ios-local-auth-user-presence-proof-exists"
]);
const iosRustCryptographyTestsReady = nativeResults
  .filter((result) => /ios_callback|mobile_ffi_dispatcher_callback_store/u.test(result.id))
  .every((result) => result.ok) &&
  nativeResults.some((result) => /ios_callback/u.test(result.id));
const macosPlatformCrypto = macosPlatformCryptoCoverage(macosUserPresenceProofReport);
const ubuntuPlatformCrypto = ubuntuPlatformCryptoCoverage(ubuntuVmSecretStoreReport);
const macosReleaseCliProof = genericProofCoverage(
  macosReleaseCliProofReport,
  physicalReportRefs.macosReleaseCliProof
);
const ubuntuReleaseCliProof = genericProofCoverage(
  ubuntuReleaseCliProofReport,
  physicalReportRefs.ubuntuReleaseCliProof
);
const ubuntuLinuxAdaptiveCustodyProof = genericProofCoverage(
  ubuntuLinuxAdaptiveCustodyProofReport,
  physicalReportRefs.ubuntuLinuxAdaptiveCustodyProof
);
const ubuntuLinuxPackageUpdateProof = genericProofCoverage(
  ubuntuLinuxPackageUpdateProofReport,
  physicalReportRefs.ubuntuLinuxPackageUpdateProof
);
const windowsImplementation = windowsImplementationCoverage(
  windowsImplementationReport
);

const androidPhysicalSecretStoreBindingReady = androidPlatformCrypto.ready &&
  androidInstallLaunch.ok &&
  androidInstallLaunch.physicalDeviceProofReady &&
  androidInstallLaunch.runtimeStatusRedacted &&
  androidInstallLaunch.rawPayloadExportSurfaceAbsent;
const iosPhysicalSecretStoreBindingReady = false;
const iosUserPresencePolicyReady = iosPlatformContractReady &&
  iosRustCryptographyTestsReady;
const hostClientCryptographyAcceptance = {
  platform: process.platform,
  rustCryptographyAcceptanceReady,
  platformProofReady:
    process.platform === "darwin"
      ? macosPlatformCrypto.ready
      : process.platform === "linux"
        ? ubuntuPlatformCrypto.ready
        : false,
  ready: rustCryptographyAcceptanceReady &&
    (process.platform === "darwin"
      ? macosPlatformCrypto.ready
      : process.platform === "linux"
        ? ubuntuPlatformCrypto.ready
        : false),
  evidenceSources: [
    ...(process.platform === "darwin" ? [macosPlatformCrypto.report] : []),
    ...(process.platform === "linux" ? [ubuntuPlatformCrypto.report] : [])
  ]
};

const platformMatrix = [
  {
    platform: "android",
    backend: "AndroidKeyStore",
    status: androidPhysicalSecretStoreBindingReady
      ? "physical-verified-partial"
      : androidPlatformCrypto.ready
        ? "contract-verified-partial"
        : "missing",
    rustCryptographyAcceptanceReady,
    platformCryptoAcceptanceReady: androidPlatformCrypto.ready,
    physicalSecretStoreBindingReady: androidPhysicalSecretStoreBindingReady,
    localAuthPolicyReady: androidInstallLaunch.androidSystemCredentialAuthReady,
    evidenceReports: [
      ...(androidPlatformCrypto.ready ? [androidPlatformCrypto.report] : []),
      ...(androidInstallLaunch.present ? [androidInstallLaunch.report] : [])
    ],
    remainingGates: androidPhysicalSecretStoreBindingReady
      ? ["physical Android command/result, restart, replay, key rotation, revocation, and file proof"]
      : ["physical Android install/launch, KeyStore custody, and system authorization proof"]
  },
  {
    platform: "ios",
    backend: "Keychain",
    status: iosUserPresencePolicyReady ? "contract-verified-partial" : "missing",
    rustCryptographyAcceptanceReady: iosRustCryptographyTestsReady,
    platformCryptoContractReady: iosPlatformContractReady,
    physicalSecretStoreBindingReady: false,
    localAuthPolicyReady: iosUserPresencePolicyReady,
    evidenceReports: [],
    remainingGates: [
      "physical iPhone Keychain user-presence, command/result, restart, replay, key rotation, revocation, and file proof"
    ]
  },
  {
    platform: "macos",
    backend: "macos-keychain",
    status: macosPlatformCrypto.ready ? "host-verified-partial" : "missing",
    rustCryptographyAcceptanceReady,
    platformCryptoAcceptanceReady: macosPlatformCrypto.ready,
    releaseCliProofReady: macosReleaseCliProof.ready,
    evidenceReports: [
      ...(macosPlatformCrypto.present ? [macosPlatformCrypto.report] : []),
      ...(macosReleaseCliProof.present ? [macosReleaseCliProof.report] : [])
    ],
    remainingGates: ["signed local install, launch, update, and publication receipts"]
  },
  {
    platform: "ubuntu-linux",
    backend: "linux-secret-service-keyring",
    status: ubuntuPlatformCrypto.ready ? "vm-verified-partial" : "missing",
    rustCryptographyAcceptanceReady,
    platformCryptoAcceptanceReady: ubuntuPlatformCrypto.ready,
    releaseCliProofReady: ubuntuReleaseCliProof.ready,
    adaptiveCustodyReady: ubuntuLinuxAdaptiveCustodyProof.ready,
    packageUpdateReady: ubuntuLinuxPackageUpdateProof.ready,
    evidenceReports: [
      ...(ubuntuPlatformCrypto.present ? [ubuntuPlatformCrypto.report] : []),
      ...(ubuntuReleaseCliProof.present ? [ubuntuReleaseCliProof.report] : []),
      ...(ubuntuLinuxAdaptiveCustodyProof.present
        ? [ubuntuLinuxAdaptiveCustodyProof.report]
        : []),
      ...(ubuntuLinuxPackageUpdateProof.present
        ? [ubuntuLinuxPackageUpdateProof.report]
        : [])
    ],
    remainingGates: ["release package and publication receipts"]
  },
  {
    platform: "windows",
    backend: "windows-credential-manager",
    status: windowsImplementation.ready
      ? "persistent-custody-verified"
      : (windowsImplementation.conservativeBoundaryValid
        ? "persistent-custody-unverified"
        : "missing"),
    rustCryptographyAcceptanceReady,
    platformCryptoAcceptanceReady: false,
    localImplementationReady: windowsImplementation.ready,
    evidenceReports: windowsImplementation.ready ||
        windowsImplementation.conservativeBoundaryValid
      ? [windowsImplementation.report]
      : [],
    remainingGates: windowsImplementation.ready
      ? ["Windows-native Credential Manager lifecycle receipt"]
      : ["Windows-native user-authorized persistent custody implementation and lifecycle proof"]
  }
];

const verifierOk = sourceResults.every((result) => result.ok) &&
  rustCryptographyAcceptanceReady &&
  androidPlatformCrypto.ready;
const productionReady = false;
const checkedAt = new Date().toISOString();
const scopeEvidence = await createSecureClientMeshE2eeRefReportScope({
  contract,
  reportRef: reportPath,
  blocker,
  checkedAt
});
const remainingGates = dedupeRemainingGates([
  ...platformMatrix.flatMap((entry) => entry.remainingGates),
  ...(hostClientCryptographyAcceptance.ready
    ? []
    : ["current-host platform cryptography proof"]),
  "release proof bundle accepted by the client-pinned contract reducer"
]);

const report = {
  schemaVersion: "licomesh.secure-mesh.platform-secret-store-matrix-report.v2",
  evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  verifier: "tools/scripts/client-secure-mesh-platform-secret-store-matrix.mjs",
  generatedBy: "tools/scripts/client-secure-mesh-platform-secret-store-matrix.mjs",
  generatedAt: checkedAt,
  checkedAt,
  ...optionalReleaseInvocationBinding(),
  sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  blocker,
  diagnosticStatus: verifierOk ? "passed" : "incomplete",
  productionReady,
  releaseReady: false,
  evidenceKind: "redacted-client-rust-and-platform-cryptography-acceptance",
  redacted: true,
  rawPrivateMaterialIncluded: false,
  rawPlaintextIncluded: false,
  rawPublicWireBytesIncluded: false,
  reportLeakScan: true,
  ...scopeEvidence,
  ok: verifierOk,
  contractBinding: {
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    canonicalBlocker: blocker,
    canonicalBlockerCount: SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.length
  },
  physicalEvidenceConfig: {
    ref: physicalEvidenceConfig.configRef,
    schemaVersion: physicalEvidenceConfig.schemaVersion,
    linkedReportCount: Object.keys(physicalReportRefs).length,
    evidenceCommandPlatformCount:
      Object.keys(physicalEvidenceConfig.evidenceCommands).length
  },
  platformSecretStoreMatrixConfig: {
    ref: platformSecretStoreMatrixConfig.configRef,
    schemaVersion: platformSecretStoreMatrixConfig.schemaVersion,
    sourceCheckCount: sourceResults.length,
    nativeTestFilterCount: nativeResults.length
  },
  sourceResults,
  nativeResults,
  androidPlatformCrypto,
  androidInstallLaunch,
  iosPlatformContract: {
    sourceContractReady: iosPlatformContractReady,
    rustCryptographyTestsReady: iosRustCryptographyTestsReady,
    physicalProofReady: false
  },
  macosPlatformCrypto,
  ubuntuPlatformCrypto,
  macosReleaseCliProof,
  ubuntuReleaseCliProof,
  ubuntuLinuxAdaptiveCustodyProof,
  ubuntuLinuxPackageUpdateProof,
  windowsImplementation,
  hostClientCryptographyAcceptance,
  platformMatrix,
  summary: {
    verificationPassed: verifierOk,
    sourceCheckCount: sourceResults.length,
    nativeTestCount: nativeResults.length,
    rustCryptographyAcceptanceReady,
    hostClientCryptographyAcceptanceReady:
      hostClientCryptographyAcceptance.ready,
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
    androidInstallLaunchReady: androidInstallLaunch.ok,
    androidPhysicalSecretStoreBindingReady,
    androidPhysicalSystemCredentialAuthReady:
      androidInstallLaunch.androidSystemCredentialAuthReady,
    androidPhysicalKeyStoreHardwareAuthReady:
      androidInstallLaunch.androidKeyStoreHardwareAuthReady,
    androidPhysicalKeyStoreSecurityLevelName:
      androidInstallLaunch.androidKeyStoreSecurityLevelName,
    androidPhysicalKeyStoreInsideSecureHardware:
      androidInstallLaunch.androidKeyStoreInsideSecureHardware,
    androidPhysicalKeyStoreUserAuthenticationHardwareEnforced:
      androidInstallLaunch.androidKeyStoreUserAuthenticationHardwareEnforced,
    androidPhysicalKeyStoreUnlockedDeviceRequired:
      androidInstallLaunch.androidKeyStoreUnlockedDeviceRequired,
    androidPhysicalCallbackContractReady:
      androidPlatformCrypto.rustFfiActionContractReady,
    androidPhysicalRawJsonSecretOverridesProvenAbsent:
      androidInstallLaunch.rawPayloadExportSurfaceAbsent,
    androidPhysicalLifecycleReady: false,
    iosPlatformCryptoContractReady: iosPlatformContractReady,
    iosRustCryptographyTestsReady,
    iosPhysicalSecretStoreBindingReady,
    iosUserPresencePolicyReady,
    iosProductionCallbackAuthReady: false,
    iosSystemAuthorizationAttemptCount: 0,
    iosSystemAuthorizationCompleted: false,
    iosAuthorizationBatchPromptBudgetReady: false,
    iosAuthorizationBatchWithinBudget: false,
    iosCallbackAuthContextAttachedToAllOperations: false,
    iosSystemLocalAuthPromptReady: iosPlatformContractReady,
    iosKeychainAccessControlNotDowngraded: iosPlatformContractReady,
    iosNonInteractiveFailClosedReady: iosPlatformContractReady,
    iosCancelLockFailClosedReady: iosPlatformContractReady,
    iosAppPasswordPromptUsed: false,
    iosAppCredentialPromptUsed: false,
    iosKeyMaterialExported: false,
    iosPhysicalCallbackContractReady: iosPlatformContractReady,
    iosPhysicalRawJsonSecretOverridesProvenAbsent: iosPlatformContractReady,
    iosPhysicalLifecycleReady: false,
    macosReleaseCliProofReady: macosReleaseCliProof.ready,
    macosUserPresenceProofAttempted: macosPlatformCrypto.present,
    macosAdaptiveCustodyReady: macosPlatformCrypto.ready,
    macosCustodyStrategy: macosPlatformCrypto.custodyStrategy,
    macosEnabledCapabilities: macosPlatformCrypto.enabledCapabilities,
    macosExactCapabilitySetValid:
      macosPlatformCrypto.exactCapabilitySetValid,
    macosSafeOsStoreAvailable: macosPlatformCrypto.safeOsStoreAvailable,
    macosStandardKeychainAvailable:
      macosPlatformCrypto.standardKeychainAvailable,
    macosDataProtectionKeychainAvailable:
      macosPlatformCrypto.dataProtectionKeychainAvailable,
    macosUserPresenceOperationSupported:
      macosPlatformCrypto.userPresenceOperationSupported,
    macosSecureEnclaveOperationSupported:
      macosPlatformCrypto.secureEnclaveOperationSupported,
    macosAppPasswordPromptUsed:
      macosPlatformCrypto.appPasswordPromptUsed,
    macosAppCredentialPromptUsed:
      macosPlatformCrypto.appCredentialPromptUsed,
    macosSystemCredentialEntrySurface:
      "macos_local_authentication_system_prompt",
    macosSingleSystemAuthorizationContextVerified:
      macosPlatformCrypto.singleSystemAuthorizationContextVerified,
    macosPromptBudgetSatisfied:
      macosPlatformCrypto.promptBudgetSatisfied,
    macosZeroBackgroundPrompts:
      macosPlatformCrypto.zeroBackgroundPrompts,
    macosNoAutomaticAuthorizationRetry:
      macosPlatformCrypto.noAutomaticAuthorizationRetry,
    macosInteractiveAuthorizationAttemptCount:
      macosPlatformCrypto.interactiveAuthorizationAttemptCount,
    macosMaximumInteractiveAuthorizationAttemptsPerProof:
      macosPlatformCrypto.maximumInteractiveAuthorizationAttemptsPerProof,
    ubuntuVmSecretStoreReady: ubuntuPlatformCrypto.ready,
    ubuntuVmSecretStoreBackend: ubuntuPlatformCrypto.backend,
    ubuntuVmSharedSecretClassPersistenceReady:
      ubuntuPlatformCrypto.sharedSecretClassPersistenceReady,
    ubuntuVmSecretStoreAuthorizationPolicyReady:
      ubuntuPlatformCrypto.authorizationPolicyReady,
    ubuntuReleaseCliProofReady: ubuntuReleaseCliProof.ready,
    ubuntuLinuxAdaptiveCustodyReady:
      ubuntuLinuxAdaptiveCustodyProof.ready,
    ubuntuLinuxPackageUpdateReady: ubuntuLinuxPackageUpdateProof.ready,
    windowsLocalBlockersCleared:
      windowsImplementation.ready,
    windowsDpapiOrHelloProofReady:
      windowsImplementation.dpapiOrWindowsHelloProofReady,
    platformCount: platformMatrix.length,
    completedPlatformCount:
      platformMatrix.filter((entry) => entry.status === "complete").length,
    incompletePlatformCount:
      platformMatrix.filter((entry) => entry.status !== "complete").length,
    persistentCustodyUnverifiedCount:
      platformMatrix.filter((entry) =>
        entry.status === "persistent-custody-unverified").length,
    hostVerifiedPartialPlatformCount:
      platformMatrix.filter((entry) => entry.status === "host-verified-partial").length,
    vmVerifiedPartialPlatformCount:
      platformMatrix.filter((entry) => entry.status === "vm-verified-partial").length,
    productionReady,
    releaseReady: false,
    reportLeakScan: true,
    remainingGates
  }
};

assertNoLeak(report, "secure mesh platform secret-store matrix report");
atomicWriteReportJson(
  path.join(repoRoot, "build"),
  reportPath.replace(/^build\//u, ""),
  report
);

console.log(JSON.stringify({
  ok: verifierOk,
  report: reportPath,
  diagnosticStatus: report.diagnosticStatus,
  rustCryptographyAcceptanceReady,
  androidPlatformCryptoAcceptanceReady: androidPlatformCrypto.ready,
  iosPlatformCryptoContractReady: iosPlatformContractReady,
  macosPlatformCryptoAcceptanceReady: macosPlatformCrypto.ready,
  ubuntuPlatformCryptoAcceptanceReady: ubuntuPlatformCrypto.ready,
  remainingGateCount: remainingGates.length
}, null, 2));

if (!verifierOk || (strict && productionReady !== true)) {
  process.exitCode = 1;
}

}
