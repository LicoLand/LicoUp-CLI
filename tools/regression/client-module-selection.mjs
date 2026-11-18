import { spawn } from "node:child_process";
import path from "node:path";
import { CLIENT_MODULE_CATALOG } from "./client-module-catalog.mjs";
import { CLIENT_REGRESSION_STAGES } from "./client-regression-metadata.mjs";

const compiledCatalogs = new WeakMap();
const allowedPrograms = new Set(["cargo", "node"]);
const allowedHosts = new Set(["darwin", "linux", "win32"]);

export function partitionModulesByRunnableHost(modules, host) {
  if (!Array.isArray(modules)) throw new Error("client modules must be an array");
  if (!allowedHosts.has(host)) throw new Error("client module execution host is unsupported");
  const runnable = [];
  const unsupported = [];
  for (const module of modules) {
    if (module?.regression?.runnableHosts?.includes(host)) runnable.push(module);
    else unsupported.push(module);
  }
  return Object.freeze({
    runnable: Object.freeze(runnable),
    unsupported: Object.freeze(unsupported),
  });
}

export function normalizeRepoPath(value) {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("repository path must be a non-empty string");
  }
  const portable = value.replaceAll("\\", "/").replace(/^\.\//u, "");
  if (portable.startsWith("/") || /^[A-Za-z]:\//u.test(portable)) {
    throw new Error("repository path must be relative");
  }
  const normalized = path.posix.normalize(portable);
  if (normalized === "." || normalized === ".." || normalized.startsWith("../")) {
    throw new Error("repository path must stay inside the repository");
  }
  return normalized;
}

function compileInput(input) {
  if (typeof input !== "string" || input.length === 0) {
    throw new Error("module input must be a non-empty string");
  }
  if (input.endsWith("/**")) {
    const prefix = normalizeRepoPath(input.slice(0, -3));
    return (candidate) => candidate === prefix || candidate.startsWith(`${prefix}/`);
  }
  if (/[*?\[\]]/u.test(input)) {
    throw new Error("module inputs only support an exact path or a terminal /**");
  }
  const exact = normalizeRepoPath(input);
  return (candidate) => candidate === exact;
}

export function validateClientModuleCatalog(catalog = CLIENT_MODULE_CATALOG) {
  if (!Array.isArray(catalog) || catalog.length === 0) {
    throw new Error("client module catalog must not be empty");
  }
  const ids = new Set();
  for (const module of catalog) {
    if (!module || typeof module !== "object") {
      throw new Error("client module catalog entry must be an object");
    }
    if (!/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/u.test(module.id || "")) {
      throw new Error("client module id is invalid");
    }
    if (ids.has(module.id)) {
      throw new Error(`duplicate client module id: ${module.id}`);
    }
    ids.add(module.id);
    if (typeof module.kind !== "string" || module.kind.length === 0) {
      throw new Error(`client module kind is missing: ${module.id}`);
    }
    if (typeof module.summary !== "string" || module.summary.length === 0) {
      throw new Error(`client module summary is missing: ${module.id}`);
    }
    if (!Array.isArray(module.inputs) || module.inputs.length === 0) {
      throw new Error(`client module inputs are missing: ${module.id}`);
    }
    const uniqueInputs = new Set();
    for (const input of module.inputs) {
      compileInput(input);
      if (uniqueInputs.has(input)) {
        throw new Error(`duplicate input in client module: ${module.id}`);
      }
      uniqueInputs.add(input);
    }
    const moduleCommand = module.command;
    if (!moduleCommand || typeof moduleCommand !== "object") {
      throw new Error(`client module command is missing: ${module.id}`);
    }
    if (!allowedPrograms.has(moduleCommand.program)) {
      throw new Error(`client module command program is not allowed: ${module.id}`);
    }
    if (!Array.isArray(moduleCommand.args) || moduleCommand.args.some((arg) => typeof arg !== "string")) {
      throw new Error(`client module command args are invalid: ${module.id}`);
    }
    if (moduleCommand.args.some((arg) => arg.includes("client:gate:"))) {
      throw new Error(`client module must not invoke an aggregate gate: ${module.id}`);
    }
    if (moduleCommand.cwd !== ".") {
      throw new Error(`client module command cwd must remain repository-relative: ${module.id}`);
    }
    if (!Number.isSafeInteger(moduleCommand.timeoutMs) || moduleCommand.timeoutMs <= 0) {
      throw new Error(`client module command timeout is invalid: ${module.id}`);
    }
    const regression = module.regression;
    if (!regression || !CLIENT_REGRESSION_STAGES.includes(regression.stage)) {
      throw new Error(`client module regression stage is invalid: ${module.id}`);
    }
    if (regression.lane !== regression.stage || typeof regression.environment !== "string") {
      throw new Error(`client module regression lane is invalid: ${module.id}`);
    }
    if (!Number.isSafeInteger(regression.weight) || regression.weight <= 0) {
      throw new Error(`client module regression weight is invalid: ${module.id}`);
    }
    if (!Array.isArray(regression.resources) ||
        regression.resources.some((resource) => typeof resource !== "string" || resource.length === 0)) {
      throw new Error(`client module regression resources are invalid: ${module.id}`);
    }
    if (typeof regression.batchKey !== "string" || regression.batchKey.length === 0 ||
        typeof regression.internalParallelism !== "boolean") {
      throw new Error(`client module regression batching metadata is invalid: ${module.id}`);
    }
    for (const [field, hosts] of [
      ["runnableHosts", regression.runnableHosts],
      ["targetEvidenceHosts", regression.targetEvidenceHosts],
    ]) {
      if (!Array.isArray(hosts) || !Object.isFrozen(hosts) ||
          hosts.some((host) => !allowedHosts.has(host)) ||
          new Set(hosts).size !== hosts.length) {
        throw new Error(`client module ${field} is invalid: ${module.id}`);
      }
    }
    if (regression.runnableHosts.length === 0 ||
        regression.targetEvidenceHosts.some((host) => !regression.runnableHosts.includes(host))) {
      throw new Error(`client module host coverage is invalid: ${module.id}`);
    }
  }
  return true;
}

