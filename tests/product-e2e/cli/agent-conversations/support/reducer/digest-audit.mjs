import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { REPOSITORY_ROOT, SHA256_DIGEST } from "./constants.mjs";
import {
  adapterEvidenceDigestFor,
  adapterManifestDigestFor,
  capabilityMatrixDigestFor,
  driverInventoryDigestFor,
  packagedAgentIds,
  registryDigestFor,
} from "./digests.mjs";
import { ReducerError } from "./errors.mjs";
import { canonicalJson, isPlainObject } from "./json.mjs";

/**
 * Declared provenance of every digest an adapter evidence row carries.
 *
 * A digest is evidence only when a reader can say what it is a digest *of* and
 * can obtain that source. `producers` names the exact symbol that mints the
 * value; the audit fails closed when a declared producer disappears, so a field
 * can never silently lose its source.
 *
 * Source classes:
 * - `contract-input`: recomputed from the checked-in contract inputs (packaging
 *   registry, driver inventory, adapter manifests). A recorded value is either
 *   equal to the current input — the row is current — or different, which is
 *   what makes the row stale. Both outcomes are reconciled, never ignored.
 * - `record-self-hash`: recomputed from the row itself; it binds every other
 *   value in the row, so altering any recorded digest breaks it.
 * - `run-local-artifact`: minted from an artifact the acceptance run discovered
 *   on the verifying host. A checkout cannot reproduce it, so the row-level
 *   self-hash is what holds the recorded value, and a row that claims currency
 *   is additionally required to have every `contract-input` digest reproduced
 *   by the source it names.
 */
export const EVIDENCE_DIGEST_PROVENANCE = Object.freeze({
  runtimeVersionDigest: Object.freeze({
    sourceClass: "run-local-artifact",
    source: "sha256 of the Agent runtime binary the acceptance run executed",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/parity/evidence.mjs",
        marker: "export function binaryDigest(binaryPath) {",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/up-local-service.mjs",
        marker: "const runtimeVersionDigest = binaryDigest(context.binary);",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/same-session.mjs",
        marker: "const runtimeVersionDigest = binaryDigest(context.binary);",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/claude-code.mjs",
        marker: "const runtimeVersionDigest = binaryDigest(context.binary);",
      }),
    ]),
  }),
  capabilitySnapshotDigest: Object.freeze({
    sourceClass: "contract-input",
    source: "capability matrix of the agent's driver in agent-conversation-drivers.json",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/reducer/digests.mjs",
        marker: "export function capabilityMatrixDigestFor(driver) {",
      }),
    ]),
  }),
  adapterManifestDigest: Object.freeze({
    sourceClass: "contract-input",
    source: "adapter manifest document the row was verified against",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/reducer/digests.mjs",
        marker: "export function adapterManifestDigestFor(agentId) {",
      }),
    ]),
  }),
  releaseArtifactDigest: Object.freeze({
    sourceClass: "run-local-artifact",
    source: "gate artifact identity derived from the run's sidecar and runtime digests",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/up-local-service.mjs",
        marker: "const gateArtifactDigest = sha256Text(",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/same-session.mjs",
        marker: "const gateArtifactDigest = sha256Text(",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/claude-code.mjs",
        marker: "const gateArtifactDigest = sha256Text(",
      }),
    ]),
  }),
  releaseSidecarDigest: Object.freeze({
    sourceClass: "run-local-artifact",
    source: "sha256 of the host sidecar the acceptance run executed",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/up-local-service.mjs",
        marker: "const sidecarDigest = binaryDigest(context.sidecar);",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/same-session.mjs",
        marker: "const sidecarDigest = binaryDigest(context.sidecar);",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/claude-code.mjs",
        marker: "const sidecarDigest = binaryDigest(context.sidecar);",
      }),
    ]),
  }),
  productContinuityBindingDigest: Object.freeze({
    sourceClass: "run-local-artifact",
    source: "product continuity binding over the run's artifact, challenge and native session identity",
    producers: Object.freeze([
      Object.freeze({
        path: "tools/scripts/lib/agent-conversation-release-binding.mjs",
        marker: "export function productContinuityBindingDigest({",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/up-local-service.mjs",
        marker: "const productContinuityBindingDigest = sha256Text(",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/same-session.mjs",
        marker: "const productContinuityBindingDigest = sha256Text(",
      }),
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/gates/claude-code.mjs",
        marker: "const productContinuityBindingDigest = sha256Text(",
      }),
    ]),
  }),
  registryDigest: Object.freeze({
    sourceClass: "contract-input",
    source: "packaged adapter target list in the packaging registry",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/reducer/digests.mjs",
        marker: "export function registryDigestFor(agentIds) {",
      }),
    ]),
  }),
  driverInventoryDigest: Object.freeze({
    sourceClass: "contract-input",
    source: "driver inventory document (schema, contract and every driver entry)",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/reducer/digests.mjs",
        marker: "export function driverInventoryDigestFor(inventory) {",
      }),
    ]),
  }),
  evidenceDigest: Object.freeze({
    sourceClass: "record-self-hash",
    source: "the adapter evidence row itself, with evidenceDigest excluded",
    producers: Object.freeze([
      Object.freeze({
        path: "tests/product-e2e/cli/agent-conversations/support/reducer/digests.mjs",
        marker: "export function adapterEvidenceDigestFor(adapterEvidence) {",
      }),
    ]),
  }),
});

