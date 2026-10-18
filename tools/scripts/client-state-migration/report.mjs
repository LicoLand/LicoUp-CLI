import { MigrationStateError } from "./errors.mjs";
import {
  completedStepIdsFor,
  missingStepIdsFor,
  planSteps,
} from "./frontier.mjs";
import { loadDomainMarker, loadLedger, updateHandoffState } from "./ledger.mjs";
import { GATEWAY_CUSTODY_DOMAIN, gatewayCredentialMigrationDisposition, isRepairableShape, probeDomain, shapeFor } from "./probe.mjs";
import { compareProductVersion } from "./util.mjs";

export const REPORT_SCHEMA = "v0.0.1:client-state-migration-report-1";

export const VERDICT_EXIT_CODES = Object.freeze({
  healthy: 0,
  behind: 2,
  ahead: 3,
  invalid: 4,
  // A root that awaits the protected custody operation is incomplete, not
  // healthy: the tool must not certify it.
  pending_authorization: 5,
});

export const USAGE_EXIT_CODE = 64;

// Worst first. `invalid` is the fail-closed bucket: the tool will not certify a
// state it cannot fully read. `ahead` outranks `behind` because the admission is
// forward-only, so a state from a newer binary is the more urgent finding.
// Worst first. A root that is both behind and awaiting the protected custody
// operation still reports `behind` — the migration is the actionable finding —
// but a root that is otherwise current and only awaits custody is incomplete,
// never healthy.
const VERDICT_ORDER = Object.freeze([
  "invalid",
  "ahead",
  "behind",
  "pending_authorization",
  "healthy",
]);

const INVALID_CODES = Object.freeze([
  "unsupported_state_shape",
  "migration_ledger_invalid",
  "migration_plan_gap",
  "migration_plan_ambiguous",
  "migration_frontier_incomplete",
  "update_handoff_pending",
  // A durable store this runtime has no reader for. Fail closed, but name the
  // capability rather than blaming the state.
  "probe_capability_unavailable",
]);

/**
 * Read-only evaluation of one data root. Nothing here writes: `status` and
 * `doctor` differ only in how they render this report.
 */
export function evaluateMigrationState({
  root,
  frontier,
  binaryProductVersion,
  platform = process.platform,
}) {
  const ledger = loadLedger(root);
  const codes = [];
  if (ledger.code !== null) codes.push({ code: ledger.code, domainId: null });
  const handoff = updateHandoffState(root);
  if (handoff.code !== null) {
    codes.push({ code: handoff.code, domainId: null });
  } else if (handoff.present) {
    // The admission claims the handoff before it reads the ledger, and this
    // tool does not re-implement that claim. A valid pending handoff is
    // therefore reported as pending rather than certified.
    codes.push({ code: "update_handoff_pending", domainId: null });
  }
  const ledgerDocument = ledger.document;
  if (
    ledgerDocument !== null &&
    compareProductVersion(binaryProductVersion, ledgerDocument.highestAdmittedProductVersion) < 0
  ) {
    // The admission refuses to run at all in this direction.
    codes.push({ code: "state_newer_than_binary", domainId: null });
  }
  if (ledgerDocument !== null) {
    // The admission converts only the declared source and treats its own
    // target as a rerun. A ledger that names any other format — an unpublished
    // spelling, an unpublished correction, or an older published format — has
    // no conversion path and is refused, not certified.
    if (
      ledgerDocument.frontierId !== frontier.sourceFrontierId &&
      ledgerDocument.frontierId !== frontier.frontierId
    ) {
      codes.push({ code: "unsupported_state_shape", domainId: null });
    }
    for (const domainId of Object.keys(ledgerDocument.domains)) {
      if (!frontier.domains.some((domain) => domain.domainId === domainId)) {
        codes.push({ code: "migration_ledger_invalid", domainId });
      }
    }
  }

  const evaluations = frontier.domains.map((domain) =>
    evaluateDomain({ root, domain, ledgerDocument, platform }),
  );
  const domains = evaluations.map((evaluation) => evaluation.domain);
  for (const evaluation of evaluations) {
    for (const code of evaluation.codes) codes.push({ code, domainId: evaluation.domain.domainId });
  }

  return Object.freeze({
    root,
    frontierId: frontier.frontierId,
    binaryProductVersion,
    ledger: Object.freeze({
      present: ledger.present,
      readable: ledgerDocument !== null,
      frontierId: ledgerDocument?.frontierId ?? null,
      highestAdmittedProductVersion:
        ledgerDocument?.highestAdmittedProductVersion ?? null,
    }),
    domains: Object.freeze(domains),
    codes: Object.freeze(codes),
    unverified: Object.freeze(
      domains
        .filter((domain) => domain.unverified !== null)
        .map((domain) => Object.freeze({ domainId: domain.domainId, reason: domain.unverified })),
    ),
    verdict: verdictOf(codes, domains),
  });
}

