import {
  CONDITIONAL_CHECK_IDS,
  CONTRACT_VERSION,
  CORE_CHECK_IDS,
  EVIDENCE_BLOCKING_CODES,
  MINIMUM_CONSECUTIVE_PASSES,
  READINESS_SCHEMA_VERSION,
} from "./constants.mjs";
import { driverInventoryDigestFor, registryDigestFor } from "./digests.mjs";
import {
  conditionalCounts,
  coreCounts,
  evidenceBindingIsCurrent,
  evidenceCapabilitiesMatchInventory,
  indexEvidence,
  cleanPassCount,
} from "./evidence.mjs";
import { fail } from "./errors.mjs";
import { validateDriverInventory } from "./inventory.mjs";
import { assertNoSensitiveFields } from "./privacy.mjs";
import { packagedAgentIds } from "./digests.mjs";

export function adapterResult({
  driver,
  evidence,
  globalIssue,
  registryDigest,
  inventoryDigest,
}) {
  const core = coreCounts(evidence);
  const conditional = conditionalCounts(evidence);
  const consecutivePasses = cleanPassCount(evidence?.consecutivePasses);
  let status = "unverified";
  let summaryCodes = [globalIssue ?? "evidence_missing"];
  let evidenceBinding = null;

  if (driver.driverMode === "history-only") {
    status = "history-only";
    summaryCodes = ["history_only_driver"];
  } else if (driver.driverMode === "blocked") {
    status = "blocked";
    summaryCodes = [...driver.blockerCodes];
  } else if (driver.blockerCodes.length > 0) {
    summaryCodes = [
      ...driver.blockerCodes,
      ...(evidence ? [] : [globalIssue ?? "evidence_missing"]),
    ];
  } else if (evidence) {
    const bindingCurrent = evidenceBindingIsCurrent(
      evidence,
      driver,
      registryDigest,
      inventoryDigest,
    );
    if (bindingCurrent) {
      evidenceBinding = {
        agentId: evidence.agentId,
        driverId: evidence.driverId,
        runtimeProtocol: evidence.runtimeProtocol,
        harnessVersion: evidence.harnessVersion,
        runtimeVersionClass: evidence.runtimeVersionClass,
        runtimeVersionDigest: evidence.runtimeVersionDigest,
        capabilitySnapshotDigest: evidence.capabilitySnapshotDigest,
        adapterManifestDigest: evidence.adapterManifestDigest,
        releaseArtifactDigest: evidence.releaseArtifactDigest,
        releaseSidecarDigest: evidence.releaseSidecarDigest,
        productContinuityBindingDigest: evidence.productContinuityBindingDigest,
        runtimeSourceClass: evidence.runtimeSourceClass,
        registryDigest: evidence.registryDigest,
        driverInventoryDigest: evidence.driverInventoryDigest,
        evidenceDigest: evidence.evidenceDigest,
      };
    }
    const capabilitiesCurrent = evidenceCapabilitiesMatchInventory(evidence, driver);
    if (!bindingCurrent || !capabilitiesCurrent) {
      summaryCodes = ["evidence_stale_or_incomplete"];
    } else if (
      evidence.blockingCode !== undefined &&
      !EVIDENCE_BLOCKING_CODES.has(evidence.blockingCode)
    ) {
      summaryCodes = ["evidence_incomplete"];
    } else if (typeof evidence.blockingCode === "string") {
      status = "blocked";
      summaryCodes = [evidence.blockingCode];
    } else if (evidence.officialNativeLane === false) {
      status = "blocked";
      summaryCodes = ["official_native_lane_missing"];
    } else if (
      core.failed > 0 ||
      conditional.failed > 0 ||
      evidence.conversationGatePassed === false ||
      evidence.cleanupPassed === false ||
      evidence.privacyPassed === false
    ) {
      status = "failed";
      summaryCodes = [];
      if (core.failed > 0) summaryCodes.push("core_check_failed");
      if (conditional.failed > 0) summaryCodes.push("conditional_check_failed");
      if (evidence.conversationGatePassed === false) summaryCodes.push("conversation_gate_failed");
      if (evidence.cleanupPassed === false) summaryCodes.push("cleanup_failed");
      if (evidence.privacyPassed === false) summaryCodes.push("privacy_failed");
    } else {
      const requiredEvidenceComplete =
        evidence.officialNativeLane === true &&
        typeof evidence.conversationGatePassed === "boolean" &&
        typeof evidence.cleanupPassed === "boolean" &&
        typeof evidence.privacyPassed === "boolean" &&
        Number.isInteger(evidence.consecutivePasses) &&
        evidence.consecutivePasses >= MINIMUM_CONSECUTIVE_PASSES &&
        core.complete &&
        core.allPassed &&
        conditional.complete;

      if (!requiredEvidenceComplete || conditional.unverified > 0) {
        summaryCodes = ["evidence_incomplete"];
      } else if (conditional.gaps > 0) {
        status = "partial";
        summaryCodes = ["native_capability_gap"];
      } else if (
        conditional.nativeSupported === conditional.passed &&
        evidence.conversationGatePassed === true &&
        evidence.cleanupPassed === true &&
        evidence.privacyPassed === true
      ) {
        status = "ready";
        summaryCodes = ["all_required_evidence_passed"];
      }
    }
  }

  return {
    agentId: driver.agentId,
    status,
    sendEnabled: status === "ready",
    officialNativeLaneProven: evidence?.officialNativeLane === true,
    conversationGatePassed: evidence?.conversationGatePassed === true,
    cleanupPassed: evidence?.cleanupPassed === true,
    privacyPassed: evidence?.privacyPassed === true,
    consecutivePasses,
    coreChecks: {
      required: CORE_CHECK_IDS.length,
      passed: core.passed,
      failed: core.failed,
    },
    conditionalChecks: {
      total: CONDITIONAL_CHECK_IDS.length,
      nativeSupported: conditional.nativeSupported,
      passed: conditional.passed,
      gaps: conditional.gaps,
      failed: conditional.failed,
    },
    evidenceBinding,
    summaryCodes,
  };
}

export function reduceConversationParity({ packagingRegistry, inventory, evidence }) {
  const agentIds = packagedAgentIds(packagingRegistry);
  validateDriverInventory(inventory, agentIds);
  const registryDigest = registryDigestFor(agentIds);
  const inventoryDigest = driverInventoryDigestFor(inventory);
  const evidenceIndex = indexEvidence(evidence, agentIds);
  const drivers = new Map(inventory.drivers.map((driver) => [driver.agentId, driver]));

  const adapters = agentIds.map((agentId) =>
    adapterResult({
      driver: drivers.get(agentId),
      evidence: evidenceIndex.byAgent.get(agentId),
      globalIssue: evidenceIndex.globalIssue,
      registryDigest,
      inventoryDigest,
    }),
  );

  const statusCount = (status) => adapters.filter((item) => item.status === status).length;
  return {
    schemaVersion: READINESS_SCHEMA_VERSION,
    contractVersion: CONTRACT_VERSION,
    minimumConsecutivePasses: MINIMUM_CONSECUTIVE_PASSES,
    summary: {
      total: adapters.length,
      ready: statusCount("ready"),
      partial: statusCount("partial"),
      failed: statusCount("failed"),
      blocked: statusCount("blocked"),
      unverified: statusCount("unverified"),
      historyOnly: statusCount("history-only"),
      sendEnabled: adapters.filter((item) => item.sendEnabled).length,
    },
    adapters,
  };
}
