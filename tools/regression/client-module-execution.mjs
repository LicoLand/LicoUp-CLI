import { spawn } from "node:child_process";
import { chmod, mkdir, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { planClientRegressionBatches } from "./client-regression-batching.mjs";
import { partitionModulesByRunnableHost } from "./client-module-selection.mjs";
import {
  CLIENT_REGRESSION_STAGES,
  defaultRegressionCapacities,
} from "./client-regression-metadata.mjs";
import { defaultProcessTreeMetricsAdapter } from "./client-regression-metrics.mjs";
import {
  createFlutterJsonStatsCollector,
  decorateFlutterTestCommand,
  flutterTestInputPaths,
} from "./client-regression-toolchain-stats/flutter.mjs";
import {
  collectRustToolchainNativeMetrics,
  decorateRustToolchainCommand,
  isRustToolchainCommand,
  resolveRustLibtestThreads,
} from "./client-regression-toolchain-stats/rust.mjs";
import { withTestProcessLoopbackProxyExemption } from "../scripts/client-toolchain-env.mjs";
import {
  createClientRegressionReport,
  writeClientRegressionReport,
} from "./client-regression-report.mjs";
import {
  acquireTestArtifactLease,
  isManagedCargoTarget,
  resolveCargoTargetDirectory,
} from "../scripts/lib/test-artifact-lifecycle.mjs";

const NODE_TEST_ATTRIBUTION_REPORTER = "tools/regression/client-node-test-attribution-reporter.mjs";
const NODE_TEST_INPUTS_ENV = "LICO_CLIENT_NODE_TEST_INPUTS";
const NODE_TEST_ATTRIBUTION_SCHEMA = "licoup.node-test-attribution.v1";
const FLUTTER_DEPENDENCY_MODULE_ID = "regression.flutter-dependencies";
const FLUTTER_DEPENDENCY_RESOURCES = new Set(["flutter-cache", "gradle-cache"]);
const DEFAULT_CARGO_JOBS_BUDGET = 3;
const PRIVATE_DIAGNOSTIC_DIRECTORY = "build/private/client-regression";
const PRIVATE_DIAGNOSTIC_LIMIT = 2 * 1024 * 1024;

function containedWorkingDirectory(repoRoot, relativeCwd) {
  const root = path.resolve(repoRoot);
  const candidate = path.resolve(root, relativeCwd);
  const relative = path.relative(root, candidate);
  if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    throw new Error("module working directory escapes the repository");
  }
  return candidate;
}

function monotonicMilliseconds(started) {
  return Number(process.hrtime.bigint() - started) / 1_000_000;
}

function measured(value, source) {
  return Object.freeze({ status: "measured", value, source });
}

function createTailCollector(limit = 4 * 1024 * 1024) {
  let tail = "";
  return Object.freeze({
    push(chunk) {
      tail = `${tail}${Buffer.isBuffer(chunk) ? chunk.toString("utf8") : String(chunk ?? "")}`;
      if (tail.length > limit) tail = tail.slice(-limit);
    },
    finish() {
      const output = tail;
      tail = "";
      return output;
    },
  });
}

function diagnosticLogReference(batch) {
  const stableId = batch.members.length === 1 ? batch.members[0] : batch.id;
  const safeId = String(stableId || "batch")
    .replace(/[^a-z0-9_.-]+/giu, "_")
    .slice(0, 160) || "batch";
  return path.posix.join(PRIVATE_DIAGNOSTIC_DIRECTORY, `${safeId}.log`);
}

async function clearPrivateDiagnostic(repoRoot, reference) {
  await rm(path.resolve(repoRoot, reference), { force: true });
}

async function writePrivateDiagnostic(repoRoot, reference, { stdout, stderr }) {
  const destination = path.resolve(repoRoot, reference);
  const directory = path.dirname(destination);
  await mkdir(directory, { recursive: true, mode: 0o700 });
  await chmod(directory, 0o700);
  await writeFile(destination, [
    "[stdout]",
    stdout,
    "[stderr]",
    stderr,
    "",
  ].join("\n"), { mode: 0o600 });
  await chmod(destination, 0o600);
}

