import path from "node:path";
import process from "node:process";
import { clientSourceStateDigest } from "../lib/client-source-state-digest.mjs";
import {
  resolveContainedExistingPath,
  sha256Buffer,
  sha256File,
  stableReadFileSnapshot,
} from "../lib/client-release-artifact-digest.mjs";
import { inspectBoundedMacosCodePolicy } from "../lib/macos-bundle-integrity.mjs";
import {
  createReleaseClosureChallenge,
  createReleaseInvocationNonce,
  releaseClosureChallengeDigest,
  releaseInvocationNonceDigest,
  requiredReleaseClosureChallenge,
  requiredReleaseClosureStartedAt,
  requiredReleaseInvocationNonce,
} from "../lib/release-closure-challenge.mjs";
import {
  atomicWriteReportJson,
  removeContainedReportIfExists,
} from "../lib/safe-report-io.mjs";
import {
  artifactSecurityState,
  artifactSecurityStateStable,
  nestedCodePolicyReady,
} from "./artifact-security.mjs";
import {
  capabilityProofDependencyStable,
  materializeCapabilityProof,
} from "./capability-proof.mjs";
import {
  builtApp,
  clientVersionPath,
  installedApp,
  packageManifestPath,
  releaseEntitlementsPath,
  releaseEntitlementsRef,
  reportRef,
  repoRoot,
  schemaVersion,
  sourceRoots,
  verifier,
} from "./constants.mjs";
import { launchInstalledApp, plistValue } from "./process.mjs";
import { sidecarSmoke } from "./sidecar.mjs";
import { selfTest } from "./self-test.mjs";
import {
  canonicalJson,
  readJsonStable,
  requireValue,
  text,
} from "./util.mjs";
import { validateReport } from "./validate.mjs";

