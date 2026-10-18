import {
  SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_DIGESTS_FIELD,
  SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS,
  SECURE_CLIENT_MESH_PRODUCTION_EVIDENCE_PASSED_STATUSES,
  SECURE_CLIENT_MESH_PRODUCTION_EVIDENCE_REF_FIELDS,
  SECURE_CLIENT_MESH_PRODUCTION_READY_REASON,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH
} from "./secure-client-mesh-evidence-contract.mjs";

export * from "./secure-client-mesh-evidence-contract.mjs";

export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_SCHEMA_VERSION =
  "licomesh.secure-client-mesh.e2ee-evidence-bundle.v1";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_PATH =
  "build/reports/secure-client-mesh-e2ee-evidence-bundle.json";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_READY_FIELD = "productionReleaseReady";
export const SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD = "productionBlockerStates";
export const SECURE_CLIENT_GITHUB_RELEASE_READINESS_SCHEMA_VERSION =
  "licomesh.secure-client.github-release-readiness.v1";
export const SECURE_CLIENT_GITHUB_RELEASE_CLOSURE_SCHEMA_VERSION =
  "licomesh.secure-client.github-release-closure.v1";
export const SECURE_CLIENT_GITHUB_RELEASE_CLOSURE_REDUCER =
  "tools/scripts/lib/secure-client-mesh-release-contract.mjs#createSecureClientGitHubReleaseClosure";
export const SECURE_CLIENT_MESH_PRODUCTION_READINESS_REDUCER =
  "tools/scripts/lib/secure-client-mesh-release-contract.mjs#createSecureClientMeshProductionReadiness";

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function stableUniqueStrings(values = []) {
  return [...new Set((Array.isArray(values) ? values : [])
    .map((value) => String(value || "").trim())
    .filter(Boolean))]
    .sort();
}

function duplicateStrings(values = []) {
  const seen = new Set();
  const duplicates = new Set();
  for (const value of Array.isArray(values) ? values : []) {
    const normalized = String(value || "").trim();
    if (!normalized) continue;
    if (seen.has(normalized)) duplicates.add(normalized);
    seen.add(normalized);
  }
  return [...duplicates].sort();
}

export function createSecureClientMeshExternalEvidenceBundleTemplate() {
  return {
    schemaVersion: SECURE_CLIENT_MESH_E2EE_EVIDENCE_BUNDLE_SCHEMA_VERSION,
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    [SECURE_CLIENT_MESH_E2EE_EVIDENCE_READY_FIELD]: false,
    [SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD]:
      SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.map((blocker) => ({
        blocker,
        status: "missing",
        evidenceRefs: [],
        [SECURE_CLIENT_MESH_E2EE_EVIDENCE_REF_DIGESTS_FIELD]: {},
        commands: []
      }))
  };
}

export function normalizeSecureClientGitHubReleaseReadiness(value = {}) {
  const record = asRecord(value);
  const targetId = String(record.targetId || "").trim();
  const platform = String(record.platform || "").trim();
  const osFamily = String(record.osFamily || "").trim();
  const arch = String(record.arch || "").trim();
  const blockers = stableUniqueStrings(record.blockers);
  const evidenceRefs = stableUniqueStrings(record.evidenceRefs);
  const artifactDigest = String(record.artifactDigest || "").trim();
  const requestedReady = record.githubReleaseReady === true;
  const shapeAccepted = Boolean(targetId && platform && osFamily && arch) &&
    (!artifactDigest || /^sha256:[a-f0-9]{64}$/u.test(artifactDigest));
  const githubReleaseReady = shapeAccepted && requestedReady && blockers.length === 0 &&
    evidenceRefs.length > 0 && Boolean(artifactDigest);
  return {
    schemaVersion: SECURE_CLIENT_GITHUB_RELEASE_READINESS_SCHEMA_VERSION,
    targetId,
    platform,
    osFamily,
    arch,
    status: githubReleaseReady ? "ready" : String(record.status || "blocked").trim() || "blocked",
    githubReleaseReady,
    blockers,
    evidenceRefs,
    artifactDigest,
    shapeAccepted
  };
}

