import path from "node:path";

import { MigrationStateError } from "./errors.mjs";
import { completedStepIdsFor } from "./frontier.mjs";
import {
  isPlainObject,
  parseProductVersion,
  readBoundedText,
  readJsonArtifact,
  regularFileExists,
  requirePrivateStatePath,
  requireValue,
  writePrivateJsonAtomic,
} from "./util.mjs";

export const LEDGER_SCHEMA = "v0.0.1:client-state-migration-ledger-1";
export const DOMAIN_MARKER_SCHEMA = "v0.0.1:client-state-domain-marker-1";
export const MIGRATION_DIR = "client-state/migrations";
export const DOMAIN_STATE_DIR = `${MIGRATION_DIR}/domain-state`;

const LEDGER_MAX_BYTES = 256 * 1024;
const DOMAIN_MARKER_MAX_BYTES = 16 * 1024;

export function ledgerPath(root) {
  return path.join(root, MIGRATION_DIR, "ledger.json");
}

export function domainMarkerRoot(root) {
  return path.join(root, DOMAIN_STATE_DIR);
}

export function domainMarkerPath(root, domainId) {
  return path.join(domainMarkerRoot(root), `${domainId}.json`);
}

export function updateHandoffPath(root) {
  return path.join(root, MIGRATION_DIR, "update-handoff.json");
}

/**
 * An update handoff is the installer's forward-only ownership boundary: the
 * admission claims it before it reads the ledger. This tool does not validate
 * the claim, so it reports the handoff as unverified rather than certifying a
 * root whose next startup may abort before any migration runs.
 */
export function updateHandoffState(root) {
  try {
    return regularFileExists(updateHandoffPath(root))
      ? { present: true, code: null }
      : { present: false, code: null };
  } catch (error) {
    if (error instanceof MigrationStateError) return { present: false, code: error.code };
    throw error;
  }
}

/**
 * Absence is a valid empty ledger, exactly as the admission treats it. A
 * present-but-unreadable ledger is reported as a code rather than thrown, so
 * a diagnosis can still report the durable stores next to it.
 */
export function loadLedger(root) {
  const failure = { present: true, document: null, raw: null, code: "migration_ledger_invalid" };
  try {
    requirePrivateStatePath(path.join(root, MIGRATION_DIR), "directory", failure.code);
    requirePrivateStatePath(ledgerPath(root), "file", failure.code);
    const raw = readBoundedText(ledgerPath(root), LEDGER_MAX_BYTES);
    if (raw === null) return { present: false, document: null, raw: null, code: null };
    return { present: true, document: validateLedger(JSON.parse(raw)), raw, code: null };
  } catch (error) {
    // Both an unreadable document and one that is not the admission's contract
    // are the same finding, exactly as `load_ledger` reports them.
    if (error instanceof MigrationStateError || error instanceof SyntaxError) return failure;
    throw error;
  }
}

export function validateLedger(document) {
  requireValue(
    isPlainObject(document) &&
      hasExactKeys(document, ["schemaVersion", "highestAdmittedProductVersion", "frontierId", "domains"]) &&
      document.schemaVersion === LEDGER_SCHEMA &&
      typeof document.frontierId === "string" &&
      isPlainObject(document.domains),
    "migration_ledger_invalid",
  );
  parseProductVersion(document.highestAdmittedProductVersion);
  const domains = {};
  for (const [domainId, entry] of Object.entries(document.domains)) {
    requireValue(
      isPlainObject(entry) &&
        hasExactKeys(entry, ["schemaVersion", "completedStepIds"]) &&
        Number.isInteger(entry.schemaVersion) &&
        entry.schemaVersion >= 0 &&
        Array.isArray(entry.completedStepIds) &&
        entry.completedStepIds.every((stepId) => typeof stepId === "string"),
      "migration_ledger_invalid",
    );
    domains[domainId] = {
      schemaVersion: entry.schemaVersion,
      completedStepIds: [...entry.completedStepIds],
    };
  }
  return {
    schemaVersion: document.schemaVersion,
    highestAdmittedProductVersion: document.highestAdmittedProductVersion,
    frontierId: document.frontierId,
    domains,
  };
}

/**
 * The admission keeps one marker per domain under
 * `client-state/migrations/domain-state/`. A marker that exists but cannot be
 * read is an unsupported shape, never silently treated as absent state.
 */
export function loadDomainMarker(root, domainId) {
  let document;
  try {
    requirePrivateStatePath(domainMarkerRoot(root), "directory", "unsupported_state_shape");
    requirePrivateStatePath(
      domainMarkerPath(root, domainId),
      "file",
      "unsupported_state_shape",
    );
    document = readJsonArtifact(domainMarkerPath(root, domainId), DOMAIN_MARKER_MAX_BYTES);
  } catch {
    throw new MigrationStateError("unsupported_state_shape");
  }
  if (document === null) return null;
  requireValue(
    isPlainObject(document) &&
      hasExactKeys(document, ["schemaVersion", "domainId", "authoritativeSchemaVersion"]) &&
      document.schemaVersion === DOMAIN_MARKER_SCHEMA &&
      document.domainId === domainId &&
      Number.isInteger(document.authoritativeSchemaVersion) &&
      document.authoritativeSchemaVersion >= 0,
    "unsupported_state_shape",
  );
  return { authoritativeSchemaVersion: document.authoritativeSchemaVersion };
}

/**
 * `expectedBytes` is the ledger exactly as this caller read it. The admission
 * holds its own lock that Node cannot join, so the write refuses to clobber a
 * concurrent admission instead: it commits only if nothing changed since the
 * read, which keeps the forward-only ratchet monotonic.
 */
export function writeLedger(root, ledger, expectedBytes = undefined) {
  writePrivateJsonAtomic(ledgerPath(root), ledger, { expectedBytes });
}

export function writeDomainMarker(root, domainId, authoritativeSchemaVersion) {
  writePrivateJsonAtomic(domainMarkerPath(root, domainId), {
    schemaVersion: DOMAIN_MARKER_SCHEMA,
    domainId,
    authoritativeSchemaVersion,
  });
}

export function emptyLedger(frontier) {
  return {
    schemaVersion: LEDGER_SCHEMA,
    highestAdmittedProductVersion: "0.0.0",
    frontierId: frontier.frontierId,
    domains: {},
  };
}

/**
 * Records a completed domain version the way the admission's `reconcile_ledger`
 * does, including its exact completed-step list, so the next admission accepts
 * the ledger unchanged. Returns whether anything moved.
 */
export function reconcileLedgerDomain(ledger, domain, schemaVersion) {
  const expected = {
    schemaVersion,
    completedStepIds: completedStepIdsFor(domain, schemaVersion),
  };
  const current = ledger.domains[domain.domainId];
  if (current !== undefined && sameLedgerEntry(current, expected)) return false;
  ledger.domains[domain.domainId] = expected;
  return true;
}

function sameLedgerEntry(left, right) {
  return (
    left.schemaVersion === right.schemaVersion &&
    left.completedStepIds.length === right.completedStepIds.length &&
    left.completedStepIds.every((stepId, index) => stepId === right.completedStepIds[index])
  );
}

function hasExactKeys(value, expected) {
  const actual = Object.keys(value).sort();
  return (
    actual.length === expected.length &&
    [...expected].sort().every((key, index) => actual[index] === key)
  );
}