/**
 * The admission's own version resolution: a store that reports a version above
 * zero is authoritative, otherwise the domain's own marker is. A marker that
 * claims more than the store shows, or a legacy store hidden by a current
 * marker, is an unsupported shape rather than a version.
 */
export function resolveDomainVersion({ domain, observation, markerSchemaVersion }) {
  const store = observation.storeSchemaVersion;
  if (store === null) return { version: null, codes: [] };
  const codes = [];
  if (store > domain.targetSchemaVersion) codes.push("state_newer_than_binary");
  if (store > 0) {
    if (markerSchemaVersion !== null && markerSchemaVersion > store) {
      codes.push("unsupported_state_shape");
    }
    return { version: store, codes };
  }
  if (observation.present && markerSchemaVersion !== null && markerSchemaVersion !== 0) {
    codes.push("unsupported_state_shape");
  }
  return { version: markerSchemaVersion ?? 0, codes };
}

function evaluateDomain({ root, domain, ledgerDocument, platform }) {
  const domainCodes = [];
  const shape = shapeFor(domain.domainId);
  const observation = probeDomain({ root, domain, platform });
  if (observation.code !== null) domainCodes.push(observation.code);

  let markerSchemaVersion = null;
  try {
    markerSchemaVersion = loadDomainMarker(root, domain.domainId)?.authoritativeSchemaVersion ?? null;
  } catch (error) {
    if (!(error instanceof MigrationStateError)) throw error;
    domainCodes.push(error.code);
  }
  if (markerSchemaVersion !== null && markerSchemaVersion > domain.targetSchemaVersion) {
    domainCodes.push("state_newer_than_binary");
  }

  const resolved = resolveDomainVersion({ domain, observation, markerSchemaVersion });
  domainCodes.push(...resolved.codes);
  // A store this tool cannot fully read is never certified: `unverified` is a
  // fail-closed finding, not a footnote next to a green verdict.
  if (observation.unverified !== null) domainCodes.push("probe_capability_unavailable");
  let observedSchemaVersion = resolved.version;
  let pendingAuthorization = false;
  if (domain.domainId === GATEWAY_CUSTODY_DOMAIN && observedSchemaVersion === 0) {
    const disposition = gatewayCredentialMigrationDisposition(platform);
    pendingAuthorization = disposition === "requires-authorization";
    if (disposition === "not-applicable") observedSchemaVersion = domain.targetSchemaVersion;
  }

  const ledgerEntry = ledgerDocument?.domains[domain.domainId] ?? null;
  if (ledgerEntry !== null) {
    if (
      (observedSchemaVersion !== null && ledgerEntry.schemaVersion > observedSchemaVersion) ||
      ledgerEntry.schemaVersion > domain.targetSchemaVersion ||
      !sameStepIds(ledgerEntry.completedStepIds, completedStepIdsFor(domain, ledgerEntry.schemaVersion))
    ) {
      domainCodes.push("migration_ledger_invalid");
    }
  }

  const plan =
    observedSchemaVersion === null
      ? { steps: [], code: null }
      : planSteps(domain, observedSchemaVersion);
  if (plan.code !== null) domainCodes.push(plan.code);

  const missingStepIds =
    observedSchemaVersion !== null && observedSchemaVersion < domain.targetSchemaVersion
      ? missingStepIdsFor(domain, observedSchemaVersion)
      : [];

  const uniqueCodes = [...new Set(domainCodes)];
  const verdict = domainVerdict({
    domainCodes: uniqueCodes,
    observedSchemaVersion,
    targetSchemaVersion: domain.targetSchemaVersion,
    pendingAuthorization,
  });
  const evaluated = Object.freeze({
    domainId: domain.domainId,
    durability: domain.durability,
    startupScope: domain.startupScope,
    shape: observation.shape,
    targetSchemaVersion: domain.targetSchemaVersion,
    observedSchemaVersion,
    storeSchemaVersion: observation.storeSchemaVersion,
    documentSchemaVersion: observation.documentSchemaVersion,
    markerSchemaVersion,
    ledgerSchemaVersion: ledgerEntry?.schemaVersion ?? null,
    completedStepIds: Object.freeze(
      ledgerEntry === null ? [] : [...ledgerEntry.completedStepIds],
    ),
    missingStepIds: Object.freeze(missingStepIds),
    pendingAuthorization,
    // A domain with any diagnosis is not repairable, whatever its shape says.
    repairable:
      uniqueCodes.length === 0 && isRepairableShape(shape, domain.targetSchemaVersion),
    unverified: observation.unverified,
    verdict,
    codes: Object.freeze(uniqueCodes),
  });
  return { domain: evaluated, codes: uniqueCodes };
}

