import { existsSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  ANDROID_NATIVE_CARGO_TARGET,
  FLUTTER_TEST_TARGET,
  NATIVE_CARGO_TEST_TARGET,
  ROOT_CARGO_TARGET,
} from "./constants.mjs";

const BUILD_COMMANDS = new Set([
  "bench",
  "build",
  "check",
  "clippy",
  "doc",
  "llvm-cov",
  "run",
  "rustc",
  "test",
]);

function withoutTomlComment(value) {
  let quote = "";
  let escaped = false;
  for (let index = 0; index < value.length; index += 1) {
    const character = value[index];
    if (escaped) {
      escaped = false;
    } else if (quote === '"' && character === "\\") {
      escaped = true;
    } else if (quote && character === quote) {
      quote = "";
    } else if (!quote && (character === '"' || character === "'")) {
      quote = character;
    } else if (!quote && character === "#") {
      return value.slice(0, index).trim();
    }
  }
  return value.trim();
}

function tomlString(value) {
  const input = withoutTomlComment(value);
  if (input.startsWith('"')) {
    const end = input.lastIndexOf('"');
    if (end <= 0) return null;
    try {
      return JSON.parse(input.slice(0, end + 1));
    } catch {
      return null;
    }
  }
  if (input.startsWith("'")) {
    const end = input.indexOf("'", 1);
    return end < 0 ? null : input.slice(1, end);
  }
  return null;
}

export function cargoConfigTargetDirectory(contents) {
  let section = "";
  let targetDirectory = null;
  for (const line of contents.split(/\r?\n/u)) {
    const value = withoutTomlComment(line);
    if (!value) continue;
    const table = /^\[([^\]]+)\]$/u.exec(value);
    if (table) {
      section = table[1].trim();
      continue;
    }
    const dotted = /^build\.target-dir\s*=\s*(.+)$/u.exec(value);
    const local = section === "build" && /^target-dir\s*=\s*(.+)$/u.exec(value);
    const assignment = dotted || local;
    if (assignment) targetDirectory = tomlString(assignment[1]);
  }
  return targetDirectory;
}

function commandAndWorkingDirectory(args, cwd) {
  const values = [...args];
  let workingDirectory = path.resolve(cwd);
  let command = "";
  for (let index = 0; index < values.length; index += 1) {
    const argument = values[index];
    if (argument === "--") break;
    if (!command && argument.startsWith("+") && argument.length > 1) continue;
    if (argument === "-C" && values[index + 1]) {
      workingDirectory = path.resolve(workingDirectory, values[index + 1]);
      index += 1;
      continue;
    }
    if ((argument === "--config" || argument === "-Z") && values[index + 1]) {
      index += 1;
      continue;
    }
    if (argument.startsWith("--config=")) continue;
    if (argument.startsWith("-")) continue;
    if (BUILD_COMMANDS.has(argument)) {
      command = argument;
      break;
    }
    if (command) break;
  }
  return { command, workingDirectory };
}

export function cargoBuildCommand(args) {
  const { command } = commandAndWorkingDirectory(args, process.cwd());
  return BUILD_COMMANDS.has(command) ? command : null;
}

function configFiles(workingDirectory, env) {
  const files = [];
  let directory = path.resolve(workingDirectory);
  while (true) {
    const cargoDirectory = path.join(directory, ".cargo");
    const legacy = path.join(cargoDirectory, "config");
    const toml = path.join(cargoDirectory, "config.toml");
    if (existsSync(legacy)) files.push({ path: legacy, base: directory });
    else if (existsSync(toml)) files.push({ path: toml, base: directory });
    const parent = path.dirname(directory);
    if (parent === directory) break;
    directory = parent;
  }

  const cargoHome = env.CARGO_HOME || path.join(os.homedir(), ".cargo");
  const legacy = path.join(cargoHome, "config");
  const toml = path.join(cargoHome, "config.toml");
  if (existsSync(legacy)) files.push({ path: legacy, base: path.dirname(cargoHome) });
  else if (existsSync(toml)) files.push({ path: toml, base: path.dirname(cargoHome) });
  return files;
}

function readConfigTarget(file) {
  try {
    return cargoConfigTargetDirectory(readFileSync(file.path, "utf8"));
  } catch {
    return null;
  }
}

