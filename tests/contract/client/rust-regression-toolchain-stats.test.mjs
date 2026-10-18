import assert from "node:assert/strict";
import test from "node:test";
import {
  collectRustToolchainNativeMetrics,
  decorateRustToolchainCommand,
  isRustToolchainCommand,
  parseRustToolchainTerminalOutput,
  resolveRustLibtestThreads,
  verifiedLibtestReportTimeCapability,
} from "../../../tools/regression/client-regression-toolchain-stats/rust.mjs";

const focusedCargoTest = Object.freeze({
  program: "cargo",
  args: Object.freeze([
    "test",
    "--manifest-path",
    "crates/licoup-native/Cargo.toml",
    "--lib",
    "state::tests::keeps_focus",
    "--",
    "--exact",
  ]),
  cwd: ".",
  timeoutMs: 120_000,
});

test("Rust stats recognizes only supported Cargo check and test commands", () => {
  assert.equal(isRustToolchainCommand(focusedCargoTest), true);
  assert.equal(isRustToolchainCommand({ program: "cargo", args: ["+stable", "check"] }), true);
  assert.equal(isRustToolchainCommand({ program: "cargo", args: ["build"] }), false);
  assert.equal(isRustToolchainCommand({ program: "node", args: ["test"] }), false);
});

test("Cargo timings are inserted before the preserved libtest suffix", () => {
  const decorated = decorateRustToolchainCommand(focusedCargoTest);
  assert.deepEqual(decorated.command.args, [
    "test",
    "--manifest-path",
    "crates/licoup-native/Cargo.toml",
    "--lib",
    "state::tests::keeps_focus",
    "--timings",
    "--",
    "--exact",
  ]);
  assert.equal(decorated.instrumentation.cargoTimingsRequested, true);
  assert.equal(decorated.instrumentation.cargoTimingsAdded, true);
  assert.equal(decorated.instrumentation.libtestReportTimeRequested, false);
  assert.deepEqual(focusedCargoTest.args.at(-2), "--");
});

test("Rust decoration is idempotent for native timing options", () => {
  const once = decorateRustToolchainCommand(focusedCargoTest);
  const twice = decorateRustToolchainCommand(once.command);
  assert.deepEqual(twice.command.args, once.command.args);
  assert.equal(twice.instrumentation.cargoTimingsRequested, true);
  assert.equal(twice.instrumentation.cargoTimingsAdded, false);
});

test("Cargo jobs are bounded before the harness suffix and remain idempotent", () => {
  const once = decorateRustToolchainCommand(focusedCargoTest, { cargoJobs: 3 });
  assert.deepEqual(once.command.args.slice(-3), ["--jobs=3", "--", "--exact"]);
  assert.equal(once.instrumentation.cargoJobsRequested, 3);
  assert.equal(once.instrumentation.cargoJobsAdded, true);

  const twice = decorateRustToolchainCommand(once.command, { cargoJobs: 3 });
  assert.deepEqual(twice.command.args, once.command.args);
  assert.equal(twice.instrumentation.cargoJobsRequested, null);
  assert.equal(twice.instrumentation.cargoJobsAdded, false);
});

test("explicit Cargo job settings win over the scheduler recommendation", () => {
  for (const explicit of [["-j", "2"], ["-j2"], ["--jobs", "2"], ["--jobs=2"]]) {
    const command = {
      ...focusedCargoTest,
      args: ["test", ...explicit, "--", "--exact"],
    };
    const decorated = decorateRustToolchainCommand(command, { cargoJobs: 3 });
    assert.equal(decorated.command.args.includes("--jobs=3"), false);
    assert.equal(decorated.instrumentation.cargoJobsAdded, false);
  }
});

test("Cargo job recommendations reject invalid capacity values", () => {
  for (const cargoJobs of [0, -1, 1.5, Number.NaN, "2"]) {
    assert.throws(
      () => decorateRustToolchainCommand(focusedCargoTest, { cargoJobs }),
      /cargoJobs must be a positive integer or null/u,
    );
  }
});

test("explicit serial libtest is honored independently of Cargo job concurrency", () => {
  assert.equal(resolveRustLibtestThreads({
    internalConcurrency: 4,
    environment: { RUST_TEST_THREADS: "1" },
  }), 1);
  assert.equal(resolveRustLibtestThreads({
    internalConcurrency: 4,
    environment: {},
  }), 4);
  assert.equal(resolveRustLibtestThreads({
    internalConcurrency: 4,
    environment: { RUST_TEST_THREADS: "bogus" },
  }), 4);

  const decorated = decorateRustToolchainCommand(focusedCargoTest, {
    cargoJobs: 4,
    libtestThreads: resolveRustLibtestThreads({
      internalConcurrency: 4,
      environment: { RUST_TEST_THREADS: "1" },
    }),
  });
  assert.equal(decorated.command.args.includes("--jobs=4"), true);
  assert.equal(decorated.command.args.includes("--test-threads=1"), true);
  assert.equal(decorated.command.args.includes("--test-threads=4"), false);
});

