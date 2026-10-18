const DIGEST_PATTERN = /^sha256:[a-f0-9]{64}$/u;

export const licoArcBadTowerAcceptanceProducer =
  "tools/scripts/client-licoarc-badtower-acceptance.mjs";
export const licoArcBadTowerAcceptanceSchemaVersion =
  "licoup.licoarc-badtower.acceptance.v1";

const TOP_LEVEL_FIELDS = Object.freeze([
  "schemaVersion",
  "ok",
  "protocolCandidateDigest",
  "stationCandidateDigest",
  "clientCandidateDigest",
  "scenario",
  "privacy",
  "claims",
]);
const SCENARIO_FIELDS = Object.freeze([
  "freshEndpointCount",
  "positiveExchange",
  "roundTrip",
  "stationPlaintextAbsent",
  "nonConformantEnvelopeRejected",
  "transportHintsNonAuthoritative",
  "exactFiveOuterFields",
  "mobileFfiDispatch",
  "typedPendingObserved",
  "durableResultReceiptAcknowledged",
]);
const PRIVACY_FIELDS = Object.freeze([
  "redacted",
  "endpointContentIncluded",
  "ciphertextIncluded",
  "keyMaterialIncluded",
  "machineIdentityIncluded",
  "rawRuntimeDataIncluded",
]);
const CLAIM_FIELDS = Object.freeze([
  "clientRelease",
  "protocolPublication",
  "stationRelease",
  "hostedOperation",
]);
const CANDIDATE_DIGEST_FIELDS = Object.freeze([
  "protocolCandidateDigest",
  "stationCandidateDigest",
  "clientCandidateDigest",
]);

function exactKeys(value, expected) {
  return value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    JSON.stringify(Object.keys(value).sort()) === JSON.stringify([...expected].sort());
}

export function licoArcBadTowerAcceptanceReportValid(report) {
  return exactKeys(report, TOP_LEVEL_FIELDS) &&
    report.schemaVersion === licoArcBadTowerAcceptanceSchemaVersion &&
    report.ok === true &&
    DIGEST_PATTERN.test(String(report.protocolCandidateDigest || "")) &&
    DIGEST_PATTERN.test(String(report.stationCandidateDigest || "")) &&
    DIGEST_PATTERN.test(String(report.clientCandidateDigest || "")) &&
    exactKeys(report.scenario, SCENARIO_FIELDS) &&
    report.scenario.freshEndpointCount === 2 &&
    SCENARIO_FIELDS
      .filter((field) => field !== "freshEndpointCount")
      .every((field) => report.scenario[field] === true) &&
    exactKeys(report.privacy, PRIVACY_FIELDS) &&
    report.privacy.redacted === true &&
    PRIVACY_FIELDS
      .filter((field) => field !== "redacted")
      .every((field) => report.privacy[field] === false) &&
    exactKeys(report.claims, CLAIM_FIELDS) &&
    CLAIM_FIELDS.every((field) => report.claims[field] === false);
}

export function licoArcBadTowerCandidateBindingsReady(
  report,
  expectedCandidateDigests,
) {
  return licoArcBadTowerAcceptanceReportValid(report) &&
    exactKeys(expectedCandidateDigests, CANDIDATE_DIGEST_FIELDS) &&
    CANDIDATE_DIGEST_FIELDS.every((field) =>
      DIGEST_PATTERN.test(String(expectedCandidateDigests[field] || "")) &&
      report[field] === expectedCandidateDigests[field]);
}

export function licoArcBadTowerAcceptanceReady(
  report,
  expectedCandidateDigests,
) {
  return expectedCandidateDigests === undefined
    ? licoArcBadTowerAcceptanceReportValid(report)
    : licoArcBadTowerCandidateBindingsReady(report, expectedCandidateDigests);
}

export function licoArcBadTowerAcceptanceCoverage(
  report,
  expectedCandidateDigests,
) {
  const scenario = report?.scenario || {};
  const reportValid = licoArcBadTowerAcceptanceReportValid(report);
  const candidateBindingsReady = expectedCandidateDigests === undefined
    ? false
    : licoArcBadTowerCandidateBindingsReady(report, expectedCandidateDigests);
  return Object.freeze({
    ready: expectedCandidateDigests === undefined
      ? reportValid
      : candidateBindingsReady,
    reportValid,
    candidateBindingsReady,
    freshEndpointCount: Number.isSafeInteger(scenario.freshEndpointCount)
      ? scenario.freshEndpointCount
      : 0,
    positiveExchange: scenario.positiveExchange === true,
    roundTrip: scenario.roundTrip === true,
    stationPlaintextAbsent: scenario.stationPlaintextAbsent === true,
    nonConformantEnvelopeRejected:
      scenario.nonConformantEnvelopeRejected === true,
    transportHintsNonAuthoritative:
      scenario.transportHintsNonAuthoritative === true,
    exactFiveOuterFields: scenario.exactFiveOuterFields === true,
    mobileFfiDispatch: scenario.mobileFfiDispatch === true,
    typedPendingObserved: scenario.typedPendingObserved === true,
    durableResultReceiptAcknowledged:
      scenario.durableResultReceiptAcknowledged === true,
  });
}

export function validateLicoArcBadTowerAcceptanceReport(
  report,
  expectedCandidateDigests,
) {
  if (!licoArcBadTowerAcceptanceReady(report, expectedCandidateDigests)) {
    throw new Error("Lico Arc BadTower acceptance report is invalid or incomplete");
  }
  return Object.freeze(report);
}
