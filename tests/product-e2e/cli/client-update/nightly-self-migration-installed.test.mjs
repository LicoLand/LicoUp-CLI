import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../../..", import.meta.url)));
const installRoot = process.env.LICO_CLIENT_INSTALL_DIR
  ? path.resolve(process.env.LICO_CLIENT_INSTALL_DIR)
  : "/Applications";
const installedApp = path.join(installRoot, "LicoUp.app");
const installedCli = path.join(installedApp, "Contents", "Helpers", "LicoUpCustody.app", "Contents", "MacOS", "licoup-cli");
const installedMain = path.join(installedApp, "Contents", "MacOS", "licoup");
const infoPlist = path.join(installedApp, "Contents", "Info.plist");
const productVersion = JSON.parse(
  readFileSync(path.join(repoRoot, "tools", "client-version.json"), "utf8"),
).productVersion;

function runJson(argumentsList) {
  const result = spawnSync(installedCli, argumentsList, {
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
    timeout: 30_000,
  });
  assert.equal(result.status, 0, "installed client command failed");
  assert.equal(result.error, undefined, "installed client command could not run");
  assert.ok(result.stdout.trim(), "installed client command returned no JSON");
  return JSON.parse(result.stdout);
}

test("installed Nightly owns one identity and admits state incrementally", (context) => {
  assert.ok(existsSync(installedMain), "installed application executable is missing");
  assert.ok(existsSync(installedCli), "installed native sidecar is missing");
  assert.ok(existsSync(infoPlist), "installed application identity is missing");

  const bundleId = spawnSync(
    "/usr/bin/plutil",
    ["-extract", "CFBundleIdentifier", "raw", "-o", "-", infoPlist],
    { encoding: "utf8", timeout: 10_000 },
  );
  assert.equal(bundleId.status, 0, "installed bundle identity could not be read");
  assert.equal(bundleId.stdout.trim(), "land.lico.licoup");

  const root = mkdtempSync(path.join(tmpdir(), "licoup-installed-migration-"));
  context.after(() => rmSync(root, { recursive: true, force: true }));
  const stateRoot = path.join(root, "update-state");
  const dataRoot = path.join(root, "portable-data");

  const status = runJson([
    "update",
    "status",
    "--source",
    "local",
    "--state-root",
    stateRoot,
  ]);
  assert.equal(status.runningVersion, productVersion);
  assert.equal(status.runningReleaseTrack, "nightly");
  assert.equal(status.targetReleaseTrack, "nightly");

  const first = runJson(["state", "admit", dataRoot]);
  assert.equal(first.status, "ready");
  assert.equal(first.runningProductVersion, productVersion);
  assert.equal(first.runningReleaseTrack, "nightly");
  assert.ok(first.appliedDomainIds.length > 0);

  const second = runJson(["state", "admit", dataRoot]);
  assert.equal(second.status, "ready");
  assert.deepEqual(second.appliedDomainIds, []);
  assert.ok(second.skippedDomainIds.length > 0);
  assert.equal(second.frontierId, first.frontierId);
});
