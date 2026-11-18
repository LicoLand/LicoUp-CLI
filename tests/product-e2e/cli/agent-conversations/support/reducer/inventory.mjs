import {
  CAPABILITY_MATRIX_FIELDS,
  CONDITIONAL_CHECK_IDS,
  CONTRACT_VERSION,
  CORE_CHECK_IDS,
  DRIVER_MODES,
  INVENTORY_BLOCKING_CODES,
  INVENTORY_CONTRACT_FIELDS,
  INVENTORY_DRIVER_FIELDS,
  INVENTORY_SCHEMA_VERSION,
  INVENTORY_TOP_LEVEL_FIELDS,
  LANE_FAMILIES,
  LIFECYCLE_EVIDENCE_FIELDS,
  MINIMUM_CONSECUTIVE_PASSES,
  SAFE_CODE,
  SHA256_DIGEST,
} from "./constants.mjs";
import { capabilityMatrixDigestFor } from "./digests.mjs";
import { fail } from "./errors.mjs";
import { assertOnlyFields, canonicalJson, isPlainObject } from "./json.mjs";
import { assertNoSensitiveFields } from "./privacy.mjs";

export function validateDriverInventory(inventory, agentIds) {
  assertNoSensitiveFields(inventory);
  assertOnlyFields(inventory, INVENTORY_TOP_LEVEL_FIELDS, "driver_inventory_invalid");
  if (
    inventory.schemaVersion !== INVENTORY_SCHEMA_VERSION ||
    inventory.contractVersion !== CONTRACT_VERSION ||
    !Array.isArray(inventory.drivers)
  ) {
    fail("driver_inventory_invalid");
  }

  assertOnlyFields(
    inventory.evidenceContract,
    INVENTORY_CONTRACT_FIELDS,
    "driver_inventory_invalid",
  );
  const contract = inventory.evidenceContract;
  if (
    contract.minimumConsecutivePasses !== MINIMUM_CONSECUTIVE_PASSES ||
    canonicalJson(contract.coreChecks) !== canonicalJson(CORE_CHECK_IDS) ||
    canonicalJson(contract.conditionalChecks) !== canonicalJson(CONDITIONAL_CHECK_IDS) ||
    canonicalJson(contract.requiredBooleans) !==
      canonicalJson([
        "officialNativeLane",
        "conversationGatePassed",
        "cleanupPassed",
        "privacyPassed",
      ]) ||
    canonicalJson(contract.requiredCounts) !== canonicalJson(["consecutivePasses"]) ||
    canonicalJson(contract.requiredDigests) !==
      canonicalJson([
        "runtimeVersionDigest",
        "capabilitySnapshotDigest",
        "adapterManifestDigest",
        "releaseArtifactDigest",
        "releaseSidecarDigest",
        "productContinuityBindingDigest",
        "registryDigest",
        "driverInventoryDigest",
        "evidenceDigest",
      ]) ||
    canonicalJson(contract.requiredBindings) !==
      canonicalJson([
        "agentId",
        "driverId",
        "runtimeProtocol",
        "harnessVersion",
        "runtimeVersionClass",
        "runtimeSourceClass",
      ])
  ) {
    fail("driver_inventory_invalid");
  }

  const inventoryIds = [];
  for (const driver of inventory.drivers) {
    assertOnlyFields(driver, INVENTORY_DRIVER_FIELDS, "driver_inventory_invalid");
    if (
      !SAFE_CODE.test(driver.agentId ?? "") ||
      !SAFE_CODE.test(driver.driverId ?? "") ||
      !SAFE_CODE.test(driver.runtimeProtocol ?? "") ||
      !SAFE_CODE.test(driver.officialNativeLaneKind ?? "") ||
      typeof driver.historyReadable !== "boolean" ||
      !DRIVER_MODES.has(driver.driverMode) ||
      !Array.isArray(driver.blockerCodes) ||
      driver.blockerCodes.some((code) => !INVENTORY_BLOCKING_CODES.has(code))
    ) {
      fail("driver_inventory_invalid");
    }
    if (driver.driverMode === "blocked" && driver.blockerCodes.length === 0) {
      fail("driver_inventory_invalid");
    }
    if (driver.driverMode === "history-only" && driver.blockerCodes.length > 0) {
      fail("driver_inventory_invalid");
    }
    if (driver.capabilityMatrix !== undefined) {
      if (!isPlainObject(driver.capabilityMatrix)) {
        fail("driver_inventory_invalid");
      }
      assertOnlyFields(
        driver.capabilityMatrix,
        CAPABILITY_MATRIX_FIELDS,
        "driver_inventory_invalid",
      );
      if (
        !LANE_FAMILIES.has(driver.capabilityMatrix.laneFamily) ||
        typeof driver.capabilityMatrix.openNew !== "boolean" ||
        typeof driver.capabilityMatrix.exactResume !== "boolean" ||
        typeof driver.capabilityMatrix.streaming !== "boolean" ||
        typeof driver.capabilityMatrix.cancel !== "boolean" ||
        typeof driver.capabilityMatrix.structuredEvents !== "boolean" ||
        typeof driver.capabilityMatrix.approvals !== "boolean" ||
        typeof driver.capabilityMatrix.multimodal !== "boolean" ||
        typeof driver.capabilityMatrix.usageStatus !== "boolean" ||
        typeof driver.capabilityMatrix.officialLane !== "boolean" ||
        driver.capabilityMatrix.hostSurvivesGuiDisconnect !== true ||
        driver.capabilityMatrix.activeTurnReattach !== true ||
        driver.capabilityMatrix.orderedCursorReplay !== true ||
        (driver.capabilityMatrix.processLocalContinuation !== undefined &&
          typeof driver.capabilityMatrix.processLocalContinuation !== "boolean") ||
        (driver.capabilityMatrix.permissionModes !== undefined &&
          (!Array.isArray(driver.capabilityMatrix.permissionModes) ||
            driver.capabilityMatrix.permissionModes.some(
              (mode) => typeof mode !== "string" || mode.length === 0,
            )))
      ) {
        fail("driver_inventory_invalid");
      }
    }
    if (!isPlainObject(driver.lifecycleEvidence)) {
      fail("driver_inventory_invalid");
    }
    assertOnlyFields(
      driver.lifecycleEvidence,
      LIFECYCLE_EVIDENCE_FIELDS,
      "driver_inventory_invalid",
    );
    if (
      typeof driver.lifecycleEvidence.accepted !== "boolean" ||
      typeof driver.lifecycleEvidence.processing !== "boolean" ||
      typeof driver.lifecycleEvidence.responding !== "boolean" ||
      typeof driver.lifecycleEvidence.completed !== "boolean"
    ) {
      fail("driver_inventory_invalid");
    }
    inventoryIds.push(driver.agentId);
  }

  if (new Set(inventoryIds).size !== inventoryIds.length) {
    fail("driver_inventory_duplicate_agent");
  }
  const canonical = [...agentIds].sort();
  const inventoried = [...inventoryIds].sort();
  if (canonicalJson(canonical) !== canonicalJson(inventoried)) {
    fail("registry_inventory_mismatch");
  }
  return inventory;
}
