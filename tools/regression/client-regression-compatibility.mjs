import process from "node:process";
import {
  executeClientRegressionBatches,
  runClientRegressionCommand,
} from "./client-module-execution.mjs";
import {
  CLIENT_COMPATIBILITY_ENTRIES,
  validateClientRegressionEntries,
} from "./client-regression-entries/index.mjs";

function compatibilityBatch(entry) {
  return Object.freeze({
    id: `${entry.kind}-${entry.id}`,
    stage: "compatibility",
    lane: entry.lane,
    toolchain: "compatibility",
    weight: 1,
    resources: entry.resources,
    members: Object.freeze([entry.id]),
    command: entry.liveCommand,
    attribution: "exact",
  });
}

function agentStaticBatch(entry) {
  return Object.freeze({
    id: `agent-static-${entry.id}`,
    stage: "compatibility",
    lane: `agent:${entry.id}:static`,
    toolchain: "compatibility",
    weight: 1,
    resources: Object.freeze([]),
    members: Object.freeze([entry.id]),
    attribution: "exact",
    command: Object.freeze({
      program: "node",
      args: Object.freeze([
        "tools/scripts/client-agent-adapter-standard.mjs",
        "--agent",
        entry.id,
      ]),
      cwd: ".",
      timeoutMs: 5 * 60_000,
    }),
  });
}

function mergeConcurrency(target, execution) {
  target.maximumWeight = Math.max(target.maximumWeight, execution.concurrency.maximumWeight);
  target.maximumProcesses = Math.max(target.maximumProcesses, execution.concurrency.maximumProcesses);
  for (const [pool, peak] of Object.entries(execution.concurrency.poolPeaks)) {
    target.poolPeaks[pool] = Math.max(target.poolPeaks[pool] || 0, peak);
  }
}

