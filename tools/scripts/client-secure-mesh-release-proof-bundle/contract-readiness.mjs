import { dedupeRemainingGates, stableStringList } from "./lists.mjs";

export function consumerVerifiedReleaseArtifacts(updateReport = {}) {
  if (updateReport?.dryRun === true || !Array.isArray(updateReport?.productionArtifacts)) {
    return [];
  }
  return updateReport.productionArtifacts
    .map((artifact) => ({
      targetId: String(artifact?.targetId || "").trim(),
      artifactDigest: String(artifact?.artifactDigest || artifact?.sha256 || "").trim()
    }))
    .filter((artifact) => artifact.targetId);
}

export function contractReadinessGates(readiness = {}, label = "evidence report") {
  const gates = [
    ...dedupeRemainingGates(readiness.remainingGates),
    ...stableStringList(readiness.missingRequiredReadyFields)
      .map((field) => `required ready field missing: ${field}`),
    ...stableStringList(readiness.explicitNotReadyFields)
      .map((field) => `ready field explicitly false: ${field}`),
    ...stableStringList(readiness.missingRequiredScopeClaims)
      .map((claim) => `required scope claim missing: ${claim}`),
    ...stableStringList(readiness.missingRequiredScopeEvidenceClaims)
      .map((claim) => `required scope evidence receipt missing: ${claim}`),
    ...stableStringList(readiness.missingRequiredScopeEvidenceAuthorityClaims)
      .map((claim) => `required scope evidence authority missing: ${claim}`),
    ...stableStringList(readiness.missingRequiredScopeEvidenceCheckedAtClaims)
      .map((claim) => `required scope evidence checkedAt missing: ${claim}`),
    ...(readiness.okAccepted === true ? [] : ["ok/verificationOk not accepted"]),
    ...(readiness.schemaMatches === true ? [] : ["evidence-ref schema mismatch"]),
    ...(readiness.sourceOfTruthAccepted === true ? [] : ["sourceOfTruth not accepted"]),
    ...(readiness.blockerMatches === true ? [] : ["blocker mismatch"]),
    ...(readiness.redactionAccepted === true ? [] : ["redaction or raw-material flags not accepted"]),
    ...(readiness.provenanceAccepted === true ? [] : ["provenance/authority proof not accepted"]),
    ...(readiness.freshnessAccepted === true ? [] : ["freshness not accepted"]),
    ...(readiness.blockerSemanticsAccepted === true ? [] : ["blocker semantics not accepted"]),
    ...stableStringList(readiness.freshnessReasons),
    ...stableStringList(readiness.blockerSemanticsReasons)
  ];
  return dedupeRemainingGates(gates).map((gate) => `${label}: ${gate}`);
}

export function summarizeContractReadiness(readiness = {}, label = "evidence report") {
  const remainingGates = contractReadinessGates(readiness, label);
  return {
    ready: readiness.ready === true,
    reason: readiness.ready === true ? "evidence-report-ready" : "evidence-report-not-ready",
    okAccepted: readiness.okAccepted === true,
    schemaMatches: readiness.schemaMatches === true,
    sourceOfTruthAccepted: readiness.sourceOfTruthAccepted === true,
    redactionAccepted: readiness.redactionAccepted === true,
    provenanceAccepted: readiness.provenanceAccepted === true,
    freshnessAccepted: readiness.freshnessAccepted === true,
    blockerSemanticsAccepted: readiness.blockerSemanticsAccepted === true,
    evidenceAuthorityAccepted: readiness.evidenceAuthorityAccepted === true,
    authorityProofRequired: readiness.authorityProofRequired === true,
    authorityProofAccepted: readiness.authorityProofAccepted === true,
    remainingGates,
    remainingGateCount: remainingGates.length,
    missingRequiredReadyFields: stableStringList(readiness.missingRequiredReadyFields),
    explicitNotReadyFields: stableStringList(readiness.explicitNotReadyFields),
    missingRequiredScopeClaims: stableStringList(readiness.missingRequiredScopeClaims),
    missingRequiredScopeEvidenceClaims:
      stableStringList(readiness.missingRequiredScopeEvidenceClaims)
  };
}
