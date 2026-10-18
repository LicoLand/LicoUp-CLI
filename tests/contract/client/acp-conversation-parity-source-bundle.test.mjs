import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const facadeRef = "tests/product-e2e/cli/agent-conversations/support/parity-facade.mjs";
const moduleRoot = "tests/product-e2e/cli/agent-conversations/support/parity";
const leaves = Object.freeze([
  "agent-ids.mjs",
  "cli.mjs",
  "clients/acp-client.mjs",
  "clients/app-server-client.mjs",
  "clients/copilot-sdk-client.mjs",
  "clients/pi-rpc-client.mjs",
  "clients/stdio-rpc-client.mjs",
  "constants.mjs",
  "errors.mjs",
  "evidence.mjs",
  "live-gate.mjs",
  "live.mjs",
  "native/acp-settings.mjs",
  "native/acp-turn.mjs",
  "native/app-server.mjs",
  "native/cursor-cli.mjs",
  "native/pi.mjs",
  "packaging.mjs",
  "process-local-round.mjs",
  "process.mjs",
  "results.mjs",
  "round-facts.mjs",
  "run-round.mjs",
  "run.mjs",
  "self-test/acp-oracles.mjs",
  "self-test/fake-runtime.mjs",
  "self-test/runner.mjs",
  "session-cleanup.mjs",
  "session-query.mjs",
  "sidecar.mjs",
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
      const childDirectory = `${relativeDirectory}/${entry.name}`;
      if (entry.isDirectory()) await visit(childDirectory, childPrefix);
      else if (entry.isFile() && entry.name.endsWith(".mjs")) {
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

test("acp conversation parity facade is a thin serial CLI entry", async () => {
  const facade = await read(facadeRef);
  assert.match(facade, /runAcpConversationParityCli/u);
  assert.equal(facade.includes("function "), false);
  assert.equal(facade.includes("class "), false);
  assert.equal(facade.includes("spawn"), false);
  assert.equal(facade.includes("readFileSync"), false);
  const module = await import(
    `${pathToFileURL(path.join(repoRoot, moduleRoot, "run.mjs")).href}?source-bundle`
  );
  assert.equal(typeof module.runAcpConversationParityCli, "function");
});

test("acp conversation parity owns exactly thirty bounded ordinary modules", async () => {
  assert.deepEqual(await collectModules(moduleRoot), [...leaves]);
  const source = await sources();
  for (const leaf of Object.keys(source)) {
    assert.equal(source[leaf].includes("../../parity-facade.mjs"), false);
  }
  assert.equal(findImportCycle(source), null);
});

test("cli, live-gate, rounds, sessions, and clients each have one authority", async () => {
  const source = await sources();
  assert.deepEqual(declarationOwners(source, "parseArguments"), ["cli.mjs"]);
  assert.deepEqual(declarationOwners(source, "printLiveGateChecklist"), [
    "live-gate.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "parityModelForAgent"), [
    "agent-ids.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "runRound"), ["run-round.mjs"]);
  assert.deepEqual(declarationOwners(source, "runLive"), ["live.mjs"]);
  assert.deepEqual(declarationOwners(source, "runSelfTest"), [
    "self-test/runner.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "cleanupSession"), [
    "session-cleanup.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "listSessions"), [
    "session-query.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "AcpClient"), [
    "clients/acp-client.mjs",
  ]);
  assert.deepEqual(declarationOwners(source, "runAcpConversationParityCli"), [
    "run.mjs",
  ]);
});

test("self-test dry-run preserves passed status without live agent binaries", () => {
  const result = spawnSync(
    process.execPath,
    [path.join(repoRoot, facadeRef), "--self-test"],
    {
      cwd: repoRoot,
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
    },
  );
  assert.equal(result.status, 0, result.stderr || result.stdout.slice(0, 400));
  const payload = JSON.parse(result.stdout);
  assert.equal(payload.status, "passed");
  assert.equal(payload.cleanupVerified, true);
  assert.equal(payload.strictRounds, 1);
  assert.equal(payload.verificationTurns, 2);
  assert.equal(payload.verificationSessions, 1);
  assert.equal(payload.openNew, true);
  assert.equal(payload.exactResume, true);
  assert.equal(payload.verificationWorkflowContract, true);
  assert.equal(payload.exactResumeIdentityFailClosed, true);
  assert.equal(typeof payload.evidenceDigest, "string");
  assert.equal(payload.evidenceDigest.length, 64);
});

test("print-live-gate remains a non-mutating checklist", () => {
  const result = spawnSync(
    process.execPath,
    [path.join(repoRoot, facadeRef), "--print-live-gate"],
    {
      cwd: repoRoot,
      encoding: "utf8",
      maxBuffer: 4 * 1024 * 1024,
    },
  );
  assert.equal(result.status, 0, result.stderr || result.stdout.slice(0, 400));
  const payload = JSON.parse(result.stdout);
  assert.equal(payload.status, "live-gate-checklist");
  assert.equal(Array.isArray(payload.adapters), true);
  assert.ok(payload.adapters.length >= 1);
});
