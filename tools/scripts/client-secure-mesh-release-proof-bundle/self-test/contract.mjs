import { evaluateSecureClientMeshEvidenceRefReportReadiness } from "../config.mjs";
import {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} from "../config.mjs";
import { summarizeContractReadiness } from "../contract-readiness.mjs";
import { summarizeAndroidPhysicalInstallLaunchReport } from "../summarize/android-install.mjs";
import { summarizePhysicalEvidenceManifest } from "../summarize/physical-evidence.mjs";
import { summarizePhysicalMatrixReport } from "../summarize/physical-matrix.mjs";

export function runReleaseProofContractReadinessSelfTest() {
  const forgedPhysicalMatrix = {
    schemaVersion: "licomesh.secure-mesh.physical-device-matrix-report.v2",
    evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    verifier: "tools/scripts/client-secure-mesh-physical-device-matrix.mjs",
    generatedBy: "tools/scripts/client-secure-mesh-physical-device-matrix.mjs",
    checkedAt: new Date().toISOString(),
    blocker: "physical device matrix",
    ok: true,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    productionReady: false,
    releaseReady: false,
    physicalEvidenceChainReady: true,
    releaseEvidenceReady: true,
    summary: {
      physicalEvidenceChainReady: true,
      releaseEvidenceReady: true,
      remainingGates: []
    }
  };
  const forgedPhysicalEvidenceManifest = {
    schemaVersion: "licomesh.secure-mesh.physical-evidence-manifest-report.v2",
    evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    verifier: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs",
    generatedBy: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs",
    checkedAt: new Date().toISOString(),
    blocker: "physical device matrix",
    ok: true,
    ready: true,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    productionReady: false,
    releaseReady: false,
    physicalEvidenceChainReady: true,
    releaseEvidenceReady: true,
    summary: {
      ready: true,
      physicalEvidenceChainReady: true,
      releaseEvidenceReady: true,
      remainingGates: []
    }
  };
  const forgedPhysicalMatrixReadiness =
    evaluateSecureClientMeshEvidenceRefReportReadiness(
      forgedPhysicalMatrix,
      "physical device matrix"
    );
  const forgedPhysicalEvidenceManifestReadiness =
    evaluateSecureClientMeshEvidenceRefReportReadiness(
      forgedPhysicalEvidenceManifest,
      "physical device matrix"
    );
  const legacyPhysicalMatrix = summarizePhysicalMatrixReport({
    ...forgedPhysicalMatrix,
    schemaVersion: "licomesh.secure-mesh.physical-device-matrix-report.v1"
  });
  const legacyPhysicalEvidenceManifest = summarizePhysicalEvidenceManifest({
    ...forgedPhysicalEvidenceManifest,
    schemaVersion: "licomesh.secure-mesh.physical-evidence-manifest-report.v1"
  });
  const androidPhysicalInstallLaunchFixture = {
    schemaVersion: "licomesh.secure-mesh.android-physical-install-launch-report.v3",
    ok: true,
    physicalDevice: true,
    summary: {
      apkReady: true,
      installReady: true,
      launchReady: true,
      runtimeStatusReady: true,
      nativeRuntimeReady: true,
      androidCustodyReady: true,
      adaptiveAuthorizationReady: true,
      evidenceBindingReady: true,
    },
    runtimeStatus: {
      freshOneShotAuthorizationPolicyReady: true,
      mobileRelaySecretStore: {
        provider: "AndroidKeyStore",
        ffiBoundary: "jni",
        secretTransport: "jni_callback_in_process_secret_bytes",
        secretStoreBackend: "android-keystore",
        secretStoreContract: "rust_secure_mesh_secret_store_handle_v1",
        secretStoreAccountPrefix: "mobileRelayE2ee",
        secretStoreNamespace: "mobileRelayRuntime",
        sharedRustSecretStoreHandleContract: true,
        portableConfigAuthority: "rust_generation_cas",
        kotlinConfigReadWrite: false,
        statusProbeSideEffectFree: true,
        decryptedSecretCrossesJniInProcess: true,
        getNotFoundSeparatedFromFailure: true,
        rawJsonSecretOverridesProvenAbsent: true,
        rawJsonSecretOverridesUsed: false,
        androidKeyMaterialExported: false,
        applicationAuthorizationGrantRequired: true,
      },
      userAuthentication: {
        physicalUserPresenceRequired: true,
        systemAuthenticationOnly: true,
        appLockScreenCredentialCollection: false,
        appCredentialPromptUsed: false,
        appPasswordPromptUsed: false,
        systemCredentialPromptReused: false,
        systemCredentialPromptReusedFromPendingRequest: false,
        authorizationGrantPersisted: false,
        authorizationGrantExtendedByDispatch: false,
      },
    },
  };
  const androidPhysicalInstallLaunch =
    summarizeAndroidPhysicalInstallLaunchReport(androidPhysicalInstallLaunchFixture);
  const androidWithoutOneShotAuthorization =
    summarizeAndroidPhysicalInstallLaunchReport({
      ...androidPhysicalInstallLaunchFixture,
      runtimeStatus: {
        ...androidPhysicalInstallLaunchFixture.runtimeStatus,
        freshOneShotAuthorizationPolicyReady: false,
        userAuthentication: {
          ...androidPhysicalInstallLaunchFixture.runtimeStatus.userAuthentication,
          authorizationGrantPersisted: true,
        },
      },
    });
  const ok =
    forgedPhysicalMatrixReadiness.ready === false &&
    forgedPhysicalEvidenceManifestReadiness.ready === false &&
    legacyPhysicalMatrix.inputIntegrityReady === false &&
    legacyPhysicalEvidenceManifest.inputIntegrityReady === false &&
    androidPhysicalInstallLaunch.localReadyDiagnostic === true &&
    androidWithoutOneShotAuthorization.localReadyDiagnostic === false &&
    !Object.hasOwn(androidPhysicalInstallLaunch, "ready");
  return {
    ok,
    forgedPhysicalMatrixSummaryReadyRejected:
      forgedPhysicalMatrixReadiness.ready === false,
    forgedPhysicalEvidenceManifestSummaryReadyRejected:
      forgedPhysicalEvidenceManifestReadiness.ready === false,
    legacyPhysicalMatrixSchemaRejected:
      legacyPhysicalMatrix.inputIntegrityReady === false,
    legacyPhysicalEvidenceManifestSchemaRejected:
      legacyPhysicalEvidenceManifest.inputIntegrityReady === false,
    androidPhysicalInstallLaunchLocalReadyDiagnosticOnly:
      androidPhysicalInstallLaunch.localReadyDiagnostic === true &&
      !Object.hasOwn(androidPhysicalInstallLaunch, "ready"),
    androidOneShotAuthorizationRequired:
      androidWithoutOneShotAuthorization.localReadyDiagnostic === false,
    forgedPhysicalMatrixContractRemainingGateCount:
      Number(forgedPhysicalMatrixReadiness.remainingGateCount || 0),
    forgedPhysicalEvidenceManifestContractRemainingGateCount:
      Number(forgedPhysicalEvidenceManifestReadiness.remainingGateCount || 0)
  };
}
