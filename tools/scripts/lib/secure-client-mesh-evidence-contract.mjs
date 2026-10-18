import {
  licoArcBadTowerAcceptanceReady,
  licoArcBadTowerAcceptanceSchemaVersion,
} from "./licoarc-badtower-acceptance-report.mjs";

export const SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH =
  "tools/scripts/lib/secure-client-mesh-release-contract.mjs";
export const SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_SCHEMA_VERSION =
  "licomesh.secure-client-mesh.e2ee-authority-proof.v1";
export const SECURE_CLIENT_MESH_E2EE_AUTHORITY_TRUST_ROOT_SCHEMA_VERSION =
  "licomesh.secure-client-mesh.e2ee-authority-trust-root.v1";
export const SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_FIELD = "authorityProof";
export const SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM = "ed25519";
export const SECURE_CLIENT_MESH_E2EE_AUTHORITY_TRUST_ROOT_ENV =
  "LICO_SECURE_CLIENT_MESH_E2EE_AUTHORITY_TRUST_ROOT";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION =
  "licomesh.secure-client-mesh.e2ee-evidence-ref-report.v1";
export const SECURE_CLIENT_MESH_E2EE_PLACEHOLDER_GENERATOR =
  "secure-client-mesh-e2ee-evidence.placeholder";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_ABSENCE_RECEIPT_FIELD = "absenceReceipt";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_ABSENCE_RECEIPT_SCHEMA_VERSION =
  "licomesh.secure-client-mesh.e2ee-evidence-absence-receipt.v1";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_ABSENCE_RECEIPT_TYPE = "evidence-absence";
const SERVER_SCRIPTS_PREFIX = "tools/" + "server" + "-scripts/";

export const SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS = Object.freeze([
  "pairwise/content crypto audit",
  "platform secret-store binding",
  "physical device matrix",
  "Lico Arc BadTower interoperability",
  "encrypted file handoff",
  "trust UX",
  "release proof bundle"
]);

export const SECURE_CLIENT_MESH_PRODUCTION_BLOCKER_REASON =
  `Production E2EE remains blocked until accepted evidence covers ${SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.join(", ")}.`;
export const SECURE_CLIENT_MESH_PRODUCTION_READY_REASON =
  "Production E2EE external client evidence covers every canonical Secure Client Mesh blocker.";
export const SECURE_CLIENT_MESH_PRODUCTION_EVIDENCE_PASSED_STATUSES = Object.freeze(["passed", "complete", "completed"]);
export const SECURE_CLIENT_MESH_PRODUCTION_EVIDENCE_REF_FIELDS = Object.freeze(["evidenceRefs"]);
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_DIGESTS_FIELD = "evidenceRefDigests";
export const SECURE_CLIENT_MESH_EVIDENCE_REPORT_READY_FIELDS = Object.freeze([
  "productionReleaseReady",
  "releaseReady",
  "productionReady"
]);
export const SECURE_CLIENT_MESH_EVIDENCE_REF_REPORT_REQUIRED_READY_FIELDS = Object.freeze([
  "productionReady",
  "releaseReady"
]);

export const SECURE_CLIENT_MESH_EVIDENCE_REF_REPORT_REQUIRED_SCOPE_CLAIMS_BY_BLOCKER = Object.freeze({
  "pairwise/content crypto audit": Object.freeze(["clientRuntimeClaims", "independentCryptoReviewClaims"]),
  "platform secret-store binding": Object.freeze(["clientRuntimeClaims", "platformSecretStoreClaims"]),
  "physical device matrix": Object.freeze(["physicalDeviceClaims"]),
  "Lico Arc BadTower interoperability": Object.freeze(["clientRuntimeClaims", "stationInteroperabilityClaims"]),
  "encrypted file handoff": Object.freeze(["clientRuntimeClaims", "encryptedFileHandoffClaims"]),
  "trust UX": Object.freeze(["trustUxClaims"]),
  "release proof bundle": Object.freeze(["releaseArtifactClaims"])
});

export const SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY = Object.freeze({
  externalClient: "external-client",
  independentAudit: "independent-audit"
});

