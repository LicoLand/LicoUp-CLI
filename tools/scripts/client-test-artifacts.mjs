#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { realpathSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  completeTestArtifactWorkspace,
  inventoryTestArtifacts,
  pruneReclaimableTestArtifacts,
  pruneTestArtifactTarget,
  testArtifactStatus,
} from "./lib/test-artifact-lifecycle.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));

export function parseArtifactCommand(argv) {
  const [command, ...args] = argv;
  if (command === "status" && args.length === 0) {
    return Object.freeze({ command, dryRun: false, repoRoot: null, targetPath: null });
  }
  if (command === "prune" && args.length === 0) {
    return Object.freeze({ command, dryRun: false, repoRoot: null, targetPath: null });
  }

  const parsed = { command, dryRun: false, repoRoot: null, targetPath: null };
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--dry-run" && !parsed.dryRun) {
      parsed.dryRun = true;
    } else if (argument === "--repo-root" && args[index + 1] && !parsed.repoRoot) {
      parsed.repoRoot = args[index + 1];
      index += 1;
    } else if (argument === "--target" && args[index + 1] && !parsed.targetPath) {
      parsed.targetPath = args[index + 1];
      index += 1;
    } else {
      return null;
    }
  }
  const valid =
    (command === "inventory" && parsed.targetPath === null) ||
    (command === "prune" && (Boolean(parsed.targetPath) || !parsed.repoRoot)) ||
    (command === "workspace-complete" && parsed.targetPath === null);
  return valid ? Object.freeze(parsed) : null;
}

function gitCheckoutIdentity(directory) {
  const root = realpathSync(directory);
  const top = execFileSync("git", ["rev-parse", "--show-toplevel"], {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
  if (realpathSync(top) !== root) throw new Error("workspace root is invalid");
  const common = execFileSync("git", ["rev-parse", "--git-common-dir"], {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "ignore"],
  }).trim();
  return realpathSync(path.resolve(root, common));
}

function resolveWorkspaceRoot(requestedRoot, currentRepoRoot = repoRoot) {
  if (!requestedRoot) return currentRepoRoot;
  const candidate = path.resolve(requestedRoot);
  try {
    const expectedCommon = gitCheckoutIdentity(currentRepoRoot);
    const candidateCommon = gitCheckoutIdentity(candidate);
    if (candidateCommon !== expectedCommon) throw new Error("workspace repository differs");
    return candidate;
  } catch {
    throw new Error("workspace root must be a checkout of this repository");
  }
}

export function runArtifactCommand(parsed, { currentRepoRoot = repoRoot, env = process.env } = {}) {
  if (!parsed) return null;
  if (parsed.command === "status") {
    return { ok: true, operation: "status", ...testArtifactStatus({ repoRoot: currentRepoRoot }) };
  }
  if (parsed.command === "prune" && !parsed.targetPath) {
    const result = pruneReclaimableTestArtifacts({ repoRoot: currentRepoRoot, dryRun: parsed.dryRun });
    return {
      ok: result.failed === 0,
      operation: parsed.dryRun ? "prune-dry-run" : "prune",
      ...result,
    };
  }
  if (parsed.command === "inventory") {
    const targetRoot = resolveWorkspaceRoot(parsed.repoRoot, currentRepoRoot);
    return {
      ok: true,
      operation: "inventory",
      ...inventoryTestArtifacts({ repoRoot: targetRoot, env }),
    };
  }
  if (parsed.command === "prune" && parsed.targetPath) {
    const targetRoot = resolveWorkspaceRoot(parsed.repoRoot, currentRepoRoot);
    const result = pruneTestArtifactTarget({
      repoRoot: targetRoot,
      targetPath: parsed.targetPath,
      dryRun: parsed.dryRun,
      env,
    });
    return {
      ok: result.failed === 0,
      operation: parsed.dryRun ? "prune-target-dry-run" : "prune-target",
      relativeTarget: parsed.targetPath,
      ...result,
    };
  }
  if (parsed.command === "workspace-complete") {
    const targetRoot = resolveWorkspaceRoot(parsed.repoRoot, currentRepoRoot);
    const result = completeTestArtifactWorkspace({
      repoRoot: targetRoot,
      dryRun: parsed.dryRun,
      env,
    });
    return {
      ok: result.failed === 0,
      operation: parsed.dryRun ? "workspace-complete-dry-run" : "workspace-complete",
      ...result,
    };
  }
  return null;
}

function usage() {
  process.stderr.write(
    "Usage: client-test-artifacts.mjs status | inventory [--repo-root <checkout>] | prune [--dry-run] | prune --target <relative-target> [--repo-root <checkout>] [--dry-run] | workspace-complete [--repo-root <checkout>] [--dry-run]\n",
  );
}

function main() {
  const parsed = parseArtifactCommand(process.argv.slice(2));
  if (!parsed) {
    usage();
    process.exitCode = 2;
    return;
  }
  try {
    const result = runArtifactCommand(parsed);
    if (!result) {
      usage();
      process.exitCode = 2;
      return;
    }
    process.stdout.write(`${JSON.stringify(result)}\n`);
    if (!result.ok) process.exitCode = 1;
  } catch {
    process.stderr.write("Artifact lifecycle operation failed.\n");
    process.exitCode = 1;
  }
}

if (fileURLToPath(import.meta.url) === path.resolve(process.argv[1] || "")) main();