function createSafeReceiptCollector(limit = 1024 * 1024) {
  const output = createTailCollector(limit);
  return Object.freeze({
    push: output.push,
    finish() {
      const lines = output.finish().split(/\r?\n/u).reverse();
      for (const line of lines) {
        const trimmed = line.trim();
        if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) continue;
        try {
          const receipt = JSON.parse(trimmed);
          for (const key of ["reasonCode", "errorCode", "reason"]) {
            const value = receipt?.[key];
            if (/^[a-z0-9_.:+-]{1,160}$/u.test(value || "")) return value;
          }
        } catch {
          // Compatibility commands may emit private diagnostic text before
          // their bounded receipt. Only the final allowlisted code is kept.
        }
      }
      return null;
    },
  });
}

function createNodeTestAttributionCollector(batch, inputCount) {
  const output = createTailCollector(1024 * 1024);
  return Object.freeze({
    push: output.push,
    finish() {
      for (const line of output.finish().split(/\r?\n/u).reverse()) {
        const trimmed = line.trim();
        if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) continue;
        try {
          const receipt = JSON.parse(trimmed);
          const indexes = receipt?.failedInputIndexes;
          if (receipt?.schemaVersion !== NODE_TEST_ATTRIBUTION_SCHEMA ||
              receipt?.complete !== true || receipt?.inputCount !== inputCount ||
              !Array.isArray(indexes) || !indexes.every((index) =>
                Number.isInteger(index) && index >= 0 && index < inputCount)) {
            continue;
          }
          const failedIndexes = new Set(indexes);
          const failedMembers = batch.inputOwners
            .filter((owner) => owner.indexes.some((index) => failedIndexes.has(index)))
            .map((owner) => owner.member);
          return Object.freeze({
            attributionComplete: true,
            failedMembers: Object.freeze([...new Set(failedMembers)]),
          });
        } catch {
          // Only the numeric reporter receipt is accepted as attribution.
        }
      }
      return Object.freeze({
        attributionComplete: false,
        failedMembers: Object.freeze([]),
      });
    },
  });
}

function nodeTestInputs(command) {
  if (command.program !== "node" || command.args[0] !== "--test") return [];
  return command.args.slice(1).filter((argument) => !argument.startsWith("-"));
}

function relativeReporterPath(repoRoot, commandCwd) {
  const workingDirectory = path.resolve(repoRoot, commandCwd);
  const reporter = path.relative(
    workingDirectory,
    path.resolve(repoRoot, NODE_TEST_ATTRIBUTION_REPORTER),
  );
  const normalized = reporter.replaceAll("\\", "/");
  return normalized.startsWith(".") ? normalized : `./${normalized}`;
}

function cargoJobsBudget(environment) {
  const configured = environment?.CARGO_BUILD_JOBS;
  if (configured === undefined || String(configured).trim() === "") {
    return DEFAULT_CARGO_JOBS_BUDGET;
  }
  const normalized = String(configured).trim();
  if (!/^[1-9][0-9]*$/u.test(normalized)) {
    throw new Error("CARGO_BUILD_JOBS must be a positive integer");
  }
  return Number(normalized);
}

function flutterCommandWorkingDirectory(repoRoot, command) {
  const separator = command.args.indexOf("--");
  const runnerArgs = separator >= 0 ? command.args.slice(0, separator) : [];
  const cwdIndex = runnerArgs.indexOf("--cwd");
  const relative = cwdIndex >= 0 ? runnerArgs[cwdIndex + 1] : command.cwd;
  return containedWorkingDirectory(repoRoot, relative || ".");
}

