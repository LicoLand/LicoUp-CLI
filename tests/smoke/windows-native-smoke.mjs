#!/usr/bin/env node

/**
 * Windows native smoke (static + win32-native halves).
 *
 * CI home: the `windows-native-smoke` job in
 * .github/workflows/client-regression.yml (nightly, non-gating). The static
 * half (package-script portability, Windows file security) always runs; the
 * native half (cargo build, `targets scan`, secret-store lifecycle + redaction)
 * only executes on win32 runners.
 *
 * Placement choice (documented per CI-regression task): the file stays at
 * tests/smoke/ instead of being relocated to local-only because
 * tools/scripts/config/secure-mesh-windows-implementation.json references this
 * exact path in its source checks, and verify-client-plan asserts that binding;
 * moving it would break those checks. The Windows runner job is therefore the
 * option that keeps per-PR gates green.
 *
 * Local-only usage: `npm run client:native:smoke:windows`. On non-Windows
 * hosts the native half is skipped and the smoke still passes its static half.
 */

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, readFileSync } from "node:fs";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { sanitizeError } from "../../tools/scripts/lib/sanitize-error.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));
const progressPath = path.join(repoRoot, "build", "test-reports", "windows-native-smoke-progress.jsonl");

async function recordProgress(step, status, detail = {}) {
  await fs.mkdir(path.dirname(progressPath), { recursive: true });
  const entry = { time: new Date().toISOString(), step, status, ...detail };
  appendFileSync(progressPath, `${JSON.stringify(entry)}\n`);
  console.log(`[windows-smoke] ${step}: ${status}`);
}

function killProcessTree(pid) {
  if (!pid) return;
  if (process.platform === "win32") {
    spawnSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
  } else {
    try {
      process.kill(-pid, "SIGTERM");
    } catch {
      try {
        process.kill(pid, "SIGTERM");
      } catch {}
    }
  }
}

function run(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd || repoRoot,
      env: { ...process.env, ...(options.env || {}) },
      stdio: options.inherit ? "inherit" : ["ignore", "pipe", "pipe"],
      windowsHide: true
    });
    const stdout = [];
    const stderr = [];
    if (!options.inherit) {
      child.stdout.on("data", (chunk) => stdout.push(Buffer.from(chunk)));
      child.stderr.on("data", (chunk) => stderr.push(Buffer.from(chunk)));
    }
    const timer = options.timeoutMs
      ? setTimeout(() => {
          killProcessTree(child.pid);
          setTimeout(() => finish(1, "timeout"), 5000).unref?.();
        }, options.timeoutMs)
      : null;
    timer?.unref?.();
    let settled = false;
    const finish = (code, signal) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      resolve({
        code,
        signal,
        stdout: Buffer.concat(stdout).toString("utf8").trim(),
        stderr: Buffer.concat(stderr).toString("utf8").trim()
      });
    };
    child.once("error", reject);
    if (options.resolveOnExit) {
      child.once("exit", (code, signal) => setTimeout(() => finish(code, signal), 50));
    } else {
      child.once("close", finish);
    }
  });
}

async function runChecked(command, args, options = {}) {
  const result = await run(command, args, options);
  assert.equal(
    result.code,
    0,
    `${command} ${args.join(" ")} failed: ${result.stderr || result.stdout || result.signal || result.code}`
  );
  return result;
}

async function runJson(command, args, options = {}) {
  const result = await runChecked(command, args, options);
  try {
    return JSON.parse(result.stdout || "{}");
  } catch (error) {
    throw new Error(`${command} ${args.join(" ")} did not return JSON: ${error.message}\n${result.stdout}`);
  }
}

async function fileExists(filePath) {
  try {
    const stat = await fs.stat(filePath);
    return stat.isFile();
  } catch {
    return false;
  }
}

async function fileSize(filePath) {
  try {
    const stat = await fs.stat(filePath);
    return stat.isFile() ? stat.size : 0;
  } catch {
    return 0;
  }
}

function assertNoPosixOnlyPackageScripts() {
  const pkg = JSON.parse(readFileSync(path.join(repoRoot, "package.json"), "utf8"));
  const critical = [
    "client:native:test",
    "client:native:test:coverage",
    ...Object.keys(pkg.scripts).filter((key) => key.startsWith("client:verify:"))
  ];
  for (const key of critical) {
    const script = String(pkg.scripts[key] || "");
    assert.equal(/^bash\b/.test(script), false, `${key} must not require bash`);
    assert.equal(/\bCARGO_TARGET_DIR=/.test(script), false, `${key} must not use POSIX env assignment`);
    assert.equal(/\bmkdir -p\b/.test(script), false, `${key} must not use POSIX mkdir -p`);
  }
}