export const SECURE_CLIENT_MESH_EVIDENCE_SCOPE_CLAIM_AUTHORITY_BY_CLAIM = Object.freeze({
  clientRuntimeClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  independentCryptoReviewClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  platformSecretStoreClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  physicalDeviceClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  stationInteroperabilityClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  encryptedFileHandoffClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  trustUxClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  releaseArtifactClaims: Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit])
});

export const SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY_BY_BLOCKER = Object.freeze({
  "pairwise/content crypto audit": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  "platform secret-store binding": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient]),
  "physical device matrix": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  "Lico Arc BadTower interoperability": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit]),
  "encrypted file handoff": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient]),
  "trust UX": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient]),
  "release proof bundle": Object.freeze([SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient, SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit])
});

export function requiredSecureClientMeshEvidenceRefScopeClaims(blocker = "") {
  return [...(SECURE_CLIENT_MESH_EVIDENCE_REF_REPORT_REQUIRED_SCOPE_CLAIMS_BY_BLOCKER[String(blocker || "").trim()] || [])];
}

export function requiredSecureClientMeshEvidenceScopeClaimAuthorities(claim = "") {
  return [...(SECURE_CLIENT_MESH_EVIDENCE_SCOPE_CLAIM_AUTHORITY_BY_CLAIM[String(claim || "").trim()] || [])];
}

export function requiredSecureClientMeshEvidenceRefScopeClaimAuthorities(blocker = "") {
  return Object.fromEntries(requiredSecureClientMeshEvidenceRefScopeClaims(blocker)
    .map((claim) => [claim, requiredSecureClientMeshEvidenceScopeClaimAuthorities(claim)]));
}

export function secureClientMeshEvidenceAuthorityForBlocker(blocker = "") {
  const canonicalBlocker = String(blocker || "").trim();
  return {
    blocker: canonicalBlocker,
    evidenceAuthorities: [...(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY_BY_BLOCKER[canonicalBlocker] || [])]
  };
}

export function secureClientMeshEvidenceAuthorityProofRequiredForBlocker(blocker = "") {
  const authority = secureClientMeshEvidenceAuthorityForBlocker(blocker);
  return authority.evidenceAuthorities.includes(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient) ||
    authority.evidenceAuthorities.includes(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit);
}

export function secureClientMeshEvidenceAuthorityRequirements() {
  return SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.map((blocker) => ({
    ...secureClientMeshEvidenceAuthorityForBlocker(blocker),
    requiredScopeClaims: requiredSecureClientMeshEvidenceRefScopeClaims(blocker),
    requiredScopeClaimAuthorities: requiredSecureClientMeshEvidenceRefScopeClaimAuthorities(blocker),
    signedAuthorityProofRequired: secureClientMeshEvidenceAuthorityProofRequiredForBlocker(blocker)
  }));
}