function prepareToolchainCommand(batch, { repoRoot, environment = process.env } = {}) {
  if (isRustToolchainCommand(batch.command)) {
    const stdout = createTailCollector(2 * 1024 * 1024);
    const stderr = createTailCollector(2 * 1024 * 1024);
    const runnerBudget = cargoJobsBudget(environment);
    const decorated = decorateRustToolchainCommand(batch.command, {
      cargoJobs: Math.min(batch.internalConcurrency || runnerBudget, runnerBudget),
      libtestThreads: resolveRustLibtestThreads({
        internalConcurrency: batch.internalConcurrency,
        environment,
      }),
    });
    return Object.freeze({
      command: decorated.command,
      pushStdout: stdout.push,
      pushStderr: stderr.push,
      finish(exitCode) {
        return Object.freeze({
          kind: "rust",
          metrics: collectRustToolchainNativeMetrics({
            output: `${stdout.finish()}\n${stderr.finish()}`,
            exitCode,
            instrumentation: decorated.instrumentation,
          }),
        });
      },
    });
  }
  const decorated = decorateFlutterTestCommand(batch.command, {
    concurrency: batch.internalConcurrency || batch.weight,
  });
  if (decorated.supported) {
    const inputs = flutterTestInputPaths(decorated.command);
    const collector = createFlutterJsonStatsCollector({
      repoRoot,
      commandCwd: flutterCommandWorkingDirectory(repoRoot, decorated.command),
      inputFiles: inputs,
    });
    return Object.freeze({
      command: decorated.command,
      pushStdout: collector.push,
      // The JSON reporter is a stdout protocol. Keeping stderr separate
      // prevents wrapper chatter from corrupting a partial JSON line.
      pushStderr() {},
      finish() {
        const metrics = collector.finish();
        const attribution = collector.attribution();
        const failedIndexes = new Set(attribution.failedInputIndexes);
        const failedMembers = attribution.attributionComplete && batch.inputOwners
          ? batch.inputOwners
            .filter((owner) => owner.indexes.some((index) => failedIndexes.has(index)))
            .map((owner) => owner.member)
          : [];
        return Object.freeze({
          kind: "flutter",
          metrics,
          attributionComplete: attribution.attributionComplete,
          failedMembers: Object.freeze([...new Set(failedMembers)]),
        });
      },
    });
  }
  if (batch.toolchain === "node-test" && batch.members.length > 1 && batch.inputOwners) {
    const inputs = nodeTestInputs(batch.command);
    const collector = createNodeTestAttributionCollector(batch, inputs.length);
    const reporter = relativeReporterPath(repoRoot, batch.command.cwd);
    return Object.freeze({
      command: Object.freeze({
        ...batch.command,
        args: Object.freeze([
          "--test",
          `--test-reporter=${reporter}`,
          ...batch.command.args.slice(1),
        ]),
      }),
      environment: Object.freeze({
        [NODE_TEST_INPUTS_ENV]: JSON.stringify(inputs),
      }),
      pushStdout: collector.push,
      pushStderr() {},
      finish() {
        return Object.freeze({
          kind: "node-test",
          metrics: null,
          ...collector.finish(),
        });
      },
    });
  }
  if (batch.toolchain === "compatibility") {
    const receipt = createSafeReceiptCollector();
    return Object.freeze({
      command: batch.command,
      pushStdout: receipt.push,
      pushStderr() {},
      finish() {
        return Object.freeze({
          kind: "compatibility",
          metrics: null,
          failureReason: receipt.finish(),
        });
      },
    });
  }
  return Object.freeze({
    command: batch.command,
    pushStdout() {},
    pushStderr() {},
    finish() { return null; },
  });
}

function metricFrom(processTree, toolchain, key) {
  const processMetric = processTree?.[key];
  if (processMetric?.status === "measured") return processMetric;
  return toolchain?.metrics?.[key] || processMetric;
}

