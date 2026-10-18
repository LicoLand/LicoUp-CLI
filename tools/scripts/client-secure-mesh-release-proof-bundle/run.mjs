import { randomUUID } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { createSecureClientMeshE2eeRefReportScope } from "../lib/secure-client-mesh-e2ee-ref-report.mjs";
import {
  CANONICAL_CLIENT_SOURCE_ROOTS,
  clientSourceStateDigest,
} from "../lib/client-source-state-digest.mjs";
import {
  captureLicoArcBadTowerCandidateBinding,
  licoArcBadTowerCandidateSnapshotsMatch,
} from "../lib/licoarc-badtower-candidate-binding.mjs";
import { atomicWriteReportJson, resolveSafeReportPath } from "../lib/safe-report-io.mjs";
import { parseReleaseProofArgs } from "./cli.mjs";
import {
  androidPhysicalInstallLaunchReportPath,
  androidPlatformCryptoReportPath,
  contract,
  evaluateSecureClientMeshEvidenceRefReportReadiness,
  physicalEvidenceManifestReportPath,
  physicalMatrixReportPath,
  platformCryptoReportPath,
  stationAcceptanceReportPath,
  repoRoot,
  reportPath,
  reportRedactionReportPath,
  rustCryptoReportPath,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  sourceChecks,
  updateReleaseReportPath,
  windowsImplementationReportPath,
} from "./config.mjs";
import { VERIFIER_REF } from "./constants.mjs";
import { summarizeContractReadiness } from "./contract-readiness.mjs";
import { summarizeReleaseInputFreshness } from "./freshness.mjs";
import { readJson, readJsonIfPresent } from "./io.mjs";
import { dedupeRemainingGates, stableStringList } from "./lists.mjs";
import { assertNoLeak } from "./privacy.mjs";
import { buildReleaseProofReport } from "./report.mjs";
import { runClientLicoArcCryptoInputsReadinessSelfTest } from "./self-test/client-licoarc-crypto.mjs";
import { runReleaseProofContractReadinessSelfTest } from "./self-test/contract.mjs";
import { runReleaseInputFreshnessSelfTest } from "./self-test/freshness.mjs";
import { runPhysicalEvidenceManifestReadinessSelfTest } from "./self-test/physical-evidence.mjs";
import { runReportRedactionFreshnessSelfTest } from "./self-test/redaction.mjs";
import { summarizeAndroidPhysicalInstallLaunchReport } from "./summarize/android-install.mjs";
import { summarizeClientLicoArcCryptoInputs } from "./summarize/client-licoarc-crypto.mjs";
import { summarizePhysicalEvidenceManifest } from "./summarize/physical-evidence.mjs";
import { summarizePhysicalMatrixReport } from "./summarize/physical-matrix.mjs";
import { summarizeReportRedactionProof } from "./summarize/redaction.mjs";
import { summarizeUpdateReport } from "./summarize/update.mjs";
import { summarizeWindowsImplementation } from "./summarize/windows.mjs";
import {
  evaluateSourceCheck,
  runPhysicalEvidenceManifestVerifier,
  runReportRedactionVerifier,
  runUpdateReleaseVerifier,
} from "./verifiers.mjs";