export function evaluateSecureClientMeshEvidenceAbsenceReceipt(report = {}, expectedBlocker = "") {
  const record = asRecord(report);
  const receipt = asRecord(record[SECURE_CLIENT_MESH_E2EE_EVIDENCE_ABSENCE_RECEIPT_FIELD]);
  const provided = Object.keys(receipt).length > 0;
  if (!provided) {
    return { provided: false, accepted: false, reasons: ["absence-receipt-not-provided"] };
  }
  const blocker = String(receipt.blocker || "").trim();
  const expected = String(expectedBlocker || "").trim();
  const checkedAt = String(receipt.checkedAt || "").trim();
  const authority = String(receipt.authority || receipt.evidenceAuthority || "").trim();
  const reasonCode = String(receipt.reasonCode || "").trim();
  const requiredAuthorities = secureClientMeshEvidenceAuthorityForBlocker(expected).evidenceAuthorities;
  const schemaAccepted = receipt.schemaVersion === SECURE_CLIENT_MESH_E2EE_EVIDENCE_ABSENCE_RECEIPT_SCHEMA_VERSION;
  const typeAccepted = receipt.receiptType === SECURE_CLIENT_MESH_E2EE_EVIDENCE_ABSENCE_RECEIPT_TYPE;
  const blockerMatches = Boolean(blocker && expected && blocker === expected);
  const authorityAccepted = requiredAuthorities.includes(authority);
  const checkedAtAccepted = Number.isFinite(Date.parse(checkedAt));
  const redactionAccepted = receipt.redacted === true;
  const reasonCodeAccepted = Boolean(reasonCode);
  const accepted = schemaAccepted && typeAccepted && blockerMatches && authorityAccepted &&
    checkedAtAccepted && redactionAccepted && reasonCodeAccepted;
  return {
    provided,
    accepted,
    schemaAccepted,
    typeAccepted,
    blocker,
    expectedBlocker: expected,
    blockerMatches,
    authority,
    authorityAccepted,
    requiredAuthorities,
    checkedAt,
    checkedAtAccepted,
    redactionAccepted,
    reasonCode,
    reasonCodeAccepted,
    releaseEvidence: false,
    blockerClosingAuthority: false,
    reasons: [
      schemaAccepted ? "" : "absence-receipt-schema-mismatch",
      typeAccepted ? "" : "absence-receipt-type-mismatch",
      blockerMatches ? "" : "absence-receipt-blocker-mismatch",
      authorityAccepted ? "" : "absence-receipt-authority-not-accepted",
      checkedAtAccepted ? "" : "absence-receipt-checked-at-invalid",
      redactionAccepted ? "" : "absence-receipt-redaction-missing",
      reasonCodeAccepted ? "" : "absence-receipt-reason-code-missing"
    ].filter(Boolean)
  };
}

function freshnessDiagnostic(freshUntil = "", referenceMs = 0, label = "evidence") {
  const value = String(freshUntil || "").trim();
  if (!value) {
    return { provided: false, accepted: false, freshUntil: "", reason: `${label}-fresh-until-missing` };
  }
  const freshUntilMs = Date.parse(value);
  if (!Number.isFinite(freshUntilMs)) {
    return { provided: true, accepted: false, freshUntil: value, reason: `${label}-fresh-until-invalid` };
  }
  if (Number.isFinite(referenceMs) && freshUntilMs < referenceMs) {
    return { provided: true, accepted: false, freshUntil: new Date(freshUntilMs).toISOString(), reason: `${label}-freshness-expired` };
  }
  return { provided: true, accepted: true, freshUntil: new Date(freshUntilMs).toISOString(), reason: "" };
}

export function evaluateSecureClientMeshEvidenceFreshness(report = {}, options = {}) {
  const record = asRecord(report);
  const summary = asRecord(record.summary);
  const checkedAt = String(record.checkedAt || summary.checkedAt || "").trim();
  const referenceAt = String(options.freshnessNow || options.now || new Date().toISOString()).trim();
  const referenceMs = Date.parse(referenceAt);
  const reportFreshness = freshnessDiagnostic(record.freshUntil || summary.freshUntil, referenceMs, "evidence-report");
  const scopeEvidence = asRecord(record.scopeEvidence);
  const scopeEvidenceFreshness = Object.fromEntries(Object.entries(scopeEvidence).map(([claim, rawReceipt]) => {
    const receipt = asRecord(rawReceipt);
    return [claim, freshnessDiagnostic(receipt.freshUntil, referenceMs, `scope-evidence-${claim}`)];
  }));
  const reasons = [
    reportFreshness.reason,
    ...Object.values(scopeEvidenceFreshness).map((diagnostic) => diagnostic.reason)
  ].filter(Boolean);
  return {
    accepted: reasons.length === 0,
    referenceAt,
    referenceAtAccepted: Number.isFinite(referenceMs),
    freshUntil: reportFreshness.freshUntil,
    reportFreshness,
    scopeEvidenceFreshness,
    reasons
  };
}