function compileCatalog(catalog) {
  const cached = compiledCatalogs.get(catalog);
  if (cached) return cached;
  validateClientModuleCatalog(catalog);
  const compiled = catalog.map((module) => Object.freeze({
    module,
    matchers: Object.freeze(module.inputs.map(compileInput)),
  }));
  compiledCatalogs.set(catalog, compiled);
  return compiled;
}

export function selectModulesForChangedPaths(changedPaths, catalog = CLIENT_MODULE_CATALOG) {
  if (!Array.isArray(changedPaths)) {
    throw new Error("changed paths must be an array");
  }
  const candidates = [...new Set(changedPaths.map(normalizeRepoPath))];
  if (candidates.length === 0) return [];
  return compileCatalog(catalog)
    .filter(({ matchers }) => candidates.some((candidate) =>
      matchers.some((matches) => matches(candidate))))
    .map(({ module }) => module);
}

export function selectModulesById(moduleIds, catalog = CLIENT_MODULE_CATALOG) {
  if (!Array.isArray(moduleIds) || moduleIds.length === 0) {
    throw new Error("at least one client module id is required");
  }
  validateClientModuleCatalog(catalog);
  const requested = new Set();
  for (const id of moduleIds) {
    if (!/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/u.test(id || "")) {
      throw new Error("client module id is invalid");
    }
    requested.add(id);
  }
  const selected = catalog.filter((module) => requested.has(module.id));
  const missing = [...requested].filter((id) => !selected.some((module) => module.id === id));
  if (missing.length > 0) {
    throw new Error(`unknown client module: ${missing.join(", ")}`);
  }
  return selected;
}

export function selectModulesByLane(lanes, catalog = CLIENT_MODULE_CATALOG) {
  if (!Array.isArray(lanes) || lanes.length === 0) {
    throw new Error("at least one client regression lane is required");
  }
  validateClientModuleCatalog(catalog);
  const requested = new Set(lanes);
  const known = new Set(catalog.map((module) => module.regression.lane));
  const missing = [...requested].filter((lane) => !known.has(lane));
  if (missing.length > 0) throw new Error(`unknown client regression lane: ${missing.join(", ")}`);
  return catalog.filter((module) => requested.has(module.regression.lane));
}

export function validateChangedFromRevision(value) {
  if (typeof value !== "string" || value.length === 0 || value.length > 256) {
    throw new Error("changed-from revision is invalid");
  }
  if (value.startsWith("-") || /[\0-\x20\x7f]/u.test(value)) {
    throw new Error("changed-from revision is invalid");
  }
  return value;
}

export function parseNulDelimitedPaths(value) {
  const text = Buffer.isBuffer(value) ? value.toString("utf8") : String(value || "");
  return text
    .split("\0")
    .filter(Boolean)
    .map(normalizeRepoPath);
}

function gitPathOutput(program, args, options, spawnImpl) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    let outputBytes = 0;
    let overflow = false;
    let child;
    try {
      child = spawnImpl(program, args, options);
    } catch {
      reject(new Error("git change inspection could not start"));
      return;
    }
    child.once("error", () => reject(new Error("git change inspection could not start")));
    child.stdout.on("data", (chunk) => {
      if (overflow) return;
      const buffer = Buffer.from(chunk);
      outputBytes += buffer.length;
      if (outputBytes > 16 * 1024 * 1024) {
        chunks.length = 0;
        overflow = true;
      } else {
        chunks.push(buffer);
      }
    });
    child.stderr?.resume?.();
    child.once("close", (code) => {
      if (code !== 0 || overflow) reject(new Error("git change inspection failed"));
      else resolve(Buffer.concat(chunks, outputBytes));
    });
  });
}

export async function changedPathsSince({
  revision,
  repoRoot,
  spawnImpl = spawn,
}) {
  const safeRevision = validateChangedFromRevision(revision);
  const commonOptions = Object.freeze({
    cwd: repoRoot,
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  let tracked;
  let untracked;
  try {
    [tracked, untracked] = await Promise.all([
      gitPathOutput("git", ["diff", "--no-renames", "--name-only", "-z", safeRevision, "--"],
        commonOptions, spawnImpl),
      gitPathOutput("git", ["ls-files", "--others", "--exclude-standard", "-z", "--"],
        commonOptions, spawnImpl),
    ]);
  } catch {
    throw new Error("unable to inspect changes from the requested revision");
  }
  return [...new Set([
    ...parseNulDelimitedPaths(tracked),
    ...parseNulDelimitedPaths(untracked),
  ])];
}
