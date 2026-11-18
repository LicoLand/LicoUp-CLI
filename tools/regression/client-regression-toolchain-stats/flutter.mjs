import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { sanitizeError } from "../../scripts/lib/sanitize-error.mjs";

const TOOLCHAIN_RUNNER = "tools/scripts/client-toolchain-runner.mjs";
const FLUTTER_EXECUTABLES = new Set([
  "flutter",
  "flutter.bat",
  "flutter.cmd",
  "flutter.exe",
]);
const RESULT_VALUES = new Set(["success", "failure", "error"]);

export const FLUTTER_JSON_REPORTER_LINE_LIMIT = 1024 * 1024;
export const FLUTTER_FAILURE_DIAGNOSTIC_LIMIT = 32;

const FLUTTER_DIAGNOSTIC_TEXT_LIMIT = 360;

function measured(value) {
  return Object.freeze({ status: "measured", value });
}

function unavailable(reason) {
  return Object.freeze({ status: "unavailable", reason });
}

function normalizedScript(value) {
  return String(value || "").replaceAll("\\", "/");
}

function commandParts(command) {
  if (!command || command.program !== "node" || !Array.isArray(command.args)) {
    return null;
  }
  if (normalizedScript(command.args[0]) !== TOOLCHAIN_RUNNER) return null;
  const separator = command.args.indexOf("--");
  if (separator < 0) return null;
  const executable = path.basename(String(command.args[separator + 1] || "")).toLowerCase();
  const flutterArgs = command.args.slice(separator + 2);
  if (!FLUTTER_EXECUTABLES.has(executable) || flutterArgs[0] !== "test") return null;
  return { separator, flutterArgs };
}

export function isCompatibleFlutterTestCommand(command) {
  return commandParts(command) !== null;
}

export function flutterTestInputPaths(command) {
  const parts = commandParts(command);
  if (!parts) return Object.freeze([]);
  const tail = parts.flutterArgs.slice(1);
  const nameIndex = tail.indexOf("--name");
  const optionEnd = nameIndex >= 0 ? nameIndex : tail.length;
  return Object.freeze(tail.slice(0, optionEnd)
    .filter((value) => !value.startsWith("-")));
}

function withoutValueOption(args, longName, shortName) {
  const output = [];
  for (let index = 0; index < args.length; index += 1) {
    const value = args[index];
    if (value === longName || value === shortName) {
      index += 1;
      continue;
    }
    if (value.startsWith(`${longName}=`) ||
        (shortName && (value.startsWith(`${shortName}=`) ||
          (value.startsWith(shortName) && value.length > shortName.length)))) {
      continue;
    }
    output.push(value);
  }
  return output;
}

export function hasFlutterTestReporter(args) {
  return args.some((value, index) =>
    value === "--reporter" || value === "-r" ||
    value.startsWith("--reporter=") ||
    (value.startsWith("-r") && value.length > 2 && index > 0));
}

export function withFlutterJsonReporter(args) {
  return [
    ...withoutValueOption(args, "--reporter", "-r"),
    "--reporter=json",
  ];
}

export function boundedFlutterTestConcurrency(value, {
  availableParallelism = os.availableParallelism(),
} = {}) {
  const requested = Number(value);
  const available = Number(availableParallelism);
  if (!Number.isFinite(requested) || requested < 1) {
    throw new Error("Flutter test concurrency must be a positive finite number");
  }
  if (!Number.isFinite(available) || available < 1) {
    throw new Error("Available parallelism must be a positive finite number");
  }
  return Math.max(1, Math.min(Math.floor(requested), Math.floor(available)));
}

export function decorateFlutterTestCommand(command, {
  concurrency,
  availableParallelism = os.availableParallelism(),
} = {}) {
  const parts = commandParts(command);
  if (!parts) return Object.freeze({ supported: false, command });

  const boundedConcurrency = boundedFlutterTestConcurrency(concurrency, {
    availableParallelism,
  });
  let flutterArgs = withFlutterJsonReporter(parts.flutterArgs);
  flutterArgs = withoutValueOption(flutterArgs, "--concurrency", "-j");
  flutterArgs = [
    ...flutterArgs,
    `--concurrency=${boundedConcurrency}`,
  ];

  return Object.freeze({
    supported: true,
    concurrency: boundedConcurrency,
    command: Object.freeze({
      ...command,
      args: Object.freeze([
        ...command.args.slice(0, parts.separator + 2),
        ...flutterArgs,
      ]),
    }),
  });
}

function validNonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

