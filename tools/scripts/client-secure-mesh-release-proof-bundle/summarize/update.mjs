import { consumerVerifiedReleaseArtifacts } from "../contract-readiness.mjs";
import { reportRecord } from "../lists.mjs";

export function summarizeProductionClosureStatus(status = {}) {
  const installerExecutionStatus = status.installerExecutionStatus || {};
  const androidProductionUpdateStatus = status.androidProductionUpdateStatus || {};
  return {
    present: Boolean(status && Object.keys(status).length > 0),
    rawProductionKeyMaterialIncluded: status.rawProductionKeyMaterialIncluded === true,
    productionInstallerExecutionReady: status.productionInstallerExecutionReady === true,
    dryRunPlansCoverTargetLabels: installerExecutionStatus.dryRunPlansCoverTargetLabels === true,
    productionHostExecutionReady: installerExecutionStatus.productionHostExecutionReady === true,
    dryRunPlanCount: Number(installerExecutionStatus.dryRunPlanCount || 0),
    productionTargetCount: Number(installerExecutionStatus.productionTargetCount || 0),
    androidPhysicalInstallLaunchReady: androidProductionUpdateStatus.physicalInstallLaunchReady === true
  };
}

export function summarizeUpdateReport(report = {}) {
  report = reportRecord(report);
  const positiveChecks = Array.isArray(report.positiveChecks) ? report.positiveChecks : [];
  const negativeChecks = Array.isArray(report.negativeChecks) ? report.negativeChecks : [];
  const macosReleaseBundleEvidence = summarizeMacosReleaseBundleEvidence(report.macosReleaseBundleEvidence);
  const productionClosureStatus = summarizeProductionClosureStatus(report.productionClosureStatus);
  return {
    ok: report.ok === true,
    productionReady: report.productionReady === true,
    dryRun: report.dryRun === true,
    targetCount: Array.isArray(report.productionTargetLabels) ? report.productionTargetLabels.length : 0,
    productionTargetLabels: (Array.isArray(report.productionTargetLabels) ? report.productionTargetLabels : [])
      .map((item) => String(item || "").trim())
      .filter(Boolean),
    productionArtifacts: report.dryRun === true
      ? []
      : consumerVerifiedReleaseArtifacts(report),
    productionInstallerExecutionReady: productionClosureStatus.productionInstallerExecutionReady === true,
    signedRevocationVerified: positiveChecks.some((item) => item?.name === "signed revocation list verifies" && item.ok === true),
    macosActualReleaseBundleVerified: macosReleaseBundleEvidence.localBundleShapeVerified === true,
    macosReleaseBundleEvidence,
    productionClosureStatus,
    downgradeRejected: negativeChecks.some((item) => item?.name === "downgrade is rejected without signed policy allowance" && item.ok === true),
    tamperRejected: negativeChecks.some((item) => item?.name === "tampered manifest signature is rejected" && item.ok === true),
    unsupportedPlatformRejected: negativeChecks.some((item) => item?.name === "unsupported platform is rejected" && item.ok === true)
  };
}

export function summarizeMacosReleaseBundleEvidence(evidence = {}) {
  evidence = reportRecord(evidence);
  const artifacts = Array.isArray(evidence.artifacts) ? evidence.artifacts : [];
  return {
    present: Boolean(evidence && Object.keys(evidence).length > 0),
    attempted: evidence.attempted === true,
    ok: evidence.ok === true,
    localBundleShapeVerified: evidence.ok === true &&
      evidence.dryRun === false &&
      evidence.artifactKind === "actual-release-bundle" &&
      evidence.signingKind === "local-ad-hoc-codesign" &&
      evidence.verificationExitCode === 0 &&
      evidence.codesignVerifyExitCode === 0 &&
      artifacts.length >= 2 &&
      artifacts.every((artifact) =>
        artifact?.platform === "macos" &&
        artifact?.mode === "release" &&
        artifact?.signingKind === "local-ad-hoc-codesign" &&
        artifact?.productionEntitlementsRequested === true &&
        artifact?.entitlementProfile === "production-release" &&
        artifact?.entitlementsFile === "apps/desktop/macos/Runner/ProductionRelease.entitlements" &&
        Number(artifact?.flutterExecutableBytes || 0) > 0 &&
        Number(artifact?.licoClientBytes || 0) > 0
      ),
    status: String(evidence.status || ""),
    artifactKind: String(evidence.artifactKind || ""),
    signingKind: String(evidence.signingKind || ""),
    gatekeeperVerified: evidence.gatekeeperVerified === true,
    productionEntitlementsRequested:
      artifacts.every((artifact) => artifact?.productionEntitlementsRequested === true),
    productionEntitlementProfileReady:
      artifacts.every((artifact) => artifact?.entitlementProfile === "production-release"),
    productionEntitlementsFileReady:
      artifacts.every((artifact) => artifact?.entitlementsFile === "apps/desktop/macos/Runner/ProductionRelease.entitlements"),
    artifactCount: artifacts.length,
    artifactKinds: artifacts.map((artifact) => String(artifact?.kind || "")).filter(Boolean),
    remainingProductionProofCount: Array.isArray(evidence.remainingProductionProofs)
      ? evidence.remainingProductionProofs.length
      : 0
  };
}
