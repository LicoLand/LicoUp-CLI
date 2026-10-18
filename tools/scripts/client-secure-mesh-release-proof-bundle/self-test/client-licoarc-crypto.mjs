import {
  androidPlatformCryptoReportPath,
  platformCryptoReportPath,
  stationAcceptanceReportPath,
  rustCryptoReportPath
} from "../config.mjs";
import {
  androidPlatformCryptoSchemaVersion,
  platformCryptoSchemaVersion,
  rustCryptoSchemaVersion
} from "../constants.mjs";
import {
  summarizeClientLicoArcCryptoInputs,
} from "../summarize/client-licoarc-crypto.mjs";
import {
  ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT,
} from "../../lib/secure-mesh-physical-report-coverage.mjs";

export function runClientLicoArcCryptoInputsReadinessSelfTest() {
  const digest = `sha256:${"a".repeat(64)}`;
  const stationAcceptance = {
    schemaVersion: "licoup.licoarc-badtower.acceptance.v1",
    ok: true,
    protocolCandidateDigest: digest,
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
      mobileFfiDispatch: true,
      typedPendingObserved: true,
      durableResultReceiptAcknowledged: true,
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
  const stationCandidateBindings = {
    protocolCandidateDigest: stationAcceptance.protocolCandidateDigest,
    stationCandidateDigest: stationAcceptance.stationCandidateDigest,
    clientCandidateDigest: stationAcceptance.clientCandidateDigest,
  };
  const rustCrypto = {
    ok: true,
    schemaVersion: rustCryptoSchemaVersion,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    nativeResults: [{ id: "native-crypto", ok: true }],
    vectorCorpus: {
      ok: true,
      redacted: true,
      rawPrivateMaterialIncluded: false,
      rawPlaintextIncluded: false,
      rawPublicWireBytesIncluded: false,
      entryCount: 1
    },
    summary: {
      verificationPassed: true,
      metadataResistanceReady: true,
      nativeTestCount: 1,
      vectorCorpusGenerated: true,
      reviewSignoffReady: true,
      reviewerSignatureVerified: true,
      releaseOwnerSignatureVerified: true
    }
  };
  const platformCrypto = {
    ok: true,
    schemaVersion: platformCryptoSchemaVersion,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    nativeResults: [{ id: "platform-crypto", ok: true }],
    platformMatrix: [{ platform: "test", status: "complete" }],
    summary: {
      verificationPassed: true,
      nativeTestCount: 1,
      platformCount: 1,
      hostNativeSecretStoreReady: true
    }
  };
  const androidPlatformCrypto = {
    ok: true,
    schemaVersion: androidPlatformCryptoSchemaVersion,
    verifier: "tools/scripts/client-android-native-tests.mjs",
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    reportLeakScan: true,
    summary: {
      ok: true,
      platformCryptoAcceptanceReady: true,
      platformCustodyContractReady: true,
      platformAuthorizationContractReady: true,
      rustFfiActionContractReady: true,
      mlsMemberRemoveReleaseActionReady: true,
      unknownReleaseActionsFailClosed: true,
      nativeTestClassCount: ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT,
      privatePathsIncluded: false
    }
  };
  const reportRedactionProof = {
    ready: true,
    scannedRefs: [
      stationAcceptanceReportPath,
      rustCryptoReportPath,
      platformCryptoReportPath,
      androidPlatformCryptoReportPath
    ]
  };
  const complete = summarizeClientLicoArcCryptoInputs({
    stationAcceptance,
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const summarizeScenarioMutation = (scenarioPatch) =>
    summarizeClientLicoArcCryptoInputs({
      stationAcceptance: {
        ...stationAcceptance,
        scenario: { ...stationAcceptance.scenario, ...scenarioPatch },
      },
      rustCrypto,
      platformCrypto,
      androidPlatformCrypto,
      reportRedactionProof,
      stationCandidateBindings,
      stationCandidateInputsStable: true,
    });
  const invalidFreshEndpointCount =
    summarizeScenarioMutation({ freshEndpointCount: 1 });
  const missingPositiveExchange =
    summarizeScenarioMutation({ positiveExchange: false });
  const missingRoundTrip = summarizeScenarioMutation({ roundTrip: false });
  const stationPlaintextPresent =
    summarizeScenarioMutation({ stationPlaintextAbsent: false });
  const nonConformantEnvelopeAccepted =
    summarizeScenarioMutation({ nonConformantEnvelopeRejected: false });
  const transportHintAuthoritative =
    summarizeScenarioMutation({ transportHintsNonAuthoritative: false });
  const invalidOuterFieldContract =
    summarizeScenarioMutation({ exactFiveOuterFields: false });
  const missingMobileFfiDispatch =
    summarizeScenarioMutation({ mobileFfiDispatch: false });
  const missingTypedPending =
    summarizeScenarioMutation({ typedPendingObserved: false });
  const missingDurableResultReceipt =
    summarizeScenarioMutation({ durableResultReceiptAcknowledged: false });
  const releaseClaim = summarizeClientLicoArcCryptoInputs({
    stationAcceptance: {
      ...stationAcceptance,
      claims: { ...stationAcceptance.claims, clientRelease: true },
    },
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const privacyLeak = summarizeClientLicoArcCryptoInputs({
    stationAcceptance: {
      ...stationAcceptance,
      privacy: { ...stationAcceptance.privacy, endpointContentIncluded: true },
    },
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const rawRustPlaintext = summarizeClientLicoArcCryptoInputs({
    stationAcceptance,
    rustCrypto: { ...rustCrypto, rawPlaintextIncluded: true },
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const rawAndroidPrivateMaterial = summarizeClientLicoArcCryptoInputs({
    stationAcceptance,
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto: {
      ...androidPlatformCrypto,
      rawPrivateMaterialIncluded: true
    },
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const legacyPlatformCrypto = summarizeClientLicoArcCryptoInputs({
    stationAcceptance,
    rustCrypto,
    platformCrypto: {
      ...platformCrypto,
      schemaVersion: "licomesh.secure-mesh.platform-secret-store-matrix-report.v1"
    },
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const staleClientCandidate = summarizeClientLicoArcCryptoInputs({
    stationAcceptance: {
      ...stationAcceptance,
      clientCandidateDigest: `sha256:${"d".repeat(64)}`,
    },
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: true,
  });
  const tamperedProtocolCandidate = summarizeClientLicoArcCryptoInputs({
    stationAcceptance,
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings: {
      ...stationCandidateBindings,
      protocolCandidateDigest: `sha256:${"e".repeat(64)}`,
    },
    stationCandidateInputsStable: true,
  });
  const mutatedStationInput = summarizeClientLicoArcCryptoInputs({
    stationAcceptance,
    rustCrypto,
    platformCrypto,
    androidPlatformCrypto,
    reportRedactionProof,
    stationCandidateBindings,
    stationCandidateInputsStable: false,
  });
  const ok = complete.ready === true &&
    invalidFreshEndpointCount.ready === false &&
    missingPositiveExchange.ready === false &&
    missingRoundTrip.ready === false &&
    stationPlaintextPresent.ready === false &&
    nonConformantEnvelopeAccepted.ready === false &&
    transportHintAuthoritative.ready === false &&
    invalidOuterFieldContract.ready === false &&
    missingMobileFfiDispatch.ready === false &&
    missingTypedPending.ready === false &&
    missingDurableResultReceipt.ready === false &&
    releaseClaim.ready === false &&
    privacyLeak.ready === false &&
    rawRustPlaintext.ready === false &&
    rawAndroidPrivateMaterial.ready === false &&
    legacyPlatformCrypto.ready === false &&
    staleClientCandidate.ready === false &&
    tamperedProtocolCandidate.ready === false &&
    mutatedStationInput.ready === false;
  return {
    ok,
    completeEvidenceAccepted: complete.ready === true,
    invalidFreshEndpointCountRejected:
      invalidFreshEndpointCount.ready === false,
    missingPositiveExchangeRejected: missingPositiveExchange.ready === false,
    missingRoundTripRejected: missingRoundTrip.ready === false,
    stationPlaintextPresenceRejected: stationPlaintextPresent.ready === false,
    nonConformantEnvelopeAcceptanceRejected:
      nonConformantEnvelopeAccepted.ready === false,
    transportHintAuthorityRejected: transportHintAuthoritative.ready === false,
    invalidOuterFieldContractRejected:
      invalidOuterFieldContract.ready === false,
    missingMobileFfiDispatchRejected: missingMobileFfiDispatch.ready === false,
    missingTypedPendingRejected: missingTypedPending.ready === false,
    missingDurableResultReceiptRejected:
      missingDurableResultReceipt.ready === false,
    releaseClaimRejected: releaseClaim.ready === false,
    privacyLeakRejected: privacyLeak.ready === false,
    staleClientCandidateRejected: staleClientCandidate.ready === false,
    tamperedProtocolCandidateRejected:
      tamperedProtocolCandidate.ready === false,
    mutatedStationInputRejected: mutatedStationInput.ready === false,
    rawRustPlaintextRejected: rawRustPlaintext.ready === false,
    rawAndroidPrivateMaterialRejected: rawAndroidPrivateMaterial.ready === false,
    legacyPlatformCryptoSchemaRejected: legacyPlatformCrypto.ready === false
  };
}
