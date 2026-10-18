#!/usr/bin/env node
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { CLIENT_MODULE_CATALOG } from "../regression/client-module-catalog.mjs";
import { executeClientModules } from "../regression/client-module-execution.mjs";
import { runClientCompatibilityFrontier } from "../regression/client-regression-compatibility.mjs";
import { CLIENT_COMPATIBILITY_ENTRIES } from "../regression/client-regression-entries/index.mjs";
import { retrySelectionFromReport } from "../regression/client-regression-report.mjs";
import {
  changedPathsSince,
  normalizeRepoPath,
  selectModulesById,
  selectModulesByLane,
  selectModulesForChangedPaths,
  validateClientModuleCatalog,
} from "../regression/client-module-selection.mjs";

export const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const defaultReportPath = path.resolve(repoRoot, "build/reports/client-module-regression.json");

const usage = `Usage:
  npm run client:regression
  npm run client:regression:list
  npm run client:regression -- --module <module-id> [--module <module-id> ...]
  npm run client:regression -- --lane <lane-id> [--lane <lane-id> ...]
  npm run client:regression -- --agent <agent-id> [--agent <agent-id> ...]
  npm run client:regression -- --platform <platform-id> [--platform <platform-id> ...]
  npm run client:regression -- --changed-from <git-revision>
  npm run client:regression -- --retry-report build/reports/client-module-regression.json

Options:
  --list                   List modules and compatibility entries without probes.
  --module <id>            Run exact modules; repeat or use comma-separated ids.
  --lane <id>              Run one core lane; repeat or use comma-separated ids.
  --agent <id>             Run one Agent compatibility target; repeatable.
  --platform <id>          Run one platform compatibility target; repeatable.
  --changed-from <ref>     Select modules from changed repository paths.
  --retry-report <path>    Redispatch only failed/pending members from a prior report.
  --report <path>          Write the privacy-safe report under build/reports.
  --static-compatibility   Do not execute eligible live platform/Agent verifiers.
  --dry-run                Print the selection without spawning or probing.
  --help                   Show this help.
`;

function commaValues(value, label) {
  const values = String(value || "").split(",").map((item) => item.trim()).filter(Boolean);
  if (values.length === 0) throw new Error(`${label} is required`);
  return values;
}

function reportFile(value) {
  const relative = normalizeRepoPath(value);
  if (!relative.startsWith("build/reports/")) {
    throw new Error("client regression reports must stay under build/reports");
  }
  return path.resolve(repoRoot, relative);
}

export function parseClientModuleRegressionArgs(argv) {
  const options = {
    agentIds: [],
    changedFrom: null,
    dryRun: false,
    help: false,
    lanes: [],
    list: false,
    moduleIds: [],
    platformIds: [],
    reportPath: defaultReportPath,
    retryReport: null,
    staticCompatibility: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--list") options.list = true;
    else if (argument === "--dry-run") options.dryRun = true;
    else if (argument === "--static-compatibility") options.staticCompatibility = true;
    else if (argument === "--help" || argument === "-h") options.help = true;
    else if (["--module", "--lane", "--agent", "--platform", "--changed-from", "--retry-report", "--report"].includes(argument)) {
      const value = argv[++index];
      if (!value) throw new Error("client regression option value is required");
      if (argument === "--module") options.moduleIds.push(...commaValues(value, "module id"));
      else if (argument === "--lane") options.lanes.push(...commaValues(value, "lane id"));
      else if (argument === "--agent") options.agentIds.push(...commaValues(value, "agent id"));
      else if (argument === "--platform") options.platformIds.push(...commaValues(value, "platform id"));
      else if (argument === "--changed-from") options.changedFrom = value;
      else if (argument === "--retry-report") options.retryReport = reportFile(value);
      else options.reportPath = reportFile(value);
    } else if (argument.startsWith("--module=")) {
      options.moduleIds.push(...commaValues(argument.slice(9), "module id"));
    } else if (argument.startsWith("--lane=")) {
      options.lanes.push(...commaValues(argument.slice(7), "lane id"));
    } else if (argument.startsWith("--agent=")) {
      options.agentIds.push(...commaValues(argument.slice(8), "agent id"));
    } else if (argument.startsWith("--platform=")) {
      options.platformIds.push(...commaValues(argument.slice(11), "platform id"));
    } else if (argument.startsWith("--changed-from=")) options.changedFrom = argument.slice(15);
    else if (argument.startsWith("--retry-report=")) options.retryReport = reportFile(argument.slice(15));
    else if (argument.startsWith("--report=")) options.reportPath = reportFile(argument.slice(9));
    else throw new Error("unknown client module regression option");
  }
  const selectorCount = Number(options.list) + Number(options.moduleIds.length > 0) +
    Number(options.lanes.length > 0) +
    Number(options.agentIds.length > 0 || options.platformIds.length > 0) +
    Number(options.changedFrom !== null) +
    Number(options.retryReport !== null) + Number(options.help);
  if (selectorCount > 1) {
    throw new Error("choose exactly one core or compatibility selector mode");
  }
  const all = selectorCount === 0;
  if ((options.list || all) && options.dryRun) {
    throw new Error("--dry-run requires a focused selector");
  }
  return Object.freeze({
    ...options,
    all,
    agentIds: Object.freeze(options.agentIds),
    lanes: Object.freeze(options.lanes),
    moduleIds: Object.freeze(options.moduleIds),
    platformIds: Object.freeze(options.platformIds),
  });
}

