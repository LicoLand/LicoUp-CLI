import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import {
  ADAPTER_MANIFEST_DIRECTORY,
  SAFE_CODE,
} from "./constants.mjs";
import { fail } from "./errors.mjs";
import { canonicalJson, digest, isPlainObject, parseJson } from "./json.mjs";
import { assertNoSensitiveFields } from "./privacy.mjs";

export function packagedAgentIds(packagingRegistry) {
  const ids = packagingRegistry?.modules?.["target-adapters"]?.targetAdapters;
  if (
    !Array.isArray(ids) ||
    ids.length === 0 ||
    ids.some((id) => typeof id !== "string" || !SAFE_CODE.test(id)) ||
    new Set(ids).size !== ids.length
  ) {
    fail("packaging_registry_invalid");
  }
  return [...ids];
}

export function inventoryDigestValue(inventory) {
  return {
    schemaVersion: inventory.schemaVersion,
    contractVersion: inventory.contractVersion,
    evidenceContract: inventory.evidenceContract,
    drivers: [...inventory.drivers].sort((left, right) =>
      left.agentId.localeCompare(right.agentId),
    ),
  };
}

export function registryDigestFor(agentIds) {
  return digest([...agentIds].sort());
}

export function driverInventoryDigestFor(inventory) {
  return digest(inventoryDigestValue(inventory));
}

export function capabilityMatrixDigestFor(driver) {
  return digest(driver?.capabilityMatrix ?? {});
}

export function adapterManifestDigestFor(agentId) {
  if (typeof agentId !== "string" || !SAFE_CODE.test(agentId)) {
    fail("adapter_manifest_agent_invalid");
  }
  const manifest = parseJson(
    readFileSync(join(ADAPTER_MANIFEST_DIRECTORY, `${agentId}.json`), "utf8"),
    "adapter_manifest_json_invalid",
  );
  if (manifest?.identity?.agentId !== agentId) {
    fail("adapter_manifest_identity_mismatch");
  }
  return digest(manifest);
}

export function adapterEvidenceDigestFor(adapterEvidence) {
  if (!isPlainObject(adapterEvidence)) {
    fail("evidence_schema_invalid");
  }
  const { evidenceDigest: ignored, ...digestibleEvidence } = adapterEvidence;
  return digest(digestibleEvidence);
}
