import { MigrationStateError } from "./errors.mjs";
import { planSteps, requireDomain } from "./frontier.mjs";
import {
  domainMarkerRoot,
  emptyLedger,
  loadDomainMarker,
  loadLedger,
  reconcileLedgerDomain,
  writeDomainMarker,
  writeLedger,
} from "./ledger.mjs";
import {
  GATEWAY_CUSTODY_DOMAIN,
  LEGACY_MISSING_VERSION_DOMAINS,
  documentPath,
  isRepairableShape,
  probeDomain,
  shapeFor,
} from "./probe.mjs";
import { evaluateMigrationState, resolveDomainVersion } from "./report.mjs";
import {
  asU64,
  ensurePrivateDirectory,
  isPlainObject,
  readBoundedText,
  requireValue,
  writePrivateJsonAtomic,
} from "./util.mjs";

/**
 * Applies exactly one frontier step to exactly one domain, then reconciles that
 * domain's marker and ledger entry the way the admission does. At most one store
 * step runs per invocation, so a repeated call is the resume path.
 *
 * The step is refused unless the admission's own definition of it is fully
 * carried by the frontier: a shape whose internal schema marker the frontier
 * does not state (mobile relay, mobile home layout, both SQLite stores, the
 * client-state collections, native custody) belongs to the admission, not here.
 */
export function repairDomain({
  root,
  frontier,
  domainId,
  binaryProductVersion,
  platform = process.platform,
}) {
  const before = evaluateMigrationState({ root, frontier, binaryProductVersion, platform });
  const domain = requireDomain(frontier, domainId);
  // A whole-root finding is a precondition of the admission itself: an unread
  // ledger, a pending update handoff or a state from a newer binary means the
  // binary may not write anything at all.
  const rootCode = before.codes.find((entry) => entry.domainId === null);
  if (rootCode !== undefined) throw new MigrationStateError(rootCode.code);
  const entry = before.domains.find((candidate) => candidate.domainId === domainId);
  refuseUnrepairable(entry);

  const shape = shapeFor(domainId);
  const plan = planSteps(domain, entry.observedSchemaVersion);
  if (plan.code !== null) throw new MigrationStateError(plan.code);
  const step = plan.steps[0] ?? null;
  if (step !== null && !isRepairableShape(shape, step.toSchemaVersion)) {
    throw new MigrationStateError("repair_requires_native_admission");
  }

  const storeWritten = step === null ? false : applyStoreStep({ root, shape, step });
  const toSchemaVersion = step?.toSchemaVersion ?? entry.observedSchemaVersion;
  ensurePrivateDirectory(domainMarkerRoot(root));
  writeDomainMarker(root, domainId, toSchemaVersion);
  // Same postcondition the admission checks: the store and the marker must now
  // agree on the version the step claimed, or nothing further is recorded.
  requireValue(
    observedDomainVersion({ root, domain, platform }) === toSchemaVersion,
    "migration_postcondition_failed",
  );
  let ledgerUpdated = false;
  try {
    ledgerUpdated = reconcileLedger(root, frontier, domain, toSchemaVersion);
  } catch (error) {
    // The store and marker writes already happened, and every one of them is
    // idempotent, so the refusal carries that residue to the operator instead
    // of reading as if nothing had been applied.
    if (error instanceof MigrationStateError) {
      error.mutations = [
        {
          domainId,
          stepId: step?.stepId ?? null,
          toSchemaVersion,
          storeWritten,
          ledgerUpdated: false,
          applied: storeWritten,
        },
      ];
    }
    throw error;
  }

  const mutations = [
    {
      domainId,
      stepId: step?.stepId ?? null,
      toSchemaVersion,
      storeWritten,
      ledgerUpdated,
      applied: storeWritten || ledgerUpdated,
    },
  ];
  return {
    report: evaluateMigrationState({ root, frontier, binaryProductVersion, platform }),
    mutations,
  };
}

