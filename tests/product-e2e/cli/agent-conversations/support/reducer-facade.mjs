#!/usr/bin/env node

export {
  CONTRACT_VERSION,
  EVIDENCE_SCHEMA_VERSION,
  READINESS_SCHEMA_VERSION,
  INVENTORY_SCHEMA_VERSION,
  MINIMUM_CONSECUTIVE_PASSES,
  CORE_CHECK_IDS,
  CONDITIONAL_CHECK_IDS,
} from "./reducer/constants.mjs";

export { ReducerError } from "./reducer/errors.mjs";
export { assertNoSensitiveFields } from "./reducer/privacy.mjs";
export {
  packagedAgentIds,
  registryDigestFor,
  driverInventoryDigestFor,
  capabilityMatrixDigestFor,
  adapterManifestDigestFor,
  adapterEvidenceDigestFor,
} from "./reducer/digests.mjs";
export {
  CONTRACT_INPUT_DIGEST_FIELDS,
  EVIDENCE_DIGEST_FIELD_NAMES,
  EVIDENCE_DIGEST_PROVENANCE,
  auditEvidenceDigests,
} from "./reducer/digest-audit.mjs";
export { validateDriverInventory } from "./reducer/inventory.mjs";
export {
  reduceConversationParity,
} from "./reducer/reduce.mjs";
export { loadCanonicalInputs } from "./reducer/inputs.mjs";
export {
  assertReadinessMatchesReduction,
  assertReleaseReady,
  runCli,
} from "./reducer/cli.mjs";

import process from "node:process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { runCli, sanitizedFailure } from "./reducer/cli.mjs";

const invokedDirectly =
  process.argv[1] !== undefined &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href;

if (invokedDirectly) {
  try {
    process.stdout.write(`${JSON.stringify(runCli())}\n`);
  } catch (error) {
    process.stderr.write(`${JSON.stringify(sanitizedFailure(error))}\n`);
    process.exitCode = 1;
  }
}