export async function runClientCompatibilityFrontier({
  repoRoot,
  capacities,
  live = true,
  commandRunner,
  entries = CLIENT_COMPATIBILITY_ENTRIES,
} = {}) {
  validateClientRegressionEntries();
  const runner = commandRunner || ((batch) => runClientRegressionCommand(batch, { repoRoot }));
  const staticAgentBatch = Object.freeze({
    id: "agent-static-shared",
    stage: "compatibility",
    lane: "agent:static",
    toolchain: "compatibility",
    weight: 1,
    resources: Object.freeze([]),
    members: Object.freeze(entries
      .filter((entry) => entry.kind === "agent")
      .map((entry) => entry.id)),
    attribution: "inventory",
    command: Object.freeze({
      program: "node",
      args: Object.freeze(["tools/scripts/client-agent-adapter-standard.mjs", "--shared"]),
      cwd: ".",
      timeoutMs: 5 * 60_000,
    }),
  });
  const selectedAgents = entries.filter((entry) => entry.kind === "agent");
  const sharedStaticExecution = selectedAgents.length > 0
    ? await executeClientRegressionBatches([staticAgentBatch], {
      capacities,
      commandRunner: runner,
    })
    : {
      results: [],
      concurrency: { maximumWeight: 0, maximumProcesses: 0, poolPeaks: {} },
    };
  const sharedStaticPassed = selectedAgents.length === 0 ||
    sharedStaticExecution.results[0]?.status === "passed";
  const rows = [];
  const commandResults = [...sharedStaticExecution.results];
  const concurrency = {
    maximumWeight: sharedStaticExecution.concurrency.maximumWeight,
    maximumProcesses: sharedStaticExecution.concurrency.maximumProcesses,
    poolPeaks: { ...sharedStaticExecution.concurrency.poolPeaks },
  };
  const eligible = [];
  const entryByBatch = new Map();
  const probeEntries = () => Promise.all(entries.map(async (entry) => {
    const before = process.hrtime.bigint();
    let probe;
    let probeFailed = false;
    try {
      probe = await entry.probe();
    } catch {
      probeFailed = true;
      probe = Object.freeze({
        eligible: false,
        reason: "compatibility_probe_failed",
      });
    }
    return Object.freeze({
      entry,
      probe,
      probeFailed,
      durationMs: Number(process.hrtime.bigint() - before) / 1_000_000,
    });
  }));
  const emptyExecution = {
    results: [],
    concurrency: { maximumWeight: 0, maximumProcesses: 0, poolPeaks: {} },
  };
  const [agentStaticExecution, probedEntries] = await Promise.all([
    sharedStaticPassed && selectedAgents.length > 0
      ? executeClientRegressionBatches(selectedAgents.map(agentStaticBatch), {
        capacities,
        commandRunner: runner,
      })
      : Promise.resolve(emptyExecution),
    probeEntries(),
  ]);
  commandResults.push(...agentStaticExecution.results);
  mergeConcurrency(concurrency, agentStaticExecution);
  const staticByAgent = new Map(agentStaticExecution.results.map((result) =>
    [result.members[0], result]));
  for (const { entry, probe, probeFailed, durationMs } of probedEntries) {
    const agentStatic = entry.kind === "agent" ? staticByAgent.get(entry.id) : null;
    const staticPassed = entry.kind !== "agent" ||
      (sharedStaticPassed && agentStatic?.status === "passed");
    if (probeFailed) {
      rows.push({
        id: entry.id,
        kind: entry.kind,
        status: "failed",
        reason: "compatibility_probe_failed",
        staticStatus: staticPassed ? "passed" : "failed",
        liveStatus: "not-run",
        durationMs,
      });
    } else if (entry.kind === "agent" && !sharedStaticPassed) {
      rows.push({
        id: entry.id,
        kind: entry.kind,
        status: "failed",
        reason: "agent_static_shared_failed",
        staticStatus: "failed",
        liveStatus: "not-run",
        durationMs,
      });
    } else if (entry.kind === "agent" && agentStatic?.status !== "passed") {
      rows.push({
        id: entry.id,
        kind: entry.kind,
        status: "failed",
        reason: agentStatic?.reason || "agent_static_contract_failed",
        staticStatus: "failed",
        liveStatus: "not-run",
        durationMs,
      });
    } else if (!live) {
      rows.push({
        id: entry.id,
        kind: entry.kind,
        status: "unverified",
        reason: "live_execution_not_authorized",
        staticStatus: "passed",
        liveStatus: "not-run",
        durationMs,
      });
    } else if (!probe.eligible) {
      rows.push({
        id: entry.id,
        kind: entry.kind,
        status: "unverified",
        reason: probe.reason,
        staticStatus: "passed",
        liveStatus: "not-available",
        durationMs,
      });
    } else {
      const batch = compatibilityBatch(entry);
      eligible.push(batch);
      entryByBatch.set(batch.id, entry);
    }
  }
  if (eligible.length > 0) {
    const execution = await executeClientRegressionBatches(eligible, {
      capacities,
      commandRunner: runner,
    });
    commandResults.push(...execution.results);
    mergeConcurrency(concurrency, execution);
    for (const result of execution.results) {
      const entry = entryByBatch.get(result.id);
      rows.push({
        id: entry.id,
        kind: entry.kind,
        status: result.status === "passed" ? "passed" : "failed",
        reason: result.reason,
        staticStatus: "passed",
        liveStatus: result.status === "passed" ? "passed" : "failed",
        durationMs: result.durationMs,
      });
    }
  }
  const order = new Map(entries.map((entry, index) =>
    [`${entry.kind}:${entry.id}`, index]));
  rows.sort((left, right) =>
    order.get(`${left.kind}:${left.id}`) - order.get(`${right.kind}:${right.id}`));
  if (rows.filter((row) => row.kind === "agent").length !== selectedAgents.length) {
    throw new Error("agent compatibility matrix is incomplete");
  }
  return Object.freeze({
    rows: Object.freeze(rows.map((row) => Object.freeze({
      ...row,
      durationMs: Math.round(row.durationMs),
    }))),
    results: Object.freeze(commandResults),
    concurrency: Object.freeze({
      ...concurrency,
      poolPeaks: Object.freeze(concurrency.poolPeaks),
    }),
  });
}
