import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

function readText(relativePath) {
  return readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function readJson(relativePath) {
  return JSON.parse(readText(relativePath));
}

const rootCargo = readText("Cargo.toml");
const nativeCargo = readText("crates/licoup/Cargo.toml");
const runnerSource = readText("tools/scripts/client-native-performance.mjs");
const benchSource = readText("crates/licoup/benches/native_backend.rs");
const verifyWorkflow = readText(".github/workflows/verify.yml");

const privacyTokens = [
  "/Users/",
  "C:\\",
  "ghp_",
  "gho_",
  "sk-",
  "Bearer ",
  "http://",
  "https://",
  "--runtime-data",
  "transcript",
  "Transcript",
];

test("the sole release profile favors backend throughput", () => {
  assert.match(rootCargo, /\[profile\.release\]/, "release profile must exist");
  assert.match(
    rootCargo,
    /^opt-level\s*=\s*3\s*$/m,
    "release profile must use throughput optimization level three",
  );
  assert.doesNotMatch(
    rootCargo,
    /opt-level\s*=\s*["']?z["']?/i,
    "size-first optimization must not remain",
  );
  assert.doesNotMatch(
    rootCargo,
    /opt-level\s*=\s*["']?s["']?/i,
    "size-first optimization must not remain",
  );
  for (const control of [
    "codegen-units = 1",
    "lto = true",
    "panic = \"abort\"",
    "strip = true",
  ]) {
    assert.ok(rootCargo.includes(control), `release profile must retain ${control}`);
  }
  const profileSections = rootCargo.match(/^\s*\[profile\.[^\]]+\]\s*$/gm) ?? [];
  assert.equal(
    profileSections.length,
    1,
    "no legacy or compatibility release profile may remain",
  );
});

test("native clippy observes performance lints", () => {
  const clippy = verifyWorkflow
    .split("\n")
    .find((line) => line.includes("cargo clippy"));
  assert.ok(clippy, "the verify workflow must run clippy");
  assert.ok(clippy.includes("-D warnings"), "clippy must deny warnings");
  assert.ok(clippy.includes("--workspace"), "clippy must cover the whole workspace");
  assert.ok(clippy.includes("-A clippy::style"), "unrelated style policy must remain");
  assert.ok(clippy.includes("-A clippy::complexity"), "unrelated complexity policy must remain");
  assert.ok(!verifyWorkflow.includes("clippy::perf"), "performance lints must not be suppressed");
});

test("the benchmark is registered through the managed lease", () => {
  assert.ok(
    nativeCargo.includes("[dev-dependencies]"),
    "benchmark dependencies must be development-only",
  );
  assert.match(
    nativeCargo,
    /criterion\s*=\s*"0\.5"/,
    "criterion must be registered as a dev dependency",
  );
  assert.match(
    nativeCargo,
    /\[\[bench\]\]\s*name\s*=\s*"native_backend"\s*harness\s*=\s*false/,
    "the native_backend bench must use the criterion harness",
  );
  const dependenciesSection = nativeCargo.split(/\[target\./u)[0];
  const productionDependencies = dependenciesSection.split("[dev-dependencies]")[0];
  assert.ok(
    !productionDependencies.includes("criterion"),
    "criterion must not be exposed through production dependencies",
  );
});

test("the managed runner is wired and delegated", () => {
  assert.ok(runnerSource.includes("cargo-client.mjs"), "runner must delegate to the managed lease");
  assert.ok(runnerSource.includes("--smoke"), "runner must support smoke mode");
  assert.ok(runnerSource.includes("cases_completed"), "runner must record structural counters");
  assert.ok(runnerSource.includes("MAX_MS"), "runner must enforce a bounded ceiling");
  assert.doesNotMatch(
    runnerSource,
    /spawn(?:Sync)?\(\s*["']cargo["']/,
    "runner must not execute cargo directly",
  );
});

test("benchmark and runner stay synthetic and private", () => {
  for (const source of [benchSource, runnerSource]) {
    for (const token of privacyTokens) {
      assert.ok(
        !source.includes(token),
        `synthetic evidence must not contain ${token}`,
      );
    }
  }
  assert.ok(benchSource.includes("open_in_memory"), "database case must use in-memory state");
  assert.ok(benchSource.includes("synthetic"), "fixtures must be synthetic");
  assert.ok(!benchSource.includes("#[ignore]"), "benchmark cases must not be skipped");
  assert.ok(!benchSource.includes("--skip"), "benchmark cases must not be skipped");
  assert.ok(
    !runnerSource.includes("--runtime-data"),
    "runner must not accept live runtime data",
  );
});

test("no predecessor or private fixture remains", () => {
  assert.ok(
    !rootCargo.includes("opt-level = \"z\"") && !rootCargo.includes("opt-level = \"s\""),
    "the size-first optimization setting must be deleted",
  );
  assert.ok(
    !benchSource.includes("std::env"),
    "benchmark must not read environment or runtime state",
  );
});
