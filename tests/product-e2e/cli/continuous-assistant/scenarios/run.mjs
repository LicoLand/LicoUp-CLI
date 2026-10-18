import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  CASE_MARKER,
  ORACLE_MARKER,
  TARGET,
  loadCatalog,
  requiredIds,
} from "./ids.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../../../",
);

function parseCaseLines(output) {
  const cases = {};
  for (const line of output.split("\n")) {
    const index = line.indexOf(CASE_MARKER);
    if (index < 0) {
      continue;
    }
    const payload = JSON.parse(line.slice(index + CASE_MARKER.length));
    if (!payload.id || payload.executed !== true) {
      throw new Error(`scenario case line missing executed id: ${line}`);
    }
    cases[payload.id] = payload;
  }
  return cases;
}

function parseOracle(output) {
  const line = output.split("\n").find((item) => item.includes(ORACLE_MARKER));
  if (!line) {
    return null;
  }
  return JSON.parse(line.split(ORACLE_MARKER)[1]);
}

export function invokeScenarioTarget() {
  const invoked = [
    "tools/scripts/cargo-client.mjs",
    "test",
    "-p",
    "licoup-native",
    "--features",
    "test-support",
    "--test",
    TARGET,
    "--offline",
    "--",
    "--nocapture",
    "--test-threads=1",
  ];
  if (invoked.includes("continuity_host") || invoked.includes("continuity_evaluation")) {
    throw new Error("scenario wrapper must not fall back to another Rust target");
  }

  const result = spawnSync(process.execPath, invoked, {
    cwd: repoRoot,
    encoding: "utf8",
  });
  const output = `${result.stdout || ""}\n${result.stderr || ""}`;
  if (output.includes("--test continuity_host") && !output.includes(`--test ${TARGET}`)) {
    throw new Error("scenario wrapper must not fall back to continuity_host");
  }

  const cases = parseCaseLines(output);
  const executedIds = Object.keys(cases);
  if (executedIds.length === 0) {
    throw new Error(`zero scenario cases executed for ${TARGET}\n${output}`);
  }

  return {
    target: TARGET,
    status: result.status,
    output,
    cases,
    oracle: parseOracle(output),
    requiredIds: requiredIds(loadCatalog()),
  };
}

const invokedAsCli = process.argv[1] && process.argv[1].endsWith("run.mjs");
if (invokedAsCli) {
  const run = invokeScenarioTarget();
  process.stdout.write(
    `${JSON.stringify({
      target: run.target,
      status: run.status,
      executed: Object.keys(run.cases).length,
      oracle: run.oracle,
    })}\n`,
  );
  process.exit(run.status === 0 ? 0 : run.status ?? 1);
}