export function createSecureClientGitHubReleaseClosure({
  selectedTargetIds = [],
  targetReadiness = [],
  artifacts = [],
  knownTargetIds = []
} = {}) {
  const selected = stableUniqueStrings(selectedTargetIds);
  const selectedDuplicates = duplicateStrings(selectedTargetIds);
  const known = new Set(stableUniqueStrings(knownTargetIds));
  const normalizedReadiness = (Array.isArray(targetReadiness) ? targetReadiness : [])
    .map(normalizeSecureClientGitHubReleaseReadiness)
    .sort((left, right) => left.targetId.localeCompare(right.targetId));
  const readinessDuplicates = duplicateStrings(normalizedReadiness.map((item) => item.targetId));
  const readinessByTarget = new Map(normalizedReadiness.map((item) => [item.targetId, item]));
  const normalizedArtifacts = (Array.isArray(artifacts) ? artifacts : [])
    .map((artifact) => {
      const record = asRecord(artifact);
      return {
        targetId: String(record.targetId || "").trim(),
        artifactDigest: String(record.artifactDigest || record.sha256 || "").trim()
      };
    })
    .filter((artifact) => artifact.targetId);
  const artifactTargetIds = normalizedArtifacts.map((artifact) => artifact.targetId);
  const artifactByTarget = new Map(normalizedArtifacts.map((artifact) => [artifact.targetId, artifact]));
  const artifactDuplicates = duplicateStrings(artifactTargetIds);
  const artifactSet = new Set(artifactTargetIds);
  const selectedSet = new Set(selected);
  const unknownSelectedTargetIds = known.size === 0
    ? []
    : selected.filter((targetId) => !known.has(targetId));
  const missingReadinessTargetIds = selected.filter((targetId) => !readinessByTarget.has(targetId));
  const blockedSelectedTargetIds = selected.filter((targetId) =>
    readinessByTarget.has(targetId) && readinessByTarget.get(targetId).githubReleaseReady !== true
  );
  const missingArtifactTargetIds = selected.filter((targetId) => !artifactSet.has(targetId));
  const unselectedArtifactTargetIds = stableUniqueStrings(
    artifactTargetIds.filter((targetId) => !selectedSet.has(targetId))
  );
  const artifactDigestMismatchTargetIds = selected.filter((targetId) => {
    const readiness = readinessByTarget.get(targetId);
    const artifact = artifactByTarget.get(targetId);
    return Boolean(readiness && artifact) && readiness.artifactDigest !== artifact.artifactDigest;
  });
  const githubReleaseReady = selected.length > 0 &&
    selectedDuplicates.length === 0 &&
    unknownSelectedTargetIds.length === 0 &&
    readinessDuplicates.length === 0 &&
    artifactDuplicates.length === 0 &&
    missingReadinessTargetIds.length === 0 &&
    blockedSelectedTargetIds.length === 0 &&
    missingArtifactTargetIds.length === 0 &&
    unselectedArtifactTargetIds.length === 0 &&
    artifactDigestMismatchTargetIds.length === 0 &&
    artifactTargetIds.length === selected.length;
  return {
    schemaVersion: SECURE_CLIENT_GITHUB_RELEASE_CLOSURE_SCHEMA_VERSION,
    githubReleaseReducer: SECURE_CLIENT_GITHUB_RELEASE_CLOSURE_REDUCER,
    selectedTargetIds: selected,
    targetReadiness: selected.map((targetId) => readinessByTarget.get(targetId)).filter(Boolean),
    githubReleaseStatus: githubReleaseReady ? "ready" : "blocked",
    githubReleaseReady,
    blockers: {
      emptySelection: selected.length === 0,
      duplicateSelectedTargetIds: selectedDuplicates,
      unknownSelectedTargetIds,
      duplicateReadinessTargetIds: readinessDuplicates,
      missingReadinessTargetIds,
      blockedSelectedTargetIds,
      duplicateArtifactTargetIds: artifactDuplicates,
      missingArtifactTargetIds,
      unselectedArtifactTargetIds,
      artifactDigestMismatchTargetIds
    }
  };
}

export function secureClientMeshProductionEvidenceEntries(evidence = {}) {
  const states = asRecord(evidence)[SECURE_CLIENT_MESH_E2EE_EVIDENCE_BLOCKER_STATES_FIELD];
  return Array.isArray(states) ? states.map((entry) => [entry?.blocker || "", entry]) : [];
}

export function normalizeSecureClientMeshProductionEvidenceStatus(value) {
  return String(asRecord(value).status || "").trim().toLowerCase();
}

export function collectSecureClientMeshProductionEvidenceRefs(value) {
  const record = asRecord(value);
  return SECURE_CLIENT_MESH_PRODUCTION_EVIDENCE_REF_FIELDS
    .flatMap((field) => [].concat(record[field] || []));
}

export function countSecureClientMeshProductionEvidenceRefs(value) {
  return collectSecureClientMeshProductionEvidenceRefs(value)
    .map((ref) => String(ref || "").trim())
    .filter(Boolean)
    .length;
}

export function normalizeSecureClientMeshProductionBlockerStates(evidence = {}) {
  const canonical = new Set(SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS);
  const statesByBlocker = new Map();
  for (const [rawBlocker, rawValue] of secureClientMeshProductionEvidenceEntries(evidence)) {
    const blocker = String(rawBlocker || "").trim();
    if (!canonical.has(blocker)) continue;
    const status = normalizeSecureClientMeshProductionEvidenceStatus(rawValue);
    const evidenceRefCount = countSecureClientMeshProductionEvidenceRefs(rawValue);
    const passed = asRecord(rawValue).passed === true &&
      SECURE_CLIENT_MESH_PRODUCTION_EVIDENCE_PASSED_STATUSES.includes(status) &&
      evidenceRefCount > 0;
    statesByBlocker.set(blocker, {
      blocker,
      status: status || "missing",
      passed,
      evidenceRefCount
    });
  }
  return SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS.map((blocker) =>
    statesByBlocker.get(blocker) || { blocker, status: "missing", passed: false, evidenceRefCount: 0 }
  );
}

export function createSecureClientMeshProductionReadiness(evidence = {}) {
  const productionBlockerStates = normalizeSecureClientMeshProductionBlockerStates(evidence);
  const productionBlockers = productionBlockerStates
    .filter((state) => state.passed !== true)
    .map((state) => state.blocker);
  const productionReleaseReady = productionBlockers.length === 0;
  return {
    sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
    productionReadinessReducer: SECURE_CLIENT_MESH_PRODUCTION_READINESS_REDUCER,
    productionReleaseStatus: productionReleaseReady ? "ready" : "blocked",
    productionReleaseReady,
    productionBlockers,
    productionBlockerStates,
    productionBlockerReason: productionReleaseReady
      ? SECURE_CLIENT_MESH_PRODUCTION_READY_REASON
      : `Production E2EE remains blocked until accepted evidence covers ${productionBlockers.join(", ")}.`
  };
}
