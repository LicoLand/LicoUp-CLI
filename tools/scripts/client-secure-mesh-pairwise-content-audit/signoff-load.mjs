import { verifyPairwiseReviewSignoff } from "../lib/review-signoff-verifier.mjs";
import { assertNoLeak, sanitizeError } from "./privacy.mjs";
import { safeReportRef } from "./signoff.mjs";

export function createSignoffLoaders({
  readText,
  reviewAuthorities,
  sourceOfTruth,
}) {
async function loadVectorCorpusSnapshot(relativePath) {
  const ref = safeReportRef(relativePath);
  if (!ref) {
    return {
      report: "",
      present: false,
      ok: false,
      corpusDigest: "",
      recomputedCorpusDigestMatched: false,
      failureSummary: "vector corpus snapshot ref is missing or unsafe"
    };
  }
  try {
    const payload = JSON.parse(await readText(ref));
    assertNoLeak(payload, "secure mesh pairwise/content vector corpus snapshot");
    const recomputedCorpusDigest = sha256Json({
      schemaVersion: payload.schemaVersion,
      entries: Array.isArray(payload.entries) ? payload.entries : [],
      reviewGate: payload.reviewGate || {}
    });
    const ok = payload.schemaVersion === "licomesh.secure-mesh.pairwise-content-vector-corpus.v1" &&
      payload.redacted === true &&
      payload.rawPrivateMaterialIncluded === false &&
      payload.rawPlaintextIncluded === false &&
      payload.rawPublicWireBytesIncluded === false &&
      payload.signoffBinding?.corpusDigest === payload.corpusDigest &&
      recomputedCorpusDigest === payload.corpusDigest;
    return {
      report: ref,
      present: true,
      ok,
      schemaVersion: String(payload.schemaVersion || ""),
      redacted: payload.redacted === true,
      corpusDigest: String(payload.corpusDigest || ""),
      signoffBindingCorpusDigest: String(payload.signoffBinding?.corpusDigest || ""),
      recomputedCorpusDigest,
      recomputedCorpusDigestMatched: recomputedCorpusDigest === payload.corpusDigest,
      failureSummary: ok ? "" : "vector corpus snapshot is missing required digest-bound fields"
    };
  } catch (error) {
    return {
      report: ref,
      present: false,
      ok: false,
      corpusDigest: "",
      recomputedCorpusDigestMatched: false,
      failureSummary: sanitizeError(error)
    };
  }
}

async function loadReviewSignoffArtifact(relativePath, corpus) {
  const binding = corpus.signoffBinding;
  try {
    const payload = JSON.parse(await readText(relativePath));
    const vectorCorpusSnapshot = await loadVectorCorpusSnapshot(payload?.vectorCorpusSnapshotReport);
    const templateSnapshotDigestMatched =
      vectorCorpusSnapshot.ok === true &&
      payload?.corpusDigest === vectorCorpusSnapshot.corpusDigest;
    const templatePresent =
      payload?.templateSchemaVersion === "licomesh.secure-mesh.pairwise-content-review-signoff-template.v2";
    const templateCompleted =
      payload?.independentCryptographicReviewComplete === true ||
      payload?.releaseOwnerSignoffComplete === true ||
      String(payload?.releaseDecision || "").trim() !== "";
    if (templatePresent && !templateCompleted) {
      return {
        report: relativePath,
        present: false,
        templatePresent: true,
        templateDigestMatched: payload?.corpusDigest === binding.corpusDigest,
        templateSnapshotPresent: vectorCorpusSnapshot.present === true,
        templateSnapshotDigestMatched,
        vectorCorpusSnapshot,
        ok: false,
        schemaVersion: String(payload?.schemaVersion || ""),
        redacted: payload?.redacted === true,
        rawPrivateMaterialIncluded: payload?.rawPrivateMaterialIncluded === true,
        rawPlaintextIncluded: payload?.rawPlaintextIncluded === true,
        rawPublicWireBytesIncluded: payload?.rawPublicWireBytesIncluded === true,
        corpusDigestMatched: false,
        corpusEntryIdsDigestMatched: false,
        sourceCheckIdsDigestMatched: false,
        nativeTestFiltersDigestMatched: false,
        independentCryptographicReviewComplete: false,
        releaseOwnerSignoffComplete: false,
        releaseDecision: "",
        productionReadyClaimed: false,
        reviewerSignatureVerified: false,
        releaseOwnerSignatureVerified: false,
        authoritiesDistinct: false,
        missingOrMismatchedFields: [],
        failureSummary: "pairwise/content review signoff template is present but not completed"
      };
    }
    const missingOrMismatchedFields = [];
    const expected = {
      schemaVersion: "licomesh.secure-mesh.pairwise-content-review-signoff.v2",
      sourceOfTruth: sourceOfTruth,
      redacted: true,
      rawPrivateMaterialIncluded: false,
      rawPlaintextIncluded: false,
      rawPublicWireBytesIncluded: false,
      corpusSchemaVersion: binding.corpusSchemaVersion,
      corpusDigest: binding.corpusDigest,
      corpusEntryCount: binding.corpusEntryCount,
      corpusEntryIdsDigest: binding.corpusEntryIdsDigest,
      sourceCheckCount: binding.sourceCheckCount,
      sourceCheckIdsDigest: binding.sourceCheckIdsDigest,
      nativeTestFilterCount: binding.nativeTestFilterCount,
      nativeTestFiltersDigest: binding.nativeTestFiltersDigest,
      sourceStateDigest: binding.sourceStateDigest,
      producerSourceDigest: binding.producerSourceDigest,
      independentCryptographicReviewComplete: true,
      releaseOwnerSignoffComplete: true,
      releaseDecision: "approved_for_release_gate",
      productionReadyClaimed: false
    };
    for (const [field, value] of Object.entries(expected)) {
      if (payload?.[field] !== value) {
        missingOrMismatchedFields.push(field);
      }
    }
    const signatureVerification = verifyPairwiseReviewSignoff({
      binding,
      signoff: payload,
      authorities: reviewAuthorities,
    });
    const ok = missingOrMismatchedFields.length === 0 &&
      signatureVerification.ready === true;
    return {
      report: relativePath,
      present: true,
      templatePresent,
      templateDigestMatched: templatePresent
        ? payload?.corpusDigest === binding.corpusDigest
        : false,
      templateSnapshotPresent: vectorCorpusSnapshot.present === true,
      templateSnapshotDigestMatched,
      vectorCorpusSnapshot,
      ok,
      schemaVersion: String(payload?.schemaVersion || ""),
      redacted: payload?.redacted === true,
      rawPrivateMaterialIncluded: payload?.rawPrivateMaterialIncluded === true,
      rawPlaintextIncluded: payload?.rawPlaintextIncluded === true,
      rawPublicWireBytesIncluded: payload?.rawPublicWireBytesIncluded === true,
      corpusDigestMatched: payload?.corpusDigest === binding.corpusDigest,
      corpusEntryIdsDigestMatched: payload?.corpusEntryIdsDigest === binding.corpusEntryIdsDigest,
      sourceCheckIdsDigestMatched: payload?.sourceCheckIdsDigest === binding.sourceCheckIdsDigest,
      nativeTestFiltersDigestMatched: payload?.nativeTestFiltersDigest === binding.nativeTestFiltersDigest,
      independentCryptographicReviewComplete:
        payload?.independentCryptographicReviewComplete === true,
      releaseOwnerSignoffComplete: payload?.releaseOwnerSignoffComplete === true,
      releaseDecision: String(payload?.releaseDecision || ""),
      productionReadyClaimed: payload?.productionReadyClaimed === true,
      reviewerSignatureVerified:
        signatureVerification.reviewerSignatureVerified === true,
      releaseOwnerSignatureVerified:
        signatureVerification.releaseOwnerSignatureVerified === true,
      authoritiesDistinct: signatureVerification.authoritiesDistinct === true,
      sourceStateDigestBound: signatureVerification.sourceStateDigestBound === true,
      producerSourceDigestBound:
        signatureVerification.producerSourceDigestBound === true,
      missingOrMismatchedFields,
      failureSummary: ok
        ? ""
        : "pairwise/content review signoff is missing required digest-bound fields"
    };
  } catch (error) {
    return {
      report: relativePath,
      present: false,
      templatePresent: false,
      templateDigestMatched: false,
      templateSnapshotPresent: false,
      templateSnapshotDigestMatched: false,
      vectorCorpusSnapshot: {
        present: false,
        ok: false,
        failureSummary: "not loaded"
      },
      ok: false,
      schemaVersion: "",
      redacted: false,
      rawPrivateMaterialIncluded: false,
      rawPlaintextIncluded: false,
      rawPublicWireBytesIncluded: false,
      corpusDigestMatched: false,
      corpusEntryIdsDigestMatched: false,
      sourceCheckIdsDigestMatched: false,
      nativeTestFiltersDigestMatched: false,
      independentCryptographicReviewComplete: false,
      releaseOwnerSignoffComplete: false,
      releaseDecision: "",
      productionReadyClaimed: false,
      reviewerSignatureVerified: false,
      releaseOwnerSignatureVerified: false,
      authoritiesDistinct: false,
      sourceStateDigestBound: false,
      producerSourceDigestBound: false,
      missingOrMismatchedFields: [],
      failureSummary: sanitizeError(error)
    };
  }
}

  return { loadVectorCorpusSnapshot, loadReviewSignoffArtifact };
}
