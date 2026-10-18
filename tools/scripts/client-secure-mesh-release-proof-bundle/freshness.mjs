import { freshnessWindows } from "./config.mjs";
import { maxReportFutureSkewSeconds } from "./constants.mjs";
import { reportRecord } from "./lists.mjs";

export function evaluateReportFreshness(report = {}, {
  label,
  maxAgeSeconds,
  checkedAt
} = {}) {
  report = reportRecord(report);
  const present = Boolean(report && Object.keys(report).length > 0);
  const timestampField = Object.prototype.hasOwnProperty.call(report, "generatedAt")
    ? "generatedAt"
    : Object.prototype.hasOwnProperty.call(report, "checkedAt")
      ? "checkedAt"
      : "";
  const timestamp = timestampField ? String(report[timestampField] || "") : "";
  const timestampMs = Date.parse(timestamp);
  const checkedAtMs = Date.parse(checkedAt);
  const nowMs = Number.isFinite(checkedAtMs) ? checkedAtMs : Date.now();
  const ageSeconds = Number.isFinite(timestampMs)
    ? Math.floor((nowMs - timestampMs) / 1000)
    : null;
  const freshUntilMs = Number.isFinite(timestampMs)
    ? timestampMs + (Number(maxAgeSeconds || 0) * 1000)
    : NaN;
  const failures = [];
  if (!present) {
    failures.push("report_present");
  }
  if (!timestampField) {
    failures.push("timestamp_present");
  }
  if (timestampField && !Number.isFinite(timestampMs)) {
    failures.push("timestamp_parseable");
  }
  if (Number.isFinite(timestampMs) && timestampMs - nowMs > maxReportFutureSkewSeconds * 1000) {
    failures.push("timestamp_not_future");
  }
  if (Number.isFinite(timestampMs) && nowMs - timestampMs > Number(maxAgeSeconds || 0) * 1000) {
    failures.push("timestamp_not_stale");
  }
  const ready = failures.length === 0;
  return {
    label,
    ready,
    status: ready
      ? "current"
      : failures.includes("report_present")
        ? "missing_report"
        : failures.includes("timestamp_not_stale")
        ? "stale"
        : failures.includes("timestamp_not_future")
          ? "future"
          : failures.includes("timestamp_parseable")
            ? "invalid_timestamp"
            : failures.includes("timestamp_present")
              ? "missing_timestamp"
              : "unknown",
    timestampField,
    generatedAt: timestamp,
    checkedAt,
    maxAgeSeconds: Number(maxAgeSeconds || 0),
    maxFutureSkewSeconds: maxReportFutureSkewSeconds,
    ageSeconds,
    freshUntil: Number.isFinite(freshUntilMs) ? new Date(freshUntilMs).toISOString() : "",
    failures,
    failureCount: failures.length
  };
}

export function summarizeReleaseInputFreshness({
  updateRelease = {},
  physicalMatrix = {},
  androidPhysicalInstallLaunch = {},
  physicalEvidenceManifest = {}
} = {}, checkedAt) {
  const checks = [
    evaluateReportFreshness(updateRelease, {
      label: "update release report",
      maxAgeSeconds: freshnessWindows.updateReleaseSeconds,
      checkedAt
    }),
    evaluateReportFreshness(physicalMatrix, {
      label: "physical device matrix report",
      maxAgeSeconds: freshnessWindows.physicalMatrixSeconds,
      checkedAt
    }),
    evaluateReportFreshness(androidPhysicalInstallLaunch, {
      label: "Android physical install/launch report",
      maxAgeSeconds: freshnessWindows.androidPhysicalInstallLaunchSeconds,
      checkedAt
    }),
    evaluateReportFreshness(physicalEvidenceManifest, {
      label: "physical evidence manifest report",
      maxAgeSeconds: freshnessWindows.physicalEvidenceManifestSeconds,
      checkedAt
    })
  ];
  const failed = checks.filter((check) => check.ready !== true);
  return {
    ready: failed.length === 0,
    checkedAt,
    checkCount: checks.length,
    currentCount: checks.length - failed.length,
    staleOrInvalidCount: failed.length,
    failedLabels: failed.map((check) => check.label),
    checks,
    remainingGates: failed.map((check) => `fresh release input required: ${check.label}`)
  };
}
