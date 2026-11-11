import { readFileSync } from "node:fs";
import {
  CANONICAL_EVIDENCE_FILE,
  DRIVER_INVENTORY_FILE,
} from "./constants.mjs";
import { parseJson } from "./json.mjs";

// The CLI ships every Agent its driver inventory declares, so the packaged
// adapter set is the inventory's own Agent set.
export function packagingRegistryFromInventory(inventory) {
  return {
    modules: {
      "target-adapters": {
        targetAdapters: (inventory?.drivers ?? []).map((driver) => driver.agentId),
      },
    },
  };
}

export function loadCanonicalInputs() {
  const inventory = parseJson(
    readFileSync(DRIVER_INVENTORY_FILE, "utf8"),
    "driver_inventory_invalid",
  );
  return {
    packagingRegistry: packagingRegistryFromInventory(inventory),
    inventory,
    evidence: parseJson(
      readFileSync(CANONICAL_EVIDENCE_FILE, "utf8"),
      "evidence_json_invalid",
    ),
  };
}
