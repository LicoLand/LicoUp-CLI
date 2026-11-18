import process from "node:process";

const CARGO_TIMING_SUBCOMMANDS = new Set(["check", "test"]);

function unavailable(reason) {
  return Object.freeze({ status: "unavailable", reason });
}

function measured(value, source) {
  return Object.freeze({ status: "measured", value: Object.freeze(value), source });
}

function commandArgs(command) {
  return Array.isArray(command?.args) ? command.args : [];
}

function cargoSubcommandIndex(args) {
  return typeof args[0] === "string" && args[0].startsWith("+") ? 1 : 0;
}

function hasArgument(args, value, before = args.length) {
  return args.slice(0, before).some((argument) =>
    argument === value || argument.startsWith(`${value}=`));
}

function hasCargoJobsArgument(args, before = args.length) {
  return args.slice(0, before).some((argument) =>
    argument === "-j"
    || argument === "--jobs"
    || /^-j[0-9]+$/u.test(argument)
    || argument.startsWith("--jobs="));
}

function milliseconds(seconds) {
  return Math.round(Number(seconds) * 1_000_000) / 1_000;
}

function aggregateDurations(values) {
  if (values.length === 0) return null;
  const totalMs = values.reduce((total, value) => total + value, 0);
  return Object.freeze({
    count: values.length,
    totalMs: Math.round(totalMs * 1_000) / 1_000,
    minimumMs: Math.min(...values),
    maximumMs: Math.max(...values),
  });
}

export function isRustToolchainCommand(command) {
  if (command?.program !== "cargo") return false;
  const args = commandArgs(command);
  return CARGO_TIMING_SUBCOMMANDS.has(args[cargoSubcommandIndex(args)]);
}

export function verifiedLibtestReportTimeCapability(probe) {
  // A successful invocation of the selected test harness with
  // `--report-time --list` is the only accepted proof. Merely finding the
  // option in help is insufficient because stable libtest currently lists it
  // as requiring `-Z unstable-options`.
  return Object.freeze({
    supported: probe?.kind === "libtest-report-time-list"
      && probe?.exitCode === 0
      && probe?.requestedReportTime === true,
    source: "selected_harness_probe",
  });
}

function parsePositiveInteger(value) {
  if (value == null || value === "") return null;
  const parsed = Number(String(value).trim());
  if (!Number.isInteger(parsed) || parsed <= 0) return null;
  return parsed;
}

export function resolveRustLibtestThreads({
  internalConcurrency = null,
  environment = process.env,
} = {}) {
  if (internalConcurrency !== null && (!Number.isInteger(internalConcurrency) || internalConcurrency <= 0)) {
    throw new TypeError("internalConcurrency must be a positive integer or null");
  }
  const explicit = parsePositiveInteger(environment?.RUST_TEST_THREADS);
  if (explicit !== null) return explicit;
  return internalConcurrency;
}

