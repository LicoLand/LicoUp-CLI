import { evaluateReportFreshness } from "../freshness.mjs";

export function runReleaseInputFreshnessSelfTest() {
  const checkedAt = "2026-07-07T12:00:00.000Z";
  const current = evaluateReportFreshness({ generatedAt: "2026-07-07T11:59:00.000Z" }, {
    label: "self-test current",
    maxAgeSeconds: 300,
    checkedAt
  });
  const stale = evaluateReportFreshness({ generatedAt: "2026-07-07T11:00:00.000Z" }, {
    label: "self-test stale",
    maxAgeSeconds: 300,
    checkedAt
  });
  const future = evaluateReportFreshness({ generatedAt: "2026-07-07T12:10:01.000Z" }, {
    label: "self-test future",
    maxAgeSeconds: 300,
    checkedAt
  });
  const missing = evaluateReportFreshness({}, {
    label: "self-test missing",
    maxAgeSeconds: 300,
    checkedAt
  });
  const nullMissing = evaluateReportFreshness(null, {
    label: "self-test null missing",
    maxAgeSeconds: 300,
    checkedAt
  });
  const invalid = evaluateReportFreshness({ generatedAt: "not-a-date" }, {
    label: "self-test invalid",
    maxAgeSeconds: 300,
    checkedAt
  });
  const ok = current.ready === true &&
    stale.ready === false &&
    stale.status === "stale" &&
    future.ready === false &&
    future.status === "future" &&
    missing.ready === false &&
    missing.status === "missing_report" &&
    nullMissing.ready === false &&
    nullMissing.status === "missing_report" &&
    invalid.ready === false &&
    invalid.status === "invalid_timestamp";
  return {
    ok,
    currentAccepted: current.ready === true,
    staleRejected: stale.ready === false,
    futureRejected: future.ready === false,
    missingRejected: missing.ready === false,
    nullMissingRejected: nullMissing.ready === false,
    invalidTimestampRejected: invalid.ready === false
  };
}