function evaluateSecureClientMeshEvidenceRefReportBlockerSemantics(record = {}, canonicalBlocker = "") {
  if (canonicalBlocker !== "Lico Arc BadTower interoperability") {
    return {
      accepted: true,
      schemaVersion: "",
      expectedSchemaVersion: "",
      schemaAccepted: true,
      requiredScenarioFields: [],
      scenarioFieldClaims: {},
      scenarioClaimCount: 0,
      scenarioClaimCountAccepted: true,
      reasons: []
    };
  }
  const schemaVersion = String(record.schemaVersion || "").trim();
  const requiredScenarioFields = [
    "freshEndpointCount",
    "positiveExchange",
    "roundTrip",
    "stationPlaintextAbsent",
    "nonConformantEnvelopeRejected",
    "transportHintsNonAuthoritative",
    "exactFiveOuterFields",
  ];
  const scenario = asRecord(record.scenario);
  const scenarioFieldClaims = Object.fromEntries(requiredScenarioFields.map((field) => [
    field,
    field === "freshEndpointCount"
      ? scenario[field] === 2
      : scenario[field] === true,
  ]));
  const missingScenarioFields = requiredScenarioFields
    .filter((field) => scenarioFieldClaims[field] !== true);
  const scenarioClaimCount = Object.values(scenarioFieldClaims).filter(Boolean).length;
  const scenarioClaimCountAccepted =
    scenarioClaimCount === requiredScenarioFields.length;
  const schemaAccepted =
    schemaVersion === licoArcBadTowerAcceptanceSchemaVersion;
  const strictReportAccepted = licoArcBadTowerAcceptanceReady(record);
  const reasons = [
    schemaAccepted ? "" : "licoarc-badtower-schema-mismatch",
    strictReportAccepted ? "" : "licoarc-badtower-strict-report-invalid",
    scenarioClaimCountAccepted
      ? ""
      : "licoarc-badtower-scenario-incomplete",
    ...missingScenarioFields.map(
      (field) => `licoarc-badtower-scenario-${field}-missing`,
    ),
  ].filter(Boolean);
  return {
    accepted: reasons.length === 0,
    schemaVersion,
    expectedSchemaVersion: licoArcBadTowerAcceptanceSchemaVersion,
    schemaAccepted,
    strictReportAccepted,
    requiredScenarioFields,
    scenarioFieldClaims,
    scenarioClaimCount,
    scenarioClaimCountAccepted,
    reasons
  };
}