function domainVerdict({
  domainCodes,
  observedSchemaVersion,
  targetSchemaVersion,
  pendingAuthorization,
}) {
  if (domainCodes.some((code) => INVALID_CODES.includes(code))) return "invalid";
  if (domainCodes.includes("state_newer_than_binary")) return "ahead";
  if (pendingAuthorization) return "pending_authorization";
  if (observedSchemaVersion === null) return "invalid";
  return observedSchemaVersion < targetSchemaVersion ? "behind" : "healthy";
}

function verdictOf(codes, domains) {
  const verdicts = new Set(domains.map((domain) => domain.verdict));
  for (const entry of codes) verdicts.add(verdictCode(entry.code));
  for (const verdict of VERDICT_ORDER) {
    if (verdict !== "healthy" && verdicts.has(verdict)) return verdict;
  }
  return "healthy";
}

function verdictCode(code) {
  if (code === "state_newer_than_binary") return "ahead";
  return INVALID_CODES.includes(code) ? "invalid" : "healthy";
}

function sameStepIds(left, right) {
  return left.length === right.length && left.every((stepId, index) => stepId === right[index]);
}

export function exitCodeForVerdict(verdict) {
  return VERDICT_EXIT_CODES[verdict] ?? VERDICT_EXIT_CODES.invalid;
}

/**
 * The one versioned envelope. It carries codes, versions and step ids only:
 * never a path, a stored value, a prompt or credential material.
 */
export function migrationEnvelope({ command, report, mutations = [] }) {
  return Object.freeze({
    schemaVersion: REPORT_SCHEMA,
    command,
    verdict: report.verdict,
    exitCode: exitCodeForVerdict(report.verdict),
    frontierId: report.frontierId,
    binaryProductVersion: report.binaryProductVersion,
    ledger: {
      present: report.ledger.present,
      readable: report.ledger.readable,
      frontierId: report.ledger.frontierId,
      highestAdmittedProductVersion: report.ledger.highestAdmittedProductVersion,
    },
    domains: report.domains.map((domain) => ({
      domainId: domain.domainId,
      durability: domain.durability,
      startupScope: domain.startupScope,
      shape: domain.shape,
      targetSchemaVersion: domain.targetSchemaVersion,
      observedSchemaVersion: domain.observedSchemaVersion,
      documentSchemaVersion: domain.documentSchemaVersion,
      markerSchemaVersion: domain.markerSchemaVersion,
      ledgerSchemaVersion: domain.ledgerSchemaVersion,
      completedStepIds: [...domain.completedStepIds],
      missingStepIds: [...domain.missingStepIds],
      pendingAuthorization: domain.pendingAuthorization,
      repairable: domain.repairable,
      unverified: domain.unverified,
      verdict: domain.verdict,
      codes: [...domain.codes],
    })),
    codes: report.codes.map((entry) => ({ code: entry.code, domainId: entry.domainId })),
    unverified: report.unverified.map((entry) => ({ ...entry })),
    mutations: mutations.map((mutation) => ({ ...mutation })),
  });
}