function validEvent(value) {
  return value &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    typeof value.type === "string" &&
    validNonNegativeInteger(value.time);
}

function unavailableAggregate(reason) {
  return Object.freeze({
    suiteCount: unavailable(reason),
    testCount: unavailable(reason),
    passedCount: unavailable(reason),
    failedCount: unavailable(reason),
    skippedCount: unavailable(reason),
    totalTestDurationMs: unavailable(reason),
    longestTestDurationMs: unavailable(reason),
    directCpuMs: unavailable("flutter_test_reporter_metric_unavailable"),
    descendantCpuMs: unavailable("flutter_test_reporter_metric_unavailable"),
    peakResidentBytes: unavailable("flutter_test_reporter_metric_unavailable"),
  });
}

function normalizeDiagnosticText(value, limit = FLUTTER_DIAGNOSTIC_TEXT_LIMIT) {
  return sanitizeError(String(value ?? ""))
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/gu, " ")
    .replace(/\b(?:https?|wss?):\/\/[^\s<>"'`]+/giu, "<endpoint>")
    .replace(/\b(?:localhost|(?:\d{1,3}\.){3}\d{1,3})(?::\d{1,5})?\b/giu, "<endpoint>")
    .replace(/\b[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}\b/gu, "[redacted-email]")
    .replace(
      /\b(authorization|token|secret|password|api[-_ ]?key|private[-_ ]?key)\b\s*[:=]\s*(?:"[^"]*"|'[^']*'|\S+)/giu,
      "$1=[redacted]",
    )
    .replace(/~[\\/][^\s<>"'`]+/gu, "<local-path>")
    .replace(/\/(?:[^\s<>"'`/]+\/)+[^\s<>"'`]*/gu, "<local-path>")
    .replace(/[A-Za-z]:[\\/][^\s<>"'`]*/gu, "<local-path>")
    .replace(/\b[A-Za-z0-9+/_=-]{48,}\b/gu, "[redacted-value]")
    .replace(/\s+/gu, " ")
    .trim()
    .slice(0, limit);
}

function conciseFlutterError(value) {
  const lines = String(value ?? "")
    .split(/\r?\n/gu)
    .map((line) => normalizeDiagnosticText(line))
    .filter((line) => line && !/^#\d+\s/u.test(line));
  return normalizeDiagnosticText(lines.slice(0, 3).join(" | ")) ||
    "test_failed_without_safe_reporter_detail";
}

function repoRelativeTestPath(value, { repoRoot, commandCwd }) {
  if (typeof value !== "string" || value.length === 0 || !repoRoot) return null;
  let candidate = value;
  try {
    if (candidate.startsWith("file:")) candidate = fileURLToPath(candidate);
  } catch {
    return null;
  }
  if (!candidate.endsWith(".dart")) return null;
  if (/^[A-Za-z][A-Za-z0-9+.-]*:/u.test(candidate) &&
      !/^[A-Za-z]:[\\/]/u.test(candidate)) return null;
  const absolute = path.isAbsolute(candidate)
    ? path.resolve(candidate)
    : path.resolve(commandCwd || repoRoot, candidate);
  const relative = path.relative(path.resolve(repoRoot), absolute);
  if (!relative || relative === ".." || relative.startsWith(`..${path.sep}`) ||
      path.isAbsolute(relative)) return null;
  return relative.split(path.sep).join("/");
}

function repoRelativeInputPath(value, { repoRoot, commandCwd }) {
  if (typeof value !== "string" || value.length === 0 || !repoRoot) return null;
  if (/^[A-Za-z][A-Za-z0-9+.-]*:/u.test(value) && !/^[A-Za-z]:[\\/]/u.test(value)) {
    return null;
  }
  const absolute = path.isAbsolute(value)
    ? path.resolve(value)
    : path.resolve(commandCwd || repoRoot, value);
  const relative = path.relative(path.resolve(repoRoot), absolute);
  if (!relative || relative === ".." || relative.startsWith(`..${path.sep}`) ||
      path.isAbsolute(relative)) return null;
  return relative.split(path.sep).join("/").replace(/\/$/u, "");
}

function repoPathFromReporterValue(value, context) {
  const direct = repoRelativeTestPath(value, context);
  if (direct) return direct;
  if (typeof value !== "string") return null;
  const root = path.resolve(context.repoRoot || "");
  const rootIndex = root ? value.indexOf(root) : -1;
  if (rootIndex >= 0) {
    const dartEnd = value.indexOf(".dart", rootIndex);
    if (dartEnd >= 0) {
      const embedded = repoRelativeTestPath(value.slice(rootIndex, dartEnd + 5), context);
      if (embedded) return embedded;
    }
  }
  const relativeMatch = /(?:^|[\s("'`])((?:test|integration_test)\/[^\s<>"'`]+\.dart)\b/u
    .exec(value);
  return relativeMatch ? repoRelativeTestPath(relativeMatch[1], context) : null;
}

function reporterTestPath(test, suites, context) {
  const suiteId = test?.suiteID ?? test?.suiteId ?? test?.suite_id;
  const suitePath = suites.get(suiteId);
  if (suitePath) return suitePath;
  for (const value of [test?.url, test?.rootUrl, test?.root_url, test?.path, test?.name]) {
    const relative = repoPathFromReporterValue(value, context);
    if (relative) return relative;
  }
  return null;
}

function reporterStackLocation(value, context) {
  if (typeof value !== "string") return "";
  for (const line of value.split(/\r?\n/gu)) {
    const file = repoPathFromReporterValue(line, context);
    if (!file) continue;
    const location = /\.dart(?::|\s+)(\d+):(\d+)/u.exec(line);
    return location ? `${file}:${location[1]}:${location[2]}` : file;
  }
  return "";
}

export function createFlutterJsonStatsCollector({
  lineLimit = FLUTTER_JSON_REPORTER_LINE_LIMIT,
  failureLimit = FLUTTER_FAILURE_DIAGNOSTIC_LIMIT,
  repoRoot = null,
  commandCwd = repoRoot,
  inputFiles = [],
} = {}) {
  if (!Number.isInteger(lineLimit) || lineLimit < 1) {
    throw new Error("Flutter JSON reporter line limit must be a positive integer");
  }
  if (!Number.isInteger(failureLimit) || failureLimit < 1) {
    throw new Error("Flutter failure diagnostic limit must be a positive integer");
  }

  let pending = "";
  let discardingOversizedLine = false;
  let sawStart = false;
  let sawDone = false;
  const suiteIds = new Set();
  const suitePaths = new Map();
  const testStarts = new Map();
  const testDetails = new Map();
  const testErrors = new Map();
  const completedTests = new Set();
  const normalizedInputs = inputFiles.map((input) =>
    repoRelativeInputPath(input, { repoRoot, commandCwd }));
  const executedInputTests = normalizedInputs.map(() => 0);
  const failedInputIndexes = new Set();
  const testInputIndexes = new Map();
  let unmatchedEvidence = false;
  const failures = [];
  let testCount = 0;
  let passedCount = 0;
  let failedCount = 0;
  let skippedCount = 0;
  let durationCount = 0;
  let totalTestDurationMs = 0;
  let longestTestDurationMs = 0;

  const accept = (event) => {
    if (!validEvent(event)) return;
    if (event.type === "start") {
      if (typeof event.protocolVersion !== "string" ||
          !/^0\.1\.\d+$/u.test(event.protocolVersion)) return;
      sawStart = true;
      return;
    }
    if (!sawStart) return;
    if (event.type === "suite") {
      if (validNonNegativeInteger(event.suite?.id)) {
        suiteIds.add(event.suite.id);
        const relative = repoPathFromReporterValue(event.suite.path, { repoRoot, commandCwd });
        if (relative) suitePaths.set(event.suite.id, relative);
      }
      return;
    }
    if (event.type === "testStart") {
      if (validNonNegativeInteger(event.test?.id) && !testStarts.has(event.test.id)) {
        testStarts.set(event.test.id, event.time);
        const file = reporterTestPath(event.test, suitePaths, { repoRoot, commandCwd });
        testDetails.set(event.test.id, {
          file,
          name: normalizeDiagnosticText(event.test.name, 240) || "unnamed_test",
        });
        const indexes = normalizedInputs.flatMap((input, index) =>
          input && file && (file === input || file.startsWith(`${input}/`)) ? [index] : []);
        testInputIndexes.set(event.test.id, indexes);
      }
      return;
    }
    if (event.type === "error" && validNonNegativeInteger(event.testID) &&
        typeof event.error === "string") {
      const errors = testErrors.get(event.testID) || [];
      if (errors.length < 2) {
        const location = reporterStackLocation(event.stackTrace, { repoRoot, commandCwd });
        errors.push(`${conciseFlutterError(event.error)}${location ? ` @ ${location}` : ""}`);
      }
      testErrors.set(event.testID, errors);
      return;
    }
    if (event.type === "testDone") {
      if (!validNonNegativeInteger(event.testID) ||
          typeof event.hidden !== "boolean" ||
          typeof event.skipped !== "boolean" ||
          !RESULT_VALUES.has(event.result) ||
          completedTests.has(event.testID)) return;
      completedTests.add(event.testID);
      const startedAt = testStarts.get(event.testID);
      testStarts.delete(event.testID);
      const detail = testDetails.get(event.testID);
      testDetails.delete(event.testID);
      const inputIndexes = testInputIndexes.get(event.testID) || [];
      testInputIndexes.delete(event.testID);
      const errors = testErrors.get(event.testID) || [];
      testErrors.delete(event.testID);
      const executed = event.result !== "success" || (!event.hidden && !event.skipped);
      if (executed && inputIndexes.length === 0) unmatchedEvidence = true;
      for (const index of inputIndexes) {
        if (executed) executedInputTests[index] += 1;
        if (event.result !== "success") failedInputIndexes.add(index);
      }
      if (event.result !== "success" && failures.length < failureLimit) {
        failures.push(Object.freeze({
          file: detail?.file || "unknown_test_file",
          name: detail?.name || "unnamed_test",
          error: errors.join(" | ").slice(0, FLUTTER_DIAGNOSTIC_TEXT_LIMIT) ||
            "test_failed_without_safe_reporter_detail",
        }));
      }
      if (event.hidden) return;
      testCount += 1;
      if (event.skipped) skippedCount += 1;
      else if (event.result === "success") passedCount += 1;
      else failedCount += 1;
      if (validNonNegativeInteger(startedAt) && event.time >= startedAt) {
        const duration = event.time - startedAt;
        durationCount += 1;
        totalTestDurationMs += duration;
        longestTestDurationMs = Math.max(longestTestDurationMs, duration);
      }
      return;
    }
    if (event.type === "done" && typeof event.success === "boolean") {
      sawDone = true;
    }
  };

  const acceptLine = (line) => {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) return;
    try {
      accept(JSON.parse(trimmed));
    } catch {
      // Wrapper chatter and malformed JSON are not reporter evidence.
    }
  };

  const push = (chunk) => {
    const text = Buffer.isBuffer(chunk) ? chunk.toString("utf8") : String(chunk ?? "");
    const fragments = text.split("\n");
    for (const [index, fragment] of fragments.entries()) {
      const terminated = index < fragments.length - 1;
      if (discardingOversizedLine) {
        if (terminated) discardingOversizedLine = false;
        continue;
      }
      const candidate = pending + fragment;
      if (candidate.length > lineLimit) {
        pending = "";
        discardingOversizedLine = !terminated;
        continue;
      }
      pending = candidate;
      if (terminated) {
        acceptLine(pending.endsWith("\r") ? pending.slice(0, -1) : pending);
        pending = "";
      }
    }
  };

  const finish = () => {
    if (pending && !discardingOversizedLine) acceptLine(pending);
    pending = "";
    if (!sawStart || !sawDone) return unavailableAggregate("flutter_json_reporter_incomplete");
    const durationsComplete = durationCount === testCount;
    return Object.freeze({
      suiteCount: measured(suiteIds.size),
      testCount: measured(testCount),
      passedCount: measured(passedCount),
      failedCount: measured(failedCount),
      skippedCount: measured(skippedCount),
      totalTestDurationMs: durationsComplete
        ? measured(totalTestDurationMs)
        : unavailable("flutter_test_duration_incomplete"),
      longestTestDurationMs: durationsComplete
        ? measured(longestTestDurationMs)
        : unavailable("flutter_test_duration_incomplete"),
      directCpuMs: unavailable("flutter_test_reporter_metric_unavailable"),
      descendantCpuMs: unavailable("flutter_test_reporter_metric_unavailable"),
      peakResidentBytes: unavailable("flutter_test_reporter_metric_unavailable"),
    });
  };

  const failureDiagnostics = () => Object.freeze([...failures]);

  const attribution = () => {
    const complete = sawStart && sawDone && !unmatchedEvidence && testStarts.size === 0 &&
      normalizedInputs.length > 0 && normalizedInputs.every(Boolean) &&
      executedInputTests.every((count) => count > 0);
    return Object.freeze({
      attributionComplete: complete,
      failedInputIndexes: Object.freeze([...failedInputIndexes]
        .sort((left, right) => left - right)),
    });
  };

  return Object.freeze({ push, finish, failureDiagnostics, attribution });
}
