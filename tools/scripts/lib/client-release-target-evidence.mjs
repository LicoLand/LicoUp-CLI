const SHA256 = /^sha256:[a-f0-9]{64}$/u;

import {
  ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT,
} from "./secure-mesh-physical-report-coverage.mjs";

function text(value) {
  return String(value || "").trim();
}

export function releaseCliTargetEvidenceReady(
  report,
  { platform, sourceStateDigest, runtimeExecutableDigest },
) {
  const summary = report?.summary || {};
  return report?.schemaVersion ===
      "licomesh.secure-mesh.release-cli-proof-report.v1" &&
    report?.verifier === "tools/scripts/client-secure-mesh-release-cli-proof.mjs" &&
    report?.ok === true && report?.platform === platform &&
    report?.artifactKind === "release-cli-binary" &&
    report?.sourceStateDigest === sourceStateDigest &&
    SHA256.test(text(runtimeExecutableDigest)) &&
    report?.cliArtifactDigest === runtimeExecutableDigest &&
    summary.releaseCliProofReady === true && summary.statusReady === true &&
    summary.commandExecuteReady === true && summary.commandReplayRejected === true &&
    summary.filePolicyReady === true && summary.fileRouteReady === true &&
    summary.fileReceiveDestinationReady === true &&
    summary.fileReceiveConfirmationReady === true &&
    summary.trustPolicyReady === true && report?.redacted === true &&
    report?.rawPrivateMaterialIncluded === false &&
    report?.rawPlaintextIncluded === false &&
    report?.rawPublicWireBytesIncluded === false;
}

export function androidPlatformCryptoEvidenceReady(report) {
  const summary = report?.summary || {};
  return report?.schemaVersion ===
      "licomesh.secure-mesh.android-platform-crypto-acceptance.v1" &&
    report?.verifier === "tools/scripts/client-android-native-tests.mjs" &&
    report?.ok === true && report?.platform === "android" &&
    summary.ok === true &&
    summary.platformCryptoAcceptanceReady === true &&
    summary.platformCustodyContractReady === true &&
    summary.platformAuthorizationContractReady === true &&
    summary.rustFfiActionContractReady === true &&
    summary.mlsMemberRemoveReleaseActionReady === true &&
    summary.unknownReleaseActionsFailClosed === true &&
    summary.nativeTestClassCount ===
      ANDROID_PLATFORM_CRYPTO_NATIVE_TEST_CLASS_COUNT &&
    summary.privatePathsIncluded === false &&
    report?.redacted === true && report?.rawPrivateMaterialIncluded === false &&
    report?.rawPlaintextIncluded === false &&
    report?.rawPublicWireBytesIncluded === false;
}

export function requireReleaseCliTargetEvidence(report, expected) {
  if (!releaseCliTargetEvidenceReady(report, expected)) {
    throw new Error("release CLI target evidence contract is incomplete");
  }
  return true;
}

export function requireAndroidPlatformCryptoEvidence(report) {
  if (!androidPlatformCryptoEvidenceReady(report)) {
    throw new Error("Android platform cryptography evidence contract is incomplete");
  }
  return true;
}
