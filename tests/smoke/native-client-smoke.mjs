#!/usr/bin/env node
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import {
  acquireTestArtifactLease,
  NATIVE_CARGO_TEST_TARGET,
} from "../../tools/scripts/lib/test-artifact-lifecycle.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const cargoArtifactLease = acquireTestArtifactLease({
  repoRoot,
  scope: "native-client-smoke",
  targetPath: NATIVE_CARGO_TEST_TARGET,
});
let portableDir = "";
const argumentsSet = new Set(process.argv.slice(2));
const runtimeDataAuthorized = argumentsSet.has("--runtime-data");
const maxOutputBytes = 4 * 1024 * 1024;

const optionsValid = [...argumentsSet].every((argument) =>
  argument === "--runtime-data");

function runClient(args) {
  return new Promise((resolve) => {
    const child = spawn("cargo", [
      "run",
      "--quiet",
      "--manifest-path",
      "crates/licoup-native/Cargo.toml",
      "--bin",
      "licoup-cli",
      "--",
      ...args
    ], {
      cwd: repoRoot,
      env: {
        ...process.env,
        CARGO_TARGET_DIR: cargoArtifactLease.targetPath,
        LICOUP_HOME: portableDir
      },
      stdio: ["ignore", "pipe", "pipe"]
    });
    const stdout = [];
    const stderr = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let outputExceeded = false;
    const collect = (target, kind) => (chunk) => {
      const bytes = Buffer.from(chunk);
      if (kind === "stdout") stdoutBytes += bytes.length;
      else stderrBytes += bytes.length;
      if (stdoutBytes + stderrBytes > maxOutputBytes) {
        outputExceeded = true;
        child.kill("SIGKILL");
        return;
      }
      target.push(bytes);
    };
    child.stdout.on("data", collect(stdout, "stdout"));
    child.stderr.on("data", collect(stderr, "stderr"));
    let settled = false;
    child.once("error", () => {
      if (settled) return;
      settled = true;
      resolve({
        code: -1,
        stdout: "",
        stderr: "",
        stdoutBytes,
        stderrBytes,
        outputExceeded,
      });
    });
    child.once("close", (code) => {
      if (settled) return;
      settled = true;
      resolve({
        code,
        stdout: Buffer.concat(stdout).toString("utf8").trim(),
        stderr: Buffer.concat(stderr).toString("utf8").trim(),
        stdoutBytes,
        stderrBytes,
        outputExceeded,
      });
    });
  });
}

async function runJson(args, commandId) {
  const result = await runClient(args);
  assert.equal(result.outputExceeded, false,
    `native_smoke_output_limit:${commandId}:${result.stdoutBytes}:${result.stderrBytes}`);
  assert.equal(result.code, 0,
    `native_smoke_command_failed:${commandId}:${result.stdoutBytes}:${result.stderrBytes}`);
  try {
    return JSON.parse(result.stdout || "{}");
  } catch {
    throw new Error(`native_smoke_json_invalid:${commandId}:${result.stdoutBytes}`);
  }
}

let smokeFailed = false;
let smokeStage = "setup";
try {
  portableDir = await fs.mkdtemp(path.join(os.tmpdir(), "lico-native-client-smoke-"));
  assert.equal(optionsValid, true, "native_smoke_option_invalid");
  smokeStage = "usage";
  const help = await runClient(["help"]);
  assert.equal(help.code, 0);
  assert.equal(help.stderr.includes("Usage:"), true, "native_smoke_usage_missing");

  smokeStage = "state-settings";
  const settings = await runJson(["state", "get", "settings"], "state-settings");
  assert.equal(settings.ok, true);
  assert.equal(settings.collection, "settings");
  assert.equal(settings.document?.schemaVersion, "v0.0.1:schema:definition-1");

  smokeStage = "targets-scan";
  const targets = await runJson([
    "targets",
    "scan",
    "--include-accessible-environments",
    String(runtimeDataAuthorized),
    "--include-history-model-catalog",
    String(runtimeDataAuthorized),
  ], "targets-scan");
  assert.equal(targets.ok, true);
  assert.equal(Array.isArray(targets.candidates), true);
  assert.ok(targets.candidates.some((candidate) => candidate.target === "codex"));
  if (process.platform === "win32") {
    for (const candidate of targets.candidates) {
      assert.equal(
        String(candidate.configPath || "").includes("Library/Application Support"),
        false,
        `${candidate.target} should not expose a macOS-only default config path on Windows`
      );
    }
  }

  smokeStage = "snapshot-profiles";
  const profiles = await runJson(
    ["snapshots", "profiles", "list"],
    "snapshot-profiles",
  );
  assert.equal(profiles.ok, true);
  assert.equal(Array.isArray(profiles.profiles), true);

  smokeStage = "activity-list";
  const activity = await runJson(["activity", "list"], "activity-list");
  assert.equal(activity.ok, true);
  assert.equal(Array.isArray(activity.events), true);

  console.log(JSON.stringify({
    ok: true,
    runtimeDataAuthorized,
    privateRuntimeOutputIncluded: false,
  }));
} catch {
  smokeFailed = true;
  console.error(JSON.stringify({
    ok: false,
    error: "native_smoke_failed",
    stage: smokeStage,
    rawRuntimeOutputIncluded: false,
    privatePathsIncluded: false,
  }));
} finally {
  if (portableDir) {
    try {
      await fs.rm(portableDir, { recursive: true, force: true });
    } catch {
      smokeFailed = true;
      console.error(JSON.stringify({
        ok: false,
        error: "native_smoke_cleanup_failed",
        rawRuntimeOutputIncluded: false,
        privatePathsIncluded: false,
      }));
    }
  }
  cargoArtifactLease.release();
}
if (smokeFailed) process.exitCode = 1;