export const EVIDENCE_DIGEST_FIELD_NAMES = Object.freeze(
  Object.keys(EVIDENCE_DIGEST_PROVENANCE),
);

/**
 * Digest fields that a row may only carry while the row is current. Every one of
 * them is recomputed from a checked-in contract input, so a recorded value that
 * differs from the recomputation is a stale binding, and a recorded value that
 * equals it on a row the projection calls stale is an unexplained binding.
 */
export const CONTRACT_INPUT_DIGEST_FIELDS = Object.freeze(
  EVIDENCE_DIGEST_FIELD_NAMES.filter(
    (field) => EVIDENCE_DIGEST_PROVENANCE[field].sourceClass === "contract-input",
  ),
);

function readProducer(path, marker) {
  try {
    return readFileSync(resolve(REPOSITORY_ROOT, path), "utf8").includes(marker);
  } catch {
    return false;
  }
}

/**
 * Reconcile every digest an adapter evidence row records with its declared
 * source, and fail closed on anything that cannot be reconciled.
 *
 * This is the gate that makes the recorded digests falsifiable:
 * - `evidence_digest_field_unclassified`: a digest-valued field exists that the
 *   provenance table (or the inventory evidence contract) does not declare.
 * - `evidence_digest_producer_missing`: a declared producer symbol no longer
 *   exists, so the field's source can no longer be obtained.
 * - `evidence_digest_missing` / `evidence_digest_invalid`: a declared digest is
 *   absent or is not a sha256 value.
 * - `evidence_digest_mismatch`: the row self-hash does not cover the row as
 *   recorded. Any alteration of any recorded digest breaks this.
 * - `evidence_digest_source_mismatch`: a current row's contract-input digest
 *   differs from the contract input it claims.
 * - `evidence_digest_staleness_unattributed`: a row the projection does not
 *   present as current, whose driver the reduction does evaluate, yet whose
 *   contract-input digests all equal the current inputs, or whose row-level
 *   binding drifted. The recorded digests would then not explain the row's own
 *   state.
 */
