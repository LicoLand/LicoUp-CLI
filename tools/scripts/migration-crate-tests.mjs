#!/usr/bin/env node
// Run the standalone migration crate's suites under the planned candidate identity.
//
// The released-root fixtures are frozen from the published product v0.2.1, whose ledger
// records product high-water `0.2.1`. The client's own admission guard refuses a binary
// older than that high-water, and the delivery never lowers a released fixture to fit a
// development build. The intended candidate identity therefore comes from the real
// build-metadata owner: the native build script injects `LICO_CLIENT_PRODUCT_VERSION`,
// and this runner passes the workspace product version (the planned release) to it.
//
// The runner also builds the real `licoup-cli` binary and hands its path to the
// cross-container interoperability suite, which restores archives produced by the client
// CLI through the tool and archives produced by the tool through the client CLI. `cargo
// test` only exports `CARGO_BIN_EXE_*` for the package's own binaries, so the path is
// constructed here, in the same target directory and under the same identity.
//
// Everything runs offline and locked against the checkout's isolated target directory.
// --native-recovery selects the native recovery and relocation consumers under that
// same identity. Other arguments are forwarded to the selected Cargo test invocation.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { acquireTestArtifactLease, NATIVE_CARGO_TEST_TARGET } from "./lib/test-artifact-lifecycle.mjs";

const workspaceRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const jobs = process.env.CARGO_BUILD_JOBS || "3";

function fail(message) {
  throw new Error(`migration-crate-tests: ${message}`);
}

function workspaceProductVersion() {
  const manifest = readFileSync(path.join(workspaceRoot, "Cargo.toml"), "utf8");
  const section = manifest.match(/\[workspace\.package\]([\s\S]*?)(?:\n\[|$)/u);
  if (!section) return "";
  const version = section[1].match(/^\s*version\s*=\s*"([^"]+)"/mu);
  return version ? version[1] : "";
}

function run(command, args, env) {
  const result = spawnSync(command, args, {
    cwd: workspaceRoot,
    env,
    stdio: "inherit",
    shell: false
  });
  if (result.error) fail(`${command} failed to start: ${result.error.message}`);
  return result.status ?? 1;
}

function metadata(env) {
  const result = spawnSync(
    "cargo",
    ["metadata", "--offline", "--locked", "--format-version", "1"],
    {
      cwd: workspaceRoot,
      env,
      encoding: "utf8",
      shell: false,
      maxBuffer: 64 * 1024 * 1024
    }
  );
  if (result.error) fail(`cargo metadata failed to start: ${result.error.message}`);
  if (result.status !== 0) {
    process.stderr.write(result.stderr ?? "");
    fail("cargo metadata refused the workspace; the lockfile is not current");
  }
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    fail(`cargo metadata did not produce JSON: ${error.message}`);
  }
}

export function migrationCrateTestArgs(forwarded = []) {
  const args = [...forwarded];
  const nativeRecovery = args[0] === "--native-recovery";
  if (nativeRecovery) args.shift();
  const selection = nativeRecovery
    ? ["-p", "licoup-native", "--test", "local_recovery", "--test", "data_home_process"]
    : ["-p", "licoup-migrate"];
  return ["test", "--offline", "--locked", "--no-fail-fast", ...selection, ...args];
}

function main() {
  const productVersion = workspaceProductVersion();
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/u.test(productVersion)) {
    fail(`the workspace product version is not a semantic version: ${JSON.stringify(productVersion)}`);
  }

  const lease = acquireTestArtifactLease({
    repoRoot: workspaceRoot,
    scope: "migration-crate-tests",
    targetPath: NATIVE_CARGO_TEST_TARGET
  });
  try {
    const environment = {
      ...process.env,
      CARGO_BUILD_JOBS: jobs,
      CARGO_TARGET_DIR: lease.targetPath,
      LICO_MOBILE_RELAY_NATIVE_SECRET_STORE: "disabled",
      // The native build-metadata owner supplies the candidate identity; the
      // frozen released fixture's product high-water is never changed.
      LICO_CLIENT_PRODUCT_VERSION: productVersion
    };

    const targetDirectory = metadata(environment).target_directory;
    if (typeof targetDirectory !== "string" || targetDirectory.length === 0) {
      fail("cargo metadata did not report a target directory");
    }

    const cliBinary = path.join(
      targetDirectory,
      "debug",
      process.platform === "win32" ? "licoup-cli.exe" : "licoup-cli"
    );
    const buildStatus = run(
      "cargo",
      ["build", "--offline", "--locked", "-j", jobs, "-p", "licoup-native", "--bin", "licoup-cli"],
      environment
    );
    if (buildStatus !== 0) {
      fail("the client CLI binary did not build; the interoperability oracle cannot run");
    }

    environment.LICOUP_MIGRATE_CLIENT_CLI = cliBinary;
    process.stdout.write(`migration-crate-tests: candidate identity ${productVersion}\n`);
    return run(
      "cargo",
      ["test", "-j", jobs, ...migrationCrateTestArgs(process.argv.slice(2)).slice(1)],
      environment
    );
  } finally {
    lease.release();
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main();
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}
