#!/usr/bin/env node
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { sanitizeError } from "./lib/sanitize-error.mjs";
import {
  acquireTestArtifactLease,
  cargoBuildCommand,
  isManagedCargoTarget,
  resolveCargoTargetDirectory,
} from "./lib/test-artifact-lifecycle.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));

// The CLI-owned product-version authority for `LICO_CLIENT_PRODUCT_VERSION`
// (`crates/licoup-native/build.rs` consumes it). In the monorepo this came from
// `apps/desktop/scripts/package-client/build/native.mjs` reading
// `tools/client-version.json`; this repository owns `tools/cli-version.json`
// instead. Keep the same manifest shape and validation.
function clientProductVersion() {
  const manifest = JSON.parse(readFileSync(
    path.join(repoRoot, "tools", "cli-version.json"),
    "utf8",
  ));
  const version = String(manifest.productVersion || "").trim();
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`Invalid productVersion in tools/cli-version.json: ${version}`);
  }
  return version;
}

export function cargoClientTargetPath({
  repoRoot: root,
  args,
  env = process.env,
  cwd = root,
}) {
  if (!cargoBuildCommand(args)) return null;
  const target = resolveCargoTargetDirectory({
    args,
    env,
    cwd,
    workspaceRoot: root,
  });
  return target.insideWorkspace && isManagedCargoTarget(root, target.relativePath)
    ? target.relativePath
    : null;
}

export function runCargoClient(args, {
  repoRoot: root = repoRoot,
  env = process.env,
  cwd = root,
  spawnCargo = spawn,
  leaseFactory = acquireTestArtifactLease,
} = {}) {
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--output-path" && args[index + 1]) {
      mkdirSync(path.dirname(path.resolve(root, args[index + 1])), { recursive: true });
    }
  }
  if (args.includes("llvm-cov")) {
    mkdirSync(path.join(root, "build", "coverage", "crates", "licoup-native"), { recursive: true });
  }

  const targetPath = cargoClientTargetPath({ repoRoot: root, args, env, cwd });
  const lease = targetPath
    ? leaseFactory({ repoRoot: root, scope: "cargo-client", targetPath })
    : null;
  let leaseReleased = false;
  const releaseLease = () => {
    if (leaseReleased || !lease) return;
    lease.release();
    leaseReleased = true;
  };

  let child;
  try {
    child = spawnCargo("cargo", args, {
      cwd,
      env: {
        ...env,
        // Every produced binary carries the candidate identity; an explicit
        // caller identity wins when exercising another product version.
        LICO_CLIENT_PRODUCT_VERSION: env.LICO_CLIENT_PRODUCT_VERSION || clientProductVersion(),
      },
      stdio: "inherit",
      windowsHide: true,
    });
    if (lease && Number.isSafeInteger(child.pid) && child.pid > 0) {
      try {
        lease.trackProcess(child.pid);
      } catch {
        // The wrapper lease remains active until Cargo exits.
      }
    }
  } catch (error) {
    releaseLease();
    throw error;
  }

  child.once("error", (error) => {
    releaseLease();
    process.stderr.write(`${sanitizeError(error)}\n`);
    process.exitCode = 1;
  });

  child.once("exit", (code, signal) => {
    releaseLease();
    if (signal) {
      process.stderr.write(`cargo terminated by ${signal}\n`);
      process.exitCode = 1;
      return;
    }
    process.exitCode = code ?? 1;
  });
  return child;
}

function main() {
  try {
    runCargoClient(process.argv.slice(2));
  } catch (error) {
    process.stderr.write(`${sanitizeError(error)}\n`);
    process.exitCode = 1;
  }
}

if (fileURLToPath(import.meta.url) === path.resolve(process.argv[1] || "")) main();
