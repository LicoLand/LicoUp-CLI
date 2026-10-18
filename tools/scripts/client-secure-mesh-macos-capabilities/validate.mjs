import { capabilityProofRef, schemaVersion, sha256Pattern, verifier } from "./constants.mjs";
import { capabilityProofDependencyReady } from "./capability-proof.mjs";
import { containsPrivateValue } from "./privacy.mjs";
import { text } from "./util.mjs";

export function validateReport(report) {
  const receipt = report.receipts?.[0];
  const claims = report.distributionClaims || {};
  const digests = [
    report.sourceStateDigest,
    report.closureChallengeDigest,
    report.invocationNonceDigest,
    report.buildManifestDigest,
    report.capabilityProofDigest,
    report.dependencies?.[0]?.digest,
    receipt?.artifactDigest,
    receipt?.runtimeExecutableDigest,
    receipt?.signatureMetadataDigest,
    receipt?.entitlementsDigest,
  ];
  return (
    report.schemaVersion === schemaVersion &&
    report.verifier === verifier &&
    Number.isFinite(Date.parse(text(report.generatedAt))) &&
    report.platform === "macos" &&
    report.redacted === true &&
    report.reportLeakScan === true &&
    report.rawRuntimeOutputIncluded === false &&
    report.rawPrivateMaterialIncluded === false &&
    report.nonBlockingDistributionGuidance?.blocking === false &&
    Array.isArray(report.dependencies) && report.dependencies.length === 1 &&
    capabilityProofDependencyReady(report.dependencies[0]) &&
    digests.every((digest) => sha256Pattern.test(text(digest))) &&
    receipt?.targetId === "macos-arm64" &&
    receipt?.artifactKind === "macos-app-bundle" &&
    receipt?.signatureKind === "local-identity-codesign" &&
    receipt?.platformLocalSignatureReady === true &&
    receipt?.hardenedRuntime === true &&
    receipt?.nestedCodeMinimalEntitlements === true &&
    receipt?.entitlementsMatch === true &&
    receipt?.installedArtifactMatched === true &&
    receipt?.installReceiptReady === true &&
    receipt?.nonBlockingDistributionGuidance?.blocking === false &&
    receipt?.launchReady === true &&
    receipt?.newProcessReady === true &&
    receipt?.startedAfterInvocation === true &&
    receipt?.executableWithinInstalledBundle === true &&
    receipt?.closureChallengeBound === true &&
    receipt?.invocationNonceBound === true &&
    receipt?.stableProcessWindowReady === true &&
    receipt?.postLaunchArtifactStable === true &&
    receipt?.smokeReady === true &&
    receipt?.capabilityProofReady === true &&
    !containsPrivateValue(report)
  );
}
