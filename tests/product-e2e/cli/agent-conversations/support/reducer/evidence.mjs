import {
  ADAPTER_EVIDENCE_FIELDS,
  CONDITIONAL_CHECK_IDS,
  CONDITIONAL_EVIDENCE_FIELDS,
  CONDITIONAL_RESULTS,
  CONTRACT_VERSION,
  CORE_CHECK_IDS,
  CORE_RESULTS,
  EVIDENCE_BLOCKING_CODES,
  EVIDENCE_SCHEMA_VERSION,
  EVIDENCE_TOP_LEVEL_FIELDS,
  NATIVE_SUPPORT_RESULTS,
  SAFE_CODE,
  SHA256_DIGEST,
} from "./constants.mjs";
import {
  adapterEvidenceDigestFor,
  adapterManifestDigestFor,
  capabilityMatrixDigestFor,
} from "./digests.mjs";
import { fail } from "./errors.mjs";
import { assertOnlyFields, isPlainObject } from "./json.mjs";
import { assertNoSensitiveFields } from "./privacy.mjs";

export const CONDITIONAL_CAPABILITY_FIELDS = Object.freeze({
  "C-01": "streaming",
  "C-02": "structuredEvents",
  "C-03": "approvals",
  "C-04": "multimodal",
  "C-05": "interruptSteer",
  "C-06": "usageStatus",
});

export function indexEvidence(evidence, agentIds) {
  if (evidence === undefined || evidence === null) {
    return { byAgent: new Map(), globalIssue: "evidence_missing" };
  }

  assertNoSensitiveFields(evidence);
  assertOnlyFields(evidence, EVIDENCE_TOP_LEVEL_FIELDS, "evidence_schema_invalid");
  if (
    evidence.schemaVersion !== EVIDENCE_SCHEMA_VERSION ||
    evidence.contractVersion !== CONTRACT_VERSION ||
    !Array.isArray(evidence.adapters)
  ) {
    return { byAgent: new Map(), globalIssue: "evidence_schema_mismatch" };
  }

  const canonicalAgents = new Set(agentIds);
  const byAgent = new Map();
  for (const adapter of evidence.adapters) {
    assertOnlyFields(adapter, ADAPTER_EVIDENCE_FIELDS, "evidence_schema_invalid");
    if (!SAFE_CODE.test(adapter.agentId ?? "")) {
      fail("evidence_schema_invalid");
    }
    if (!canonicalAgents.has(adapter.agentId)) {
      fail("evidence_registry_mismatch");
    }
    if (byAgent.has(adapter.agentId)) {
      fail("evidence_duplicate_agent");
    }
    if (isPlainObject(adapter.coreChecks)) {
      for (const key of Object.keys(adapter.coreChecks)) {
        if (!CORE_CHECK_IDS.includes(key)) {
          fail("evidence_schema_invalid");
        }
      }
    }
    if (isPlainObject(adapter.conditionalChecks)) {
      for (const [key, conditional] of Object.entries(adapter.conditionalChecks)) {
        if (!CONDITIONAL_CHECK_IDS.includes(key)) {
          fail("evidence_schema_invalid");
        }
        assertOnlyFields(
          conditional,
          CONDITIONAL_EVIDENCE_FIELDS,
          "evidence_schema_invalid",
        );
      }
    }
    byAgent.set(adapter.agentId, adapter);
  }
  return { byAgent, globalIssue: null };
}

export function evidenceBindingIsCurrent(evidence, driver, registryDigest, inventoryDigest) {
  return (
    SAFE_CODE.test(evidence.driverId ?? "") &&
    evidence.driverId === driver.driverId &&
    SAFE_CODE.test(evidence.runtimeProtocol ?? "") &&
    evidence.runtimeProtocol === driver.runtimeProtocol &&
    SAFE_CODE.test(evidence.harnessVersion ?? "") &&
    SAFE_CODE.test(evidence.runtimeVersionClass ?? "") &&
    SHA256_DIGEST.test(evidence.runtimeVersionDigest ?? "") &&
    evidence.capabilitySnapshotDigest === capabilityMatrixDigestFor(driver) &&
    evidence.adapterManifestDigest === adapterManifestDigestFor(driver.agentId) &&
    SHA256_DIGEST.test(evidence.releaseArtifactDigest ?? "") &&
    SHA256_DIGEST.test(evidence.releaseSidecarDigest ?? "") &&
    SHA256_DIGEST.test(evidence.productContinuityBindingDigest ?? "") &&
    SAFE_CODE.test(evidence.runtimeSourceClass ?? "") &&
    SHA256_DIGEST.test(evidence.registryDigest ?? "") &&
    evidence.registryDigest === registryDigest &&
    SHA256_DIGEST.test(evidence.driverInventoryDigest ?? "") &&
    evidence.driverInventoryDigest === inventoryDigest &&
    SHA256_DIGEST.test(evidence.evidenceDigest ?? "") &&
    evidence.evidenceDigest === adapterEvidenceDigestFor(evidence)
  );
}

export function evidenceCapabilitiesMatchInventory(evidence, driver) {
  const matrix = driver?.capabilityMatrix ?? {};
  if (evidence?.officialNativeLane === true && matrix.officialLane !== true) {
    return false;
  }
  const checks = evidence?.conditionalChecks;
  if (!isPlainObject(checks)) return false;
  return CONDITIONAL_CHECK_IDS.every((id) => {
    const check = checks[id];
    if (!isPlainObject(check)) return false;
    const supported = matrix[CONDITIONAL_CAPABILITY_FIELDS[id]] === true;
    return check.nativeSupport === (supported ? "supported" : "unsupported");
  });
}

export function coreCounts(evidence) {
  const checks = isPlainObject(evidence?.coreChecks) ? evidence.coreChecks : {};
  return {
    passed: CORE_CHECK_IDS.filter((id) => checks[id] === "pass").length,
    failed: CORE_CHECK_IDS.filter((id) => checks[id] === "fail").length,
    complete: CORE_CHECK_IDS.every((id) => CORE_RESULTS.has(checks[id])),
    allPassed: CORE_CHECK_IDS.every((id) => checks[id] === "pass"),
  };
}

export function conditionalCounts(evidence) {
  const checks = isPlainObject(evidence?.conditionalChecks)
    ? evidence.conditionalChecks
    : {};
  let nativeSupported = 0;
  let passed = 0;
  let gaps = 0;
  let failed = 0;
  let unverified = 0;
  let complete = true;

  for (const id of CONDITIONAL_CHECK_IDS) {
    const check = checks[id];
    if (
      !isPlainObject(check) ||
      !NATIVE_SUPPORT_RESULTS.has(check.nativeSupport) ||
      !CONDITIONAL_RESULTS.has(check.result)
    ) {
      complete = false;
      unverified += 1;
      continue;
    }
    if (check.nativeSupport === "supported") {
      nativeSupported += 1;
      if (check.result === "pass") passed += 1;
      else if (check.result === "fail") failed += 1;
      else if (check.result === "gap") gaps += 1;
      else unverified += 1;
      if (check.result === "unsupported-by-native") complete = false;
    } else if (check.nativeSupport === "unsupported") {
      if (check.result !== "unsupported-by-native") complete = false;
    } else {
      if (check.result !== "unverified") complete = false;
      unverified += 1;
    }
  }
  return { nativeSupported, passed, gaps, failed, unverified, complete };
}

export function cleanPassCount(value) {
  return Number.isInteger(value) && value >= 0 && value <= 1_000 ? value : 0;
}
