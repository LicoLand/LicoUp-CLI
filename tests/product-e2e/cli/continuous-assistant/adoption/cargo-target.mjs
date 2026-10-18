import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../../../",
);
export const TARGET = "continuity_adoption";
export const REQUIRED_TESTS = [
  "ac_05_001_default_policy_is_enabled_offline_and_blocks_automatic",
  "ac_05_001_owner_disable_persists_and_blocks_only_new_automatic",
  "ac_05_001_live_admission_persists_provenance_and_advances_stage",
  "ac_05_001_owner_issued_live_requires_stored_authority",
  "ac_05_001_within_process_revoke_denies_automatic_without_reopen",
  "ac_05_001_same_responsibility_multi_identity_cannot_expand",
  "ac_05_001_distinct_responsibility_qualification_expands",
  "ac_05_001_successful_collection_once_replay_cannot_mutate",
  "ac_05_001_bound_runtime_collects_typed_observations_without_hermetic_observer",
  "ac_05_001_native_failure_malformed_or_stale_reply_never_qualifies",
  "ac_05_001_missing_empty_or_mismatched_corpus_fails_before_native",
  "ac_05_001_partial_native_failure_retries_as_unknown_without_rerun",
  "ac_05_001_preinvoke_failure_releases_and_retries_after_fix",
  "ac_05_001_stored_observation_field_mutation_rejects_reload",
  "ac_05_002_migration_is_idempotent_and_preserves_facts",
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
    const marker = "CONTINUITY_ADOPTION_ORACLE:";
    const index = line.indexOf(marker);
    if (index < 0) {
      continue;
    }
    const rest = line.slice(index + marker.length);
    const split = rest.indexOf(":");
    if (split < 0) {
      continue;
    }
    oracles[rest.slice(0, split)] = JSON.parse(rest.slice(split + 1));
  }
  return oracles;
}

export function invokeAdoptionTarget() {
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
  if (
    invoked.includes("continuity_host") ||
    invoked.includes("continuity_evaluation") ||
    invoked.includes("continuity_scenarios")
  ) {
    throw new Error("adoption wrapper must not fall back to another Rust target");
  }

  const result = spawnSync(process.execPath, invoked, {
    cwd: repoRoot,
    encoding: "utf8",
  });
  const output = `${result.stdout || ""}\n${result.stderr || ""}`;
  if (output.includes("--test continuity_host") && !output.includes(`--test ${TARGET}`)) {
    throw new Error("adoption wrapper must not fall back to continuity_host");
  }

  const tests = parseTests(output);
  const summary = parseSummary(output);
  const executed = summary.passed + summary.failed + summary.ignored;
  if (summary.running === 0 || executed === 0) {
    throw new Error(`zero tests executed for ${TARGET}\n${output}`);
  }
  if (result.status !== 0) {
    throw new Error(`adoption target exited ${result.status}\n${output}`);
  }
  if (summary.failed > 0 || summary.ignored > 0) {
    throw new Error(
      `adoption target must have zero failed and ignored tests (failed=${summary.failed}, ignored=${summary.ignored})\n${output}`,
    );
  }
  const missing = REQUIRED_TESTS.filter((name) => tests[name] !== "ok");
  if (missing.length > 0) {
    throw new Error(
      `required adoption tests missing or not ok: ${missing.join(",")}\n${output}`,
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
