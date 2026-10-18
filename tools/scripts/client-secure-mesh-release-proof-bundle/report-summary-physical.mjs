import { stableStringList } from "./lists.mjs";

export function buildReleaseProofSummaryPhysical({
  ok,
  sourceResults,
  updateReleaseVerifier,
  physicalEvidenceManifestVerifier,
  reportRedactionVerifier,
  updateReleaseReport,
  releaseInputFreshness,
  releaseInputFreshnessSelfTest,
  physicalEvidenceManifest,
  physicalEvidenceManifestReadinessSelfTest,
  releaseProofContractReadinessSelfTest,
  physicalMatrixContractReadiness,
  physicalEvidenceManifestContractReadiness,
  reportRedactionProof,
  redactionFreshnessSelfTest,
  physicalMatrixReport,
  androidPhysicalInstallLaunchReport,
  ubuntuLinuxPackageUpdateReady,
  windowsLocalImplementationReady,
  windowsNativeHostEvidenceReady,
  productionReady,
  remainingGates
}) {
  return {
		    physicalMatrixAllPhysicalScenariosReady:
		      physicalMatrixReport.allPhysicalScenariosReady === true,
		    physicalMatrixInputIntegrityReady:
		      physicalMatrixReport.inputIntegrityReady === true,
		    physicalMatrixInputSchemaStatus:
		      String(physicalMatrixReport.inputSchemaStatus || "unknown"),
		    physicalMatrixInputSchemaFailureCount:
		      Number(physicalMatrixReport.inputSchemaFailureCount || 0),
			    physicalMatrixLocalPhysicalEvidenceChainReadyDiagnostic:
			      physicalMatrixReport.localPhysicalEvidenceChainReadyDiagnostic === true,
		    physicalMatrixLocalEvidenceChainCompleteDiagnostic:
		      physicalMatrixReport.localEvidenceChainCompleteDiagnostic === true,
		    physicalMatrixLocalReleaseEvidenceReadyDiagnostic:
		      physicalMatrixReport.localReleaseEvidenceReadyDiagnostic === true,
		    androidPhysicalInstallLaunchLocalReadyDiagnostic:
		      androidPhysicalInstallLaunchReport.localReadyDiagnostic === true,
    androidPhysicalInstallLaunchJniSecretCallbackInProcessReady:
      androidPhysicalInstallLaunchReport.jniSecretCallbackInProcessReady === true,
    androidPhysicalInstallLaunchStatusProbeSideEffectFree:
      androidPhysicalInstallLaunchReport.statusProbeSideEffectFree === true,
    androidPhysicalInstallLaunchFreshOneShotAuthorizationPolicyReady:
      androidPhysicalInstallLaunchReport.freshOneShotAuthorizationPolicyReady === true,
    androidPhysicalInstallLaunchAndroidKeyMaterialExportedPresent:
      androidPhysicalInstallLaunchReport.androidKeyMaterialExportedPresent === true,
    androidPhysicalInstallLaunchAndroidKeyMaterialExported:
      androidPhysicalInstallLaunchReport.androidKeyMaterialExported === true,
    physicalMatrixAndroidPlatformSecretStoreReady:
      physicalMatrixReport.androidPlatformSecretStoreReady === true,
    physicalMatrixAndroidPhysicalSecretStoreBindingReady:
      physicalMatrixReport.androidPhysicalSecretStoreBindingReady === true,
	    physicalMatrixAndroidPhysicalSystemCredentialAuthReady:
	      physicalMatrixReport.androidPhysicalSystemCredentialAuthReady === true,
    physicalMatrixAndroidPhysicalKeyStoreHardwareAuthReady:
      physicalMatrixReport.androidPhysicalKeyStoreHardwareAuthReady === true,
    physicalMatrixAndroidPhysicalKeyStoreSecurityLevelName:
      String(physicalMatrixReport.androidPhysicalKeyStoreSecurityLevelName || ""),
    physicalMatrixAndroidPhysicalKeyStoreInsideSecureHardware:
      physicalMatrixReport.androidPhysicalKeyStoreInsideSecureHardware === true,
    physicalMatrixAndroidPhysicalKeyStoreUserAuthenticationHardwareEnforced:
      physicalMatrixReport.androidPhysicalKeyStoreUserAuthenticationHardwareEnforced === true,
    physicalMatrixAndroidPhysicalKeyStoreUnlockedDeviceRequired:
      physicalMatrixReport.androidPhysicalKeyStoreUnlockedDeviceRequired === true,
	    physicalMatrixAndroidPhysicalCallbackContractReady:
	      physicalMatrixReport.androidPhysicalCallbackContractReady === true,
    physicalMatrixAndroidPhysicalRawJsonSecretOverridesProvenAbsent:
      physicalMatrixReport.androidPhysicalRawJsonSecretOverridesProvenAbsent === true,
    physicalMatrixAndroidPhysicalRawJsonSecretOverridesUsed:
      physicalMatrixReport.androidPhysicalRawJsonSecretOverridesUsed === true,
    physicalMatrixAndroidPhysicalRawJsonSecretOverridesUnknown:
      physicalMatrixReport.androidPhysicalRawJsonSecretOverridesUnknown === true,
    physicalMatrixAndroidInstallLaunchSchemaDrift:
      physicalMatrixReport.androidPhysicalInstallLaunchSchemaDrift === true,
    physicalMatrixAndroidInstallLaunchSchemaDriftFieldCount:
      Number(physicalMatrixReport.androidPhysicalInstallLaunchSchemaDriftFieldCount || 0),
    physicalMatrixAndroidInstallLaunchSchemaStatus:
      String(physicalMatrixReport.androidPhysicalInstallLaunchSchemaStatus || "unknown"),
    physicalMatrixAndroidAppPasswordPromptUsed:
      physicalMatrixReport.androidPhysicalAppPasswordPromptUsed === true,
	    physicalMatrixAndroidMissingFieldsAbsent:
	      physicalMatrixReport.androidPhysicalMissingFieldsAbsent === true,
	    physicalMatrixAndroidMissingFieldAuditPresent:
	      physicalMatrixReport.androidPhysicalMissingFieldAuditPresent === true,
	    physicalMatrixAndroidMissingFields:
	      stableStringList(physicalMatrixReport.androidPhysicalMissingFields),
	    physicalMatrixAndroidMissingFieldCount:
	      Number(physicalMatrixReport.androidPhysicalMissingFieldCount || 0),
	    physicalMatrixAndroidWeakProofFieldsAbsent:
	      physicalMatrixReport.androidPhysicalWeakProofFieldsAbsent === true,
	    physicalMatrixAndroidWeakProofFieldAuditPresent:
	      physicalMatrixReport.androidPhysicalWeakProofFieldAuditPresent === true,
	    physicalMatrixAndroidWeakProofFields:
	      stableStringList(physicalMatrixReport.androidPhysicalWeakProofFields),
	    physicalMatrixAndroidWeakProofFieldCount:
	      Number(physicalMatrixReport.androidPhysicalWeakProofFieldCount || 0),
    physicalMatrixIosPlatformSecretStoreReady:
      physicalMatrixReport.iosPlatformSecretStoreReady === true,
    physicalMatrixIosPhysicalSecretStoreBindingReady:
      physicalMatrixReport.iosPhysicalSecretStoreBindingReady === true,
	    physicalMatrixIosUserPresencePolicyReady:
	      physicalMatrixReport.iosUserPresencePolicyReady === true,
	    physicalMatrixIosProductionCallbackAuthReady:
	      physicalMatrixReport.iosProductionCallbackAuthReady === true,
	    physicalMatrixIosCallbackReadsUseSharedLAContext:
	      physicalMatrixReport.iosCallbackReadsUseSharedLAContext === true,
	    physicalMatrixIosSingleSystemAuthorizationContextVerified:
	      physicalMatrixReport.iosSingleSystemAuthorizationContextVerified === true,
	    physicalMatrixIosCallbackAuthContextAttachedToAllReads:
	      physicalMatrixReport.iosCallbackAuthContextAttachedToAllReads === true,
	    physicalMatrixAppPasswordPromptUsedPresent:
	      physicalMatrixReport.appPasswordPromptUsedPresent === true,
	    physicalMatrixAppCredentialPromptUsedPresent:
	      physicalMatrixReport.appCredentialPromptUsedPresent === true,
	    physicalMatrixKeyMaterialExportedPresent:
	      physicalMatrixReport.keyMaterialExportedPresent === true,
	    physicalMatrixIosSystemLocalAuthPromptReady:
	      physicalMatrixReport.iosSystemLocalAuthPromptReady === true,
    physicalMatrixIosKeychainAccessControlNotDowngraded:
      physicalMatrixReport.iosKeychainAccessControlNotDowngraded === true,
    physicalMatrixIosNonInteractiveFailClosedReady:
      physicalMatrixReport.iosNonInteractiveFailClosedReady === true,
    physicalMatrixIosCancelLockFailClosedReady:
      physicalMatrixReport.iosCancelLockFailClosedReady === true,
    physicalMatrixIosAppPasswordPromptUsed:
      physicalMatrixReport.iosAppPasswordPromptUsed === true,
    physicalMatrixIosAppCredentialPromptUsed:
      physicalMatrixReport.iosAppCredentialPromptUsed === true,
    physicalMatrixIosKeyMaterialExported:
      physicalMatrixReport.iosKeyMaterialExported === true,
    physicalMatrixIosPhysicalCallbackContractReady:
      physicalMatrixReport.iosPhysicalCallbackContractReady === true,
    physicalMatrixIosPhysicalRawJsonSecretOverridesProvenAbsent:
      physicalMatrixReport.iosPhysicalRawJsonSecretOverridesProvenAbsent === true,
    physicalMatrixMacosUserPresencePolicyReady:
      physicalMatrixReport.macosUserPresencePolicyReady === true,
    physicalMatrixMacosSingleSystemAuthorizationContextVerified:
      physicalMatrixReport.macosSingleSystemAuthorizationContextVerified === true,
    physicalMatrixMacosInteractiveAuthorizationPromptBudgetReady:
      physicalMatrixReport.macosInteractiveAuthorizationPromptBudgetReady === true,
    physicalMatrixMacosAppPasswordPromptUsed:
      physicalMatrixReport.macosAppPasswordPromptUsed === true,
    physicalMatrixMacosAppCredentialPromptUsed:
      physicalMatrixReport.macosAppCredentialPromptUsed === true,
    physicalMatrixMacosSystemCredentialEntrySurface:
      physicalMatrixReport.macosSystemCredentialEntrySurface,
    physicalEvidenceManifestAndroidPlatformSecretStoreReady:
      physicalEvidenceManifest.androidPlatformSecretStoreReady === true,
    physicalEvidenceManifestAndroidPhysicalSecretStoreBindingReady:
      physicalEvidenceManifest.androidPhysicalSecretStoreBindingReady === true,
	    physicalEvidenceManifestAndroidPhysicalSystemCredentialAuthReady:
	      physicalEvidenceManifest.androidPhysicalSystemCredentialAuthReady === true,
    physicalEvidenceManifestAndroidPhysicalKeyStoreHardwareAuthReady:
      physicalEvidenceManifest.androidPhysicalKeyStoreHardwareAuthReady === true,
    physicalEvidenceManifestAndroidPhysicalKeyStoreSecurityLevelName:
      String(physicalEvidenceManifest.androidPhysicalKeyStoreSecurityLevelName || ""),
    physicalEvidenceManifestAndroidPhysicalKeyStoreInsideSecureHardware:
      physicalEvidenceManifest.androidPhysicalKeyStoreInsideSecureHardware === true,
    physicalEvidenceManifestAndroidPhysicalKeyStoreUserAuthenticationHardwareEnforced:
      physicalEvidenceManifest.androidPhysicalKeyStoreUserAuthenticationHardwareEnforced === true,
    physicalEvidenceManifestAndroidPhysicalKeyStoreUnlockedDeviceRequired:
      physicalEvidenceManifest.androidPhysicalKeyStoreUnlockedDeviceRequired === true,
	    physicalEvidenceManifestAndroidPhysicalCallbackContractReady:
	      physicalEvidenceManifest.androidPhysicalCallbackContractReady === true,
    physicalEvidenceManifestAndroidPhysicalRawJsonSecretOverridesProvenAbsent:
      physicalEvidenceManifest.androidPhysicalRawJsonSecretOverridesProvenAbsent === true,
    physicalEvidenceManifestAndroidPhysicalRawJsonSecretOverridesUsed:
      physicalEvidenceManifest.androidPhysicalRawJsonSecretOverridesUsed === true,
    physicalEvidenceManifestAndroidPhysicalRawJsonSecretOverridesUnknown:
      physicalEvidenceManifest.androidPhysicalRawJsonSecretOverridesUnknown === true,
    physicalEvidenceManifestAndroidInstallLaunchSchemaDrift:
      physicalEvidenceManifest.androidPhysicalInstallLaunchSchemaDrift === true,
    physicalEvidenceManifestAndroidInstallLaunchSchemaDriftFieldCount:
      Number(physicalEvidenceManifest.androidPhysicalInstallLaunchSchemaDriftFieldCount || 0),
    physicalEvidenceManifestAndroidInstallLaunchSchemaStatus:
      String(physicalEvidenceManifest.androidPhysicalInstallLaunchSchemaStatus || "unknown"),
	    physicalEvidenceManifestAndroidMissingFieldsAbsent:
	      physicalEvidenceManifest.androidPhysicalMissingFieldsAbsent === true,
	    physicalEvidenceManifestAndroidMissingFieldAuditPresent:
	      physicalEvidenceManifest.androidPhysicalMissingFieldAuditPresent === true,
	    physicalEvidenceManifestAndroidMissingFields:
	      stableStringList(physicalEvidenceManifest.androidPhysicalMissingFields),
	    physicalEvidenceManifestAndroidMissingFieldCount:
	      Number(physicalEvidenceManifest.androidPhysicalMissingFieldCount || 0),
	    physicalEvidenceManifestAndroidWeakProofFieldsAbsent:
	      physicalEvidenceManifest.androidPhysicalWeakProofFieldsAbsent === true,
	    physicalEvidenceManifestAndroidWeakProofFieldAuditPresent:
	      physicalEvidenceManifest.androidPhysicalWeakProofFieldAuditPresent === true,
	    physicalEvidenceManifestAndroidWeakProofFields:
	      stableStringList(physicalEvidenceManifest.androidPhysicalWeakProofFields),
	    physicalEvidenceManifestAndroidWeakProofFieldCount:
	      Number(physicalEvidenceManifest.androidPhysicalWeakProofFieldCount || 0),
	    physicalEvidenceManifestAndroidUserAuthenticationRequested:
	      physicalEvidenceManifest.androidUserAuthenticationRequested === true,
	    physicalEvidenceManifestAndroidUserAuthenticationPromptStarted:
	      physicalEvidenceManifest.androidUserAuthenticationPromptStarted === true,
	    physicalEvidenceManifestAndroidSystemCredentialPromptNotCompleted:
	      physicalEvidenceManifest.androidSystemCredentialPromptNotCompleted === true,
	    physicalEvidenceManifestAndroidUserAuthenticationBlockerReason:
	      physicalEvidenceManifest.androidUserAuthenticationBlockerReason,
	    physicalEvidenceManifestAndroidUserAuthenticationUserActionRequired:
	      physicalEvidenceManifest.androidUserAuthenticationUserActionRequired,
	    physicalEvidenceManifestAndroidUserAuthenticationDiagnosticCode:
	      physicalEvidenceManifest.androidUserAuthenticationDiagnosticCode,
	    physicalEvidenceManifestAndroidUserAuthenticationResultCodePresent:
	      physicalEvidenceManifest.androidUserAuthenticationResultCodePresent === true,
	    physicalEvidenceManifestAndroidUserAuthenticationResultCode:
	      Number(physicalEvidenceManifest.androidUserAuthenticationResultCode || 0),
	    physicalEvidenceManifestAndroidUserAuthenticationCredentialEntrySurface:
	      physicalEvidenceManifest.androidUserAuthenticationCredentialEntrySurface,
	    physicalEvidenceManifestAndroidUserAuthenticationSystemAuthenticationOnly:
	      physicalEvidenceManifest.androidUserAuthenticationSystemAuthenticationOnly === true,
	    physicalEvidenceManifestAndroidUserAuthenticationAppLockScreenCredentialCollection:
	      physicalEvidenceManifest.androidUserAuthenticationAppLockScreenCredentialCollection === true,
	    physicalEvidenceManifestAndroidUserAuthenticationKeyMaterialExported:
	      physicalEvidenceManifest.androidUserAuthenticationKeyMaterialExported === true,
	    physicalEvidenceManifestAndroidLocalSecretStore:
	      physicalEvidenceManifest.androidLocalSecretStore,
	    physicalEvidenceManifestIosPlatformSecretStoreReady:
	      physicalEvidenceManifest.iosPlatformSecretStoreReady === true,
    physicalEvidenceManifestIosPhysicalSecretStoreBindingReady:
      physicalEvidenceManifest.iosPhysicalSecretStoreBindingReady === true,
	    physicalEvidenceManifestIosUserPresencePolicyReady:
	      physicalEvidenceManifest.iosUserPresencePolicyReady === true,
	    physicalEvidenceManifestIosDeviceTrustBlockerEvidence:
	      physicalEvidenceManifest.iosDeviceTrustBlockerEvidence || {},
	    physicalEvidenceManifestIosUserPresenceMissingFields:
	      stableStringList(physicalEvidenceManifest.iosUserPresenceMissingFields),
	    physicalEvidenceManifestIosUserPresenceMissingFieldCount:
	      Number(physicalEvidenceManifest.iosUserPresenceMissingFieldCount || 0),
	    physicalEvidenceManifestIosUserPresenceMissingFieldsAbsent:
	      physicalEvidenceManifest.iosUserPresenceMissingFieldsAbsent === true,
	    physicalEvidenceManifestIosPhysicalPrerequisiteMissingFields:
	      stableStringList(physicalEvidenceManifest.iosPhysicalPrerequisiteMissingFields),
	    physicalEvidenceManifestIosPhysicalPrerequisiteMissingFieldCount:
	      Number(physicalEvidenceManifest.iosPhysicalPrerequisiteMissingFieldCount || 0),
	    physicalEvidenceManifestIosPhysicalPrerequisiteMissingFieldsAbsent:
	      physicalEvidenceManifest.iosPhysicalPrerequisiteMissingFieldsAbsent === true,
	    physicalEvidenceManifestIosProductionCallbackAuthReady:
	      physicalEvidenceManifest.iosProductionCallbackAuthReady === true,
	    physicalEvidenceManifestIosCallbackReadsUseSharedLAContext:
	      physicalEvidenceManifest.iosCallbackReadsUseSharedLAContext === true,
	    physicalEvidenceManifestIosSingleSystemAuthorizationContextVerified:
	      physicalEvidenceManifest.iosSingleSystemAuthorizationContextVerified === true,
	    physicalEvidenceManifestIosCallbackAuthContextAttachedToAllReads:
	      physicalEvidenceManifest.iosCallbackAuthContextAttachedToAllReads === true,
	    physicalEvidenceManifestAppPasswordPromptUsedPresent:
	      physicalEvidenceManifest.appPasswordPromptUsedPresent === true,
	    physicalEvidenceManifestAppCredentialPromptUsedPresent:
	      physicalEvidenceManifest.appCredentialPromptUsedPresent === true,
	    physicalEvidenceManifestKeyMaterialExportedPresent:
	      physicalEvidenceManifest.keyMaterialExportedPresent === true,
	    physicalEvidenceManifestIosSystemLocalAuthPromptReady:
      physicalEvidenceManifest.iosSystemLocalAuthPromptReady === true,
    physicalEvidenceManifestIosKeychainAccessControlNotDowngraded:
      physicalEvidenceManifest.iosKeychainAccessControlNotDowngraded === true,
    physicalEvidenceManifestIosNonInteractiveFailClosedReady:
      physicalEvidenceManifest.iosNonInteractiveFailClosedReady === true,
    physicalEvidenceManifestIosCancelLockFailClosedReady:
      physicalEvidenceManifest.iosCancelLockFailClosedReady === true,
    physicalEvidenceManifestIosAppPasswordPromptUsed:
      physicalEvidenceManifest.iosAppPasswordPromptUsed === true,
    physicalEvidenceManifestIosAppCredentialPromptUsed:
      physicalEvidenceManifest.iosAppCredentialPromptUsed === true,
    physicalEvidenceManifestIosKeyMaterialExported:
      physicalEvidenceManifest.iosKeyMaterialExported === true,
    physicalEvidenceManifestIosPhysicalCallbackContractReady:
      physicalEvidenceManifest.iosPhysicalCallbackContractReady === true,
	    physicalEvidenceManifestIosPhysicalRawJsonSecretOverridesProvenAbsent:
	      physicalEvidenceManifest.iosPhysicalRawJsonSecretOverridesProvenAbsent === true,
	    physicalEvidenceManifestIosWaitForDeviceAttempted:
	      physicalEvidenceManifest.iosWaitForDeviceAttempted === true,
	    physicalEvidenceManifestIosWaitForDeviceTimeoutSeconds:
	      Number(physicalEvidenceManifest.iosWaitForDeviceTimeoutSeconds || 0),
	    physicalEvidenceManifestIosRemediationDeviceIdentifiersIncluded:
	      physicalEvidenceManifest.iosRemediationDeviceIdentifiersIncluded === true,
	    physicalEvidenceManifestIosRemediationSawUnavailablePhysicalDevice:
	      physicalEvidenceManifest.iosRemediationSawUnavailablePhysicalDevice === true,
	    physicalEvidenceManifestCurrentIosDeviceTrustState:
	      physicalEvidenceManifest.currentIosDeviceTrustState,
	    physicalEvidenceManifestCurrentIosTrustBlockerStaleCandidate:
	      physicalEvidenceManifest.currentIosTrustBlockerStaleCandidate === true,
	    physicalEvidenceManifestIosLocalSecretStore:
	      physicalEvidenceManifest.iosLocalSecretStore,
		    macosProductionEntitlementTemplateReady:
		      physicalEvidenceManifest.macosProductionEntitlementTemplateReady === true,
		    physicalEvidenceManifestMacosProductionEntitlementFailClosedReady:
		      physicalEvidenceManifest.macosProductionEntitlementFailClosedReady === true,
		    physicalEvidenceManifestMacosProductionEntitlementGateAccepted:
		      physicalEvidenceManifest.macosProductionEntitlementGateAccepted === true,
		    physicalEvidenceManifestMacosProductionEntitlementMissingFailClosed:
		      physicalEvidenceManifest.macosProductionEntitlementMissingFailClosed === true,
		    physicalEvidenceManifestMacosStandardKeychainRejectedForProduction:
		      physicalEvidenceManifest.macosStandardKeychainRejectedForProduction === true,
		    physicalEvidenceManifestMacosStandardKeychainUserPresenceAcceptedForProduction:
		      physicalEvidenceManifest.macosStandardKeychainUserPresenceAcceptedForProduction === true,
		    physicalEvidenceManifestMacosStandardKeychainFallbackFailClosedReady:
		      physicalEvidenceManifest.macosStandardKeychainFallbackFailClosedReady === true,
		    physicalEvidenceManifestMacosKeyringReleaseEvidenceReady:
		      physicalEvidenceManifest.macosKeyringReleaseEvidenceReady === true,
	    physicalEvidenceManifestMacosLocalSecretStore:
	      physicalEvidenceManifest.macosLocalSecretStore,
	    physicalEvidenceManifestMacosHostSecretStoreReady:
	      physicalEvidenceManifest.macosHostSecretStoreReady === true,
	    physicalEvidenceManifestMacosReleaseBundleShapeReady:
	      physicalEvidenceManifest.macosReleaseBundleShapeReady === true,
	    physicalEvidenceManifestMacosReleaseCliProofReady:
	      physicalEvidenceManifest.macosReleaseCliProofReady === true,
	    physicalEvidenceManifestMacosUserPresenceProofAttempted:
	      physicalEvidenceManifest.macosUserPresenceProofAttempted === true,
	    physicalEvidenceManifestMacosUserPresenceFailClosedUntilProductionEntitled:
	      physicalEvidenceManifest.macosUserPresenceFailClosedUntilProductionEntitled === true,
	    physicalEvidenceManifestMacosUserPresenceBlockerCategory:
	      physicalEvidenceManifest.macosUserPresenceBlockerCategory,
	    macosUserPresencePolicyReady: physicalEvidenceManifest.macosUserPresencePolicyReady === true,
    macosSingleSystemAuthorizationContextVerified:
      physicalEvidenceManifest.macosSingleSystemAuthorizationContextVerified === true,
    macosInteractiveAuthorizationPromptBudgetReady:
      physicalEvidenceManifest.macosInteractiveAuthorizationPromptBudgetReady === true,
    macosInteractiveAuthorizationAttemptCount:
      physicalEvidenceManifest.macosInteractiveAuthorizationAttemptCount,
    macosMaximumInteractiveAuthorizationAttemptsPerProof:
      physicalEvidenceManifest.macosMaximumInteractiveAuthorizationAttemptsPerProof,
    macosAppPasswordPromptUsed:
      physicalEvidenceManifest.macosAppPasswordPromptUsed === true,
    macosAppCredentialPromptUsed:
      physicalEvidenceManifest.macosAppCredentialPromptUsed === true,
    macosSystemCredentialEntrySurface:
      physicalEvidenceManifest.macosSystemCredentialEntrySurface,
    androidUserAuthenticationBlockedBeforeKeyStoreE2e:
      physicalEvidenceManifest.androidUserAuthenticationBlockedBeforeKeyStoreE2e === true,
    androidUserAuthenticationAppCredentialPromptUsed:
      physicalEvidenceManifest.androidUserAuthenticationAppCredentialPromptUsed === true,
    androidUserAuthenticationAppPasswordPromptUsed:
      physicalEvidenceManifest.androidUserAuthenticationAppPasswordPromptUsed === true,
    iosPhysicalDeviceDiscovered: physicalEvidenceManifest.iosPhysicalDeviceDiscovered === true,
    iosDeveloperModeOrDeviceTrustBlocked:
      physicalEvidenceManifest.iosDeveloperModeOrDeviceTrustBlocked === true,
	    iosReleaseBuiltDesktopCliSelected:
	      physicalEvidenceManifest.iosReleaseBuiltDesktopCliSelected === true,
	    physicalEvidenceManifestUbuntuLinuxReleaseEvidenceReady:
	      physicalEvidenceManifest.ubuntuLinuxReleaseEvidenceReady === true,
	    physicalEvidenceManifestUbuntuLinuxLocalSecretStore:
	      physicalEvidenceManifest.ubuntuLinuxLocalSecretStore,
		    physicalEvidenceManifestUbuntuLinuxHostSecretStoreReady:
		      physicalEvidenceManifest.ubuntuLinuxHostSecretStoreReady === true,
		    physicalEvidenceManifestUbuntuLinuxSecretStoreAuthorizationPolicyPresent:
		      physicalEvidenceManifest.ubuntuLinuxSecretStoreAuthorizationPolicyPresent === true,
		    physicalEvidenceManifestUbuntuLinuxSecretStoreAuthorizationPolicyReady:
		      physicalEvidenceManifest.ubuntuLinuxSecretStoreAuthorizationPolicyReady === true,
		    ubuntuLinuxPackageUpdateReady,
    windowsLocalImplementationReady,
    windowsNativeHostEvidenceReady,
    macosActualReleaseBundleVerified: updateReleaseReport.macosActualReleaseBundleVerified === true,
    productionReady,
    releaseReady: false,
    reportLeakScan: true,
    remainingGates

  };
}
