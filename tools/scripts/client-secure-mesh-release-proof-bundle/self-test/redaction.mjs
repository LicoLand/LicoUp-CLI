import { sha256FileIfPresent } from "../io.mjs";
import { summarizeReportRedactionProof } from "../summarize/redaction.mjs";

export async function runReportRedactionFreshnessSelfTest() {
  const fixtureRef = "tools/scripts/client-secure-mesh-release-proof-bundle.mjs";
  const fixtureDigest = await sha256FileIfPresent(fixtureRef);
  const runId = "self-test-redaction-run";
  const baseReport = {
    ok: true,
    redacted: true,
    diagnosticStatus: "passed",
    redactionRunId: runId,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawLocalPathIncluded: false,
    rawIdentityMaterialIncluded: false,
    scannedRefs: [fixtureRef],
    scannedRefDigests: [{ ref: fixtureRef, sha256: fixtureDigest }],
    summary: {
      reportRedactionReady: true,
      selfTestReady: true,
      scannedReportCount: 1,
      scannedRefDigestCount: 1,
      hitCount: 0,
      releaseProofInputsOnly: true
    }
  };
  const good = await summarizeReportRedactionProof(baseReport, runId);
  const duplicateDigest = await summarizeReportRedactionProof({
    ...baseReport,
    scannedRefDigests: [
      { ref: fixtureRef, sha256: fixtureDigest },
      { ref: fixtureRef, sha256: fixtureDigest }
    ],
    summary: {
      ...baseReport.summary,
      scannedRefDigestCount: 2
    }
  }, runId);
  const extraDigest = await summarizeReportRedactionProof({
    ...baseReport,
    scannedRefDigests: [
      { ref: fixtureRef, sha256: fixtureDigest },
      { ref: "build/reports/self-test-extra-report.json", sha256: fixtureDigest }
    ],
    summary: {
      ...baseReport.summary,
      scannedRefDigestCount: 2
    }
  }, runId);
  const staleDigest = await summarizeReportRedactionProof({
    ...baseReport,
    scannedRefDigests: [{ ref: fixtureRef, sha256: "sha256:self-test-stale-digest" }]
  }, runId);
  const runIdMismatch = await summarizeReportRedactionProof(baseReport, "self-test-other-run");
  const ok = good.ready === true &&
    duplicateDigest.ready === false &&
    duplicateDigest.digestManifestExact === false &&
    extraDigest.ready === false &&
    extraDigest.digestManifestExact === false &&
    staleDigest.ready === false &&
    staleDigest.scannedRefDigestsCurrent === false &&
    runIdMismatch.ready === false &&
    runIdMismatch.redactionRunIdMatched === false;
  return {
    ok,
    positiveAccepted: good.ready === true,
    duplicateDigestRejected: duplicateDigest.ready === false,
    extraDigestRejected: extraDigest.ready === false,
    staleDigestRejected: staleDigest.ready === false,
    runIdMismatchRejected: runIdMismatch.ready === false
  };
}