function commandMetrics({ durationMs, processTree, toolchain, toolchainId }) {
  const nativeMetrics = toolchain?.metrics || null;
  const nativeSummary = nativeMetrics
    ? Object.freeze(Object.fromEntries(Object.entries(nativeMetrics)
      .filter(([key]) => !["directCpuMs", "descendantCpuMs", "peakResidentBytes"].includes(key))))
    : Object.freeze({
      status: "unavailable",
      reason: "toolchain_native_metrics_unavailable",
    });
  return Object.freeze({
    wallTimeMs: measured(durationMs, "monotonic_clock"),
    directCpuMs: metricFrom(processTree, toolchain, "directCpuMs"),
    descendantCpuMs: metricFrom(processTree, toolchain, "descendantCpuMs"),
    peakResidentBytes: metricFrom(processTree, toolchain, "peakResidentBytes"),
    toolchainNative: Object.freeze({
      kind: toolchain?.kind || toolchainId,
      ...nativeSummary,
    }),
  });
}

export async function runClientRegressionCommand(batch, {
  repoRoot,
  environment = process.env,
  leaseFactory = acquireTestArtifactLease,
  spawnImpl = spawn,
  metricsAdapter = defaultProcessTreeMetricsAdapter(),
} = {}) {
  const prepared = prepareToolchainCommand(batch, { repoRoot, environment });
  const { program, args, cwd } = prepared.command;
  const executable = program === "node" ? process.execPath : program;
  const workingDirectory = containedWorkingDirectory(repoRoot, cwd);
  const cargoTarget = program === "cargo"
    ? resolveCargoTargetDirectory({
      args,
      cwd: workingDirectory,
      env: environment,
      workspaceRoot: repoRoot,
    })
    : null;
  const lease = cargoTarget?.insideWorkspace &&
      isManagedCargoTarget(repoRoot, cargoTarget.relativePath)
    ? leaseFactory({ repoRoot, scope: batch.id, targetPath: cargoTarget.relativePath })
    : null;
  const started = process.hrtime.bigint();
  let reason = null;
  let status = "passed";
  let exitCode = null;
  let childPid = null;
  const diagnosticReference = diagnosticLogReference(batch);
  const diagnosticStdout = createTailCollector(PRIVATE_DIAGNOSTIC_LIMIT);
  const diagnosticStderr = createTailCollector(PRIVATE_DIAGNOSTIC_LIMIT);
  try {
    await clearPrivateDiagnostic(repoRoot, diagnosticReference);
    await new Promise((resolve) => {
      let settled = false;
      const finish = (nextStatus, nextReason) => {
        if (settled) return;
        settled = true;
        status = nextStatus;
        reason = nextReason;
        resolve();
      };
      let child;
      try {
        child = spawnImpl(executable, [...args], {
          cwd: workingDirectory,
          env: {
            ...withTestProcessLoopbackProxyExemption(environment),
            ...prepared.environment,
          },
          shell: false,
          stdio: ["ignore", "pipe", "pipe"],
          windowsHide: true,
        });
        childPid = Number.isInteger(child.pid) ? child.pid : null;
      } catch {
        finish("failed", "process_start_failed");
        return;
      }
      if (lease && childPid > 0) {
        try {
          lease.trackProcess?.(childPid);
        } catch {
          // The parent regression process remains covered by the lease.
        }
      }
      child.stdout?.on?.("data", (chunk) => {
        diagnosticStdout.push(chunk);
        prepared.pushStdout(chunk);
      });
      child.stderr?.on?.("data", (chunk) => {
        diagnosticStderr.push(chunk);
        prepared.pushStderr(chunk);
      });
      child.once?.("error", () => {
        if (childPid === null) {
          finish("failed", "process_start_failed");
        } else {
          status = "failed";
          reason = "process_start_failed";
        }
      });
      child.once?.("close", (code, signal) => {
        if (reason === "process_start_failed") {
          finish("failed", reason);
          return;
        }
        exitCode = Number.isInteger(code) ? code : null;
        if (code === 0) finish("passed", null);
        else finish("failed", signal ? "command_signaled" : "command_failed");
      });
    });
    const durationMs = Math.round(monotonicMilliseconds(started));
    const privateOutput = Object.freeze({
      stdout: diagnosticStdout.finish(),
      stderr: diagnosticStderr.finish(),
    });
    if (status === "failed") {
      await writePrivateDiagnostic(repoRoot, diagnosticReference, privateOutput);
    }
    const processTree = await metricsAdapter.measure({
      batchId: batch.id,
      childPid,
      exitCode,
    });
    const toolchain = prepared.finish(exitCode);
    if (status === "failed" && toolchain?.failureReason) {
      reason = toolchain.failureReason;
    }
    const metrics = commandMetrics({
      durationMs,
      processTree,
      toolchain,
      toolchainId: batch.toolchain,
    });
    const attributedMembers = status === "failed" &&
      toolchain?.attributionComplete === true &&
      toolchain.failedMembers.length > 0
      ? toolchain.failedMembers
      : null;
    const attributedPassedMembers = attributedMembers
      ? batch.members.filter((member) => !attributedMembers.includes(member))
      : [];
    return Object.freeze({
      id: batch.id,
      stage: batch.stage,
      lane: batch.lane,
      toolchain: batch.toolchain,
      status: status === "failed" && batch.members.length > 1 && !attributedMembers
        ? "attribution-pending"
        : status,
      reason,
      durationMs,
      members: attributedMembers || batch.members,
      metrics,
      ...(status === "failed" ? { diagnosticLog: diagnosticReference } : {}),
      ...(attributedPassedMembers.length > 0
        ? { attributedPassedMembers: Object.freeze(attributedPassedMembers) }
        : {}),
    });
  } finally {
    lease?.release();
  }
}