export async function runSecureMeshReleaseProofBundleCli(
  argv = process.argv.slice(2),
) {
  const args = parseReleaseProofArgs(argv);
  const blocker = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.find(
    (item) => item === "release proof bundle",
  );
  if (!blocker) {
    throw new Error(
      "Client-pinned Secure Client Mesh contract does not define release proof bundle blocker",
    );
  }
  if (args.clientLicoArcCryptoReadinessSelfTest) {
    const selfTest = runClientLicoArcCryptoInputsReadinessSelfTest();
    console.log(JSON.stringify(selfTest, null, 2));
    if (selfTest.ok !== true) {
      process.exitCode = 1;
    }
    return;
  }
  if (args.releaseProofContractReadinessSelfTest) {
    const selfTest = runReleaseProofContractReadinessSelfTest();
    console.log(JSON.stringify(selfTest, null, 2));
    if (selfTest.ok !== true) {
      process.exitCode = 1;
    }
    return;
  }

  const currentClientCandidateDigest = clientSourceStateDigest(
    repoRoot,
    CANONICAL_CLIENT_SOURCE_ROOTS,
  );
  const stationCandidateBefore =
    captureLicoArcBadTowerCandidateBinding({
      clientCandidateDigest: currentClientCandidateDigest,
    });
  const sourceResults = [];
  for (const check of sourceChecks) {
    sourceResults.push(await evaluateSourceCheck(check));
  }
  const updateReleaseVerifier = runUpdateReleaseVerifier();
  const physicalEvidenceManifestVerifier = runPhysicalEvidenceManifestVerifier();
  const releaseProofRedactionRunId =
    `secure-mesh-release-redaction:${randomUUID()}`;
  const reportRedactionVerifier = runReportRedactionVerifier(
    releaseProofRedactionRunId,
  );
  const updateReleaseReportRaw = updateReleaseVerifier.ok
    ? await readJson(updateReleaseReportPath)
    : {};
  const physicalMatrixReportRaw = await readJsonIfPresent(
    physicalMatrixReportPath,
  );
  const androidPhysicalInstallLaunchReportRaw = await readJsonIfPresent(
    androidPhysicalInstallLaunchReportPath,
  );
  const physicalEvidenceManifestReportRaw = await readJsonIfPresent(
    physicalEvidenceManifestReportPath,
  );
  const checkedAt = new Date().toISOString();
  const releaseInputFreshness = summarizeReleaseInputFreshness({
    updateRelease: updateReleaseReportRaw,
    physicalMatrix: physicalMatrixReportRaw,
    androidPhysicalInstallLaunch: androidPhysicalInstallLaunchReportRaw,
    physicalEvidenceManifest: physicalEvidenceManifestReportRaw,
  }, checkedAt);
  const releaseInputFreshnessSelfTest = runReleaseInputFreshnessSelfTest();
  const updateReleaseReport = updateReleaseVerifier.ok
    ? summarizeUpdateReport(updateReleaseReportRaw)
    : {};
  const physicalMatrixReport = summarizePhysicalMatrixReport(
    physicalMatrixReportRaw,
  );
  const androidPhysicalInstallLaunchReport =
    summarizeAndroidPhysicalInstallLaunchReport(
      androidPhysicalInstallLaunchReportRaw,
    );
  const physicalEvidenceManifest = summarizePhysicalEvidenceManifest(
    physicalEvidenceManifestReportRaw,
  );
  const physicalMatrixContractReadiness = summarizeContractReadiness(
    evaluateSecureClientMeshEvidenceRefReportReadiness(
      physicalMatrixReportRaw,
      "physical device matrix",
    ),
    "physical matrix contract readiness",
  );
  const physicalEvidenceManifestContractReadiness = summarizeContractReadiness(
    evaluateSecureClientMeshEvidenceRefReportReadiness(
      physicalEvidenceManifestReportRaw,
      "physical device matrix",
    ),
    "physical evidence manifest contract readiness",
  );
  const windowsImplementation = summarizeWindowsImplementation(
    await readJsonIfPresent(windowsImplementationReportPath),
  );
  const reportRedactionProof = await summarizeReportRedactionProof(
    await readJsonIfPresent(reportRedactionReportPath),
    releaseProofRedactionRunId,
  );
  const redactionFreshnessSelfTest = await runReportRedactionFreshnessSelfTest();
  const physicalEvidenceManifestReadinessSelfTest =
    runPhysicalEvidenceManifestReadinessSelfTest();
  const releaseProofContractReadinessSelfTest =
    runReleaseProofContractReadinessSelfTest();
  const clientLicoArcCryptoInputsReadinessSelfTest =
    runClientLicoArcCryptoInputsReadinessSelfTest();
  const stationCandidateAfter =
    captureLicoArcBadTowerCandidateBinding({
      clientCandidateDigest: currentClientCandidateDigest,
    });
  const stationCandidateInputsStable =
    licoArcBadTowerCandidateSnapshotsMatch(
      stationCandidateBefore,
      stationCandidateAfter,
    ) &&
    clientSourceStateDigest(repoRoot, CANONICAL_CLIENT_SOURCE_ROOTS) ===
      currentClientCandidateDigest;
  const clientLicoArcCryptoInputs = summarizeClientLicoArcCryptoInputs({
    stationAcceptance: await readJsonIfPresent(stationAcceptanceReportPath),
    rustCrypto: await readJsonIfPresent(rustCryptoReportPath),
    platformCrypto: await readJsonIfPresent(platformCryptoReportPath),
    androidPlatformCrypto: await readJsonIfPresent(
      androidPlatformCryptoReportPath,
    ),
    reportRedactionProof,
    stationCandidateBindings: stationCandidateAfter.bindings,
    stationCandidateInputsStable,
  });

  const ok = sourceResults.every((check) => check.ok) &&
    updateReleaseVerifier.ok &&
    physicalEvidenceManifestVerifier.ok &&
    reportRedactionVerifier.ok &&
    updateReleaseReport.ok === true &&
    physicalMatrixReport.inputIntegrityReady === true &&
    physicalEvidenceManifest.inputIntegrityReady === true &&
    reportRedactionProof.ready === true &&
    redactionFreshnessSelfTest.ok === true &&
    physicalEvidenceManifestReadinessSelfTest.ok === true &&
    releaseProofContractReadinessSelfTest.ok === true &&
    clientLicoArcCryptoInputsReadinessSelfTest.ok === true &&
    releaseInputFreshnessSelfTest.ok === true &&
    clientLicoArcCryptoInputs.ready === true &&
    clientLicoArcCryptoInputs.androidPlatformCryptoReportReady === true;
  const productionReady = false;
  const scopeEvidence = await createSecureClientMeshE2eeRefReportScope({
    contract,
    reportRef: reportPath,
    blocker,
    checkedAt,
  });
  const ubuntuLinuxPackageUpdateReady =
    physicalEvidenceManifest.ubuntuLinuxPackageUpdateReady === true;
  const windowsLocalImplementationReady =
    windowsImplementation.ready === true &&
    physicalEvidenceManifest.windowsLocalImplementationReady === true;
  const windowsNativeHostEvidenceReady =
    physicalEvidenceManifest.windowsNativeHostEvidenceReady === true;
  const remainingGates = dedupeRemainingGates([
    ...(
      windowsLocalImplementationReady
        ? []
        : [
          ubuntuLinuxPackageUpdateReady
            ? "Windows installer/package execution proof on declared production hosts"
            : "Windows and Linux installer/package execution proof on declared production hosts",
        ]
    ),
    ...(windowsNativeHostEvidenceReady
      ? []
      : ["Windows installer or portable replacement execution proof"]),
    ...(reportRedactionProof.ready === true
      ? []
      : ["redacted report leakage scan over release evidence inputs"]),
    ...(physicalMatrixReport.inputIntegrityReady === true
      ? []
      : ["physical device matrix v2 schema and producer integrity ready"]),
    ...(physicalEvidenceManifest.inputIntegrityReady === true
      ? []
      : ["physical evidence manifest v2 schema and producer integrity ready"]),
    ...(clientLicoArcCryptoInputs.ready === true
      ? []
      : clientLicoArcCryptoInputs.remainingGates),
    ...(releaseInputFreshness.ready === true
      ? []
      : releaseInputFreshness.remainingGates),
    ...(physicalMatrixContractReadiness.ready === true
      ? []
      : (physicalMatrixContractReadiness.remainingGates.length > 0
        ? physicalMatrixContractReadiness.remainingGates
        : ["physical device matrix contract evidence ready"])),
    ...(physicalEvidenceManifestContractReadiness.ready === true
      ? []
      : (physicalEvidenceManifestContractReadiness.remainingGates.length > 0
        ? physicalEvidenceManifestContractReadiness.remainingGates
        : ["physical evidence manifest contract evidence ready"])),
  ]);

  const report = buildReleaseProofReport({
    ok,
    blocker,
    checkedAt,
    scopeEvidence,
    sourceResults,
    updateReleaseVerifier,
    physicalEvidenceManifestVerifier,
    updateReleaseReport,
    physicalMatrixReport,
    physicalMatrixContractReadiness,
    physicalEvidenceManifest,
    physicalEvidenceManifestContractReadiness,
    windowsImplementation,
    reportRedactionVerifier,
    reportRedactionProof,
    redactionFreshnessSelfTest,
    physicalEvidenceManifestReadinessSelfTest,
    releaseProofContractReadinessSelfTest,
    clientLicoArcCryptoInputsReadinessSelfTest,
    releaseInputFreshness,
    releaseInputFreshnessSelfTest,
    clientLicoArcCryptoInputs,
    androidPhysicalInstallLaunchReport,
    remainingGates,
    productionReady,
  });

  assertNoLeak(report, "secure mesh release proof bundle report");
  const safeReportPath = resolveSafeReportPath(repoRoot, reportPath);
  await fs.mkdir(path.dirname(safeReportPath), { recursive: true });
  atomicWriteReportJson(repoRoot, reportPath, report);

  console.log(JSON.stringify({
    ok,
    report: reportPath,
    sourceOfTruth: report.sourceOfTruth,
    blocker: report.blocker,
    diagnosticStatus: report.diagnosticStatus,
    productionReady,
    sourceCheckCount: sourceResults.length,
    updateReleaseVerifierPassed: updateReleaseVerifier.ok,
    physicalEvidenceManifestContractReadinessReady:
      physicalEvidenceManifestContractReadiness.ready === true,
    physicalEvidenceManifestLocalReadyDiagnostic:
      physicalEvidenceManifest.localReadyDiagnostic === true,
    physicalEvidenceManifestLocalReleaseEvidenceReadyDiagnostic:
      physicalEvidenceManifest.localReleaseEvidenceReadyDiagnostic === true,
    releaseInputFreshnessReady: releaseInputFreshness.ready === true,
    releaseInputFreshnessStaleOrInvalidCount:
      releaseInputFreshness.staleOrInvalidCount,
    physicalMatrixLinked: physicalMatrixReport.ok === true,
    physicalMatrixContractReadinessReady:
      physicalMatrixContractReadiness.ready === true,
    physicalMatrixLocalPhysicalEvidenceChainReadyDiagnostic:
      physicalMatrixReport.localPhysicalEvidenceChainReadyDiagnostic === true,
    androidPhysicalInstallLaunchLocalReadyDiagnostic:
      androidPhysicalInstallLaunchReport.localReadyDiagnostic === true,
    physicalMatrixPartialScenarioCount: physicalMatrixReport.partialScenarioCount,
    physicalMatrixAndroidPlatformSecretStoreReady:
      physicalMatrixReport.androidPlatformSecretStoreReady === true,
    physicalMatrixAndroidMissingFields:
      stableStringList(physicalMatrixReport.androidPhysicalMissingFields),
    physicalMatrixAndroidMissingFieldCount:
      Number(physicalMatrixReport.androidPhysicalMissingFieldCount || 0),
    physicalMatrixAndroidWeakProofFieldsAbsent:
      physicalMatrixReport.androidPhysicalWeakProofFieldsAbsent === true,
    physicalMatrixAndroidWeakProofFields:
      stableStringList(physicalMatrixReport.androidPhysicalWeakProofFields),
    physicalMatrixAndroidWeakProofFieldCount:
      Number(physicalMatrixReport.androidPhysicalWeakProofFieldCount || 0),
    physicalEvidenceManifestAndroidMissingFields:
      stableStringList(physicalEvidenceManifest.androidPhysicalMissingFields),
    physicalEvidenceManifestAndroidMissingFieldCount:
      Number(physicalEvidenceManifest.androidPhysicalMissingFieldCount || 0),
    physicalEvidenceManifestAndroidWeakProofFields:
      stableStringList(physicalEvidenceManifest.androidPhysicalWeakProofFields),
    physicalEvidenceManifestAndroidWeakProofFieldCount:
      Number(physicalEvidenceManifest.androidPhysicalWeakProofFieldCount || 0),
    physicalMatrixIosPlatformSecretStoreReady:
      physicalMatrixReport.iosPlatformSecretStoreReady === true,
    physicalMatrixIosUserPresencePolicyReady:
      physicalMatrixReport.iosUserPresencePolicyReady === true,
    physicalEvidenceManifestIosUserPresencePolicyReady:
      physicalEvidenceManifest.iosUserPresencePolicyReady === true,
    reportRedactionReady: reportRedactionProof.ready === true,
    clientLicoArcCryptoInputsReady: clientLicoArcCryptoInputs.ready === true,
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
    rustCryptoReportReady: clientLicoArcCryptoInputs.rustCryptoReportReady === true,
    rustCryptoReviewReady: clientLicoArcCryptoInputs.rustCryptoReviewReady === true,
    platformCryptoReportReady:
      clientLicoArcCryptoInputs.platformCryptoReportReady === true,
    androidPlatformCryptoReportReady:
      clientLicoArcCryptoInputs.androidPlatformCryptoReportReady === true,
    windowsLocalImplementationReady,
    windowsNativeHostEvidenceReady,
    macosActualReleaseBundleVerified:
      updateReleaseReport.macosActualReleaseBundleVerified === true,
    releaseTargetCount: updateReleaseReport.targetCount || 0,
    remainingGateCount: report.summary.remainingGates.length,
    verifier: VERIFIER_REF,
  }, null, 2));

  if (!ok || (args.strict && productionReady !== true)) {
    process.exitCode = 1;
  }
}