test("libtest threads are bounded without changing focused harness arguments", () => {
  const decorated = decorateRustToolchainCommand(focusedCargoTest, { libtestThreads: 2 });
  assert.deepEqual(decorated.command.args.slice(-3), ["--", "--exact", "--test-threads=2"]);
  assert.equal(decorated.instrumentation.libtestThreadsRequested, 2);
  assert.equal(decorated.instrumentation.libtestThreadsAdded, true);

  const repeated = decorateRustToolchainCommand(decorated.command, { libtestThreads: 2 });
  assert.deepEqual(repeated.command.args, decorated.command.args);
  assert.equal(repeated.instrumentation.libtestThreadsAdded, false);
});

test("explicit libtest thread settings win and Cargo check is untouched", () => {
  for (const explicit of [["--test-threads", "4"], ["--test-threads=4"]]) {
    const command = {
      ...focusedCargoTest,
      args: [...focusedCargoTest.args, ...explicit],
    };
    const decorated = decorateRustToolchainCommand(command, { libtestThreads: 2 });
    assert.equal(decorated.command.args.includes("--test-threads=2"), false);
    assert.equal(decorated.instrumentation.libtestThreadsAdded, false);
  }

  const check = decorateRustToolchainCommand(
    { program: "cargo", args: ["check"], cwd: ".", timeoutMs: 1_000 },
    { libtestThreads: 2 },
  );
  assert.equal(check.command.args.includes("--test-threads=2"), false);
  assert.equal(check.command.args.includes("--"), false);
});

test("libtest thread recommendations reject invalid capacity values", () => {
  for (const libtestThreads of [0, -1, 1.5, Number.NaN, "2"]) {
    assert.throws(
      () => decorateRustToolchainCommand(focusedCargoTest, { libtestThreads }),
      /libtestThreads must be a positive integer or null/u,
    );
  }
});

test("libtest report-time requires a successful selected-harness probe", () => {
  const rejected = verifiedLibtestReportTimeCapability({
    kind: "libtest-help-text",
    exitCode: 0,
    requestedReportTime: false,
  });
  assert.equal(rejected.supported, false);

  const verified = verifiedLibtestReportTimeCapability({
    kind: "libtest-report-time-list",
    exitCode: 0,
    requestedReportTime: true,
  });
  const decorated = decorateRustToolchainCommand(focusedCargoTest, {
    libtestReportTimeCapability: verified,
  });
  assert.deepEqual(decorated.command.args.slice(-3), ["--", "--exact", "--report-time"]);
  assert.equal(decorated.instrumentation.libtestReportTimeRequested, true);
});

test("terminal timing parsing returns anonymous deterministic aggregates", () => {
  const parsed = parseRustToolchainTerminalOutput([
    "running 2 tests",
    "test private::first ... ok <0.125s>",
    "test sensitive::second ... FAILED <1.500s>",
    "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.75s",
    "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.25s",
  ].join("\n"), { libtestReportTimeEnabled: true });

  assert.deepEqual(parsed.libtestSuiteWallTime.value, {
    count: 2,
    totalMs: 2_000,
    minimumMs: 250,
    maximumMs: 1_750,
  });
  assert.deepEqual(parsed.libtestCaseWallTime.value, {
    count: 2,
    totalMs: 1_625,
    minimumMs: 125,
    maximumMs: 1_500,
  });
  assert.doesNotMatch(JSON.stringify(parsed), /private|path|first|second/u);
});

test("native metrics never invent CPU or RSS and identify human-only Cargo timings", () => {
  const metrics = collectRustToolchainNativeMetrics({
    output: "test result: ok. 1 passed; 0 failed; finished in 0.01s\n",
    exitCode: 0,
    instrumentation: { cargoTimingsRequested: true, libtestReportTimeRequested: false },
  });
  assert.deepEqual(metrics.cargoBuildTimingReport.value, {
    generated: true,
    format: "html",
    machineReadable: false,
  });
  assert.equal(metrics.libtestSuiteWallTime.status, "measured");
  assert.equal(metrics.libtestCaseWallTime.status, "unavailable");
  assert.equal(metrics.directCpuMs.status, "unavailable");
  assert.equal(metrics.descendantCpuMs.status, "unavailable");
  assert.equal(metrics.peakResidentBytes.status, "unavailable");
});