export function decorateRustToolchainCommand(command, {
  cargoTimingsSupported = true,
  cargoJobs = null,
  libtestThreads = null,
  libtestReportTimeCapability = null,
} = {}) {
  if (cargoJobs !== null && (!Number.isInteger(cargoJobs) || cargoJobs <= 0)) {
    throw new TypeError("cargoJobs must be a positive integer or null");
  }
  if (libtestThreads !== null && (!Number.isInteger(libtestThreads) || libtestThreads <= 0)) {
    throw new TypeError("libtestThreads must be a positive integer or null");
  }
  if (!isRustToolchainCommand(command)) {
    return Object.freeze({
      command,
      instrumentation: Object.freeze({
        cargoTimingsRequested: false,
        libtestReportTimeRequested: false,
        cargoJobsRequested: null,
        cargoJobsAdded: false,
        libtestThreadsRequested: null,
        libtestThreadsAdded: false,
      }),
    });
  }

  const args = [...commandArgs(command)];
  const subcommandIndex = cargoSubcommandIndex(args);
  const isTest = args[subcommandIndex] === "test";
  let separator = args.indexOf("--", subcommandIndex + 1);
  const cargoBoundary = separator < 0 ? args.length : separator;
  const cargoTimingsPresent = hasArgument(args, "--timings", cargoBoundary);
  const cargoTimingsAdded = cargoTimingsSupported && !cargoTimingsPresent;
  if (cargoTimingsAdded) {
    args.splice(cargoBoundary, 0, "--timings");
    if (separator >= 0) separator += 1;
  }
  const updatedCargoBoundary = separator < 0 ? args.length : separator;
  const cargoJobsPresent = hasCargoJobsArgument(args, updatedCargoBoundary);
  const cargoJobsAdded = cargoJobs !== null && !cargoJobsPresent;
  if (cargoJobsAdded) {
    args.splice(updatedCargoBoundary, 0, `--jobs=${cargoJobs}`);
    if (separator >= 0) separator += 1;
  }

  const reportTimeVerified = libtestReportTimeCapability?.supported === true;
  const harnessHasReportTime = separator >= 0
    && hasArgument(args, "--report-time", args.length)
    && args.slice(separator + 1).some((argument) => argument === "--report-time");
  const harnessHasTestThreads = separator >= 0
    && args.slice(separator + 1).some((argument) =>
      argument === "--test-threads" || argument.startsWith("--test-threads="));
  const libtestThreadsAdded = isTest && libtestThreads !== null && !harnessHasTestThreads;
  if (libtestThreadsAdded) {
    if (separator < 0) {
      args.push("--", `--test-threads=${libtestThreads}`);
      separator = args.length - 2;
    } else {
      args.push(`--test-threads=${libtestThreads}`);
    }
  }
  let libtestReportTimeAdded = false;
  if (isTest && reportTimeVerified && !harnessHasReportTime) {
    if (separator < 0) {
      args.push("--", "--report-time");
    } else {
      args.push("--report-time");
    }
    libtestReportTimeAdded = true;
  }

  return Object.freeze({
    command: Object.freeze({ ...command, args: Object.freeze(args) }),
    instrumentation: Object.freeze({
      cargoTimingsRequested: cargoTimingsPresent || cargoTimingsAdded,
      cargoTimingsAdded,
      cargoJobsRequested: cargoJobsAdded ? cargoJobs : null,
      cargoJobsAdded,
      libtestThreadsRequested: libtestThreadsAdded ? libtestThreads : null,
      libtestThreadsAdded,
      libtestReportTimeRequested: reportTimeVerified
        && (harnessHasReportTime || libtestReportTimeAdded),
      libtestReportTimeAdded,
    }),
  });
}

export function parseRustToolchainTerminalOutput(output, {
  libtestReportTimeEnabled = false,
} = {}) {
  const text = Buffer.isBuffer(output) ? output.toString("utf8") : String(output || "");
  const suiteDurations = [];
  const caseDurations = [];
  for (const line of text.split(/\r?\n/u)) {
    const suite = line.match(/^test result: (?:ok|FAILED)\..*?finished in ([0-9]+(?:\.[0-9]+)?)s$/u);
    if (suite) suiteDurations.push(milliseconds(suite[1]));
    if (!libtestReportTimeEnabled) continue;
    // `--report-time` includes the test identity earlier on the line. Only
    // retain the final numeric duration token; names, paths, and output never
    // leave this in-memory parser.
    const testCase = line.match(/^test .+ \.\.\. (?:ok|FAILED|ignored) <([0-9]+(?:\.[0-9]+)?)s>$/u);
    if (testCase) caseDurations.push(milliseconds(testCase[1]));
  }

  const suites = aggregateDurations(suiteDurations);
  const cases = aggregateDurations(caseDurations);
  return Object.freeze({
    libtestSuiteWallTime: suites
      ? measured(suites, "libtest_terminal_summary")
      : unavailable("libtest_terminal_summary_absent"),
    libtestCaseWallTime: cases
      ? measured(cases, "libtest_report_time")
      : unavailable(libtestReportTimeEnabled
        ? "libtest_report_time_samples_absent"
        : "libtest_report_time_not_verified"),
  });
}

export function collectRustToolchainNativeMetrics({
  output,
  exitCode,
  instrumentation = {},
} = {}) {
  const terminal = parseRustToolchainTerminalOutput(output, {
    libtestReportTimeEnabled: instrumentation.libtestReportTimeRequested === true,
  });
  return Object.freeze({
    cargoBuildTimingReport: instrumentation.cargoTimingsRequested === true && exitCode === 0
      ? measured({ generated: true, format: "html", machineReadable: false }, "cargo_timings")
      : unavailable(instrumentation.cargoTimingsRequested === true
        ? "cargo_command_did_not_complete_successfully"
        : "cargo_timings_not_requested"),
    libtestSuiteWallTime: terminal.libtestSuiteWallTime,
    libtestCaseWallTime: terminal.libtestCaseWallTime,
    directCpuMs: unavailable("stable_rust_native_process_cpu_unavailable"),
    descendantCpuMs: unavailable("stable_rust_native_process_tree_cpu_unavailable"),
    peakResidentBytes: unavailable("stable_rust_native_peak_rss_unavailable"),
  });
}