export function auditEvidenceDigests({
  evidence,
  packagingRegistry,
  inventory,
  readiness,
}) {
  const violations = [];
  const add = (code, detail) => violations.push({ code, ...detail });

  const agentIds = packagedAgentIds(packagingRegistry);
  const registryDigest = registryDigestFor(agentIds);
  const inventoryDigest = driverInventoryDigestFor(inventory);
  const drivers = new Map(
    (Array.isArray(inventory?.drivers) ? inventory.drivers : []).map((driver) => [
      driver.agentId,
      driver,
    ]),
  );
  const projection = new Map(
    (Array.isArray(readiness?.adapters) ? readiness.adapters : []).map((entry) => [
      entry.agentId,
      entry,
    ]),
  );

  const declaredContractDigests = inventory?.evidenceContract?.requiredDigests;
  if (
    !Array.isArray(declaredContractDigests) ||
    canonicalJson([...declaredContractDigests].sort()) !==
      canonicalJson([...EVIDENCE_DIGEST_FIELD_NAMES].sort())
  ) {
    add("evidence_digest_field_unclassified", {
      reason: "evidence_contract_digest_fields",
      declared: EVIDENCE_DIGEST_FIELD_NAMES,
      contract: declaredContractDigests ?? null,
    });
  }

  for (const [field, provenance] of Object.entries(EVIDENCE_DIGEST_PROVENANCE)) {
    for (const producer of provenance.producers) {
      if (!readProducer(producer.path, producer.marker)) {
        add("evidence_digest_producer_missing", {
          field,
          path: producer.path,
          marker: producer.marker,
        });
      }
    }
  }

  const adapters = [];
  if (!isPlainObject(evidence) || !Array.isArray(evidence.adapters)) {
    add("evidence_schema_invalid", { reason: "evidence_adapters_missing" });
  } else {
    for (const row of evidence.adapters) {
      const agentId = isPlainObject(row) ? row.agentId : undefined;
      const driver = drivers.get(agentId);
      if (!driver) {
        add("evidence_registry_mismatch", { agentId: agentId ?? null });
        continue;
      }
      for (const field of Object.keys(row)) {
        if (
          EVIDENCE_DIGEST_PROVENANCE[field] === undefined &&
          typeof row[field] === "string" &&
          row[field].startsWith("sha256:")
        ) {
          add("evidence_digest_field_unclassified", { agentId, field });
        }
      }
      const missing = EVIDENCE_DIGEST_FIELD_NAMES.filter(
        (field) => row[field] === undefined,
      );
      if (missing.length > 0) {
        add("evidence_digest_missing", { agentId, fields: missing });
        continue;
      }
      const malformed = EVIDENCE_DIGEST_FIELD_NAMES.filter(
        (field) => !SHA256_DIGEST.test(row[field]),
      );
      if (malformed.length > 0) {
        add("evidence_digest_invalid", { agentId, fields: malformed });
        continue;
      }

      const recomputed = {
        capabilitySnapshotDigest: capabilityMatrixDigestFor(driver),
        adapterManifestDigest: adapterManifestDigestFor(agentId),
        registryDigest,
        driverInventoryDigest: inventoryDigest,
      };
      const selfHash = adapterEvidenceDigestFor(row);
      if (row.evidenceDigest !== selfHash) {
        add("evidence_digest_mismatch", {
          agentId,
          field: "evidenceDigest",
          recorded: row.evidenceDigest,
          recomputed: selfHash,
        });
      }

      const staleContractFields = CONTRACT_INPUT_DIGEST_FIELDS.filter(
        (field) => row[field] !== recomputed[field],
      );
      const bindingEvaluated =
        driver.driverMode === "conversation" && driver.blockerCodes.length === 0;
      const entry = projection.get(agentId);
      if (entry === undefined) {
        add("evidence_readiness_missing", { agentId });
      } else {
        const current =
          entry.evidenceBinding !== null && entry.evidenceBinding !== undefined
            ? true
            : entry.status === "ready";
        if (current && staleContractFields.length > 0) {
          add("evidence_digest_source_mismatch", {
            agentId,
            fields: staleContractFields,
            recorded: Object.fromEntries(
              staleContractFields.map((field) => [field, row[field]]),
            ),
            recomputed: Object.fromEntries(
              staleContractFields.map((field) => [field, recomputed[field]]),
            ),
          });
        }
        if (!current) {
          if (
            row.driverId !== driver.driverId ||
            row.runtimeProtocol !== driver.runtimeProtocol
          ) {
            add("evidence_digest_staleness_unattributed", {
              agentId,
              reason: "row_binding_drifted",
            });
          } else if (bindingEvaluated && staleContractFields.length === 0) {
            // A blocked or history-only driver reports without consulting the
            // binding at all. Otherwise the recorded contract digests are the
            // only admissible explanation for this row not being current.
            add("evidence_digest_staleness_unattributed", {
              agentId,
              reason: "contract_digests_match_current_sources",
            });
          }
        }
      }

      adapters.push({
        agentId,
        status: entry?.status ?? null,
        current: entry?.evidenceBinding !== null && entry?.evidenceBinding !== undefined,
        staleContractFields,
        digests: Object.fromEntries(
          EVIDENCE_DIGEST_FIELD_NAMES.map((field) => [
            field,
            {
              sourceClass: EVIDENCE_DIGEST_PROVENANCE[field].sourceClass,
              recorded: row[field],
              recomputed:
                field === "evidenceDigest"
                  ? selfHash
                  : (recomputed[field] ?? null),
            },
          ]),
        ),
      });
    }
  }

  if (violations.length > 0) {
    throw new ReducerError(violations[0].code, violations);
  }

  return {
    adapters,
    digestCount: adapters.length * EVIDENCE_DIGEST_FIELD_NAMES.length,
    fieldCount: EVIDENCE_DIGEST_FIELD_NAMES.length,
    sourceClasses: Object.fromEntries(
      ["contract-input", "record-self-hash", "run-local-artifact"].map((sourceClass) => [
        sourceClass,
        EVIDENCE_DIGEST_FIELD_NAMES.filter(
          (field) => EVIDENCE_DIGEST_PROVENANCE[field].sourceClass === sourceClass,
        ).length,
      ]),
    ),
  };
}