/** The version the admission itself would plan from, after a marker write. */
function observedDomainVersion({ root, domain, platform }) {
  const observation = probeDomain({ root, domain, platform });
  const marker = loadDomainMarker(root, domain.domainId);
  return resolveDomainVersion({
    domain,
    observation,
    markerSchemaVersion: marker?.authoritativeSchemaVersion ?? null,
  }).version;
}

function refuseUnrepairable(entry) {
  if (entry.codes.includes("state_newer_than_binary") || entry.verdict === "ahead") {
    throw new MigrationStateError("state_newer_than_binary");
  }
  if (entry.codes.length > 0) throw new MigrationStateError(entry.codes[0]);
  if (entry.pendingAuthorization || entry.domainId === GATEWAY_CUSTODY_DOMAIN) {
    throw new MigrationStateError("migration_authorization_required");
  }
}

/**
 * Writes the one durable document the step owns, and nothing else. A document
 * that is not in the state the step starts from is a conflict: another writer
 * changed it, so this invocation stops without touching it.
 */
function applyStoreStep({ root, shape, step }) {
  if (shape.kind === "json-document") {
    return stampDocument(documentPath(root, shape), shape, step);
  }
  if (shape.kind === "agent-tab-order") {
    return wrapTabOrder(documentPath(root, shape), shape, step);
  }
  throw new MigrationStateError("repair_requires_native_admission");
}

/**
 * Reads the document with its exact bytes, so the write can be a compare-and-
 * swap against what this invocation actually saw. The store belongs to the
 * running client, not to this tool.
 */
function readDocument(pathname) {
  const raw = readBoundedText(pathname);
  if (raw === null) return null;
  try {
    return { raw, value: JSON.parse(raw) };
  } catch {
    throw new MigrationStateError("repair_conflict");
  }
}

function stampDocument(pathname, shape, step) {
  const document = readDocument(pathname);
  // An absent document is absent state: the admission records the step without
  // creating the file, and the next writer creates it at its current shape.
  if (document === null) return false;
  requireValue(isPlainObject(document.value), "repair_conflict");
  // Already at the step's target: a rerun after a partial apply is a no-op on
  // the store, exactly as the admission treats a store it finds migrated.
  if (asU64(document.value.schemaVersion) === shape.schemaVersion) return false;
  // The same rule the probe applies: a document that records no version is the
  // legacy source shape of the domains that publish one, and only such a
  // legacy document may be stamped.
  requireValue(
    asU64(document.value.schemaVersion) === undefined &&
      step.fromSchemaVersion === 0 &&
      LEGACY_MISSING_VERSION_DOMAINS.includes(shape.domain),
    "repair_conflict",
  );
  writePrivateJsonAtomic(
    pathname,
    { ...document.value, schemaVersion: shape.schemaVersion },
    { expectedBytes: document.raw },
  );
  return true;
}

function wrapTabOrder(pathname, shape, step) {
  const document = readDocument(pathname);
  if (document === null) return false;
  if (isPlainObject(document.value)) {
    if (asU64(document.value.schemaVersion) === shape.schemaVersion) return false;
    throw new MigrationStateError("repair_conflict");
  }
  requireValue(
    Array.isArray(document.value) && step.fromSchemaVersion === 0,
    "repair_conflict",
  );
  writePrivateJsonAtomic(
    pathname,
    { schemaVersion: shape.schemaVersion, order: document.value },
    { expectedBytes: document.raw },
  );
  return true;
}

/**
 * Re-reads the ledger immediately before reconciling, so a concurrent admission
 * that already advanced the domain is honoured rather than overwritten.
 */
function reconcileLedger(root, frontier, domain, schemaVersion) {
  const loaded = loadLedger(root);
  requireValue(loaded.code === null, "migration_ledger_invalid");
  const ledger = loaded.document ?? emptyLedger(frontier);
  if (!reconcileLedgerDomain(ledger, domain, schemaVersion)) return false;
  writeLedger(root, ledger, loaded.raw);
  return true;
}
