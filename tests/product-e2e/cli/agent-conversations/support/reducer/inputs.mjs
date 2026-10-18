import { readFileSync } from "node:fs";
import {
  CANONICAL_EVIDENCE_FILE,
  DRIVER_INVENTORY_FILE,
  PACKAGING_REGISTRY_FILE,
} from "./constants.mjs";
import { parseJson } from "./json.mjs";

export function loadCanonicalInputs() {
  return {
    packagingRegistry: parseJson(
      readFileSync(PACKAGING_REGISTRY_FILE, "utf8"),
      "packaging_registry_invalid",
    ),
    inventory: parseJson(
      readFileSync(DRIVER_INVENTORY_FILE, "utf8"),
      "driver_inventory_invalid",
    ),
    evidence: parseJson(
      readFileSync(CANONICAL_EVIDENCE_FILE, "utf8"),
      "evidence_json_invalid",
    ),
  };
}
