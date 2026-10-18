import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const facadeRef = "tools/scripts/client-secure-mesh-release-proof-bundle.mjs";
const moduleRoot = "tools/scripts/client-secure-mesh-release-proof-bundle";
const leaves = Object.freeze([
  "cli.mjs",
  "config.mjs",
  "constants.mjs",
  "contract-readiness.mjs",
  "freshness.mjs",
  "integrity.mjs",
  "io.mjs",
  "lists.mjs",
  "privacy.mjs",
  "report-summary-core.mjs",
  "report-summary-physical.mjs",
  "report-summary.mjs",
  "report.mjs",
  "run.mjs",
  "self-test/client-licoarc-crypto.mjs",
  "self-test/contract.mjs",
  "self-test/freshness.mjs",
  "self-test/physical-evidence.mjs",
  "self-test/redaction.mjs",
  "summarize/android-install.mjs",
  "summarize/client-licoarc-crypto.mjs",
  "summarize/physical-evidence.mjs",
  "summarize/physical-matrix.mjs",
  "summarize/redaction.mjs",
  "summarize/update.mjs",
  "summarize/windows.mjs",
  "verifiers.mjs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(
    await Promise.all(
      leaves.map(async (leaf) => [leaf, await read(`${moduleRoot}/${leaf}`)]),
    ),
  );
}

async function collectModules(relativeRoot) {
  const found = [];
  async function visit(relativeDirectory, prefix = "") {
    const entries = await fs.readdir(path.join(repoRoot, relativeDirectory), {
      withFileTypes: true,
    });
    for (const entry of entries) {
      const childPrefix = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        await visit(`${relativeDirectory}/${entry.name}`, childPrefix);
      } else if (entry.isFile() && entry.name.endsWith(".mjs")) {
        found.push(childPrefix);
      }
    }
  }
  await visit(relativeRoot);
  return found.sort();
}

function declarationOwners(source, name) {
  const patterns = [
    new RegExp(`(?:export\\s+)?(?:async\\s+)?function\\s+${name}\\s*\\(`, "u"),
    new RegExp(`(?:export\\s+)?class\\s+${name}\\b`, "u"),
    new RegExp(`(?:export\\s+)?const\\s+${name}\\s*=`, "u"),
  ];
  return Object.entries(source)
    .filter(([, value]) => patterns.some((pattern) => pattern.test(value)))
    .map(([leaf]) => leaf)
    .sort();
}

function findImportCycle(source) {
  const graph = new Map();
  for (const [leaf, value] of Object.entries(source)) {
    const dependencies = [];
    for (const match of value.matchAll(/from\s+"(\.{1,2}\/[^"\n]+)"/gu)) {
      const resolved = path.posix.normalize(
        path.posix.join(path.posix.dirname(leaf), match[1]),
      );
      if (source[resolved]) dependencies.push(resolved);
    }
    graph.set(leaf, dependencies);
  }
  const visiting = new Set();
  const visited = new Set();
  let cycle = null;
  function dfs(node, stack) {
    if (visiting.has(node)) {
      cycle = [...stack, node];
      return true;
    }
    if (visited.has(node)) return false;
    visiting.add(node);
    stack.push(node);
    for (const next of graph.get(node) || []) {
      if (dfs(next, stack)) return true;
    }
    stack.pop();
    visiting.delete(node);
    visited.add(node);
    return false;
  }
  for (const node of graph.keys()) {
    if (dfs(node, [])) return cycle;
  }
  return null;
}

test("secure mesh release proof facade is a thin serial CLI entry", async () => {
  const facade = await read(facadeRef);
  assert.match(facade, /runSecureMeshReleaseProofBundleCli/u);
  assert.equal(facade.includes("function "), false);
  assert.equal(facade.includes("class "), false);
  assert.equal(facade.includes("spawn"), false);
  assert.equal(facade.includes("readFileSync"), false);
  const module = await import(
    `${pathToFileURL(path.join(repoRoot, moduleRoot, "run.mjs")).href}?source-bundle`
  );
  assert.equal(typeof module.runSecureMeshReleaseProofBundleCli, "function");
});

test("secure mesh release proof owns exactly twenty-seven bounded ordinary modules", async () => {
  assert.deepEqual(await collectModules(moduleRoot), [...leaves]);
  const source = await sources();
  for (const leaf of Object.keys(source)) {
    assert.equal(
      source[leaf].includes("../client-secure-mesh-release-proof-bundle.mjs"),
      false,
    );
  }
  assert.equal(findImportCycle(source), null);
});

test("cli, summarize, report, and self-test each have one authority", async () => {
  const source = await sources();
  assert.deepEqual(declarationOwners(source, "parseReleaseProofArgs"), [
    "cli.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "runSecureMeshReleaseProofBundleCli"), [
    "run.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "buildReleaseProofReport"), [
    "report.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "buildReleaseProofSummary"), [
    "report-summary.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "summarizePhysicalEvidenceManifest"), [
    "summarize/physical-evidence.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "summarizePhysicalMatrixReport"), [
    "summarize/physical-matrix.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "runReleaseProofContractReadinessSelfTest"), [
    "self-test/contract.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "runClientLicoArcCryptoInputsReadinessSelfTest"), [
    "self-test/client-licoarc-crypto.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "evaluateSourceCheck"), [
    "verifiers.mjs",
  ]);
});

test("release-proof self-test dry-runs preserve fail-closed contracts", () => {
  for (const flag of [
    "--client-licoarc-crypto-readiness-self-test",
    "--release-proof-contract-readiness-self-test",
  ]) {
    const result = spawnSync(
      process.execPath,
      [path.join(repoRoot, facadeRef), flag],
      {
        cwd: repoRoot,
        encoding: "utf8",
        maxBuffer: 4 * 1024 * 1024,
        timeout: 60_000,
      },
    );
    assert.equal(result.status, 0, result.stderr || result.stdout.slice(0, 400));
    const payload = JSON.parse(result.stdout);
    assert.equal(payload.ok, true);
  }
});
