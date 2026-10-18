import { reportRedactionReportPath } from "../config.mjs";
import { sha256FileIfPresent } from "../io.mjs";

export async function summarizeReportRedactionProof(report = {}, expectedRedactionRunId = "") {
  const summary = report?.summary || {};
  const present = Boolean(report && Object.keys(report).length > 0);
  const scannedRefs = Array.isArray(report?.scannedRefs)
    ? report.scannedRefs.map((item) => String(item || "")).filter(Boolean)
    : [];
  const scannedRefDigestEntries = Array.isArray(report?.scannedRefDigests)
    ? report.scannedRefDigests
        .map((entry) => ({
          ref: String(entry?.ref || ""),
          sha256: String(entry?.sha256 || "")
        }))
        .filter((entry) => entry.ref && entry.sha256)
    : [];
  const digestEntriesByRef = new Map();
  const duplicateDigestRefs = [];
  for (const entry of scannedRefDigestEntries) {
    if (digestEntriesByRef.has(entry.ref)) {
      duplicateDigestRefs.push(entry.ref);
      continue;
    }
    digestEntriesByRef.set(entry.ref, entry.sha256);
  }
  const scannedRefSet = new Set(scannedRefs);
  const extraDigestRefs = scannedRefDigestEntries
    .map((entry) => entry.ref)
    .filter((ref) => !scannedRefSet.has(ref));
  const staleOrMissingDigestRefs = [];
  for (const ref of scannedRefs) {
    const expectedDigest = digestEntriesByRef.get(ref) || "";
    const actualDigest = await sha256FileIfPresent(ref);
    if (!expectedDigest || !actualDigest || expectedDigest !== actualDigest) {
      staleOrMissingDigestRefs.push(ref);
    }
  }
  const digestManifestExact = scannedRefs.length > 0 &&
    scannedRefs.length === scannedRefSet.size &&
    scannedRefDigestEntries.length === scannedRefs.length &&
    Number(summary.scannedRefDigestCount || 0) === scannedRefDigestEntries.length &&
    duplicateDigestRefs.length === 0 &&
    extraDigestRefs.length === 0;
  const scannedRefDigestsCurrent = scannedRefs.length > 0 &&
    digestManifestExact &&
    staleOrMissingDigestRefs.length === 0;
  const redactionRunIdMatched = String(report?.redactionRunId || "") === expectedRedactionRunId &&
    expectedRedactionRunId.length > 0;
  const ready = report?.ok === true &&
    report?.redacted === true &&
    report?.diagnosticStatus === "passed" &&
    summary.reportRedactionReady === true &&
    summary.selfTestReady === true &&
    Number(summary.scannedReportCount || 0) > 0 &&
    scannedRefDigestsCurrent &&
    redactionRunIdMatched &&
    Number(summary.hitCount || 0) === 0 &&
    summary.releaseProofInputsOnly === true &&
    report?.rawPrivateMaterialIncluded !== true &&
    report?.rawPlaintextIncluded !== true &&
    report?.rawLocalPathIncluded !== true &&
    report?.rawIdentityMaterialIncluded !== true;
  return {
    report: reportRedactionReportPath,
    present,
    ok: report?.ok === true,
    redacted: report?.redacted === true,
    diagnosticStatus: String(report?.diagnosticStatus || ""),
    reportRedactionReady: summary.reportRedactionReady === true,
    selfTestReady: summary.selfTestReady === true,
    scannedRefs,
    scannedRefDigestCount: scannedRefDigestEntries.length,
    digestManifestExact,
    scannedRefDigestsCurrent,
    duplicateDigestRefs,
    extraDigestRefs,
    staleOrMissingDigestRefs,
    redactionRunIdMatched,
    scannedReportCount: Number(summary.scannedReportCount || 0),
    missingReportCount: Number(summary.missingReportCount || 0),
    hitCount: Number(summary.hitCount || 0),
    releaseProofInputsOnly: summary.releaseProofInputsOnly === true,
    ready
  };
}
