import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

export const CLIENT_REGRESSION_REPORT_SCHEMA = "licoup.client-regression-report.v1";
export const LEGACY_INCOMPLETE_BASELINE_MS = 1_161_000;
const PRIVATE_DIAGNOSTIC_PATTERN =
  /^build\/private\/client-regression\/[a-z0-9_.-]+\.log$/iu;

function terminalStatus(results, compatibility) {
  return results.some((result) => result.status !== "passed") ||
      compatibility.some((row) => row.status === "failed")
    ? "failed"
    : "passed";
}

function reason(value, fallback = "metric_not_reported") {
  return /^[a-z0-9_.:+-]{1,160}$/u.test(value || "") ? value : fallback;
}

function unavailable(value) {
  return Object.freeze({
    status: "unavailable",
    reason: reason(value?.reason),
  });
}

function numericMetric(value) {
  return value?.status === "measured" && Number.isFinite(value.value) && value.value >= 0
    ? Object.freeze({ status: "measured", value: value.value })
    : unavailable(value);
}

function durationAggregate(value) {
  const aggregate = value?.value;
  if (value?.status !== "measured" || !aggregate ||
      !["count", "totalMs", "minimumMs", "maximumMs"].every((key) =>
        Number.isFinite(aggregate[key]) && aggregate[key] >= 0)) {
    return unavailable(value);
  }
  return Object.freeze({
    status: "measured",
    value: Object.freeze({
      count: aggregate.count,
      totalMs: aggregate.totalMs,
      minimumMs: aggregate.minimumMs,
      maximumMs: aggregate.maximumMs,
    }),
  });
}

function safeToolchainNative(value) {
  const kind = ["rust", "flutter", "node", "node-test", "gradle", "compatibility"]
    .includes(value?.kind) ? value.kind : "unknown";
  if (value?.status === "unavailable") {
    return Object.freeze({ kind, status: "unavailable", reason: reason(value.reason) });
  }
  if (kind === "rust") {
    const cargo = value?.cargoBuildTimingReport;
    const safeCargo = cargo?.status === "measured" && cargo.value?.generated === true
      ? Object.freeze({
        status: "measured",
        value: Object.freeze({ generated: true, format: "html", machineReadable: false }),
      })
      : unavailable(cargo);
    return Object.freeze({
      kind,
      cargoBuildTimingReport: safeCargo,
      libtestSuiteWallTime: durationAggregate(value?.libtestSuiteWallTime),
      libtestCaseWallTime: durationAggregate(value?.libtestCaseWallTime),
    });
  }
  if (kind === "flutter") {
    return Object.freeze({
      kind,
      suiteCount: numericMetric(value?.suiteCount),
      testCount: numericMetric(value?.testCount),
      passedCount: numericMetric(value?.passedCount),
      failedCount: numericMetric(value?.failedCount),
      skippedCount: numericMetric(value?.skippedCount),
      totalTestDurationMs: numericMetric(value?.totalTestDurationMs),
      longestTestDurationMs: numericMetric(value?.longestTestDurationMs),
    });
  }
  return Object.freeze({
    kind,
    status: "unavailable",
    reason: "toolchain_native_metrics_unavailable",
  });
}

function safeMetrics(value) {
  return Object.freeze({
    wallTimeMs: numericMetric(value?.wallTimeMs),
    directCpuMs: numericMetric(value?.directCpuMs),
    descendantCpuMs: numericMetric(value?.descendantCpuMs),
    peakResidentBytes: numericMetric(value?.peakResidentBytes),
    toolchainNative: safeToolchainNative(value?.toolchainNative),
  });
}

function safeDiagnosticLog(value, status) {
  if (value === undefined || value === null) return null;
  if (!["failed", "attribution-pending"].includes(status) ||
      typeof value !== "string" || !PRIVATE_DIAGNOSTIC_PATTERN.test(value)) {
    throw new Error("client regression diagnostic log reference is invalid");
  }
  return value;
}

function safeEvidenceHead(value) {
  if (value === undefined || value === null) return null;
  if (typeof value !== "string" || !/^[a-f0-9]{40}$/u.test(value)) {
    throw new Error("client regression evidence head is invalid");
  }
  return value;
}