function expandAttributedResult(batch, result) {
  const { attributedPassedMembers, ...primaryFields } = result;
  const primary = Object.freeze(primaryFields);
  if (result.status !== "failed" || !Array.isArray(attributedPassedMembers) ||
      attributedPassedMembers.length === 0) {
    return Object.freeze([primary]);
  }
  const passedSet = new Set(attributedPassedMembers);
  const passedMembers = batch.members.filter((member) => passedSet.has(member));
  if (passedMembers.length !== attributedPassedMembers.length ||
      passedMembers.some((member) => result.members.includes(member))) {
    return Object.freeze([primary]);
  }
  return Object.freeze([
    primary,
    Object.freeze({
      id: `${batch.id}.passed`,
      stage: result.stage,
      lane: result.lane,
      toolchain: result.toolchain,
      status: "passed",
      reason: null,
      durationMs: result.durationMs,
      members: Object.freeze(passedMembers),
      metrics: result.metrics,
    }),
  ]);
}

function fits(batch, usage, capacities) {
  if (usage.global + batch.weight > capacities.global) return false;
  if ((usage.pools[batch.toolchain] || 0) + batch.weight > capacities.pools[batch.toolchain]) {
    return false;
  }
  return batch.resources.every((resource) =>
    (usage.resources[resource] || 0) + 1 <= (capacities.resources[resource] || 1));
}

function adjustUsage(batch, usage, direction) {
  usage.global += direction * batch.weight;
  usage.pools[batch.toolchain] = (usage.pools[batch.toolchain] || 0) + direction * batch.weight;
  for (const resource of batch.resources) {
    usage.resources[resource] = (usage.resources[resource] || 0) + direction;
  }
}

