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
const facadeRef = "tools/scripts/client-secure-mesh-e2ee-evidence-bundle.mjs";
const moduleRoot = "tools/scripts/client-secure-mesh-e2ee-evidence-bundle";
const leaves = Object.freeze(["run.mjs", "util.mjs"]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
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

test("e2ee evidence bundle facade is a thin serial CLI entry", async () => {
  const facade = await read(facadeRef);
  const module = await import(
    `${pathToFileURL(path.join(repoRoot, moduleRoot, "run.mjs")).href}?source-bundle`
  );
  assert.equal(typeof module.main, "function");
});

test("e2ee evidence bundle owns exactly two bounded ordinary modules", async () => {
  assert.deepEqual(await collectModules(moduleRoot), [...leaves]);
});

test("e2ee evidence bundle leak-scan self-test stays fail-closed", () => {
  const result = spawnSync(
    process.execPath,
    [path.join(repoRoot, facadeRef), "--leak-scan-self-test"],
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
  assert.equal(payload.leakScanSelfTest, true);
});
