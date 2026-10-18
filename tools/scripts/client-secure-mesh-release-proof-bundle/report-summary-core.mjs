import { stableStringList } from "./lists.mjs";

export function buildReleaseProofSummaryCore({
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
  clientLicoArcCryptoInputs,
  clientLicoArcCryptoInputsReadinessSelfTest,
  physicalMatrixReport,
  androidPhysicalInstallLaunchReport,
  ubuntuLinuxPackageUpdateReady,
  windowsLocalImplementationReady,
  windowsNativeHostEvidenceReady,
  productionReady,
  remainingGates
}) {
  return {
	    verificationPassed: ok,
	    bundleDiagnosticOk:
	      sourceResults.every((check) => check.ok) &&
	      updateReleaseVerifier.ok &&
	      reportRedactionVerifier.ok &&
	      updateReleaseReport.ok === true &&
	      releaseInputFreshness.ready === true,
	    sourceCheckCount: sourceResults.length,
    releaseInputFreshnessReady: releaseInputFreshness.ready === true,
    releaseInputFreshnessCurrentCount: releaseInputFreshness.currentCount,
    releaseInputFreshnessStaleOrInvalidCount: releaseInputFreshness.staleOrInvalidCount,
    releaseInputFreshnessFailedLabels: releaseInputFreshness.failedLabels,
    releaseInputFreshnessSelfTestReady: releaseInputFreshnessSelfTest.ok === true,
    updateReleaseVerifierPassed: updateReleaseVerifier.ok,
    physicalEvidenceManifestVerifierPassed: physicalEvidenceManifestVerifier.ok,
    reportRedactionVerifierPassed: reportRedactionVerifier.ok,
    reportRedactionReady: reportRedactionProof.ready === true,
    redactionFreshnessSelfTestReady: redactionFreshnessSelfTest.ok === true,
	    physicalEvidenceManifestReadinessSelfTestReady:
	      physicalEvidenceManifestReadinessSelfTest.ok === true,
	    releaseProofContractReadinessSelfTestReady:
	      releaseProofContractReadinessSelfTest.ok === true,
	    forgedPhysicalMatrixSummaryReadyRejected:
	      releaseProofContractReadinessSelfTest.forgedPhysicalMatrixSummaryReadyRejected === true,
	    forgedPhysicalEvidenceManifestSummaryReadyRejected:
	      releaseProofContractReadinessSelfTest.forgedPhysicalEvidenceManifestSummaryReadyRejected === true,
	    legacyPhysicalMatrixSchemaRejected:
	      releaseProofContractReadinessSelfTest.legacyPhysicalMatrixSchemaRejected === true,
	    legacyPhysicalEvidenceManifestSchemaRejected:
	      releaseProofContractReadinessSelfTest.legacyPhysicalEvidenceManifestSchemaRejected === true,
	    androidPhysicalInstallLaunchLocalReadyDiagnosticOnly:
	      releaseProofContractReadinessSelfTest.androidPhysicalInstallLaunchLocalReadyDiagnosticOnly === true,
	    androidPhysicalInstallLaunchOneShotAuthorizationRequired:
	      releaseProofContractReadinessSelfTest.androidOneShotAuthorizationRequired === true,
	    physicalMatrixContractReadinessReady:
	      physicalMatrixContractReadiness.ready === true,
	    physicalMatrixContractReadinessReason:
	      physicalMatrixContractReadiness.reason,
	    physicalMatrixContractReadinessRemainingGateCount:
	      physicalMatrixContractReadiness.remainingGateCount,
	    physicalMatrixContractReadinessSourceOfTruthAccepted:
	      physicalMatrixContractReadiness.sourceOfTruthAccepted === true,
	    physicalMatrixContractReadinessProvenanceAccepted:
	      physicalMatrixContractReadiness.provenanceAccepted === true,
	    physicalMatrixContractReadinessScopeAccepted:
	      physicalMatrixContractReadiness.missingRequiredScopeClaims.length === 0 &&
	      physicalMatrixContractReadiness.missingRequiredScopeEvidenceClaims.length === 0,
	    physicalEvidenceManifestContractReadinessReady:
	      physicalEvidenceManifestContractReadiness.ready === true,
	    physicalEvidenceManifestContractReadinessReason:
	      physicalEvidenceManifestContractReadiness.reason,
	    physicalEvidenceManifestContractReadinessRemainingGateCount:
	      physicalEvidenceManifestContractReadiness.remainingGateCount,
	    physicalEvidenceManifestContractReadinessSourceOfTruthAccepted:
	      physicalEvidenceManifestContractReadiness.sourceOfTruthAccepted === true,
	    physicalEvidenceManifestContractReadinessProvenanceAccepted:
	      physicalEvidenceManifestContractReadiness.provenanceAccepted === true,
	    physicalEvidenceManifestContractReadinessScopeAccepted:
	      physicalEvidenceManifestContractReadiness.missingRequiredScopeClaims.length === 0 &&
	      physicalEvidenceManifestContractReadiness.missingRequiredScopeEvidenceClaims.length === 0,
	    physicalEvidenceManifestDiagnosticOnlyRejected:
      physicalEvidenceManifestReadinessSelfTest.diagnosticOnlyRejected === true,
    physicalEvidenceManifestReleaseEvidenceRequired:
      physicalEvidenceManifestReadinessSelfTest.releaseEvidenceRequired === true,
    physicalEvidenceManifestLegacySchemaRejected:
      physicalEvidenceManifestReadinessSelfTest.legacySchemaRejected === true,
    physicalEvidenceManifestPlatformSystemAuthorizationReady:
      physicalEvidenceManifest.platformSystemAuthorizationReleaseReady === true,
    physicalEvidenceManifestAndroidSystemCredentialReleaseReady:
      physicalEvidenceManifest.androidSystemCredentialReleaseReady === true,
    physicalEvidenceManifestMacosSingleSystemAuthorizationReleaseReady:
      physicalEvidenceManifest.macosSingleSystemAuthorizationReleaseReady === true,
    physicalEvidenceManifestIosSystemLocalAuthReleaseReady:
      physicalEvidenceManifest.iosSystemLocalAuthReleaseReady === true,
    physicalEvidenceManifestPlatformSystemAuthorizationRequired:
      physicalEvidenceManifestReadinessSelfTest.platformSystemAuthorizationRequired === true,
    physicalEvidenceManifestAndroidAppPasswordPromptRejected:
      physicalEvidenceManifestReadinessSelfTest.androidAppPasswordPromptRejected === true,
    physicalEvidenceManifestMacosRepeatedAuthorizationRejected:
      physicalEvidenceManifestReadinessSelfTest.macosRepeatedAuthorizationRejected === true,
    physicalEvidenceManifestMacosAppPasswordPromptRejected:
      physicalEvidenceManifestReadinessSelfTest.macosAppPasswordPromptRejected === true,
    physicalEvidenceManifestIosAppCredentialPromptRejected:
      physicalEvidenceManifestReadinessSelfTest.iosAppCredentialPromptRejected === true,
    releaseInputRedactionDigestsCurrent:
      reportRedactionProof.scannedRefDigestsCurrent === true,
    releaseInputRedactionDigestManifestExact:
      reportRedactionProof.digestManifestExact === true,
    releaseInputRedactionRunIdMatched:
      reportRedactionProof.redactionRunIdMatched === true,
    releaseInputRedactionDigestCount:
      reportRedactionProof.scannedRefDigestCount,
    clientLicoArcCryptoInputsReady:
      clientLicoArcCryptoInputs.ready === true,
    clientLicoArcCryptoInputsReadinessSelfTestReady:
      clientLicoArcCryptoInputsReadinessSelfTest.ok === true,
    completeClientLicoArcCryptoEvidenceAccepted:
      clientLicoArcCryptoInputsReadinessSelfTest.completeEvidenceAccepted === true,
    invalidFreshEndpointCountRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .invalidFreshEndpointCountRejected === true,
    missingPositiveExchangeRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .missingPositiveExchangeRejected === true,
    missingRoundTripRejected:
      clientLicoArcCryptoInputsReadinessSelfTest.missingRoundTripRejected === true,
    stationPlaintextPresenceRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .stationPlaintextPresenceRejected === true,
    nonConformantEnvelopeAcceptanceRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .nonConformantEnvelopeAcceptanceRejected === true,
    transportHintAuthorityRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .transportHintAuthorityRejected === true,
    invalidOuterFieldContractRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .invalidOuterFieldContractRejected === true,
    stationReleaseClaimRejected:
      clientLicoArcCryptoInputsReadinessSelfTest.releaseClaimRejected === true,
    stationPrivacyLeakRejected:
      clientLicoArcCryptoInputsReadinessSelfTest.privacyLeakRejected === true,
    staleClientCandidateRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .staleClientCandidateRejected === true,
    tamperedProtocolCandidateRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .tamperedProtocolCandidateRejected === true,
    mutatedStationInputRejected:
      clientLicoArcCryptoInputsReadinessSelfTest
        .mutatedStationInputRejected === true,
    rawRustCryptoPlaintextRejected:
      clientLicoArcCryptoInputsReadinessSelfTest.rawRustPlaintextRejected === true,
    rawAndroidCryptoPrivateMaterialRejected:
      clientLicoArcCryptoInputsReadinessSelfTest.rawAndroidPrivateMaterialRejected === true,
    legacyPlatformCryptoSchemaRejected:
      clientLicoArcCryptoInputsReadinessSelfTest.legacyPlatformCryptoSchemaRejected === true,
    releaseInputRedactionCoversClientRefs:
      clientLicoArcCryptoInputs.releaseInputRedactionCoversClientRefs === true,
    stationAcceptanceContractReady:
      clientLicoArcCryptoInputs.stationAcceptanceContractReady === true,
    stationCandidateBindingsReady:
      clientLicoArcCryptoInputs.stationCandidateBindingsReady === true,
    stationCandidateInputsStable:
      clientLicoArcCryptoInputs.stationCandidateInputsStable === true,
    stationAcceptanceFreshEndpointCount:
      clientLicoArcCryptoInputs.stationAcceptanceFreshEndpointCount,
    stationAcceptancePositiveExchange:
      clientLicoArcCryptoInputs.stationAcceptancePositiveExchange === true,
    stationAcceptanceRoundTrip:
      clientLicoArcCryptoInputs.stationAcceptanceRoundTrip === true,
    stationAcceptancePlaintextAbsent:
      clientLicoArcCryptoInputs.stationAcceptancePlaintextAbsent === true,
    stationAcceptanceNonConformantEnvelopeRejected:
      clientLicoArcCryptoInputs
        .stationAcceptanceNonConformantEnvelopeRejected === true,
    stationAcceptanceTransportHintsNonAuthoritative:
      clientLicoArcCryptoInputs
        .stationAcceptanceTransportHintsNonAuthoritative === true,
    stationAcceptanceExactFiveOuterFields:
      clientLicoArcCryptoInputs.stationAcceptanceExactFiveOuterFields === true,
    rustCryptoReportReady:
      clientLicoArcCryptoInputs.rustCryptoReportReady === true,
    rustCryptoNativeTestsReady:
      clientLicoArcCryptoInputs.rustCryptoNativeTestsReady === true,
    rustCryptoVectorCorpusReady:
      clientLicoArcCryptoInputs.rustCryptoVectorCorpusReady === true,
    rustCryptoReviewReady:
      clientLicoArcCryptoInputs.rustCryptoReviewReady === true,
    platformCryptoReportReady:
      clientLicoArcCryptoInputs.platformCryptoReportReady === true,
    androidPlatformCryptoReportReady:
      clientLicoArcCryptoInputs.androidPlatformCryptoReportReady === true,
			    physicalEvidenceManifestLocalReadyDiagnostic:
			      physicalEvidenceManifest.localReadyDiagnostic === true,
		    physicalEvidenceManifestInputIntegrityReady:
		      physicalEvidenceManifest.inputIntegrityReady === true,
		    physicalEvidenceManifestInputSchemaStatus:
		      String(physicalEvidenceManifest.inputSchemaStatus || "unknown"),
		    physicalEvidenceManifestInputSchemaFailureCount:
		      Number(physicalEvidenceManifest.inputSchemaFailureCount || 0),
		    physicalEvidenceManifestDiagnosticOk:
		      physicalEvidenceManifest.diagnosticOk === true,
	    physicalEvidenceManifestRedactionReady:
	      physicalEvidenceManifest.redactionReady === true,
	    physicalEvidenceManifestIntegrityReady:
	      physicalEvidenceManifest.manifestIntegrityReady === true,
	    physicalEvidenceManifestChainReady:
	      physicalEvidenceManifest.physicalEvidenceChainReady === true,
	    physicalEvidenceManifestEvidenceChainComplete:
	      physicalEvidenceManifest.evidenceChainComplete === true,
		    physicalEvidenceManifestLocalReleaseEvidenceReadyDiagnostic:
		      physicalEvidenceManifest.localReleaseEvidenceReadyDiagnostic === true,
  };
}