export function main() {
  requireValue(
    process.platform === "darwin" && process.arch === "arm64",
    "macos_arm64_platform_required",
  );
  removeContainedReportIfExists(repoRoot, reportRef);
  const inheritedClosure = text(process.env.LICO_CLIENT_RELEASE_CLOSURE_CHALLENGE);
  const challenge = inheritedClosure
    ? requiredReleaseClosureChallenge()
    : createReleaseClosureChallenge();
  const invocationNonce = inheritedClosure
    ? requiredReleaseInvocationNonce()
    : createReleaseInvocationNonce();
  const closureStartedAt = inheritedClosure
    ? requiredReleaseClosureStartedAt()
    : { value: new Date().toISOString(), milliseconds: Date.now() };
  const closureChallengeDigest = releaseClosureChallengeDigest(challenge);
  const invocationNonceDigest = releaseInvocationNonceDigest(invocationNonce);
  const built = resolveContainedExistingPath(repoRoot, builtApp, {
    expectedKind: "directory",
  });
  const installed = resolveContainedExistingPath("/Applications", installedApp, {
    expectedKind: "directory",
  });
  const manifestPath = resolveContainedExistingPath(repoRoot, packageManifestPath, {
    expectedKind: "file",
  });
  const entitlementsPath = resolveContainedExistingPath(repoRoot, releaseEntitlementsPath, {
    expectedKind: "file",
  });
  const packageManifestSnapshot = stableReadFileSnapshot(manifestPath, {
    maxBytes: 2 * 1024 * 1024,
  });
  const packageManifest = JSON.parse(packageManifestSnapshot.bytes.toString("utf8"));
  requireValue(
    packageManifest?.platform === "macos" &&
      packageManifest?.mode === "release" &&
      packageManifest?.signing?.signingKind === "local-identity-codesign" &&
      packageManifest?.signing?.localInstallIdentity === true &&
      packageManifest?.signing?.entitlementsFile === releaseEntitlementsRef &&
      packageManifest?.signing?.entitlementProfile === "release" &&
      packageManifest?.signing?.productionEntitlementsRequested === false &&
      packageManifest?.signing?.nonBlockingDistributionGuidance?.blocking === false &&
      packageManifest?.signing?.hardenedRuntime === true &&
      packageManifest?.signing?.nestedCodeMinimalEntitlements === true,
    "macos_package_manifest_policy_mismatch",
  );
  const currentSourceStateDigest = clientSourceStateDigest(repoRoot, sourceRoots);
  requireValue(packageManifest.sourceStateDigest === currentSourceStateDigest,
    "macos_artifact_source_state_stale");
  const clientVersion = readJsonStable(clientVersionPath);
  requireValue(text(clientVersion.productVersion) &&
    Number.isInteger(clientVersion.buildNumber) && clientVersion.buildNumber > 0,
  "client_version_manifest_invalid");
  const signatureResources = resolveContainedExistingPath(
    built,
    path.join(built, "Contents/_CodeSignature/CodeResources"),
    { expectedKind: "file" },
  );
  const executableName = plistValue(installed, "CFBundleExecutable");
  const initialInspectionDeadlineMs = Date.now() + 240_000;
  const builtPolicy = inspectBoundedMacosCodePolicy(
    built,
    executableName,
    entitlementsPath,
    { deadlineMs: initialInspectionDeadlineMs },
  );
  const installedPolicy = inspectBoundedMacosCodePolicy(
    installed,
    executableName,
    entitlementsPath,
    { deadlineMs: initialInspectionDeadlineMs },
  );
  const builtDigest = builtPolicy.artifactDigest;
  const installedDigest = installedPolicy.artifactDigest;
  const nestedCodeMinimalEntitlements =
    builtPolicy.nestedCodePaths.length === installedPolicy.nestedCodePaths.length &&
    nestedCodePolicyReady(builtPolicy) && nestedCodePolicyReady(installedPolicy);
  const builtSignature = builtPolicy.signature;
  const installedSignature = installedPolicy.signature;
  const signaturesMatch = builtSignature.signatureKind === installedSignature.signatureKind &&
    builtSignature.entitlementsDigest === installedSignature.entitlementsDigest;
  const signatureKind = signaturesMatch ? builtSignature.signatureKind : "unknown";
  const entitlementsMatch = builtSignature.entitlementsMatch === true &&
    installedSignature.entitlementsMatch === true && signaturesMatch;
  const platformLocalSignatureReady = builtSignature.verified === true &&
    installedSignature.verified === true &&
    builtSignature.hardenedRuntime === true &&
    installedSignature.hardenedRuntime === true &&
    signatureKind === "local-identity-codesign" && entitlementsMatch &&
    nestedCodeMinimalEntitlements;
  const installedArtifactMatched = builtDigest === installedDigest;
  const installedExecutable = resolveContainedExistingPath(
    installed,
    path.join(installed, "Contents/MacOS", executableName),
    { expectedKind: "file" },
  );
  const launch = launchInstalledApp({
    executablePath: installedExecutable,
    challenge,
    invocationNonce,
    closureStartedAtMs: closureStartedAt.milliseconds,
  });
  const smokeReady = sidecarSmoke(installed);
  const capabilityProof = materializeCapabilityProof();
  const postInspectionDeadlineMs = Date.now() + 240_000;
  const builtPolicyAfter = inspectBoundedMacosCodePolicy(
    built,
    executableName,
    entitlementsPath,
    { deadlineMs: postInspectionDeadlineMs },
  );
  const installedPolicyAfter = inspectBoundedMacosCodePolicy(
    installed,
    executableName,
    entitlementsPath,
    { deadlineMs: postInspectionDeadlineMs },
  );
  const builtDigestAfter = builtPolicyAfter.artifactDigest;
  const installedDigestAfter = installedPolicyAfter.artifactDigest;
  const builtSignatureAfter = builtPolicyAfter.signature;
  const installedSignatureAfter = installedPolicyAfter.signature;
  const builtNestedPolicyAfter =
    builtPolicyAfter.nestedCodePaths.length === builtPolicy.nestedCodePaths.length &&
    nestedCodePolicyReady(builtPolicyAfter);
  const installedNestedPolicyAfter =
    installedPolicyAfter.nestedCodePaths.length === installedPolicy.nestedCodePaths.length &&
    nestedCodePolicyReady(installedPolicyAfter);
  const postLaunchArtifactStable =
    artifactSecurityStateStable(
      artifactSecurityState(builtDigest, builtSignature, nestedCodeMinimalEntitlements),
      artifactSecurityState(builtDigestAfter, builtSignatureAfter, builtNestedPolicyAfter),
    ) &&
    artifactSecurityStateStable(
      artifactSecurityState(installedDigest, installedSignature, nestedCodeMinimalEntitlements),
      artifactSecurityState(
        installedDigestAfter,
        installedSignatureAfter,
        installedNestedPolicyAfter,
      ),
    ) && builtDigestAfter === installedDigestAfter;
  requireValue(postLaunchArtifactStable, "macos_artifact_changed_during_launch");
  requireValue(clientSourceStateDigest(repoRoot, sourceRoots) === currentSourceStateDigest,
    "macos_source_changed_during_receipt");
  requireValue(sha256File(manifestPath) === sha256Buffer(packageManifestSnapshot.bytes),
    "macos_package_manifest_changed_during_receipt");
  requireValue(capabilityProofDependencyStable(capabilityProof.dependency),
    "macos_capability_child_proof_changed_during_receipt");
  const appVersion = plistValue(installed, "CFBundleShortVersionString");
  const appBuildNumber = plistValue(installed, "CFBundleVersion");
  const expectedAppVersion = text(clientVersion.productVersion).split("-", 1)[0];
  requireValue(appVersion === expectedAppVersion &&
    appBuildNumber === String(clientVersion.buildNumber),
  "macos_installed_version_mismatch");
  const signatureMetadataDigest = sha256Buffer(Buffer.from(canonicalJson({
    signingKind: signatureKind,
    entitlementProfile: packageManifest.signing.entitlementProfile,
    entitlementsDigest: installedSignature.entitlementsDigest,
    codeResourcesDigest: sha256File(signatureResources),
  }), "utf8"));
  const receipt = {
    targetId: "macos-arm64",
    productVersion: text(clientVersion.productVersion),
    buildNumber: clientVersion.buildNumber,
    appVersion,
    appBuildNumber,
    artifactKind: "macos-app-bundle",
    artifactDigest: builtDigest,
    runtimeExecutableDigest: sha256File(resolveContainedExistingPath(
      installed,
      path.join(installed, "Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli"),
      { expectedKind: "file" },
    ), { maxBytes: 512 * 1024 * 1024 }),
    signatureMetadataDigest,
    signatureKind,
    platformLocalSignatureReady,
    hardenedRuntime: builtSignature.hardenedRuntime === true &&
      installedSignature.hardenedRuntime === true,
    nestedCodeMinimalEntitlements,
    entitlementsMatch,
    entitlementsDigest: installedSignature.entitlementsDigest,
    installedArtifactMatched,
    installReceiptReady: platformLocalSignatureReady && installedArtifactMatched,
    nonBlockingDistributionGuidance: {
      blocking: false,
      storeListingStatus: "not-configured",
      platformSigningStatus: "not-configured",
      notarizationStatus: "not-configured",
      updateChannelStatus: "not-configured",
    },
    launchReady: Object.values(launch).every((value) => value === true),
    ...launch,
    postLaunchArtifactStable,
    smokeReady,
    capabilityProofReady: capabilityProof.report.ok === true,
  };
  const report = {
    schemaVersion,
    verifier,
    generatedAt: new Date().toISOString(),
    platform: "macos",
    redacted: true,
    reportLeakScan: true,
    rawRuntimeOutputIncluded: false,
    rawPrivateMaterialIncluded: false,
    sourceStateDigest: currentSourceStateDigest,
    closureChallengeDigest,
    invocationNonceDigest,
    buildManifestDigest: sha256Buffer(packageManifestSnapshot.bytes),
    capabilityProofDigest: capabilityProof.digest,
    dependencies: [capabilityProof.dependency],
    nonBlockingDistributionGuidance: {
      blocking: false,
      storeListingStatus: "not-configured",
      platformSigningStatus: "not-configured",
      notarizationStatus: "not-configured",
      updateChannelStatus: "not-configured",
    },
    receipts: [receipt],
  };
  report.ok = validateReport(report);
  atomicWriteReportJson(repoRoot, reportRef, report);
  console.log(JSON.stringify({
    ok: report.ok,
    platform: report.platform,
    signatureKind: receipt.signatureKind,
    platformLocalSignatureReady: receipt.platformLocalSignatureReady,
    installReceiptReady: receipt.installReceiptReady,
    launchReady: receipt.launchReady,
    smokeReady: receipt.smokeReady,
    capabilityProofReady: receipt.capabilityProofReady,
  }));
  if (!report.ok) process.exitCode = 1;
}



export async function runCli(argv = process.argv.slice(2)) {
  try {
    if (argv.includes("--self-test")) selfTest();
    else main();
  } catch {
    console.error(JSON.stringify({ ok: false, error: "macos_receipt_failed" }));
    process.exitCode = 1;
  }
}
