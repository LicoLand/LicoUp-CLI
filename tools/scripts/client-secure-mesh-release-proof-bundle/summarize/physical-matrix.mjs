import { releaseInputIntegrity } from "../integrity.mjs";
import { dedupeRemainingGates, reportRecord, stableStringList } from "../lists.mjs";

export function summarizePhysicalMatrixReport(report = {}) {
  report = reportRecord(report);
  const inputIntegrity = releaseInputIntegrity(report, {
    schemaVersion: "licomesh.secure-mesh.physical-device-matrix-report.v2",
    verifier: "tools/scripts/client-secure-mesh-physical-device-matrix.mjs"
  });
  const summary = inputIntegrity.ok ? (report.summary || {}) : {};
  return {
    inputIntegrityReady: inputIntegrity.ok,
    inputSchemaStatus: inputIntegrity.status,
    inputSchemaFailureCount: inputIntegrity.failureCount,
    inputSchemaFailures: inputIntegrity.failures,
    ok: inputIntegrity.ok && report.ok === true,
    diagnosticOk: inputIntegrity.ok &&
      (report.diagnosticOk === true || summary.diagnosticOk === true || report.ok === true),
    productionReady: inputIntegrity.ok && report.productionReady === true,
    releaseReady: inputIntegrity.ok && report.releaseReady === true,
    allPhysicalScenariosReady:
      inputIntegrity.ok &&
      (report.allPhysicalScenariosReady === true ||
        summary.allPhysicalScenariosReady === true),
	    physicalEvidenceChainReady:
	      inputIntegrity.ok &&
	      (report.physicalEvidenceChainReady === true ||
	        summary.physicalEvidenceChainReady === true),
	    localPhysicalEvidenceChainReadyDiagnostic:
	      inputIntegrity.ok &&
	      (report.physicalEvidenceChainReady === true ||
	        summary.physicalEvidenceChainReady === true),
	    evidenceChainComplete:
	      inputIntegrity.ok &&
	      (report.evidenceChainComplete === true ||
	        summary.evidenceChainComplete === true),
	    localEvidenceChainCompleteDiagnostic:
	      inputIntegrity.ok &&
	      (report.evidenceChainComplete === true ||
	        summary.evidenceChainComplete === true),
	    releaseEvidenceReady:
	      inputIntegrity.ok &&
	      (report.releaseEvidenceReady === true ||
	        summary.releaseEvidenceReady === true),
	    localReleaseEvidenceReadyDiagnostic:
	      inputIntegrity.ok &&
	      (report.releaseEvidenceReady === true ||
	        summary.releaseEvidenceReady === true),
    diagnosticStatus: String(report.diagnosticStatus || ""),
    physicalScenarioCount: Number(summary.physicalScenarioCount || 0),
    partialScenarioCount: Number(summary.partialScenarioCount || 0),
    missingScenarioCount: Number(summary.missingScenarioCount || 0),
    evidenceReportCount: Number(summary.evidenceReportCount || 0),
    androidPlatformSecretStoreReady: summary.androidPlatformSecretStoreReady === true,
    androidPhysicalSecretStoreBindingReady:
      summary.androidPhysicalSecretStoreBindingReady === true,
	    androidPhysicalSystemCredentialAuthReady:
	      summary.androidPhysicalSystemCredentialAuthReady === true,
    androidPhysicalKeyStoreHardwareAuthReady:
      summary.androidPhysicalKeyStoreHardwareAuthReady === true,
    androidPhysicalKeyStoreSecurityLevelName:
      String(summary.androidPhysicalKeyStoreSecurityLevelName || ""),
    androidPhysicalKeyStoreInsideSecureHardware:
      summary.androidPhysicalKeyStoreInsideSecureHardware === true,
    androidPhysicalKeyStoreUserAuthenticationHardwareEnforced:
      summary.androidPhysicalKeyStoreUserAuthenticationHardwareEnforced === true,
    androidPhysicalKeyStoreUnlockedDeviceRequired:
      summary.androidPhysicalKeyStoreUnlockedDeviceRequired === true,
	    androidPhysicalCallbackContractReady:
	      summary.androidPhysicalCallbackContractReady === true,
    androidPhysicalRawJsonSecretOverridesProvenAbsent:
      summary.androidPhysicalRawJsonSecretOverridesProvenAbsent === true,
    androidPhysicalRawJsonSecretOverridesUsed:
      summary.androidPhysicalRawJsonSecretOverridesUsed === true,
    androidPhysicalRawJsonSecretOverridesUnknown:
      summary.androidPhysicalRawJsonSecretOverridesUnknown === true,
    androidPhysicalInstallLaunchSchemaDrift:
      summary.androidPhysicalInstallLaunchSchemaDrift === true,
    androidPhysicalInstallLaunchSchemaDriftFieldCount:
      Number(summary.androidPhysicalInstallLaunchSchemaDriftFieldCount || 0),
    androidPhysicalInstallLaunchSchemaStatus:
      String(summary.androidPhysicalInstallLaunchSchemaStatus || "unknown"),
    androidPhysicalAppPasswordPromptUsed:
      summary.androidPhysicalAppPasswordPromptUsed === true,
	    androidPhysicalMissingFieldsAbsent:
	      summary.androidPhysicalMissingFieldsAbsent === true,
	    androidPhysicalMissingFieldAuditPresent:
	      summary.androidPhysicalMissingFieldAuditPresent === true,
	    androidPhysicalMissingFields:
	      stableStringList(summary.androidPhysicalMissingFields),
	    androidPhysicalMissingFieldCount:
	      Number(summary.androidPhysicalMissingFieldCount || 0),
	    androidPhysicalWeakProofFieldsAbsent:
	      summary.androidPhysicalWeakProofFieldsAbsent === true,
	    androidPhysicalWeakProofFieldAuditPresent:
	      summary.androidPhysicalWeakProofFieldAuditPresent === true,
	    androidPhysicalWeakProofFields:
	      stableStringList(summary.androidPhysicalWeakProofFields),
	    androidPhysicalWeakProofFieldCount:
	      Number(summary.androidPhysicalWeakProofFieldCount || 0),
    iosPlatformSecretStoreReady: summary.iosPlatformSecretStoreReady === true,
    iosPhysicalSecretStoreBindingReady:
      summary.iosPhysicalSecretStoreBindingReady === true,
	    iosUserPresencePolicyReady:
	      summary.iosUserPresencePolicyReady === true,
	    iosProductionCallbackAuthReady:
	      summary.iosProductionCallbackAuthReady === true,
	    iosCallbackReadsUseSharedLAContext:
	      summary.iosCallbackReadsUseSharedLAContext === true,
	    iosSingleSystemAuthorizationContextVerified:
	      summary.iosSingleSystemAuthorizationContextVerified === true,
	    iosCallbackAuthContextAttachedToAllReads:
	      summary.iosCallbackAuthContextAttachedToAllReads === true,
	    appPasswordPromptUsedPresent:
	      summary.appPasswordPromptUsedPresent === true,
	    appCredentialPromptUsedPresent:
	      summary.appCredentialPromptUsedPresent === true,
	    keyMaterialExportedPresent:
	      summary.keyMaterialExportedPresent === true,
	    iosSystemLocalAuthPromptReady:
	      summary.iosSystemLocalAuthPromptReady === true,
    iosKeychainAccessControlNotDowngraded:
      summary.iosKeychainAccessControlNotDowngraded === true,
    iosNonInteractiveFailClosedReady:
      summary.iosNonInteractiveFailClosedReady === true,
    iosCancelLockFailClosedReady:
      summary.iosCancelLockFailClosedReady === true,
    iosAppPasswordPromptUsed:
      summary.iosAppPasswordPromptUsed === true,
    iosAppCredentialPromptUsed:
      summary.iosAppCredentialPromptUsed === true,
    iosKeyMaterialExported:
      summary.iosKeyMaterialExported === true,
	    iosPhysicalCallbackContractReady:
	      summary.iosPhysicalCallbackContractReady === true,
	    iosPhysicalRawJsonSecretOverridesProvenAbsent:
	      summary.iosPhysicalRawJsonSecretOverridesProvenAbsent === true,
	    macosProductionEntitlementFailClosedReady:
	      summary.macosProductionEntitlementFailClosedReady === true,
	    macosProductionEntitlementGateAccepted:
	      summary.macosProductionEntitlementGateAccepted === true,
	    macosProductionEntitlementMissingFailClosed:
	      summary.macosProductionEntitlementMissingFailClosed === true,
	    macosStandardKeychainRejectedForProduction:
	      summary.macosStandardKeychainRejectedForProduction === true,
	    macosStandardKeychainUserPresenceAcceptedForProduction:
	      summary.macosStandardKeychainUserPresenceAcceptedForProduction === true,
	    macosStandardKeychainFallbackFailClosedReady:
	      summary.macosStandardKeychainFallbackFailClosedReady === true,
	    macosUserPresencePolicyReady: summary.macosUserPresencePolicyReady === true,
    macosSingleSystemAuthorizationContextVerified:
      summary.macosSingleSystemAuthorizationContextVerified === true,
    macosInteractiveAuthorizationPromptBudgetReady:
      summary.macosInteractiveAuthorizationPromptBudgetReady === true,
    macosInteractiveAuthorizationAttemptCount:
      Number(summary.macosInteractiveAuthorizationAttemptCount || 0),
    macosMaximumInteractiveAuthorizationAttemptsPerProof:
      Number(summary.macosMaximumInteractiveAuthorizationAttemptsPerProof || 1),
    macosAppCredentialPromptUsed: summary.macosAppCredentialPromptUsed === true,
    macosAppPasswordPromptUsed: summary.macosAppPasswordPromptUsed === true,
    macosSystemCredentialEntrySurface:
      String(summary.macosSystemCredentialEntrySurface || ""),
    remainingGates: Array.isArray(summary.remainingGates)
      ? dedupeRemainingGates(summary.remainingGates)
      : [],
    remainingGateCount: Array.isArray(summary.remainingGates)
      ? dedupeRemainingGates(summary.remainingGates).length
      : 0
  };
}