export function evaluateSecureClientMeshEvidenceRefReportReadiness(report = {}, expectedBlocker = "", options = {}) {
  const record = asRecord(report);
  const summary = asRecord(record.summary);
  const verification = asRecord(options.authorityProofVerification);
  const readyFields = {};
  for (const field of SECURE_CLIENT_MESH_EVIDENCE_REPORT_READY_FIELDS) {
    if (typeof record[field] === "boolean") {
      readyFields[field] = record[field];
    }
  }
  const explicitNotReadyFields = Object.entries(readyFields)
    .filter(([, value]) => value !== true)
    .map(([field]) => field);
  const blocker = String(record.blocker || "").trim();
  const canonicalBlocker = String(expectedBlocker || "").trim();
  const blockerMatches = Boolean(blocker && canonicalBlocker && blocker === canonicalBlocker);
  const sourceOfTruth = String(record.sourceOfTruth || "").trim();
  const sourceOfTruthAccepted = sourceOfTruth === SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH;
  const okAccepted = record.ok === true || record.verificationOk === true;
  const schemaMatches = record.evidenceRefSchemaVersion === SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION;
  const rawMaterialFlagsAccepted = record.rawPrivateMaterialIncluded === false &&
    record.rawPlaintextIncluded === false &&
    record.rawPublicWireBytesIncluded === false;
  const reportLeakScanAccepted = record.reportLeakScan === true || summary.reportLeakScan === true;
  const redactionAccepted = record.redacted === true &&
    rawMaterialFlagsAccepted &&
    reportLeakScanAccepted;
  const generatedBy = String(record.generatedBy || "").trim();
  const verifier = String(record.verifier || "").trim();
  const checkedAt = String(record.checkedAt || summary.checkedAt || "").trim();
  const absenceReceipt = evaluateSecureClientMeshEvidenceAbsenceReceipt(record, canonicalBlocker);
  const freshness = evaluateSecureClientMeshEvidenceFreshness(record, options);
  const blockerSemantics = evaluateSecureClientMeshEvidenceRefReportBlockerSemantics(record, canonicalBlocker);
  const placeholderGenerated = generatedBy === SECURE_CLIENT_MESH_E2EE_PLACEHOLDER_GENERATOR;
  const checkedAtAccepted = Number.isFinite(Date.parse(checkedAt));
  const generatedByAccepted = Boolean(generatedBy) && !placeholderGenerated;
  const verifierAccepted = Boolean(verifier);
  const remainingGates = [
    ...[].concat(record.remainingGates || []),
    ...[].concat(summary.remainingGates || []),
    ...[].concat(record.diagnosticRemainingGaps || []),
    ...[].concat(summary.diagnosticRemainingGaps || [])
  ].map((value) => String(value || "").trim()).filter(Boolean);
  const readinessFieldCount = Object.keys(readyFields).length;
  const missingRequiredReadyFields = SECURE_CLIENT_MESH_EVIDENCE_REF_REPORT_REQUIRED_READY_FIELDS
    .filter((field) => record[field] !== true);
  const scope = asRecord(record.scope);
  const canonicalBlockerAccepted = SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.includes(canonicalBlocker);
  const requiredScopeClaims = requiredSecureClientMeshEvidenceRefScopeClaims(canonicalBlocker);
  const scopeClaims = Object.fromEntries(requiredScopeClaims.map((claim) => [
    claim,
    scope[claim] === true
  ]));
  const missingRequiredScopeClaims = requiredScopeClaims.filter((claim) => scope[claim] !== true);
  const scopeEvidence = asRecord(record.scopeEvidence);
  const scopeEvidenceReceiptDiagnostics = Object.fromEntries(requiredScopeClaims.map((claim) => {
    const receipt = asRecord(scopeEvidence[claim]);
    const evidenceType = String(receipt.evidenceType || "").trim();
    const receiptCheckedAt = String(receipt.checkedAt || "").trim();
    const receiptAuthority = String(receipt.authority || receipt.evidenceAuthority || "").trim();
    const requiredAuthorities = requiredSecureClientMeshEvidenceScopeClaimAuthorities(claim);
    const authorityAccepted = requiredAuthorities.includes(receiptAuthority);
    const checkedAtAccepted = Number.isFinite(Date.parse(receiptCheckedAt));
    const accepted = receipt.ok === true &&
      receipt.redacted === true &&
      Boolean(evidenceType) &&
      checkedAtAccepted &&
      authorityAccepted;
    return [
      claim,
      {
        accepted,
        okAccepted: receipt.ok === true,
        redactionAccepted: receipt.redacted === true,
        evidenceTypeAccepted: Boolean(evidenceType),
        checkedAt: receiptCheckedAt,
        checkedAtAccepted,
        authority: receiptAuthority,
        authorityAccepted,
        requiredAuthorities
      }
    ];
  }));
  const scopeEvidenceClaims = Object.fromEntries(Object.entries(scopeEvidenceReceiptDiagnostics)
    .map(([claim, diagnostic]) => [claim, diagnostic.accepted === true]));
  const scopeEvidenceAuthorities = Object.fromEntries(Object.entries(scopeEvidenceReceiptDiagnostics)
    .map(([claim, diagnostic]) => [claim, diagnostic.authority]));
  const requiredScopeEvidenceAuthorities = Object.fromEntries(Object.entries(scopeEvidenceReceiptDiagnostics)
    .map(([claim, diagnostic]) => [claim, diagnostic.requiredAuthorities]));
  const scopeEvidenceAuthorityClaims = Object.fromEntries(Object.entries(scopeEvidenceReceiptDiagnostics)
    .map(([claim, diagnostic]) => [claim, diagnostic.authorityAccepted === true]));
  const scopeEvidenceCheckedAtClaims = Object.fromEntries(Object.entries(scopeEvidenceReceiptDiagnostics)
    .map(([claim, diagnostic]) => [claim, diagnostic.checkedAtAccepted === true]));
  const missingRequiredScopeEvidenceClaims = requiredScopeClaims.filter((claim) => scopeEvidenceClaims[claim] !== true);
  const missingRequiredScopeEvidenceAuthorityClaims = requiredScopeClaims.filter((claim) => scopeEvidenceAuthorityClaims[claim] !== true);
  const missingRequiredScopeEvidenceCheckedAtClaims = requiredScopeClaims.filter((claim) => scopeEvidenceCheckedAtClaims[claim] !== true);
  const evidenceAuthorities = [
    ...(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY_BY_BLOCKER[canonicalBlocker] || [])
  ];
  const serverProvenanceLike = verifier.startsWith(SERVER_SCRIPTS_PREFIX) ||
    generatedBy.startsWith(SERVER_SCRIPTS_PREFIX);
  const externalOrAuditGeneratedAccepted = !serverProvenanceLike &&
    (
      evidenceAuthorities.includes(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient) ||
      evidenceAuthorities.includes(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit)
    );
  const authorityProof = asRecord(record[SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_FIELD]);
  const authorityProofRequired = externalOrAuditGeneratedAccepted;
  const authorityProofAuthority = String(authorityProof.authority || "").trim();
  const authorityProofKeyId = String(authorityProof.keyId || "").trim();
  const authorityProofSchemaAccepted = authorityProof.schemaVersion === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_SCHEMA_VERSION;
  const authorityProofSourceOfTruthAccepted = authorityProof.sourceOfTruth === SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH;
  const authorityProofAlgorithmAccepted = authorityProof.algorithm === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM;
  const authorityProofAuthorityAccepted = evidenceAuthorities.includes(authorityProofAuthority);
  const authorityProofKeyIdAccepted = Boolean(authorityProofKeyId);
  const authorityProofPayloadDigest = String(authorityProof.payloadDigest || "").trim();
  const authorityProofPayloadDigestShapeAccepted = /^sha256:[a-f0-9]{64}$/u.test(authorityProofPayloadDigest);
  const authorityProofSignatureAccepted = verification.signatureAccepted === true;
  const authorityProofPayloadDigestAccepted = verification.payloadDigestAccepted === true;
  const authorityTrustRootAccepted = verification.trustRootAccepted === true;
  const authorityProofAccepted = !authorityProofRequired || (
    authorityProofSchemaAccepted &&
    authorityProofSourceOfTruthAccepted &&
    authorityProofAlgorithmAccepted &&
    authorityProofAuthorityAccepted &&
    authorityProofKeyIdAccepted &&
    authorityProofPayloadDigestShapeAccepted &&
    authorityProofPayloadDigestAccepted &&
    authorityProofSignatureAccepted &&
    authorityTrustRootAccepted
  );
  const evidenceAuthorityAccepted = externalOrAuditGeneratedAccepted && authorityProofAccepted;
  const provenanceAccepted = verifierAccepted &&
    generatedByAccepted &&
    checkedAtAccepted &&
    evidenceAuthorityAccepted;
  const ready = okAccepted &&
    schemaMatches &&
    sourceOfTruthAccepted &&
    canonicalBlockerAccepted &&
    blockerMatches &&
    redactionAccepted &&
    provenanceAccepted &&
    remainingGates.length === 0 &&
    missingRequiredReadyFields.length === 0 &&
    missingRequiredScopeClaims.length === 0 &&
    missingRequiredScopeEvidenceClaims.length === 0 &&
    missingRequiredScopeEvidenceAuthorityClaims.length === 0 &&
    missingRequiredScopeEvidenceCheckedAtClaims.length === 0 &&
    explicitNotReadyFields.length === 0 &&
    freshness.accepted === true &&
    blockerSemantics.accepted === true;
  return {
    ready,
    okAccepted,
    schemaMatches,
    evidenceRefSchemaVersion: String(record.evidenceRefSchemaVersion || ""),
    expectedEvidenceRefSchemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_REPORT_SCHEMA_VERSION,
    sourceOfTruth,
    expectedSourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    sourceOfTruthAccepted,
    redactionAccepted,
    rawMaterialFlagsAccepted,
    reportLeakScanAccepted,
    redacted: record.redacted === true,
    rawPrivateMaterialIncluded: record.rawPrivateMaterialIncluded === true,
    rawPlaintextIncluded: record.rawPlaintextIncluded === true,
    rawPublicWireBytesIncluded: record.rawPublicWireBytesIncluded === true,
    verifier,
    verifierAccepted,
    generatedBy,
    placeholderGenerated,
    generatedByAccepted,
    checkedAt,
    checkedAtAccepted,
    freshnessAccepted: freshness.accepted === true,
    freshUntil: freshness.freshUntil,
    freshnessReferenceAt: freshness.referenceAt,
    freshnessReasons: safeProtocolDiagnosticList(freshness.reasons),
    scopeEvidenceFreshness: freshness.scopeEvidenceFreshness,
    blockerSemanticsAccepted: blockerSemantics.accepted === true,
    blockerSemanticsSchemaVersion: blockerSemantics.schemaVersion,
    blockerSemanticsExpectedSchemaVersion: blockerSemantics.expectedSchemaVersion,
    blockerSemanticsSchemaAccepted: blockerSemantics.schemaAccepted === true,
    blockerSemanticsReasons: safeProtocolDiagnosticList(blockerSemantics.reasons),
    blockerSemanticsStrictReportAccepted:
      blockerSemantics.strictReportAccepted === true,
    blockerSemanticsRequiredScenarioFields:
      blockerSemantics.requiredScenarioFields,
    blockerSemanticsScenarioFieldClaims:
      blockerSemantics.scenarioFieldClaims,
    blockerSemanticsScenarioClaimCount: blockerSemantics.scenarioClaimCount,
    blockerSemanticsScenarioClaimCountAccepted:
      blockerSemantics.scenarioClaimCountAccepted === true,
    absenceReceiptProvided: absenceReceipt.provided === true,
    absenceReceiptAccepted: absenceReceipt.accepted === true,
    absenceReceiptReleaseEvidence: false,
    absenceReceiptBlockerClosingAuthority: false,
    absenceReceiptReasons: safeProtocolDiagnosticList(absenceReceipt.reasons),
    provenanceAccepted,
    serverProvenanceLike,
    clientOrAuditProvenanceAccepted: !serverProvenanceLike,
    evidenceAuthorityAccepted,
    externalOrAuditGeneratedAccepted,
    authorityProofRequired,
    authorityProofAccepted,
    authorityProofSchemaAccepted,
    authorityProofSourceOfTruthAccepted,
    authorityProofAlgorithmAccepted,
    authorityProofAuthority,
    authorityProofAuthorityAccepted,
    authorityProofKeyId,
    authorityProofKeyIdAccepted,
    authorityProofPayloadDigest,
    authorityProofPayloadDigestShapeAccepted,
    authorityProofPayloadDigestAccepted,
    authorityProofSignatureAccepted,
    authorityTrustRootAccepted,
    authorityProofVerificationReasons: safeProtocolDiagnosticList(verification.reasons),
    evidenceAuthorities,
    blocker,
    expectedBlocker: canonicalBlocker,
    canonicalBlockerAccepted,
    blockerMatches,
    remainingGates,
    remainingGateCount: remainingGates.length,
    readinessFieldCount,
    readyFields,
    missingRequiredReadyFields,
    requiredScopeClaims,
    scopeClaims,
    scopeEvidenceClaims,
    scopeEvidenceReceiptDiagnostics,
    scopeEvidenceAuthorities,
    requiredScopeEvidenceAuthorities,
    scopeEvidenceAuthorityClaims,
    scopeEvidenceCheckedAtClaims,
    missingRequiredScopeClaims,
    missingRequiredScopeEvidenceClaims,
    missingRequiredScopeEvidenceAuthorityClaims,
    missingRequiredScopeEvidenceCheckedAtClaims,
    explicitNotReadyFields
  };
}

function safeProtocolDiagnosticList(values) {
  return [...new Set((Array.isArray(values) ? values : [])
    .map((value) => String(value || "").trim())
    .filter(Boolean))]
    .slice(0, 20);
}

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function canonicalJsonValue(value) {
  if (Array.isArray(value)) return value.map(canonicalJsonValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.keys(value)
      .filter((key) => key !== SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_FIELD)
      .sort()
      .map((key) => [key, canonicalJsonValue(value[key])]));
  }
  return value;
}

export function canonicalSecureClientMeshAuthorityProofPayload(report = {}) {
  return JSON.stringify(canonicalJsonValue(asRecord(report)));
}