function writeList(output) {
  for (const module of CLIENT_MODULE_CATALOG) {
    output.write(`${module.id}\t${module.regression.stage}\t${module.regression.toolchain}\t${module.summary}\n`);
  }
  for (const entry of CLIENT_COMPATIBILITY_ENTRIES) {
    output.write(`${entry.kind}.${entry.id}\tcompatibility\t${entry.kind}\tcapability-aware live entry\n`);
  }
}

function writeSelection(modules, output) {
  if (modules.length === 0) {
    output.write("[client-regression] no module matched the selection\n");
    return;
  }
  for (const module of modules) {
    output.write(`${module.id}\t${module.regression.stage}\t${module.regression.toolchain}\n`);
  }
}

function selectCompatibilityEntries(agentIds, platformIds) {
  const requested = new Set([
    ...agentIds.map((id) => `agent:${id}`),
    ...platformIds.map((id) => `platform:${id}`),
  ]);
  const available = new Map(CLIENT_COMPATIBILITY_ENTRIES.map((entry) =>
    [`${entry.kind}:${entry.id}`, entry]));
  for (const id of requested) {
    if (!available.has(id)) throw new Error("unknown compatibility regression target");
  }
  return CLIENT_COMPATIBILITY_ENTRIES.filter((entry) =>
    requested.has(`${entry.kind}:${entry.id}`));
}

function writeCompatibilitySelection(entries, output) {
  for (const entry of entries) output.write(`${entry.kind}.${entry.id}\tcompatibility\t${entry.kind}\n`);
}

export async function main(argv = process.argv.slice(2), {
  output = process.stdout,
  errorOutput = process.stderr,
  changedPathLoader = changedPathsSince,
  executor = executeClientModules,
  retryLoader = retrySelectionFromReport,
  compatibilityRunner = runClientCompatibilityFrontier,
} = {}) {
  try {
    validateClientModuleCatalog();
    const options = parseClientModuleRegressionArgs(argv);
    if (options.help) {
      output.write(usage);
      return 0;
    }
    if (options.list) {
      writeList(output);
      return 0;
    }
    let selected;
    let compatibilityEntries = [];
    let runKind = "focused";
    if (options.all) {
      selected = CLIENT_MODULE_CATALOG;
      compatibilityEntries = CLIENT_COMPATIBILITY_ENTRIES;
      runKind = "complete";
    } else if (options.moduleIds.length > 0) {
      selected = selectModulesById(options.moduleIds);
    } else if (options.lanes.length > 0) {
      selected = selectModulesByLane(options.lanes);
    } else if (options.agentIds.length > 0 || options.platformIds.length > 0) {
      selected = [];
      compatibilityEntries = selectCompatibilityEntries(options.agentIds, options.platformIds);
    } else if (options.retryReport) {
      const retry = await retryLoader(options.retryReport, {
        validModuleIds: CLIENT_MODULE_CATALOG.map((module) => module.id),
        validCompatibilityIds: CLIENT_COMPATIBILITY_ENTRIES.map((entry) =>
          `${entry.kind}:${entry.id}`),
      });
      selected = retry.moduleIds.length > 0 ? selectModulesById(retry.moduleIds) : [];
      const keys = new Set(retry.compatibilityIds);
      compatibilityEntries = CLIENT_COMPATIBILITY_ENTRIES.filter((entry) =>
        keys.has(`${entry.kind}:${entry.id}`));
      runKind = "retry";
    } else {
      selected = selectModulesForChangedPaths(await changedPathLoader({
        revision: options.changedFrom,
        repoRoot,
      }));
    }
    if (options.dryRun) {
      if (selected.length > 0) writeSelection(selected, output);
      if (compatibilityEntries.length > 0) writeCompatibilitySelection(compatibilityEntries, output);
      if (selected.length === 0 && compatibilityEntries.length === 0) writeSelection([], output);
      return 0;
    }
    if (selected.length === 0 && compatibilityEntries.length === 0) {
      writeSelection(selected, output);
      return 0;
    }
    const result = await executor(selected, {
      repoRoot,
      catalog: CLIENT_MODULE_CATALOG,
      output,
      reportPath: options.reportPath,
      runKind,
      compatibilityRunner: compatibilityEntries.length > 0
        ? ({ capacities }) => compatibilityRunner({
          repoRoot,
          capacities,
          live: !options.staticCompatibility,
          entries: compatibilityEntries,
        })
        : async () => [],
    });
    return result.exitCode;
  } catch (error) {
    errorOutput.write(`client module regression: ${error?.message || "failed"}\n`);
    return 2;
  }
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) process.exitCode = await main();
