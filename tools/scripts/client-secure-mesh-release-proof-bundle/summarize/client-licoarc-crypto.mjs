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
  licoArcBadTowerAcceptanceCoverage,
} from "../../lib/licoarc-badtower-acceptance-report.mjs";
import {
  ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT,
} from "../../lib/secure-mesh-physical-report-coverage.mjs";

export function redactedClientReleaseInputReady(report, expectedSchemaVersion) {
  return report?.ok === true &&
    report.schemaVersion === expectedSchemaVersion &&
    report.redacted === true &&
    report.rawPrivateMaterialIncluded === false &&
    report.rawPlaintextIncluded === false &&
    report.rawPublicWireBytesIncluded === false &&
    report.reportLeakScan === true;
}

export function summarizeClientLicoArcCryptoInputs({
  stationAcceptance = {},
  rustCrypto = {},
  platformCrypto = {},
  androidPlatformCrypto = {},
  reportRedactionProof = {},
  stationCandidateBindings = {},
  stationCandidateInputsStable = false,
} = {}) {
  const stationAcceptanceCoverage =
    licoArcBadTowerAcceptanceCoverage(
      stationAcceptance,
      stationCandidateBindings,
    );
  const rustCryptoSummary = rustCrypto.summary || {};
  const platformCryptoSummary = platformCrypto.summary || {};
  const androidPlatformCryptoSummary = androidPlatformCrypto.summary || {};
  const scannedRefs = new Set(reportRedactionProof.scannedRefs || []);
  const requiredRefs = [
    stationAcceptanceReportPath,
    rustCryptoReportPath,
    platformCryptoReportPath,
    androidPlatformCryptoReportPath
  ];
  const releaseInputRedactionCoversClientRefs =
    requiredRefs.every((ref) => scannedRefs.has(ref));

  const stationAcceptanceContractReady = stationAcceptanceCoverage.reportValid;
  const stationCandidateBindingsReady =
    stationCandidateInputsStable === true &&
    stationAcceptanceCoverage.candidateBindingsReady === true;

  const rustCryptoNativeResults = Array.isArray(rustCrypto.nativeResults)
    ? rustCrypto.nativeResults
    : [];
  const rustCryptoNativeTestsReady =
    rustCryptoNativeResults.length > 0 &&
    rustCryptoNativeResults.every((result) => result?.ok === true) &&
    rustCryptoSummary.nativeTestCount === rustCryptoNativeResults.length;
  const rustCryptoVectorCorpusReady =
    rustCrypto.vectorCorpus?.ok === true &&
    rustCrypto.vectorCorpus?.redacted === true &&
    rustCrypto.vectorCorpus?.rawPrivateMaterialIncluded === false &&
    rustCrypto.vectorCorpus?.rawPlaintextIncluded === false &&
    rustCrypto.vectorCorpus?.rawPublicWireBytesIncluded === false &&
    Number(rustCrypto.vectorCorpus?.entryCount || 0) > 0;
  const rustCryptoReportReady =
    redactedClientReleaseInputReady(rustCrypto, rustCryptoSchemaVersion) &&
    rustCryptoSummary.verificationPassed === true &&
    rustCryptoSummary.metadataResistanceReady === true &&
    rustCryptoSummary.vectorCorpusGenerated === true &&
    rustCryptoNativeTestsReady &&
    rustCryptoVectorCorpusReady;
  const rustCryptoReviewReady =
    rustCryptoSummary.reviewSignoffReady === true &&
    rustCryptoSummary.reviewerSignatureVerified === true &&
    rustCryptoSummary.releaseOwnerSignatureVerified === true;

  const platformCryptoNativeResults = Array.isArray(platformCrypto.nativeResults)
    ? platformCrypto.nativeResults
    : [];
  const platformCryptoMatrix = Array.isArray(platformCrypto.platformMatrix)
    ? platformCrypto.platformMatrix
    : [];
  const platformCryptoReportReady =
    redactedClientReleaseInputReady(platformCrypto, platformCryptoSchemaVersion) &&
    platformCryptoSummary.verificationPassed === true &&
    platformCryptoNativeResults.length > 0 &&
    platformCryptoNativeResults.every((result) => result?.ok === true) &&
    platformCryptoSummary.nativeTestCount === platformCryptoNativeResults.length &&
    platformCryptoMatrix.length > 0 &&
    platformCryptoSummary.platformCount === platformCryptoMatrix.length &&
    platformCryptoSummary.hostNativeSecretStoreReady === true;

  const androidPlatformCryptoReportReady =
    redactedClientReleaseInputReady(
      androidPlatformCrypto,
      androidPlatformCryptoSchemaVersion
    ) &&
    androidPlatformCrypto.verifier ===
      "tools/scripts/client-android-native-tests.mjs" &&
    androidPlatformCryptoSummary.ok === true &&
    androidPlatformCryptoSummary.platformCryptoAcceptanceReady === true &&
    androidPlatformCryptoSummary.platformCustodyContractReady === true &&
    androidPlatformCryptoSummary.platformAuthorizationContractReady === true &&
    androidPlatformCryptoSummary.rustFfiActionContractReady === true &&
    androidPlatformCryptoSummary.mlsMemberRemoveReleaseActionReady === true &&
    androidPlatformCryptoSummary.unknownReleaseActionsFailClosed === true &&
    androidPlatformCryptoSummary.nativeTestClassCount ===
      ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT &&
    androidPlatformCryptoSummary.privatePathsIncluded === false;

  const remainingGates = [
    ...(releaseInputRedactionCoversClientRefs && reportRedactionProof.ready === true
      ? []
      : ["release-input redaction scan covers Lico Arc and client cryptography reports"]),
    ...(stationAcceptanceContractReady
      ? []
      : ["strict Lico Arc BadTower interoperability acceptance ready"]),
    ...(stationCandidateBindingsReady
      ? []
      : ["current LicoUp, Lico Arc, and BadTower candidate bindings ready"]),
    ...(rustCryptoReportReady
      ? []
      : ["client Rust cryptography report ready"]),
    ...(rustCryptoReviewReady
      ? []
      : ["client Rust cryptographic review signatures ready"]),
    ...(platformCryptoReportReady
      ? []
      : ["client platform cryptography report ready"]),
    ...(androidPlatformCryptoReportReady
      ? []
      : ["Android platform cryptography acceptance report ready"])
  ];
  return {
    ready: remainingGates.length === 0,
    remainingGates,
    requiredRefs,
    releaseInputRedactionCoversClientRefs,
    stationAcceptanceContractReady,
    stationCandidateBindingsReady,
    stationCandidateInputsStable: stationCandidateInputsStable === true,
    stationAcceptanceFreshEndpointCount:
      stationAcceptanceCoverage.freshEndpointCount,
    stationAcceptancePositiveExchange:
      stationAcceptanceCoverage.positiveExchange,
    stationAcceptanceRoundTrip: stationAcceptanceCoverage.roundTrip,
    stationAcceptancePlaintextAbsent:
      stationAcceptanceCoverage.stationPlaintextAbsent,
    stationAcceptanceNonConformantEnvelopeRejected:
      stationAcceptanceCoverage.nonConformantEnvelopeRejected,
    stationAcceptanceTransportHintsNonAuthoritative:
      stationAcceptanceCoverage.transportHintsNonAuthoritative,
    stationAcceptanceExactFiveOuterFields:
      stationAcceptanceCoverage.exactFiveOuterFields,
    rustCryptoReportReady,
    rustCryptoNativeTestsReady,
    rustCryptoVectorCorpusReady,
    rustCryptoReviewReady,
    platformCryptoReportReady,
    androidPlatformCryptoReportReady,
    reportRefs: {
      stationAcceptance: stationAcceptanceReportPath,
      rustCrypto: rustCryptoReportPath,
      platformCrypto: platformCryptoReportPath,
      androidPlatformCrypto: androidPlatformCryptoReportPath
    }
  };
}