export function humanReport({ command, report, mutations = [] }) {
  const lines = [`licoup client-state migration ${command}`];
  lines.push(`frontier      ${report.frontierId}`);
  lines.push(`binary        ${report.binaryProductVersion}`);
  lines.push(
    `ledger        ${describeLedger(report)}`,
  );
  lines.push(`verdict       ${report.verdict} (exit ${exitCodeForVerdict(report.verdict)})`);
  if (mutations.length > 0) {
    for (const mutation of mutations) {
      lines.push(
        `applied       ${mutation.stepId ?? "none"} on ${mutation.domainId}` +
          `${mutation.applied ? "" : " (already recorded)"}`,
      );
    }
  }
  lines.push("");
  lines.push(
    "domain                          shape             target store marker ledger  state",
  );
  for (const domain of report.domains) {
    lines.push(
      [
        domain.domainId.padEnd(31),
        domain.shape.padEnd(17),
        String(domain.targetSchemaVersion).padStart(6),
        formatVersion(domain.observedSchemaVersion).padStart(5),
        formatVersion(domain.markerSchemaVersion).padStart(6),
        formatVersion(domain.ledgerSchemaVersion).padStart(6),
        ` ${describeDomainState(domain)}`,
      ].join(""),
    );
  }
  const pending = report.domains.filter((domain) => domain.missingStepIds.length > 0);
  if (pending.length > 0) {
    lines.push("");
    lines.push("pending steps");
    for (const domain of pending) {
      lines.push(`  ${domain.domainId}: ${domain.missingStepIds.join(", ")}`);
    }
  }
  if (report.codes.length > 0) {
    lines.push("");
    lines.push("codes");
    for (const entry of report.codes) {
      lines.push(`  ${entry.code}${entry.domainId === null ? "" : ` (${entry.domainId})`}`);
    }
  }
  if (report.unverified.length > 0) {
    lines.push("");
    lines.push("unverified");
    for (const entry of report.unverified) {
      lines.push(`  ${entry.domainId}: ${entry.reason}`);
    }
  }
  return `${lines.join("\n")}\n`;
}

/**
 * The gate view: one verdict, the blocking codes, and what could not be
 * verified. An unverified domain is named rather than counted as healthy.
 */
export function humanDoctor(report) {
  const lines = ["licoup client-state migration doctor"];
  lines.push(`frontier      ${report.frontierId}`);
  lines.push(`binary        ${report.binaryProductVersion}`);
  lines.push(`ledger        ${describeLedger(report)}`);
  lines.push(`verdict       ${report.verdict} (exit ${exitCodeForVerdict(report.verdict)})`);
  lines.push(
    `domains       ${report.domains.length} total, ` +
      `${countVerdict(report, "behind")} behind, ${countVerdict(report, "ahead")} ahead, ` +
      `${countVerdict(report, "invalid")} invalid, ` +
      `${countVerdict(report, "pending_authorization")} awaiting authorization`,
  );
  lines.push(
    `codes         ${report.codes.length === 0 ? "none" : report.codes
      .map((entry) => `${entry.code}${entry.domainId === null ? "" : ` (${entry.domainId})`}`)
      .join(", ")}`,
  );
  lines.push(
    `unverified    ${report.unverified.length === 0 ? "none" : report.unverified
      .map((entry) => `${entry.domainId} (${entry.reason})`)
      .join(", ")}`,
  );
  return `${lines.join("\n")}\n`;
}

function countVerdict(report, verdict) {
  return report.domains.filter((domain) => domain.verdict === verdict).length;
}

function describeLedger(report) {
  if (!report.ledger.present) return "absent (no admission recorded yet)";
  if (!report.ledger.readable) return "unreadable (migration_ledger_invalid)";
  return (
    `admitted by ${report.ledger.highestAdmittedProductVersion}` +
    ` (frontier ${report.ledger.frontierId})`
  );
}

function describeDomainState(domain) {
  if (domain.codes.length > 0) return domain.codes.join(",");
  if (domain.pendingAuthorization) return "awaiting authorized custody migration";
  if (domain.verdict === "behind") return `behind by ${domain.missingStepIds.length} step(s)`;
  if (domain.repairable) return "current (repairable)";
  return "current";
}

function formatVersion(value) {
  return value === null ? "-" : String(value);
}