export function createClientRegressionReport({
  runKind,
  startedAt,
  completedAt,
  durationMs,
  results,
  concurrency,
  compatibility = [],
  candidateHead = null,
  sourceStateDigest = null,
}) {
  if (candidateHead !== null && !/^[a-f0-9]{40}$/u.test(candidateHead)) {
    throw new Error("client regression candidate head is invalid");
  }
  if (sourceStateDigest !== null && !/^sha256:[a-f0-9]{64}$/u.test(sourceStateDigest)) {
    throw new Error("client regression source-state digest is invalid");
  }
  const safeResults = results.map((result) => {
    const diagnosticLog = safeDiagnosticLog(result.diagnosticLog, result.status);
    const evidenceHead = safeEvidenceHead(result.evidenceHead);
    return Object.freeze({
      id: result.id,
      stage: result.stage,
      lane: result.lane,
      toolchain: result.toolchain,
      status: result.status,
      reason: result.reason ? reason(result.reason, "execution_failed") : null,
      durationMs: result.durationMs,
      members: Object.freeze([...result.members]),
      metrics: safeMetrics(result.metrics),
      ...(evidenceHead ? { evidenceHead } : {}),
      ...(diagnosticLog ? { diagnosticLog } : {}),
    });
  });
  const safeCompatibility = compatibility.map((row) => Object.freeze({
    id: row.id,
    kind: row.kind,
    status: row.status,
    reason: row.reason ? reason(row.reason, "compatibility_failed") : null,
    staticStatus: row.staticStatus,
    liveStatus: row.liveStatus,
    durationMs: row.durationMs,
  }));
  return Object.freeze({
    schemaVersion: CLIENT_REGRESSION_REPORT_SCHEMA,
    ...(candidateHead ? { candidateHead } : {}),
    ...(sourceStateDigest ? { sourceStateDigest } : {}),
    runKind,
    complete: runKind === "complete",
    status: terminalStatus(safeResults, safeCompatibility),
    startedAt,
    completedAt,
    durationMs,
    concurrency: Object.freeze({ ...concurrency }),
    results: Object.freeze(safeResults),
    failures: Object.freeze(safeResults
      .filter((result) => ["failed", "attribution-pending", "blocked"].includes(result.status))
      .map((result) => Object.freeze({
        id: result.id,
        reason: result.reason,
        members: result.members,
        ...(result.diagnosticLog ? { diagnosticLog: result.diagnosticLog } : {}),
      }))),
    compatibility: Object.freeze(safeCompatibility),
    legacyBaseline: Object.freeze({
      status: "incomplete",
      lowerBoundDurationMs: LEGACY_INCOMPLETE_BASELINE_MS,
      completedDurationMs: null,
      resourceMetrics: "unavailable",
      speedupPercent: null,
    }),
  });
}

export async function writeClientRegressionReport(report, reportPath) {
  await mkdir(path.dirname(reportPath), { recursive: true });
  const temporary = `${reportPath}.tmp`;
  await writeFile(temporary, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  await rename(temporary, reportPath);
}

export async function retrySelectionFromReport(reportPath, {
  validModuleIds,
  validCompatibilityIds,
}) {
  const report = JSON.parse(await readFile(reportPath, "utf8"));
  if (report?.schemaVersion !== CLIENT_REGRESSION_REPORT_SCHEMA) {
    throw new Error("client regression retry report schema is invalid");
  }
  const allowedModules = new Set(validModuleIds);
  const allowedCompatibility = new Set(validCompatibilityIds);
  const moduleIds = [];
  for (const result of report.results || []) {
    if (!["failed", "attribution-pending", "blocked"].includes(result.status)) continue;
    if (result.stage === "compatibility") continue;
    for (const id of result.members || []) {
      if (!allowedModules.has(id)) {
        throw new Error("client regression retry report references an unknown module");
      }
      moduleIds.push(id);
    }
  }
  const compatibilityIds = [];
  for (const row of report.compatibility || []) {
    if (row.status !== "failed") continue;
    const id = `${row.kind}:${row.id}`;
    if (!allowedCompatibility.has(id)) {
      throw new Error("client regression retry report references an unknown compatibility target");
    }
    compatibilityIds.push(id);
  }
  return Object.freeze({
    moduleIds: Object.freeze([...new Set(moduleIds)]),
    compatibilityIds: Object.freeze([...new Set(compatibilityIds)]),
  });
}
