import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../../../",
);
const TARGET = "continuity_evaluation";
const REQUIRED_TESTS = [
  "ac_04_002_heldout_family_split_is_disjoint_and_version_isolated",
  "ac_04_002_fpr_and_miss_use_separate_denominators",
  "ac_04_002_all_abstain_rejects_zero_fpr_coverage",
  "ac_04_002_zero_heldout_samples_are_unknown",
  "ac_04_002_wilson_bounds_come_from_rust",
  "ac_04_002_model_runtime_prompt_resource_policy_drift_is_stale",
  "ac_04_002_unknown_cost_is_not_zero",
  "ac_04_002_synthetic_evidence_is_never_live_qualified",
  "ac_04_005_full_chain_and_direct_native_exclude_unmatched",
  "ac_04_005_unpaired_cost_must_not_enter_paired_ratio",
  "ac_04_003_live_admission_requires_authority_facts_and_reload",
];

function parseTests(output) {
  const tests = {};
  let current = null;
  for (const raw of output.split("\n")) {
    const line = raw.trim();
    const start = line.match(/^test\s+(\S+)\s+\.\.\.(.*)$/);
    if (start) {
      current = start[1];
      const rest = start[2].trim();
      if (rest === "ok" || rest === "FAILED" || rest === "ignored") {
        tests[current] = rest;
        current = null;
      }
      continue;
    }
    if (current && (line === "ok" || line === "FAILED" || line === "ignored")) {
      tests[current] = line;
      current = null;
    }
  }
  return tests;
}

function parseSummary(output) {
  const running = output.match(/running (\d+) tests/);
  const result = output.match(
    /test result:\s+\w+\.\s+(\d+) passed;\s+(\d+) failed;\s+(\d+) ignored/,
  );
  return {
    running: running ? Number(running[1]) : 0,
    passed: result ? Number(result[1]) : 0,
    failed: result ? Number(result[2]) : 0,
    ignored: result ? Number(result[3]) : 0,
  };
}

function parseOracles(output) {
  const oracles = {};
  for (const line of output.split("\n")) {
    const marker = "CONTINUITY_EVALUATION_ORACLE:";
    const index = line.indexOf(marker);
    if (index < 0) {
      continue;
    }
    const rest = line.slice(index + marker.length);
    const split = rest.indexOf(":");
    if (split < 0) {
      continue;
    }
    const label = rest.slice(0, split);
    oracles[label] = JSON.parse(rest.slice(split + 1));
  }
  return oracles;
}

export function invokeEvaluationTarget() {
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
  if (invoked.includes("continuity_host") || invoked.includes("--workspace")) {
    throw new Error("evaluation wrapper must not fall back to another Rust target");
  }

  const result = spawnSync(process.execPath, invoked, {
    cwd: repoRoot,
    encoding: "utf8",
  });
  const output = `${result.stdout || ""}\n${result.stderr || ""}`;
  if (output.includes("--test continuity_host") && !output.includes(`--test ${TARGET}`)) {
    throw new Error("evaluation wrapper must not fall back to continuity_host");
  }

  const tests = parseTests(output);
  const summary = parseSummary(output);
  const executed = summary.passed + summary.failed + summary.ignored;
  if (summary.running === 0 || executed === 0) {
    throw new Error(`zero tests executed for ${TARGET}\n${output}`);
  }
  if (result.status !== 0) {
    throw new Error(`evaluation target exited ${result.status}\n${output}`);
  }
  if (summary.failed > 0 || summary.ignored > 0) {
    throw new Error(
      `evaluation target must have zero failed and ignored tests (failed=${summary.failed}, ignored=${summary.ignored})\n${output}`,
    );
  }
  const missing = REQUIRED_TESTS.filter((name) => tests[name] !== "ok");
  if (missing.length > 0) {
    throw new Error(
      `required evaluation tests missing or not ok: ${missing.join(",")}\n${output}`,
    );
  }

  return {
    target: TARGET,
    status: result.status,
    output,
    tests,
    summary,
    oracles: parseOracles(output),
    requiredTests: REQUIRED_TESTS,
  };
}