async function assertExistingWindowsBundleIfPresent() {
  const runnableRoot = path.join(repoRoot, "build", "apps", "desktop", "runnable", "windows", "release");
  const bundleRoot = path.join(repoRoot, "build", "apps", "desktop", "bundles", "windows", "release", "bundle");
  if (!(await fileExists(path.join(runnableRoot, "licoup.exe")))) {
    console.log("windows bundle artifact check skipped; run npm run client:build -- --platform windows first");
    return;
  }
  for (const root of [runnableRoot, bundleRoot]) {
    assert.ok((await fileSize(path.join(root, "licoup.exe"))) > 0, `${root} missing non-empty licoup.exe`);
    assert.ok((await fileSize(path.join(root, "licoup.exe"))) > 0, `${root} missing non-empty licoup.exe`);
    assert.equal(
      await fileExists(path.join(root, "package-metadata", "windows", "client-manifest.json")),
      true,
      `${root} missing Windows client manifest`
    );
  }
}

async function main() {
  await fs.rm(progressPath, { force: true }).catch(() => {});
  await recordProgress("static-scripts", "start");
  assertNoPosixOnlyPackageScripts();
  await recordProgress("static-scripts", "ok");
  await recordProgress("windows-file-security", "start");
  await runChecked(process.execPath, ["tests/verify-windows-file-security-boundary.mjs"], { timeoutMs: 30000 });
  await recordProgress("windows-file-security", "ok");
  if (process.platform !== "win32") {
    console.log("windows native smoke static checks passed; full Windows smoke skipped on non-Windows");
    return;
  }

  await recordProgress("licoup-build", "start");
  await runChecked(process.execPath, ["tools/scripts/cargo-client.mjs", "build", "--manifest-path", "crates/licoup-native/Cargo.toml", "--features", "secure-mesh-acceptance-mock-kt", "--bin", "licoup-cli"], { timeoutMs: 240000 });
  await recordProgress("licoup-build", "ok");

  const licoClientExe = path.join(repoRoot, "build", "crates", "licoup-native", "target", "debug", "licoup-cli.exe");
  assert.equal(await fileExists(licoClientExe), true, `licoup.exe missing: ${licoClientExe}`);

  const portableDir = await fs.mkdtemp(path.join(os.tmpdir(), "lico-windows-native-smoke-"));
  await recordProgress("portable-dir", "created");
  const env = { LICOUP_HOME: portableDir };
  try {
    await recordProgress("targets-scan", "start");
    const targets = await runJson(licoClientExe, ["targets", "scan"], { env, timeoutMs: 30000 });
    assert.equal(targets.ok, true);
    for (const candidate of targets.candidates || []) {
      assert.equal(
        String(candidate.configPath || "").includes("Library/Application Support"),
        false,
        `${candidate.target} returned a macOS-only path on Windows`
      );
    }
    await recordProgress("targets-scan", "ok", { candidates: (targets.candidates || []).length });

    await recordProgress("windows-secret-store", "start");
    const secretStore = await runJson(licoClientExe, [
      "mobile", "relay", "e2ee", "secret-store-self-test",
    ], { env, timeoutMs: 120000 });
    assert.equal(secretStore.ok, true);
    assert.equal(secretStore.selfTestPassed, true);
    assert.equal(secretStore.backend, "memory-only-ephemeral");
    assert.equal(secretStore.sharedSecretClassRoundTripPassed, true);
    assert.equal(secretStore.sharedSecretClassPersistenceReady, false);
    assert.equal(secretStore.restartProof?.rePairRekeyRequired, true);
    assert.equal(secretStore.portableConfigPrivateMaterialRedacted, true);
    assert.equal(secretStore.rawPrivateMaterialIncluded, false);
    assert.equal(secretStore.rawPlaintextIncluded, false);
    assert.equal(secretStore.ordinaryFileSecretArtifactCount, 0);
    await recordProgress("windows-secret-store", "ok");
  } finally {
    await recordProgress("cleanup", "start");
    await fs.rm(portableDir, { recursive: true, force: true });
    await recordProgress("cleanup", "ok");
  }

  await recordProgress("windows-bundle-artifacts", "start");
  await assertExistingWindowsBundleIfPresent();
  await recordProgress("windows-bundle-artifacts", "ok");
  console.log("windows native smoke passed");
}

main().catch((error) => {
  console.error(sanitizeError(error));
  process.exitCode = 1;
});