export async function executeClientRegressionBatches(batches, {
  capacities = defaultRegressionCapacities(),
  commandRunner,
  onBatchSettled = () => {},
} = {}) {
  const pending = [...batches];
  const running = new Map();
  const results = [];
  const usage = { global: 0, pools: {}, resources: {} };
  const peaks = { global: 0, processes: 0 };
  const poolPeaks = {};
  const start = (batch) => {
    adjustUsage(batch, usage, 1);
    peaks.global = Math.max(peaks.global, usage.global);
    peaks.processes = Math.max(peaks.processes, running.size + 1);
    poolPeaks[batch.toolchain] = Math.max(
      poolPeaks[batch.toolchain] || 0,
      usage.pools[batch.toolchain],
    );
    const promise = Promise.resolve(commandRunner(batch))
      .catch(async () => {
        const processTree = await defaultProcessTreeMetricsAdapter().measure();
        return Object.freeze({
          id: batch.id,
          stage: batch.stage,
          lane: batch.lane,
          toolchain: batch.toolchain,
          status: batch.members.length > 1 ? "attribution-pending" : "failed",
          reason: "command_runner_failed",
          durationMs: 0,
          members: batch.members,
          metrics: commandMetrics({
            durationMs: 0,
            processTree,
            toolchain: null,
            toolchainId: batch.toolchain,
          }),
        });
      })
      .then((result) => ({ batch, result }));
    running.set(batch.id, promise);
  };

  while (pending.length > 0 || running.size > 0) {
    for (let index = 0; index < pending.length;) {
      const batch = pending[index];
      if (!fits(batch, usage, capacities)) {
        index += 1;
        continue;
      }
      pending.splice(index, 1);
      start(batch);
    }
    if (running.size === 0) {
      if (pending.length > 0) throw new Error("client regression resource claim exceeds capacity");
      break;
    }
    const settled = await Promise.race(running.values());
    running.delete(settled.batch.id);
    adjustUsage(settled.batch, usage, -1);
    onBatchSettled(settled.result);
    results.push(...expandAttributedResult(settled.batch, settled.result));
  }
  const memberOrder = new Map(batches.flatMap((batch) => batch.members)
    .map((member, index) => [member, index]));
  results.sort((left, right) =>
    Math.min(...left.members.map((member) => memberOrder.get(member))) -
    Math.min(...right.members.map((member) => memberOrder.get(member))));
  return Object.freeze({
    results: Object.freeze(results),
    concurrency: Object.freeze({
      maximumWeight: peaks.global,
      maximumProcesses: peaks.processes,
      poolPeaks: Object.freeze(poolPeaks),
    }),
  });
}

function resultsPassed(results) {
  return results.every((result) => result.status === "passed");
}

function blockedResults(batches, reason) {
  return batches.map((batch) => Object.freeze({
    id: batch.id,
    stage: batch.stage,
    lane: batch.lane,
    toolchain: batch.toolchain,
    status: "blocked",
    reason,
    durationMs: 0,
    members: batch.members,
    metrics: Object.freeze({}),
  }));
}

function batchContains(batch, moduleId) {
  return batch.members.includes(moduleId);
}

function requiresFlutterDependencies(batch) {
  if (batchContains(batch, FLUTTER_DEPENDENCY_MODULE_ID)) return false;
  return ["flutter", "gradle"].includes(batch.toolchain)
    || batch.resources.some((resource) => FLUTTER_DEPENDENCY_RESOURCES.has(resource));
}

