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
const facadeRef = "tests/product-e2e/cli/agent-conversations/support/reducer-facade.mjs";
const moduleRoot = "tests/product-e2e/cli/agent-conversations/support/reducer";
const leaves = Object.freeze([
  "cli.mjs",
  "constants.mjs",
  "digest-audit.mjs",
  "digests.mjs",
  "errors.mjs",
  "evidence.mjs",
  "gateway-reload.mjs",
  "inputs.mjs",
  "inventory.mjs",
  "json.mjs",
  "privacy.mjs",
  "reduce.mjs",
  "run.mjs"
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

test("agent conversation parity reducer facade is a thin serial CLI entry", async () => {
  const facade = await read(facadeRef);
  assert.equal(facade.includes("spawnSync"), false);
  assert.equal(facade.includes("readFileSync"), false);
  const module = await import(
    `${pathToFileURL(path.join(repoRoot, moduleRoot, "run.mjs")).href}?source-bundle`
  );
  assert.equal(typeof module.main, "function");
});

test("agent conversation parity reducer owns exactly thirteen bounded ordinary modules", async () => {
  assert.deepEqual(await collectModules(moduleRoot), [...leaves]);
  const source = await sources();
  for (const leaf of Object.keys(source)) {
    assert.equal(source[leaf].includes("../client-agent-conversation-parity-reducer.mjs"), false);
  }
  assert.equal(findImportCycle(source), null);
});

test("reducer --check dry-run preserves readiness receipt contract", () => {
  const result = spawnSync(
    process.execPath,
    [path.join(repoRoot, facadeRef), "--check"],
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
  assert.equal(payload.schemaVersion, "v0.0.1:client-agent-conversation-readiness-receipt-1");
  assert.equal(payload.operation, "check");
});
