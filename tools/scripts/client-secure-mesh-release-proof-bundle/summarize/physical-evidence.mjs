import { physicalEvidenceManifestReportPath } from "../config.mjs";
import { releaseInputIntegrity } from "../integrity.mjs";
import { reportRecord, stableStringList } from "../lists.mjs";

export function summarizePhysicalEvidenceManifest(report = {}) {
  report = reportRecord(report);
  const inputIntegrity = releaseInputIntegrity(report, {
    schemaVersion: "licomesh.secure-mesh.physical-evidence-manifest-report.v2",
    verifier: "tools/scripts/client-secure-mesh-physical-evidence-manifest.mjs"
  });
  const summary = inputIntegrity.ok ? (report?.summary || {}) : {};
  const present = Boolean(report && Object.keys(report).length > 0);
	  const artifactDigests = inputIntegrity.ok && Array.isArray(report?.artifactDigests) ? report.artifactDigests : [];
	  const platformCoverage = inputIntegrity.ok && Array.isArray(report?.platformCoverage) ? report.platformCoverage : [];
	  const androidCoverage = platformCoverage.find((item) => item?.platform === "android") || {};
	  const macosCoverage = platformCoverage.find((item) => item?.platform === "macos") || {};
  const ubuntuCoverage = platformCoverage.find((item) => item?.platform === "ubuntu-linux") || {};
  const iosCoverage = platformCoverage.find((item) => item?.platform === "ios") || {};
  const redactionReady = inputIntegrity.ok &&
    (report?.redactionReady === true || summary.redactionReady === true);
  const manifestIntegrityReady =
    inputIntegrity.ok &&
    (report?.manifestIntegrityReady === true || summary.manifestIntegrityReady === true);
  const physicalEvidenceChainReady =
    inputIntegrity.ok &&
    (report?.physicalEvidenceChainReady === true || summary.physicalEvidenceChainReady === true);
  const evidenceChainComplete =
    inputIntegrity.ok &&
    (report?.evidenceChainComplete === true || summary.evidenceChainComplete === true);
  const releaseEvidenceReady =
    inputIntegrity.ok &&
    (report?.releaseEvidenceReady === true ||
      summary.releaseEvidenceReady === true);
  const diagnosticIntegrityReady = present &&
    inputIntegrity.ok &&
    (report?.ok === true || report?.diagnosticOk === true || summary.diagnosticOk === true) &&
    redactionReady &&
    manifestIntegrityReady &&
    summary.allConfiguredReportsPresent === true;
	  const androidSystemCredentialNoAppCredentialCollection =
	    Object.prototype.hasOwnProperty.call(summary, "androidUserAuthenticationAppLockScreenCredentialCollection") &&
	    summary.androidUserAuthenticationAppLockScreenCredentialCollection === false &&
	    Object.prototype.hasOwnProperty.call(summary, "androidUserAuthenticationAppCredentialPromptUsed") &&
	    summary.androidUserAuthenticationAppCredentialPromptUsed === false &&
	    Object.prototype.hasOwnProperty.call(summary, "androidUserAuthenticationAppPasswordPromptUsed") &&
	    summary.androidUserAuthenticationAppPasswordPromptUsed === false &&
	    Object.prototype.hasOwnProperty.call(summary, "androidUserAuthenticationKeyMaterialExported") &&
	    summary.androidUserAuthenticationKeyMaterialExported === false;
	  const androidSystemCredentialReleaseReady = inputIntegrity.ok &&
	    summary.androidPhysicalSystemCredentialAuthReady === true &&
    summary.androidPhysicalKeyStoreHardwareAuthReady === true &&
	    summary.androidUserAuthenticationSystemAuthenticationOnly === true &&
	    String(summary.androidUserAuthenticationCredentialEntrySurface || "") === "android_system_credential_prompt" &&
	    androidSystemCredentialNoAppCredentialCollection &&
	    summary.androidPhysicalRawJsonSecretOverridesProvenAbsent === true &&
    summary.androidPhysicalMissingFieldAuditPresent === true &&
    summary.androidPhysicalMissingFieldsAbsent === true &&
    summary.androidPhysicalWeakProofFieldAuditPresent === true &&
    summary.androidPhysicalWeakProofFieldsAbsent === true;
	  const macosSingleSystemAuthorizationReleaseReady = inputIntegrity.ok &&
	    (macosCoverage.userPresencePolicyReady === true || summary.macosUserPresencePolicyReady === true) &&
	    (macosCoverage.productionEntitlementGateAccepted === true ||
	      summary.macosProductionEntitlementGateAccepted === true) &&
	    (macosCoverage.standardKeychainFallbackFailClosedReady === true ||
	      summary.macosStandardKeychainFallbackFailClosedReady === true) &&
	    macosCoverage.standardKeychainUserPresenceAcceptedForProduction !== true &&
	    summary.macosStandardKeychainUserPresenceAcceptedForProduction !== true &&
	    (macosCoverage.singleSystemAuthorizationContextVerified === true ||
	      summary.macosSingleSystemAuthorizationContextVerified === true) &&
    (macosCoverage.interactiveAuthorizationPromptBudgetReady === true ||
      summary.macosInteractiveAuthorizationPromptBudgetReady === true) &&
    (macosCoverage.interactiveAuthorizationCompletedWithinBudget === true ||
      summary.macosInteractiveAuthorizationCompletedWithinBudget === true) &&
    macosCoverage.dataProtectionSecretReadBlockedOrUnavailable !== true &&
    summary.macosDataProtectionSecretReadBlockedOrUnavailable !== true &&
    Number(macosCoverage.interactiveAuthorizationAttemptCount ||
      summary.macosInteractiveAuthorizationAttemptCount ||
      0) === 1 &&
    Number(macosCoverage.maximumInteractiveAuthorizationAttemptsPerProof ||
      summary.macosMaximumInteractiveAuthorizationAttemptsPerProof ||
      1) <= 1 &&
    macosCoverage.appPasswordPromptUsed !== true &&
    summary.macosAppPasswordPromptUsed !== true &&
    macosCoverage.appCredentialPromptUsed !== true &&
    summary.macosAppCredentialPromptUsed !== true &&
    macosCoverage.keyMaterialExported !== true &&
    summary.macosKeyMaterialExported !== true &&
    String(macosCoverage.systemCredentialEntrySurface ||
      summary.macosSystemCredentialEntrySurface ||
      "") === "macos_local_authentication_system_prompt";
  const iosSystemLocalAuthReleaseReady = inputIntegrity.ok &&
    (summary.iosUserPresencePolicyReady === true ||
      iosCoverage.platformUserPresencePolicyReady === true) &&
    (summary.iosProductionCallbackAuthReady === true ||
      iosCoverage.platformProductionCallbackAuthReady === true) &&
    (summary.iosCallbackReadsUseSharedLAContext === true ||
      iosCoverage.platformCallbackReadsUseSharedLAContext === true) &&
    (summary.iosSingleSystemAuthorizationContextVerified === true ||
      iosCoverage.platformSingleSystemAuthorizationContextVerified === true) &&
    (summary.iosCallbackAuthContextAttachedToAllReads === true ||
      iosCoverage.platformCallbackAuthContextAttachedToAllReads === true) &&
    (summary.appPasswordPromptUsedPresent === true ||
      iosCoverage.appPasswordPromptUsedPresent === true) &&
    (summary.appCredentialPromptUsedPresent === true ||
      iosCoverage.appCredentialPromptUsedPresent === true) &&
    (summary.keyMaterialExportedPresent === true ||
      iosCoverage.keyMaterialExportedPresent === true) &&
    (summary.iosSystemLocalAuthPromptReady === true ||
      iosCoverage.platformSystemLocalAuthPromptReady === true) &&
    (summary.iosKeychainAccessControlNotDowngraded === true ||
      iosCoverage.platformKeychainAccessControlNotDowngraded === true) &&
    (summary.iosNonInteractiveFailClosedReady === true ||
      iosCoverage.platformNonInteractiveFailClosedReady === true) &&
    (summary.iosCancelLockFailClosedReady === true ||
      iosCoverage.platformCancelLockFailClosedReady === true) &&
    summary.iosAppPasswordPromptUsed !== true &&
    iosCoverage.appPasswordPromptUsed !== true &&
    summary.iosAppCredentialPromptUsed !== true &&
    iosCoverage.appCredentialPromptUsed !== true &&
    summary.iosKeyMaterialExported !== true &&
    iosCoverage.keyMaterialExported !== true;
  const platformSystemAuthorizationReleaseReady =
    androidSystemCredentialReleaseReady &&
    macosSingleSystemAuthorizationReleaseReady &&
    iosSystemLocalAuthReleaseReady;
  const ready = diagnosticIntegrityReady &&
    releaseEvidenceReady &&
    evidenceChainComplete &&
    platformSystemAuthorizationReleaseReady;
  return {
    report: physicalEvidenceManifestReportPath,
    present,
    inputIntegrityReady: inputIntegrity.ok,
    inputSchemaStatus: inputIntegrity.status,
    inputSchemaFailureCount: inputIntegrity.failureCount,
    inputSchemaFailures: inputIntegrity.failures,
	    ok: inputIntegrity.ok && report?.ok === true,
	    ready,
	    localReadyDiagnostic: ready,
	    diagnosticIntegrityReady,
    platformSystemAuthorizationReleaseReady,
    androidSystemCredentialReleaseReady,
    macosSingleSystemAuthorizationReleaseReady,
    iosSystemLocalAuthReleaseReady,
    diagnosticOk: inputIntegrity.ok &&
      (report?.diagnosticOk === true || summary.diagnosticOk === true || report?.ok === true),
    okMeaning: String(report?.okMeaning || summary.okMeaning || ""),
    redacted: inputIntegrity.ok && report?.redacted === true,
    redactionReady,
    manifestIntegrityReady,
    physicalEvidenceChainReady,
    evidenceChainComplete,
	    releaseEvidenceReady,
	    localReleaseEvidenceReadyDiagnostic: releaseEvidenceReady,
    productionReady: inputIntegrity.ok && report?.productionReady === true,
    releaseReady: inputIntegrity.ok && report?.releaseReady === true,
    configuredReportCount: Number(summary.configuredReportCount || 0),
    missingConfiguredReportCount: Number(summary.missingConfiguredReportCount || 0),
    allConfiguredReportsPresent: summary.allConfiguredReportsPresent === true,
    linkedReportCount: Number(summary.linkedReportCount || 0),
	    platformCoverageCount: platformCoverage.length,
	    platformCoverage: platformCoverage.map((item) => ({
	      targetId: String(item?.targetId || ""),
	      platform: String(item?.platform || ""),
	      osFamily: String(item?.osFamily || ""),
	      arch: String(item?.arch || ""),
	      status: String(item?.status || "missing"),
	      remainingGates: stableStringList(item?.remainingGates),
	      hostSecretStoreReady: item?.hostSecretStoreReady === true,
	      platformSecretStoreReady: item?.platformSecretStoreReady === true,
	      physicalDeviceProofPresent: item?.physicalDeviceProofPresent === true,
	      releaseBundleShapeReady: item?.releaseBundleShapeReady === true,
	      releaseCliProofReady: item?.releaseCliProofReady === true,
	      packageUpdateReady: item?.packageUpdateReady === true,
	      commandResultReady: item?.commandResultReady === true,
	      installLaunchReady: item?.installLaunchReady === true,
	      userPresencePolicyReady: item?.userPresencePolicyReady === true,
	      platformSystemCredentialAuthReady: item?.platformSystemCredentialAuthReady === true,
	      platformCallbackContractReady: item?.platformCallbackContractReady === true
	    })),
    physicalProofClassCount: Array.isArray(report?.physicalProofClasses) ? report.physicalProofClasses.length : 0,
    releaseProofClassCount: Array.isArray(report?.releaseProofClasses) ? report.releaseProofClasses.length : 0,
    artifactDigestCount: artifactDigests.filter((item) => item?.present === true).length,
	    custodyStatusPresent: Boolean(report?.custodyStatus && Object.keys(report.custodyStatus).length > 0),
		    macosProductionEntitlementTemplateReady: macosCoverage.productionEntitlementTemplateReady === true,
		    macosProductionEntitlementFailClosedReady:
		      macosCoverage.productionEntitlementFailClosedReady === true ||
		      summary.macosProductionEntitlementFailClosedReady === true,
		    macosProductionEntitlementGateAccepted:
		      macosCoverage.productionEntitlementGateAccepted === true ||
		      summary.macosProductionEntitlementGateAccepted === true,
		    macosProductionEntitlementMissingFailClosed:
		      macosCoverage.productionEntitlementMissingFailClosed === true ||
		      summary.macosProductionEntitlementMissingFailClosed === true,
		    macosStandardKeychainRejectedForProduction:
		      macosCoverage.standardKeychainRejectedForProduction === true ||
		      summary.macosStandardKeychainRejectedForProduction === true,
		    macosStandardKeychainUserPresenceAcceptedForProduction:
		      macosCoverage.standardKeychainUserPresenceAcceptedForProduction === true ||
		      summary.macosStandardKeychainUserPresenceAcceptedForProduction === true,
		    macosStandardKeychainFallbackFailClosedReady:
		      macosCoverage.standardKeychainFallbackFailClosedReady === true ||
		      summary.macosStandardKeychainFallbackFailClosedReady === true,
		    macosKeyringReleaseEvidenceReady: summary.macosKeyringReleaseEvidenceReady === true,
	    macosLocalSecretStore: String(macosCoverage.localSecretStore || ""),
	    macosHostSecretStoreReady: macosCoverage.hostSecretStoreReady === true,
	    macosReleaseBundleShapeReady: macosCoverage.releaseBundleShapeReady === true,
	    macosReleaseCliProofReady: macosCoverage.releaseCliProofReady === true,
	    macosUserPresenceProofAttempted:
	      macosCoverage.userPresenceProofAttempted === true ||
	      summary.macosUserPresenceProofAttempted === true,
	    macosUserPresenceFailClosedUntilProductionEntitled:
	      macosCoverage.userPresenceFailClosedUntilProductionEntitled === true ||
	      summary.macosUserPresenceFailClosedUntilProductionEntitled === true,
	    macosUserPresenceBlockerCategory:
	      String(macosCoverage.userPresenceBlockerCategory || summary.macosUserPresenceBlockerCategory || ""),
	    macosUserPresencePolicyReady: macosCoverage.userPresencePolicyReady === true,
    macosSingleSystemAuthorizationContextVerified:
      macosCoverage.singleSystemAuthorizationContextVerified === true ||
      summary.macosSingleSystemAuthorizationContextVerified === true,
    macosInteractiveAuthorizationPromptBudgetReady:
      macosCoverage.interactiveAuthorizationPromptBudgetReady === true ||
      summary.macosInteractiveAuthorizationPromptBudgetReady === true,
    macosInteractiveAuthorizationAttemptCount:
      Number(macosCoverage.interactiveAuthorizationAttemptCount ||
        summary.macosInteractiveAuthorizationAttemptCount ||
        0),
    macosMaximumInteractiveAuthorizationAttemptsPerProof:
      Number(macosCoverage.maximumInteractiveAuthorizationAttemptsPerProof ||
        summary.macosMaximumInteractiveAuthorizationAttemptsPerProof ||
        1),
    macosAppCredentialPromptUsed:
      macosCoverage.appCredentialPromptUsed === true ||
      summary.macosAppCredentialPromptUsed === true,
    macosAppPasswordPromptUsed:
      macosCoverage.appPasswordPromptUsed === true ||
      summary.macosAppPasswordPromptUsed === true,
    macosSystemCredentialEntrySurface:
      String(macosCoverage.systemCredentialEntrySurface ||
        summary.macosSystemCredentialEntrySurface ||
	        ""),
	    ubuntuLinuxPackageUpdateReady: ubuntuCoverage.packageUpdateReady === true,
		    ubuntuLinuxReleaseEvidenceReady: summary.ubuntuLinuxReleaseEvidenceReady === true,
		    ubuntuLinuxLocalSecretStore: String(ubuntuCoverage.localSecretStore || ""),
		    ubuntuLinuxHostSecretStoreReady: ubuntuCoverage.hostSecretStoreReady === true,
		    ubuntuLinuxSecretStoreAuthorizationPolicyPresent:
		      ubuntuCoverage.secretStoreAuthorizationPolicyPresent === true ||
		      summary.ubuntuLinuxSecretStoreAuthorizationPolicyPresent === true,
		    ubuntuLinuxSecretStoreAuthorizationPolicyReady:
		      ubuntuCoverage.secretStoreAuthorizationPolicyReady === true ||
		      summary.ubuntuLinuxSecretStoreAuthorizationPolicyReady === true,
		    ubuntuLinuxReleaseCliProofReady: ubuntuCoverage.releaseCliProofReady === true,
	    ubuntuLinuxAdaptiveCustodyReady: ubuntuCoverage.adaptiveCustodyReady === true,
	    androidLocalSecretStore: String(androidCoverage.localSecretStore || ""),
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
	    androidUserAuthenticationBlockedBeforeKeyStoreE2e:
	      summary.androidUserAuthenticationBlockedBeforeKeyStoreE2e === true,
	    androidUserAuthenticationRequested:
	      summary.androidUserAuthenticationRequested === true,
	    androidUserAuthenticationPromptStarted:
	      summary.androidUserAuthenticationPromptStarted === true,
	    androidSystemCredentialPromptNotCompleted:
	      summary.androidSystemCredentialPromptNotCompleted === true,
	    androidUserAuthenticationBlockerReason:
	      String(summary.androidUserAuthenticationBlockerReason || ""),
	    androidUserAuthenticationUserActionRequired:
	      String(summary.androidUserAuthenticationUserActionRequired || ""),
	    androidUserAuthenticationDiagnosticCode:
	      String(summary.androidUserAuthenticationDiagnosticCode || ""),
	    androidUserAuthenticationResultCodePresent:
	      summary.androidUserAuthenticationResultCodePresent === true,
	    androidUserAuthenticationResultCode:
	      Number(summary.androidUserAuthenticationResultCode || 0),
	    androidUserAuthenticationCredentialEntrySurface:
	      String(summary.androidUserAuthenticationCredentialEntrySurface || ""),
	    androidUserAuthenticationSystemAuthenticationOnly:
	      summary.androidUserAuthenticationSystemAuthenticationOnly === true,
	    androidUserAuthenticationAppLockScreenCredentialCollection:
	      summary.androidUserAuthenticationAppLockScreenCredentialCollection === true,
	    androidUserAuthenticationKeyMaterialExported:
	      summary.androidUserAuthenticationKeyMaterialExported === true,
	    androidUserAuthenticationAppCredentialPromptUsed:
	      summary.androidUserAuthenticationAppCredentialPromptUsed === true,
    androidUserAuthenticationAppPasswordPromptUsed:
      summary.androidUserAuthenticationAppPasswordPromptUsed === true,
    androidUserAuthenticationPromptResult:
      String(summary.androidUserAuthenticationPromptResult || ""),
    androidKeyStoreAuthPolicyState:
      String(summary.androidKeyStoreAuthPolicyState || ""),
    iosPlatformSecretStoreReady: summary.iosPlatformSecretStoreReady === true,
    iosPhysicalSecretStoreBindingReady:
      summary.iosPhysicalSecretStoreBindingReady === true,
	    iosUserPresencePolicyReady:
	      summary.iosUserPresencePolicyReady === true ||
	      iosCoverage.platformUserPresencePolicyReady === true,
	    iosProductionCallbackAuthReady:
	      summary.iosProductionCallbackAuthReady === true ||
	      iosCoverage.platformProductionCallbackAuthReady === true,
	    iosCallbackReadsUseSharedLAContext:
	      summary.iosCallbackReadsUseSharedLAContext === true ||
	      iosCoverage.platformCallbackReadsUseSharedLAContext === true,
	    iosSingleSystemAuthorizationContextVerified:
	      summary.iosSingleSystemAuthorizationContextVerified === true ||
	      iosCoverage.platformSingleSystemAuthorizationContextVerified === true,
	    iosCallbackAuthContextAttachedToAllReads:
	      summary.iosCallbackAuthContextAttachedToAllReads === true ||
	      iosCoverage.platformCallbackAuthContextAttachedToAllReads === true,
	    appPasswordPromptUsedPresent:
	      summary.appPasswordPromptUsedPresent === true ||
	      iosCoverage.appPasswordPromptUsedPresent === true,
	    appCredentialPromptUsedPresent:
	      summary.appCredentialPromptUsedPresent === true ||
	      iosCoverage.appCredentialPromptUsedPresent === true,
	    keyMaterialExportedPresent:
	      summary.keyMaterialExportedPresent === true ||
	      iosCoverage.keyMaterialExportedPresent === true,
	    iosSystemLocalAuthPromptReady:
      summary.iosSystemLocalAuthPromptReady === true ||
      iosCoverage.platformSystemLocalAuthPromptReady === true,
    iosKeychainAccessControlNotDowngraded:
      summary.iosKeychainAccessControlNotDowngraded === true ||
      iosCoverage.platformKeychainAccessControlNotDowngraded === true,
    iosNonInteractiveFailClosedReady:
      summary.iosNonInteractiveFailClosedReady === true ||
      iosCoverage.platformNonInteractiveFailClosedReady === true,
    iosCancelLockFailClosedReady:
      summary.iosCancelLockFailClosedReady === true ||
      iosCoverage.platformCancelLockFailClosedReady === true,
    iosAppPasswordPromptUsed:
      summary.iosAppPasswordPromptUsed === true ||
      iosCoverage.appPasswordPromptUsed === true,
    iosAppCredentialPromptUsed:
      summary.iosAppCredentialPromptUsed === true ||
      iosCoverage.appCredentialPromptUsed === true,
    iosKeyMaterialExported:
      summary.iosKeyMaterialExported === true ||
      iosCoverage.keyMaterialExported === true,
    iosPhysicalCallbackContractReady:
      summary.iosPhysicalCallbackContractReady === true,
	    iosPhysicalRawJsonSecretOverridesProvenAbsent:
	      summary.iosPhysicalRawJsonSecretOverridesProvenAbsent === true,
	    iosPhysicalDeviceDiscovered: summary.iosPhysicalDeviceDiscovered === true,
	    iosWaitForDeviceAttempted: summary.iosWaitForDeviceAttempted === true,
	    iosWaitForDeviceTimeoutSeconds:
	      Number(summary.iosWaitForDeviceTimeoutSeconds || 0),
	    iosRemediationDeviceIdentifiersIncluded:
	      summary.iosRemediationDeviceIdentifiersIncluded === true,
	    iosRemediationSawUnavailablePhysicalDevice:
	      summary.iosRemediationSawUnavailablePhysicalDevice === true,
	    currentIosDeviceTrustState: String(summary.currentIosDeviceTrustState || ""),
	    currentIosTrustBlockerStaleCandidate:
	      summary.currentIosTrustBlockerStaleCandidate === true,
	    iosDeviceTrustBlockerEvidence:
	      summary.iosDeviceTrustBlockerEvidence || {},
	    iosLocalSecretStore: String(iosCoverage.localSecretStore || ""),
	    iosDeveloperModeOrDeviceTrustBlocked:
	      summary.iosDeveloperModeOrDeviceTrustBlocked === true,
    iosDeviceTrustGateResult: String(summary.iosDeviceTrustGateResult || ""),
    iosUserPresenceMissingFields:
      stableStringList(summary.iosUserPresenceMissingFields),
    iosUserPresenceMissingFieldCount:
      Number(summary.iosUserPresenceMissingFieldCount || 0),
    iosUserPresenceMissingFieldsAbsent:
      summary.iosUserPresenceMissingFieldsAbsent === true,
    iosPhysicalPrerequisiteMissingFields:
      stableStringList(summary.iosPhysicalPrerequisiteMissingFields),
    iosPhysicalPrerequisiteMissingFieldCount:
      Number(summary.iosPhysicalPrerequisiteMissingFieldCount || 0),
    iosPhysicalPrerequisiteMissingFieldsAbsent:
      summary.iosPhysicalPrerequisiteMissingFieldsAbsent === true,
    iosReleaseBuiltDesktopCliSelected: summary.iosReleaseBuiltDesktopCliSelected === true,
    boundaryGateSummary: {
      android: String(summary.boundaryGateSummary?.android || ""),
      ios: String(summary.boundaryGateSummary?.ios || "")
    },
    windowsLocalImplementationReady:
      summary.windowsLocalImplementationReady === true,
    windowsNativeHostEvidenceReady:
      summary.windowsNativeHostEvidenceReady === true,
    productionAcceptedEvidenceRecordsReady: releaseEvidenceReady === true &&
      platformSystemAuthorizationReleaseReady &&
      manifestIntegrityReady === true &&
      physicalEvidenceChainReady === true &&
      evidenceChainComplete === true &&
      report?.productionReady !== true &&
      report?.releaseReady !== true &&
      Number(summary.linkedReportCount || 0) > 0 &&
      platformCoverage.length >= 5 &&
      artifactDigests.some((item) => item?.present === true)
  };
}
