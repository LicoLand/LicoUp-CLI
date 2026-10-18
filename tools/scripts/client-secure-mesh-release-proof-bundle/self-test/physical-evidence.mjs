import {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} from "../config.mjs";
import { summarizePhysicalEvidenceManifest } from "../summarize/physical-evidence.mjs";

export function runPhysicalEvidenceManifestReadinessSelfTest() {
  const baseReport = {
    schemaVersion: "licomesh.secure-mesh.physical-evidence-manifest-report.v2",
    evidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    verifier: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs",
    generatedBy: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs",
    blocker: "physical device matrix",
    ok: true,
    diagnosticOk: true,
    okMeaning: "manifest_integrity_not_production_evidence",
    redacted: true,
    reportLeakScan: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    redactionReady: true,
    manifestIntegrityReady: true,
    physicalEvidenceChainReady: true,
    evidenceChainComplete: false,
    releaseEvidenceReady: false,
    ready: false,
    productionReady: false,
    releaseReady: false,
    platformCoverage: [
	      {
	        platform: "macos",
	        userPresencePolicyReady: true,
	        productionEntitlementFailClosedReady: true,
	        productionEntitlementGateAccepted: true,
	        productionEntitlementMissingFailClosed: true,
	        standardKeychainRejectedForProduction: true,
	        standardKeychainUserPresenceAcceptedForProduction: false,
	        standardKeychainFallbackFailClosedReady: true,
	        singleSystemAuthorizationContextVerified: true,
        interactiveAuthorizationPromptBudgetReady: true,
        interactiveAuthorizationCompletedWithinBudget: true,
        dataProtectionSecretReadBlockedOrUnavailable: false,
        interactiveAuthorizationAttemptCount: 1,
        maximumInteractiveAuthorizationAttemptsPerProof: 1,
        appPasswordPromptUsed: false,
        appCredentialPromptUsed: false,
        systemCredentialEntrySurface: "macos_local_authentication_system_prompt",
        keyMaterialExported: false
      },
      {
        platform: "ios",
        platformUserPresencePolicyReady: true,
        platformProductionCallbackAuthReady: true,
        platformCallbackReadsUseSharedLAContext: true,
        platformSingleSystemAuthorizationContextVerified: true,
        platformCallbackAuthContextAttachedToAllReads: true,
        platformSystemLocalAuthPromptReady: true,
        platformKeychainAccessControlNotDowngraded: true,
        platformNonInteractiveFailClosedReady: true,
        platformCancelLockFailClosedReady: true,
        appPasswordPromptUsedPresent: true,
        appPasswordPromptUsed: false,
        appCredentialPromptUsedPresent: true,
        appCredentialPromptUsed: false,
        keyMaterialExportedPresent: true,
        keyMaterialExported: false
      }
    ],
    summary: {
      diagnosticOk: true,
      allConfiguredReportsPresent: true,
      redactionReady: true,
      manifestIntegrityReady: true,
      physicalEvidenceChainReady: true,
      evidenceChainComplete: false,
	      releaseEvidenceReady: false,
	      androidPhysicalSystemCredentialAuthReady: true,
      androidPhysicalKeyStoreHardwareAuthReady: true,
      androidPhysicalKeyStoreSecurityLevelName: "trusted_execution_environment",
      androidPhysicalKeyStoreInsideSecureHardware: true,
      androidPhysicalKeyStoreUserAuthenticationHardwareEnforced: true,
      androidPhysicalKeyStoreUnlockedDeviceRequired: true,
	      androidUserAuthenticationSystemAuthenticationOnly: true,
      androidUserAuthenticationCredentialEntrySurface: "android_system_credential_prompt",
      androidUserAuthenticationAppLockScreenCredentialCollection: false,
      androidUserAuthenticationAppCredentialPromptUsed: false,
      androidUserAuthenticationAppPasswordPromptUsed: false,
      androidUserAuthenticationKeyMaterialExported: false,
      androidPhysicalRawJsonSecretOverridesProvenAbsent: true,
      androidPhysicalMissingFieldAuditPresent: true,
      androidPhysicalMissingFieldsAbsent: true,
	      androidPhysicalWeakProofFieldAuditPresent: true,
	      androidPhysicalWeakProofFieldsAbsent: true,
	      macosProductionEntitlementFailClosedReady: true,
	      macosProductionEntitlementGateAccepted: true,
	      macosProductionEntitlementMissingFailClosed: true,
	      macosStandardKeychainRejectedForProduction: true,
	      macosStandardKeychainUserPresenceAcceptedForProduction: false,
	      macosStandardKeychainFallbackFailClosedReady: true,
	      macosUserPresencePolicyReady: true,
      macosSingleSystemAuthorizationContextVerified: true,
      macosInteractiveAuthorizationPromptBudgetReady: true,
      macosInteractiveAuthorizationCompletedWithinBudget: true,
      macosDataProtectionSecretReadBlockedOrUnavailable: false,
      macosInteractiveAuthorizationAttemptCount: 1,
      macosMaximumInteractiveAuthorizationAttemptsPerProof: 1,
      macosAppPasswordPromptUsed: false,
      macosAppCredentialPromptUsed: false,
      macosKeyMaterialExported: false,
      macosSystemCredentialEntrySurface: "macos_local_authentication_system_prompt",
      iosUserPresencePolicyReady: true,
      iosProductionCallbackAuthReady: true,
      iosCallbackReadsUseSharedLAContext: true,
      iosSingleSystemAuthorizationContextVerified: true,
      iosCallbackAuthContextAttachedToAllReads: true,
      appPasswordPromptUsedPresent: true,
      appCredentialPromptUsedPresent: true,
      keyMaterialExportedPresent: true,
      iosSystemLocalAuthPromptReady: true,
      iosKeychainAccessControlNotDowngraded: true,
      iosNonInteractiveFailClosedReady: true,
      iosCancelLockFailClosedReady: true,
      iosAppPasswordPromptUsed: false,
      iosAppCredentialPromptUsed: false,
      iosKeyMaterialExported: false,
      remainingGates: ["physical device release evidence chain ready"]
    }
  };
  const diagnosticOnly = summarizePhysicalEvidenceManifest(baseReport);
  const releaseReady = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      remainingGates: []
    }
  });
  const androidAppPasswordPrompt = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      androidUserAuthenticationAppPasswordPromptUsed: true,
      remainingGates: []
    }
  });
  const macosRepeatedAuthorization = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    platformCoverage: baseReport.platformCoverage.map((item) =>
      item.platform === "macos"
        ? { ...item, interactiveAuthorizationAttemptCount: 2 }
        : item
    ),
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      macosInteractiveAuthorizationAttemptCount: 2,
      remainingGates: []
    }
  });
  const macosAppPasswordPrompt = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    platformCoverage: baseReport.platformCoverage.map((item) =>
      item.platform === "macos"
        ? { ...item, appPasswordPromptUsed: true }
        : item
    ),
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      macosAppPasswordPromptUsed: true,
      remainingGates: []
    }
  });
  const macosAuthorizationNotCompleted = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    platformCoverage: baseReport.platformCoverage.map((item) =>
      item.platform === "macos"
        ? { ...item, interactiveAuthorizationCompletedWithinBudget: false }
        : item
    ),
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      macosInteractiveAuthorizationCompletedWithinBudget: false,
      remainingGates: []
    }
  });
  const macosKeyMaterialExported = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    platformCoverage: baseReport.platformCoverage.map((item) =>
      item.platform === "macos"
        ? { ...item, keyMaterialExported: true }
        : item
    ),
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      macosKeyMaterialExported: true,
      remainingGates: []
    }
  });
  const iosAppCredentialPrompt = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: true,
    ready: true,
    platformCoverage: baseReport.platformCoverage.map((item) =>
      item.platform === "ios"
        ? { ...item, appCredentialPromptUsed: true }
        : item
    ),
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: true,
      iosAppCredentialPromptUsed: true,
      remainingGates: []
    }
  });
  const readyOnlyWithoutReleaseEvidence = summarizePhysicalEvidenceManifest({
    ...baseReport,
    okMeaning: "release_evidence_chain_ready",
    evidenceChainComplete: true,
    releaseEvidenceReady: false,
    ready: true,
    summary: {
      ...baseReport.summary,
      evidenceChainComplete: true,
      releaseEvidenceReady: false,
      remainingGates: []
    }
  });
  const legacySchema = summarizePhysicalEvidenceManifest({
    ...baseReport,
    schemaVersion: "licomesh.secure-mesh.physical-evidence-manifest-report.v1"
  });
  const ok = diagnosticOnly.inputIntegrityReady === true &&
    diagnosticOnly.diagnosticIntegrityReady === true &&
    diagnosticOnly.ready === false &&
    diagnosticOnly.releaseEvidenceReady === false &&
    diagnosticOnly.evidenceChainComplete === false &&
    releaseReady.inputIntegrityReady === true &&
    releaseReady.diagnosticIntegrityReady === true &&
    releaseReady.releaseEvidenceReady === true &&
    releaseReady.evidenceChainComplete === true &&
    releaseReady.platformSystemAuthorizationReleaseReady === true &&
    releaseReady.ready === true &&
    androidAppPasswordPrompt.ready === false &&
    androidAppPasswordPrompt.androidSystemCredentialReleaseReady === false &&
    macosRepeatedAuthorization.ready === false &&
    macosRepeatedAuthorization.macosSingleSystemAuthorizationReleaseReady === false &&
    macosAppPasswordPrompt.ready === false &&
    macosAppPasswordPrompt.macosSingleSystemAuthorizationReleaseReady === false &&
    macosAuthorizationNotCompleted.ready === false &&
    macosAuthorizationNotCompleted.macosSingleSystemAuthorizationReleaseReady === false &&
    macosKeyMaterialExported.ready === false &&
    macosKeyMaterialExported.macosSingleSystemAuthorizationReleaseReady === false &&
    iosAppCredentialPrompt.ready === false &&
    iosAppCredentialPrompt.iosSystemLocalAuthReleaseReady === false &&
    readyOnlyWithoutReleaseEvidence.ready === false &&
    readyOnlyWithoutReleaseEvidence.releaseEvidenceReady === false &&
    legacySchema.inputIntegrityReady === false &&
    legacySchema.ready === false;
  return {
    ok,
    diagnosticOnlyRejected: diagnosticOnly.ready === false,
    diagnosticIntegrityAccepted: diagnosticOnly.diagnosticIntegrityReady === true,
    releaseEvidenceRequired: diagnosticOnly.releaseEvidenceReady === false,
    evidenceChainRequired: diagnosticOnly.evidenceChainComplete === false,
    platformSystemAuthorizationRequired:
      releaseReady.platformSystemAuthorizationReleaseReady === true,
    androidAppPasswordPromptRejected: androidAppPasswordPrompt.ready === false,
    macosRepeatedAuthorizationRejected: macosRepeatedAuthorization.ready === false,
    macosAppPasswordPromptRejected: macosAppPasswordPrompt.ready === false,
    iosAppCredentialPromptRejected: iosAppCredentialPrompt.ready === false,
    readyOnlyWithoutReleaseEvidenceRejected:
      readyOnlyWithoutReleaseEvidence.ready === false,
    legacySchemaRejected:
      legacySchema.inputIntegrityReady === false && legacySchema.ready === false,
    positiveReleaseFixtureAccepted: releaseReady.ready === true
	  };
	}