export async function executeClientModules(modules, {
  repoRoot,
  catalog = modules,
  capacities = defaultRegressionCapacities(),
  commandRunner,
  output = process.stdout,
  reportPath = null,
  runKind = "complete",
  compatibilityRunner = async () => [],
  host = process.platform,
} = {}) {
  if (!Array.isArray(modules)) throw new Error("client modules must be an array");
  const hostSelection = partitionModulesByRunnableHost(modules, host);
  const hostCatalog = partitionModulesByRunnableHost(catalog, host);
  const batches = planClientRegressionBatches(hostSelection.runnable, {
    catalog: hostCatalog.runnable,
    excludedCatalog: hostCatalog.unsupported,
    narrow: runKind === "retry",
  });
  const byStage = new Map(CLIENT_REGRESSION_STAGES.map((stage) => [stage, []]));
  for (const batch of batches) byStage.get(batch.stage).push(batch);
  const runner = commandRunner || ((batch) => runClientRegressionCommand(batch, { repoRoot }));
  const startedWall = new Date();
  const startedMono = process.hrtime.bigint();
  const results = [];
  const concurrency = { maximumWeight: 0, maximumProcesses: 0, poolPeaks: {} };
  const recordResults = (settled) => {
    results.push(...settled);
  };
  const emitSettlement = (result) => {
    output.write(`[client-regression] ${result.id}: ${result.status}\n`);
  };
  const unsupportedResults = hostSelection.unsupported.map((module) => Object.freeze({
    id: `unsupported-host.${module.id}`,
    stage: module.regression.stage,
    lane: module.regression.lane,
    toolchain: module.regression.toolchain,
    status: "blocked",
    reason: "unsupported_host",
    durationMs: 0,
    members: Object.freeze([module.id]),
    metrics: Object.freeze({}),
  }));
  unsupportedResults.forEach(emitSettlement);
  results.push(...unsupportedResults);
  const merge = (execution) => {
    recordResults(execution.results);
    concurrency.maximumWeight = Math.max(concurrency.maximumWeight, execution.concurrency.maximumWeight);
    concurrency.maximumProcesses = Math.max(concurrency.maximumProcesses, execution.concurrency.maximumProcesses);
    for (const [pool, peak] of Object.entries(execution.concurrency.poolPeaks)) {
      concurrency.poolPeaks[pool] = Math.max(concurrency.poolPeaks[pool] || 0, peak);
    }
  };
  const run = async (stageBatches) => {
    if (stageBatches.length === 0) {
      return { results: [], concurrency: { maximumWeight: 0, maximumProcesses: 0, poolPeaks: {} } };
    }
    const stages = [...new Set(stageBatches.map((batch) => batch.stage))].join(",");
    output.write(`[client-regression] starting ${stages}: ${stageBatches.length} planned invocation(s)\n`);
    return executeClientRegressionBatches(stageBatches, {
      capacities,
      commandRunner: runner,
      onBatchSettled: emitSettlement,
    });
  };

  const dependencyPreparation = byStage.get("foundation")
    .filter((batch) => batchContains(batch, FLUTTER_DEPENDENCY_MODULE_ID));
  const dependencyExecution = await run(dependencyPreparation);
  merge(dependencyExecution);
  const dependenciesAvailable = resultsPassed(dependencyExecution.results);
  const runStage = async (stageBatches) => {
    const runnable = dependenciesAvailable
      ? stageBatches
      : stageBatches.filter((batch) => !requiresFlutterDependencies(batch));
    const blocked = dependenciesAvailable
      ? []
      : stageBatches.filter(requiresFlutterDependencies);
    merge(await run(runnable));
    const blockedExecution = blockedResults(blocked, "flutter_dependencies_failed");
    blockedExecution.forEach(emitSettlement);
    recordResults(blockedExecution);
  };

  await runStage(byStage.get("foundation")
    .filter((batch) => !batchContains(batch, FLUTTER_DEPENDENCY_MODULE_ID)));
  await runStage([...byStage.get("frontend"), ...byStage.get("backend")]);
  await runStage(byStage.get("integration"));
  await runStage(byStage.get("scenarios"));

  const compatibilityExecution = await compatibilityRunner({ capacities, output });
  const compatibility = Array.isArray(compatibilityExecution)
    ? compatibilityExecution
    : compatibilityExecution.rows;
  if (!Array.isArray(compatibilityExecution)) {
    merge({
      results: compatibilityExecution.results,
      concurrency: compatibilityExecution.concurrency,
    });
  }
  const completedWall = new Date();
  const report = createClientRegressionReport({
    runKind,
    startedAt: startedWall.toISOString(),
    completedAt: completedWall.toISOString(),
    durationMs: Math.round(monotonicMilliseconds(startedMono)),
    results,
    concurrency,
    compatibility,
  });
  if (reportPath) await writeClientRegressionReport(report, reportPath);
  const compatibilityFailed = compatibility.some((row) => row.status === "failed");
  const ok = resultsPassed(results) && !compatibilityFailed;
  output.write(`[client-regression] ${ok ? "passed" : "failed"}: ${results.length} invocation(s) settled\n`);
  return Object.freeze({
    ok,
    completed: Object.freeze(results.flatMap((result) =>
      result.status === "passed" ? result.members : [])),
    failures: report.failures,
    exitCode: ok ? 0 : 1,
    report,
  });
}