function configHasSeparateBuildDirectory(file) {
  try {
    let section = "";
    for (const line of readFileSync(file.path, "utf8").split(/\r?\n/u)) {
      const value = withoutTomlComment(line);
      if (!value) continue;
      const table = /^\[([^\]]+)\]$/u.exec(value);
      if (table) {
        section = table[1].trim();
        continue;
      }
      if (/^build\.build-dir\s*=/u.test(value) ||
          (section === "build" && /^build-dir\s*=/u.test(value))) return true;
    }
  } catch {
    return true;
  }
  return false;
}

export function cargoBuildDirectoryConfigured({ cwd = process.cwd(), env = process.env } = {}) {
  if (env.CARGO_BUILD_BUILD_DIR) return true;
  return configFiles(path.resolve(cwd), env).some(configHasSeparateBuildDirectory);
}

function parseConfigOverride(value, workingDirectory) {
  const assignment = /^\s*build\.target-dir\s*=\s*(.+)$/u.exec(value);
  if (assignment) {
    const targetDirectory = tomlString(assignment[1]);
    return targetDirectory === null
      ? null
      : { base: workingDirectory, value: targetDirectory };
  }
  const file = path.resolve(workingDirectory, value);
  const base = path.dirname(path.dirname(file));
  const targetDirectory = readConfigTarget({ path: file, base });
  return targetDirectory === null ? null : { base, value: targetDirectory };
}

function commandLineConfigTarget(args, workingDirectory) {
  let target = null;
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--") break;
    let value = null;
    if (argument === "--config") {
      value = args[index + 1];
      index += 1;
    } else if (argument.startsWith("--config=")) {
      value = argument.slice("--config=".length);
    }
    if (value) target = parseConfigOverride(value, workingDirectory) || target;
  }
  return target;
}

function commandLineTargetDirectory(args, workingDirectory) {
  let targetDirectory = null;
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--") break;
    if (argument === "--target-dir" && args[index + 1]) {
      targetDirectory = { base: workingDirectory, value: args[index + 1] };
      index += 1;
    } else if (argument.startsWith("--target-dir=")) {
      targetDirectory = {
        base: workingDirectory,
        value: argument.slice("--target-dir=".length),
      };
    }
  }
  return targetDirectory;
}

export function resolveCargoTargetDirectory({
  args = [],
  env = process.env,
  cwd = process.cwd(),
  workspaceRoot,
} = {}) {
  const { workingDirectory } = commandAndWorkingDirectory(args, cwd);
  let target = commandLineTargetDirectory(args, workingDirectory);
  if (!target && env.CARGO_TARGET_DIR) {
    target = { base: workingDirectory, value: env.CARGO_TARGET_DIR };
  }
  if (!target) target = commandLineConfigTarget(args, workingDirectory);
  if (!target && env.CARGO_BUILD_TARGET_DIR) {
    target = { base: workingDirectory, value: env.CARGO_BUILD_TARGET_DIR };
  }
  if (!target) {
    const files = configFiles(workingDirectory, env);
    for (const file of files) {
      const value = readConfigTarget(file);
      if (value !== null) {
        target = { base: file.base, value };
        break;
      }
    }
  }
  const workspace = path.resolve(workspaceRoot || cwd);
  const absolutePath = path.resolve(target?.base || workspace, target?.value || ROOT_CARGO_TARGET);
  const relativePath = path.relative(workspace, absolutePath).split(path.sep).join("/");
  const insideWorkspace = relativePath !== ".." &&
    !relativePath.startsWith("../") && !path.isAbsolute(relativePath);
  return Object.freeze({
    absolutePath,
    insideWorkspace,
    relativePath: insideWorkspace ? relativePath : null,
  });
}

export function isManagedCargoTarget(repoRoot, targetPath) {
  const root = path.resolve(repoRoot);
  const absoluteTarget = path.resolve(root, targetPath);
  const relative = path.relative(root, absoluteTarget);
  if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
    return false;
  }
  const normalized = relative.split(path.sep).join("/");
  if (normalized === ROOT_CARGO_TARGET ||
      normalized === NATIVE_CARGO_TEST_TARGET ||
      normalized === ANDROID_NATIVE_CARGO_TARGET) return true;
  if (normalized === FLUTTER_TEST_TARGET ||
      normalized.startsWith(`${FLUTTER_TEST_TARGET}/`)) return false;
  const nativeTargetRoot = /^build\/crates\/[^/]+\/(?:target|[a-z0-9._-]+-target)$/u;
  return nativeTargetRoot.test(normalized);
}
