import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import test from "node:test";
import { PassThrough } from "node:stream";
import { fileURLToPath } from "node:url";
import { CLIENT_MODULE_CATALOG } from "../client-module-catalog.mjs";
import {
  executeClientModules,
  executeClientRegressionBatches,
  runClientRegressionCommand,
} from "../client-module-execution.mjs";
import { planClientRegressionBatches } from "../client-regression-batching.mjs";
import {
  changedPathsSince,
  normalizeRepoPath,
  parseNulDelimitedPaths,
  selectModulesById,
  selectModulesForChangedPaths,
  validateChangedFromRevision,
  validateClientModuleCatalog,
} from "../client-module-selection.mjs";
import {
  main,
  parseClientModuleRegressionArgs,
} from "../../scripts/client-module-regression.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const runnerPath = "tools/scripts/client-module-regression.mjs";

function ids(modules) {
  return modules.map((module) => module.id);
}

function stringSink() {
  let value = "";
  return {
    write(chunk) { value += String(chunk); },
    value() { return value; },
  };
}

async function sourceFiles(relativeRoot, extension) {
  const found = [];
  async function visit(relativeDirectory) {
    const entries = await fs.readdir(path.join(repoRoot, relativeDirectory), {
      withFileTypes: true,
    });
    for (const entry of entries) {
      const relativePath = path.posix.join(relativeDirectory, entry.name);
      if (entry.isDirectory()) {
        await visit(relativePath);
      } else if (entry.isFile() && relativePath.endsWith(extension)) {
        found.push(relativePath);
      }
    }
  }
  await visit(relativeRoot);
  return found.sort();
}

export {
  assert,
  spawn,
  EventEmitter,
  PassThrough,
  fs,
  path,
  process,
  test,
  CLIENT_MODULE_CATALOG,
  executeClientModules,
  executeClientRegressionBatches,
  runClientRegressionCommand,
  planClientRegressionBatches,
  changedPathsSince,
  normalizeRepoPath,
  parseNulDelimitedPaths,
  selectModulesById,
  selectModulesForChangedPaths,
  validateChangedFromRevision,
  validateClientModuleCatalog,
  main,
  parseClientModuleRegressionArgs,
  repoRoot,
  runnerPath,
  ids,
  stringSink,
  sourceFiles,
};
