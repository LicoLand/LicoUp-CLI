import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { EventEmitter } from "node:events";
import { execFileSync, spawn } from "node:child_process";
import {
  cargoBuildCommand,
  isManagedCargoTarget,
  resolveCargoTargetDirectory,
} from "./test-artifact-lifecycle/cargo-target.mjs";
import {
  completeTestArtifactWorkspace,
  acquireTestArtifactLease,
  inventoryTestArtifacts,
  pruneTestArtifactTarget,
  pruneReclaimableTestArtifacts,
  testArtifactId,
  testArtifactStatus,
} from "./test-artifact-lifecycle.mjs";
import { targetProcessUsage } from "./test-artifact-lifecycle/process-use.mjs";
import "./test-artifact-lifecycle/process-use-cargo-fence.test.mjs";
import { cargoLockFenceSupported } from "./test-artifact-lifecycle/cargo-lock-fence.mjs";
import { cargoClientTargetPath, runCargoClient } from "../cargo-client.mjs";
import { parseArtifactCommand, runArtifactCommand } from "../client-test-artifacts.mjs";

const projectRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const cargoPruneSupported = cargoLockFenceSupported();

function temporaryRepository(t) {
  const repoRoot = realpathSync(mkdtempSync(path.join(os.tmpdir(), "licoup-artifact-test-")));
  mkdirSync(path.join(repoRoot, "build"), { recursive: true });
  execFileSync("git", ["init", "--quiet", repoRoot]);
  t.after(() => rmSync(repoRoot, { force: true, recursive: true }));
  return repoRoot;
}

function configureCargoTarget(repoRoot, targetDirectory = "build/crates/licoup-native/target") {
  const cargoDirectory = path.join(repoRoot, ".cargo");
  mkdirSync(cargoDirectory, { recursive: true });
  writeFileSync(
    path.join(cargoDirectory, "config.toml"),
    `[build]\ntarget-dir = "${targetDirectory}"\n`,
  );
}

function materialize(repoRoot, relativePath, contents = "compiled") {
  const target = path.join(repoRoot, ...relativePath.split("/"));
  mkdirSync(path.dirname(target), { recursive: true });
  writeFileSync(target, contents, { mode: 0o755 });
  const cargoOutput = /^(target|build\/crates\/[^/]+\/(?:target|[a-z0-9._-]+-target))\/([^/]+)(?:\/|$)/u
    .exec(relativePath);
  if (cargoOutput) {
    const lockDirectory = path.join(repoRoot, cargoOutput[1], cargoOutput[2]);
    mkdirSync(lockDirectory, { recursive: true });
    writeFileSync(path.join(lockDirectory, ".cargo-lock"), "");
  }
  return target;
}

test("active compiler output is protected and released output is reclaimable", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  const targetPath = "build/crates/licoup-native/target";
  const compiledFile = materialize(repoRoot, `${targetPath}/debug/client`);
  const dependencyCache = materialize(repoRoot, "build/dependency-downloads/crate.cache");
  const lease = acquireTestArtifactLease({
    repoRoot,
    scope: "native-test",
    targetPath,
  });

  assert.deepEqual(testArtifactStatus({ repoRoot }), {
    active: 1,
    cleaned: 0,
    invalid: 0,
    reclaimable: 0,
    unmanaged: 0,
  });
  assert.deepEqual(pruneReclaimableTestArtifacts({ repoRoot }), {
    active: 1,
    eligible: 0,
    failed: 0,
    removed: 0,
    skipped: 0,
    unmanaged: 0,
  });
  assert.equal(existsSync(compiledFile), true);

  assert.deepEqual(lease.release(), { state: "reclaimable" });
  assert.equal(testArtifactStatus({ repoRoot }).reclaimable, 1);
  const dryRun = pruneReclaimableTestArtifacts({ repoRoot, dryRun: true });
  assert.equal(dryRun.eligible, 1);
  assert.equal(dryRun.removed, 0);
  assert.equal(existsSync(compiledFile), true);

  const pruned = pruneReclaimableTestArtifacts({ repoRoot });
  assert.equal(pruned.removed, 1);
  assert.equal(existsSync(compiledFile), false);
  assert.equal(existsSync(dependencyCache), true);
  assert.equal(testArtifactStatus({ repoRoot }).cleaned, 1);
});

test("compiler cleanup preserves packaged and installed clients", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  const installedRoot = mkdtempSync(path.join(os.tmpdir(), "licoup-installed-test-"));
  t.after(() => rmSync(installedRoot, { force: true, recursive: true }));
  const targetPath = "build/crates/licoup-native/target";
  const compiledFile = materialize(repoRoot, `${targetPath}/release/licoup`);
  const runnableFile = materialize(
    repoRoot,
    "build/apps/desktop/runnable/macos/release/LicoUp.app/Contents/MacOS/licoup",
  );
  const installedFile = materialize(
    installedRoot,
    "LicoUp.app/Contents/MacOS/licoup",
  );
  const lease = acquireTestArtifactLease({
    repoRoot,
    scope: "client-build",
    targetPath,
  });
  lease.release();

  const result = pruneReclaimableTestArtifacts({ repoRoot });

  assert.equal(result.removed, 1);
  assert.equal(existsSync(compiledFile), false);
  const cargoLock = path.join(repoRoot, targetPath, "release", ".cargo-lock");
  assert.equal(existsSync(cargoLock), true);
  assert.equal(existsSync(path.dirname(cargoLock)), true);
  assert.equal(existsSync(path.join(repoRoot, targetPath)), true);
  assert.equal(existsSync(runnableFile), true);
  assert.equal(existsSync(installedFile), true);
});

test("all concurrent leases must finish before output becomes reclaimable", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  const targetPath = "build/crates/licoup-native/target";
  materialize(repoRoot, `${targetPath}/debug/client`);
  const first = acquireTestArtifactLease({ repoRoot, scope: "first", targetPath });
  const second = acquireTestArtifactLease({ repoRoot, scope: "second", targetPath });

  assert.deepEqual(first.release(), { state: "active" });
  assert.equal(testArtifactStatus({ repoRoot }).active, 1);
  assert.equal(pruneReclaimableTestArtifacts({ repoRoot }).removed, 0);
  assert.deepEqual(second.release(), { state: "reclaimable" });
  assert.equal(testArtifactStatus({ repoRoot }).reclaimable, 1);
});

test("dead leases require the configured grace period before recovery", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  const targetPath = "build/crates/licoup-native/target";
  const compiledFile = materialize(repoRoot, `${targetPath}/debug/client`);
  const clock = { value: Date.parse("2026-01-01T00:00:00.000Z") };
  acquireTestArtifactLease({
    deadLeaseGraceMs: 1_000,
    isAlive: () => false,
    now: () => clock.value,
    repoRoot,
    scope: "crashed",
    targetPath,
  });

  clock.value += 999;
  assert.equal(testArtifactStatus({
    deadLeaseGraceMs: 1_000,
    isAlive: () => false,
    now: () => clock.value,
    repoRoot,
  }).active, 1);
  clock.value += 1;
  const status = testArtifactStatus({
    deadLeaseGraceMs: 1_000,
    isAlive: () => false,
    now: () => clock.value,
    repoRoot,
  });
  assert.equal(status.active, 0);
  assert.equal(status.invalid, 0);
  assert.equal(status.reclaimable, 1);
  const pruned = pruneReclaimableTestArtifacts({
    deadLeaseGraceMs: 1_000,
    isAlive: () => false,
    now: () => clock.value,
    repoRoot,
  });
  assert.equal(pruned.removed, 1);
  assert.equal(existsSync(compiledFile), false);
});

test("concurrent processes reclaim a stale lock once and serialize output ownership", async (t) => {
  const repoRoot = temporaryRepository(t);
  const targetPath = "build/crates/licoup-native/target";
  const artifactId = testArtifactId(targetPath);
  const record = path.join(repoRoot, "build", ".test-artifacts", artifactId);
  const lock = path.join(record, ".lock");
  mkdirSync(lock, { recursive: true });
  writeFileSync(path.join(lock, "owner.json"), JSON.stringify({
    acquiredAt: "2000-01-01T00:00:00.000Z",
    pid: 2_000_000_000,
    token: "stale",
  }));

  const source = String.raw`
    import { existsSync, readFileSync, writeFileSync } from "node:fs";
    import path from "node:path";
    import { pathToFileURL } from "node:url";
    const root = process.argv[1];
    const id = process.argv[2];
    const sourceRoot = process.argv[3];
    const { artifactRecordPaths, withArtifactLock } = await import(
      pathToFileURL(path.join(sourceRoot, "tools/scripts/lib/test-artifact-lifecycle/registry.mjs"))
    );
    const state = path.join(root, "build", "critical-section.json");
    withArtifactLock(artifactRecordPaths(root, id), () => {
      const active = existsSync(state) ? JSON.parse(readFileSync(state, "utf8")).active : 0;
      if (active !== 0) process.exitCode = 4;
      writeFileSync(state, JSON.stringify({ active: active + 1 }));
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 30);
      writeFileSync(state, JSON.stringify({ active: 0 }));
    });
  `;

  const children = [0, 1].map(() => spawn(process.execPath, [
    "--input-type=module",
    "-e",
    source,
    repoRoot,
    artifactId,
    projectRoot,
  ], { stdio: "ignore" }));
  const exitCodes = await Promise.all(children.map((child) => new Promise((resolve) => {
    child.once("close", resolve);
  })));

  assert.deepEqual(exitCodes, [0, 0]);
  assert.deepEqual(JSON.parse(readFileSync(path.join(repoRoot, "build", "critical-section.json"), "utf8")), {
    active: 0,
  });
  assert.equal(existsSync(lock), false);
});

test("tampered descriptors and unmanaged targets fail closed", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  const targetPath = "build/crates/licoup-native/target";
  const protectedFile = materialize(repoRoot, `${targetPath}/debug/client`);
  const unmanagedFile = materialize(repoRoot, "build/agents/old-target/debug/client");
  const lease = acquireTestArtifactLease({ repoRoot, scope: "native", targetPath });
  lease.release();
  const descriptorPath = path.join(
    repoRoot,
    "build",
    ".test-artifacts",
    testArtifactId(targetPath),
    "descriptor.json",
  );
  const descriptor = JSON.parse(readFileSync(descriptorPath, "utf8"));
  descriptor.relativeTarget = "build/other-target";
  writeFileSync(descriptorPath, `${JSON.stringify(descriptor)}\n`);

  const result = pruneReclaimableTestArtifacts({ repoRoot });
  assert.equal(result.failed, 1);
  assert.equal(result.unmanaged, 1);
  assert.equal(result.removed, 0);
  assert.equal(existsSync(protectedFile), true);
  assert.equal(existsSync(unmanagedFile), true);
});

test("outside, download-cache, registry, and symlink targets are rejected", (t) => {
  const repoRoot = temporaryRepository(t);
  assert.throws(() => acquireTestArtifactLease({
    repoRoot,
    scope: "outside",
    targetPath: "../outside",
  }), /inside/u);
  assert.throws(() => acquireTestArtifactLease({
    repoRoot,
    scope: "download-cache",
    targetPath: "apps/desktop/build/registry",
  }), /download caches/u);
  assert.throws(() => acquireTestArtifactLease({
    repoRoot,
    scope: "registry",
    targetPath: "build/.test-artifacts/forged",
  }), /managed build output/u);

  if (process.platform !== "win32") {
    const outside = path.join(repoRoot, "outside");
    mkdirSync(outside);
    const nativeBuild = path.join(repoRoot, "build", "crates", "licoup-native");
    mkdirSync(nativeBuild, { recursive: true });
    symlinkSync(outside, path.join(nativeBuild, "linked-target"));
    assert.throws(() => acquireTestArtifactLease({
      repoRoot,
      scope: "symlink",
      targetPath: "build/crates/licoup-native/linked-target",
    }), /symbolic links/u);
  }
});

test("repository build-producing Cargo scripts use the lifecycle wrapper", () => {
  const packageJson = JSON.parse(readFileSync(path.join(projectRoot, "package.json"), "utf8"));
  const cargoConfig = readFileSync(path.join(projectRoot, ".cargo", "config.toml"), "utf8");
  const androidBuild = readFileSync(
    path.join(projectRoot, "apps", "desktop", "android", "app", "build.gradle.kts"),
    "utf8",
  );
  const macosProject = readFileSync(
    path.join(projectRoot, "apps", "desktop", "macos", "Runner.xcodeproj", "project.pbxproj"),
    "utf8",
  );
  const packageNativeBuild = readFileSync(
    path.join(projectRoot, "apps", "desktop", "scripts", "package-client", "build", "native.mjs"),
    "utf8",
  );
  assert.equal(
    Object.entries(packageJson.scripts).filter(([, command]) =>
      /(?:^|&&|\|)\s*cargo\s+(?:build|clippy|run|test)\b/u.test(command)).length,
    0,
  );
  assert.equal(
    packageJson.scripts["client:artifacts:status"],
    "node tools/scripts/client-test-artifacts.mjs status",
  );
  assert.equal(
    packageJson.scripts["client:artifacts:prune"],
    "node tools/scripts/client-test-artifacts.mjs prune",
  );
  assert.equal(
    packageJson.scripts["client:artifacts:inventory"],
    "node tools/scripts/client-test-artifacts.mjs inventory",
  );
  assert.equal(
    packageJson.scripts["client:artifacts:workspace-complete"],
    "node tools/scripts/client-test-artifacts.mjs workspace-complete",
  );
  assert.match(cargoConfig, /target-dir = "build\/crates\/licoup-native\/target"/u);
  assert.match(
    androidBuild,
    /repoRoot\.resolve\("build\/crates\/licoup-native\/target"\)/u,
  );
  assert.match(androidBuild, /tools\/scripts\/cargo-client\.mjs/u);
  assert.doesNotMatch(androidBuild, /commandLine\(\s*"cargo"/u);
  assert.doesNotMatch(androidBuild, /android-target/u);
  assert.match(macosProject, /tools\/scripts\/cargo-client\.mjs/u);
  assert.doesNotMatch(macosProject, /CARGO_TARGET_DIR=.*cargo build/u);
  assert.match(packageNativeBuild, /cargo-client\.mjs/u);
  assert.doesNotMatch(packageNativeBuild, /runPackageProcess\("cargo"/u);
});

test("Cargo target ownership follows target-dir flag, environment, and config", (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const env = { CARGO_HOME: path.join(repoRoot, "cargo-home") };
  assert.equal(cargoBuildCommand(["build", "--target-dir", "build/crates/licoup-native/flag-target"]), "build");
  assert.equal(cargoBuildCommand(["fetch", "--locked"]), null);
  assert.deepEqual(
    resolveCargoTargetDirectory({
      args: ["build"],
      cwd: repoRoot,
      env,
      workspaceRoot: repoRoot,
    }).relativePath,
    "build/crates/licoup-native/target",
  );
  assert.equal(
    cargoClientTargetPath({
      repoRoot,
      args: ["build", "--target-dir", "build/crates/licoup-native/flag-target"],
      cwd: repoRoot,
      env,
    }),
    "build/crates/licoup-native/flag-target",
  );
  assert.equal(
    cargoClientTargetPath({
      repoRoot,
      args: ["build"],
      cwd: repoRoot,
      env: { ...env, CARGO_TARGET_DIR: "build/crates/licoup-native/env-target" },
    }),
    "build/crates/licoup-native/env-target",
  );
  assert.equal(
    cargoClientTargetPath({
      repoRoot,
      args: ["build", "--target-dir", path.join(os.tmpdir(), "external-target")],
      cwd: repoRoot,
      env,
    }),
    null,
  );
  assert.equal(isManagedCargoTarget(repoRoot, "build/out/target"), false);
  const dedicatedTarget = "build/crates/licoup-native/env-target";
  const configuredTarget = "build/crates/licoup-native/config-target";
  assert.equal(cargoClientTargetPath({
    repoRoot,
    args: ["build", "--config", `build.target-dir="${configuredTarget}"`],
    env: { ...env, CARGO_TARGET_DIR: dedicatedTarget, CARGO_BUILD_TARGET_DIR: configuredTarget },
  }), dedicatedTarget);
  assert.equal(cargoClientTargetPath({
    repoRoot,
    args: ["build", "--config", `build.target-dir="${configuredTarget}"`],
    env: { ...env, CARGO_BUILD_TARGET_DIR: dedicatedTarget },
  }), configuredTarget);
});

test("cargo-client preserves the effective target environment and tracks its child", async (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const env = {
    CARGO_HOME: path.join(repoRoot, "cargo-home"),
    CARGO_TARGET_DIR: "build/crates/licoup-native/override-target",
  };
  const previousExitCode = process.exitCode;
  const calls = [];
  const child = new EventEmitter();
  child.pid = 4242;
  const release = { count: 0 };
  runCargoClient(["build"], {
    repoRoot,
    env,
    spawnCargo(program, args, options) {
      calls.push({ program, args, options });
      process.nextTick(() => child.emit("exit", 0, null));
      return child;
    },
    leaseFactory(options) {
      assert.equal(options.targetPath, "build/crates/licoup-native/override-target");
      return {
        trackProcess(pid) { calls.push({ trackedPid: pid }); },
        release() { release.count += 1; },
      };
    },
  });
  await new Promise((resolve) => setImmediate(resolve));
  t.after(() => { process.exitCode = previousExitCode; });
  assert.equal(calls[0].program, "cargo");
  assert.equal(calls[0].options.env.CARGO_TARGET_DIR, env.CARGO_TARGET_DIR);
  assert.equal(calls[0].options.env.LICO_CLIENT_PRODUCT_VERSION,
    JSON.parse(readFileSync(new URL("../../client-version.json", import.meta.url), "utf8")).productVersion);
  assert.equal(calls.find((call) => call.trackedPid)?.trackedPid, 4242);
  assert.equal(release.count, 1);
});

test("cargo-client preserves an explicit product identity for every build", async (t) => {
  const repoRoot = temporaryRepository(t);
  const previousExitCode = process.exitCode;
  t.after(() => { process.exitCode = previousExitCode; });
  const child = new EventEmitter();
  runCargoClient(["build"], {
    repoRoot,
    env: {
      CARGO_HOME: path.join(repoRoot, "cargo-home"),
      LICO_CLIENT_PRODUCT_VERSION: "0.4.0-synthetic",
    },
    spawnCargo(_program, _args, options) {
      assert.equal(options.env.LICO_CLIENT_PRODUCT_VERSION, "0.4.0-synthetic");
      process.nextTick(() => child.emit("exit", 0, null));
      return child;
    },
  });
  await new Promise((resolve) => setImmediate(resolve));
});

test("inventory finds unregistered compiler output without scanning deliverables", (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const compiledFile = materialize(
    repoRoot,
    "build/crates/licoup-native/target/debug/client",
    "compiler output",
  );
  materialize(repoRoot, "build/out/package.tar", "deliverable");
  const inventory = inventoryTestArtifacts({
    repoRoot,
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
  });
  assert.deepEqual(inventory.entries.map((entry) => entry.relativeTarget), [
    "build/crates/licoup-native/target",
  ]);
  assert.equal(inventory.entries[0].bytes, Buffer.byteLength("compiler output"));
  assert.equal(inventory.entries[0].state, cargoPruneSupported ? "reclaimable" : "protected");
  assert.equal(typeof inventory.entries[0].lastUsedAt, "string");
  if (cargoPruneSupported) assert.deepEqual(inventory.entries[0].protectedReasons, []);
  else assert.equal(inventory.entries[0].protectedReasons.includes("cargo-lock-unavailable"), true);
  assert.equal(existsSync(compiledFile), true);
});

test("targeted dry-run is read-only and targeted prune only removes the selected cache", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const selected = materialize(repoRoot, "build/crates/licoup-native/target/debug/client");
  const deliverable = materialize(repoRoot, "build/out/package.tar", "package");
  const localNote = path.join(
    repoRoot,
    "build",
    "crates",
    "licoup-native",
    "target",
    "debug",
    "notes.txt",
  );
  writeFileSync(localNote, "keep this local file");
  const options = {
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
    targetPath: "build/crates/licoup-native/target",
  };
  const dryRun = pruneTestArtifactTarget({ ...options, dryRun: true });
  const inventory = inventoryTestArtifacts(options).entries[0];
  assert.equal(inventory.reclaimableBytes, Buffer.byteLength("compiled"));
  assert.equal(inventory.bytes - inventory.reclaimableBytes, Buffer.byteLength("keep this local file"));
  assert.equal(dryRun.eligible, 1);
  assert.equal(dryRun.removed, false);
  assert.equal(existsSync(selected), true);
  assert.equal(existsSync(path.join(repoRoot, "build", ".test-artifacts")), false);

  const pruned = pruneTestArtifactTarget(options);
  assert.equal(pruned.removed, true);
  assert.equal(pruned.removedCount, 1);
  assert.equal(pruned.bytesAfter, Buffer.byteLength("keep this local file"));
  assert.equal(existsSync(selected), false);
  assert.equal(existsSync(localNote), true);
  assert.equal(existsSync(deliverable), true);
});

test("Cargo profile locks remain in place and block cleanup while a build owns them", {
  skip: !cargoPruneSupported,
}, async (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const compiled = materialize(repoRoot, "build/crates/licoup-native/target/debug/deps/client");
  const lockPath = path.join(repoRoot, "build", "crates", "licoup-native", "target", "debug", ".cargo-lock");
  const lockIdentity = statSync(lockPath).ino;
  const locked = spawn("python3", ["-c", [
    "import fcntl,sys",
    "handle=open(sys.argv[1],'r+b')",
    "fcntl.flock(handle.fileno(),fcntl.LOCK_EX)",
    "print('ready',flush=True)",
    "sys.stdin.readline()",
  ].join(";"), lockPath], { stdio: ["pipe", "pipe", "ignore"] });
  let output = "";
  await new Promise((resolve, reject) => {
    locked.stdout.setEncoding("utf8");
    locked.stdout.on("data", (chunk) => {
      output += chunk;
      if (output.includes("ready\n")) resolve();
    });
    locked.once("error", reject);
    locked.once("close", (code) => {
      if (!output.includes("ready\n")) reject(new Error(`lock fixture exited with ${code}`));
    });
  });

  const result = pruneTestArtifactTarget({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
    targetPath: "build/crates/licoup-native/target",
  });
  assert.equal(result.removed, false);
  assert.equal(result.active, 1);
  assert.deepEqual(result.protectedReasons, ["active-build"]);
  assert.equal(existsSync(compiled), true);

  locked.stdin.end();
  await new Promise((resolve) => locked.once("close", resolve));
  const pruned = pruneTestArtifactTarget({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
    targetPath: "build/crates/licoup-native/target",
  });
  assert.equal(pruned.removed, true);
  assert.equal(existsSync(compiled), false);
  assert.equal(statSync(lockPath).ino, lockIdentity);
  assert.equal(existsSync(path.dirname(lockPath)), true);
});

test("tracked source inside a configured output root is protected", (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const source = materialize(
    repoRoot,
    "build/crates/licoup-native/target/debug/kept-source.rs",
    "source",
  );
  execFileSync("git", ["-C", repoRoot, "add", "--", "build/crates/licoup-native/target/debug/kept-source.rs"]);
  const result = pruneTestArtifactTarget({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
    targetPath: "build/crates/licoup-native/target",
  });
  assert.equal(result.removed, false);
  assert.equal(result.protectedReasons.includes("tracked-source"), true);
  assert.equal(existsSync(source), true);
});

test("unrecognized Cargo profile layouts remain protected", (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const target = path.join(repoRoot, "build", "crates", "licoup-native", "target");
  const compiled = path.join(target, "debug", "deps", "client");
  mkdirSync(path.dirname(compiled), { recursive: true });
  writeFileSync(compiled, "compiled", { mode: 0o755 });
  const result = pruneTestArtifactTarget({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
    targetPath: "build/crates/licoup-native/target",
  });
  assert.equal(result.removed, false);
  assert.deepEqual(result.protectedReasons, ["cargo-lock-unavailable"]);
  assert.equal(existsSync(compiled), true);
});

test("unsupported Cargo lock hosts fail closed", { skip: cargoPruneSupported }, (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const compiled = materialize(repoRoot, "build/crates/licoup-native/target/debug/client");
  const inventory = inventoryTestArtifacts({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
  });
  assert.equal(inventory.entries[0].state, "protected");
  assert.equal(inventory.entries[0].protectedReasons.includes("cargo-lock-unavailable"), true);
  assert.equal(existsSync(compiled), true);
});

test("deliverable-capable Flutter build roots remain protected", (t) => {
  const repoRoot = temporaryRepository(t);
  const deliverable = materialize(repoRoot, "apps/desktop/build/release/client.tar", "package");
  const result = pruneTestArtifactTarget({
    processSnapshot: { available: true, entries: [] },
    repoRoot,
    targetPath: "apps/desktop/build",
  });
  assert.equal(result.removed, false);
  assert.deepEqual(result.protectedReasons, ["non-cargo-output-root"]);
  assert.equal(existsSync(deliverable), true);
});

test("workspace completion selects one effective target and protects active builds and binaries", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const compiled = materialize(repoRoot, "build/crates/licoup-native/target/debug/client");
  const deliverable = materialize(repoRoot, "build/out/target/package.bin", "deliverable");
  const activeBuild = completeTestArtifactWorkspace({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: {
      available: true,
      entries: [{
        pid: 4242,
        path: "/usr/bin/cargo",
        cwd: repoRoot,
        command: "cargo",
      }],
    },
    repoRoot,
  });
  assert.equal(activeBuild.removed, false);
  assert.equal(activeBuild.active, 1);
  assert.equal(activeBuild.protectedReasons.includes("active-build"), true);
  assert.equal(existsSync(compiled), true);
  assert.equal(existsSync(deliverable), true);

  const activeBinary = pruneTestArtifactTarget({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: {
      available: true,
      entries: [{ pid: 4243, path: compiled, command: "client" }],
    },
    repoRoot,
    targetPath: "build/crates/licoup-native/target",
  });
  assert.equal(activeBinary.removed, false);
  assert.deepEqual(activeBinary.protectedReasons, ["running-binary"]);
  assert.equal(existsSync(compiled), true);
});

test("workspace completion covers each known Cargo output root in the checkout", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const defaultOutput = materialize(repoRoot, "build/crates/licoup-native/target/debug/client");
  const androidOutput = materialize(
    repoRoot,
    "build/crates/licoup-native/android-target/release/android-client",
  );
  const deliverable = materialize(repoRoot, "build/out/release/client.tar", "package");

  const result = completeTestArtifactWorkspace({
    env: { CARGO_HOME: path.join(repoRoot, "cargo-home") },
    processSnapshot: { available: true, entries: [] },
    repoRoot,
  });

  assert.equal(result.removed, true);
  assert.equal(result.removedCount, 2);
  assert.deepEqual(result.relativeTargets, [
    "build/crates/licoup-native/android-target",
    "build/crates/licoup-native/target",
  ]);
  assert.equal(existsSync(defaultOutput), false);
  assert.equal(existsSync(androidOutput), false);
  assert.equal(existsSync(deliverable), true);
});

test("process probes protect Cargo output when the compiler runs outside the checkout", (t) => {
  const repoRoot = temporaryRepository(t);
  const targetPath = path.join(repoRoot, "build", "crates", "licoup-native", "target");
  const cargo = targetProcessUsage(targetPath, {
    available: true,
    entries: [{
      pid: 4244,
      path: "/usr/bin/cargo",
      cwd: os.tmpdir(),
      command: "cargo",
      arguments: ["build", "--target-dir", targetPath],
    }],
  }, repoRoot);
  assert.deepEqual(cargo.buildPids, [4244]);

  const rustc = targetProcessUsage(targetPath, {
    available: true,
    entries: [{
      pid: 4245,
      path: "/usr/bin/rustc",
      cwd: os.tmpdir(),
      command: "rustc",
      arguments: `--out-dir=${path.join(targetPath, "debug", "deps")}`,
    }],
  }, repoRoot);
  assert.deepEqual(rustc.buildPids, [4245]);
});

test("artifact CLI parses checkout-scoped inventory and maintenance commands", () => {
  assert.deepEqual(parseArtifactCommand(["prune", "--dry-run"]), {
    command: "prune",
    dryRun: true,
    repoRoot: null,
    targetPath: null,
  });
  assert.equal(parseArtifactCommand(["prune", "--repo-root", "/checkout"]), null);
  assert.deepEqual(parseArtifactCommand(["inventory", "--repo-root", "/checkout"]), {
    command: "inventory",
    dryRun: false,
    repoRoot: "/checkout",
    targetPath: null,
  });
  assert.deepEqual(parseArtifactCommand([
    "prune",
    "--target",
    "build/crates/licoup-native/target",
    "--dry-run",
  ]), {
    command: "prune",
    dryRun: true,
    repoRoot: null,
    targetPath: "build/crates/licoup-native/target",
  });
  assert.equal(parseArtifactCommand(["workspace-complete", "--target", "target"]), null);
});

test("artifact CLI dispatches one checkout and reports only relative targets", {
  skip: !cargoPruneSupported,
}, (t) => {
  const repoRoot = temporaryRepository(t);
  configureCargoTarget(repoRoot);
  const compiled = materialize(repoRoot, "build/crates/licoup-native/target/debug/client");
  const env = { CARGO_HOME: path.join(repoRoot, "cargo-home") };
  const inventory = runArtifactCommand(parseArtifactCommand([
    "inventory",
    "--repo-root",
    repoRoot,
  ]), { currentRepoRoot: repoRoot, env });
  assert.equal(inventory.ok, true);
  assert.equal(inventory.operation, "inventory");
  assert.deepEqual(inventory.entries.map((entry) => entry.relativeTarget), [
    "build/crates/licoup-native/target",
  ]);
  assert.equal(JSON.stringify(inventory).includes(repoRoot), false);

  const preview = runArtifactCommand(parseArtifactCommand(["prune", "--dry-run"]), {
    currentRepoRoot: repoRoot,
    env,
  });
  assert.equal(preview.ok, true);
  assert.equal(preview.operation, "prune-dry-run");
  assert.equal(existsSync(compiled), true);

  const splitBuildInventory = inventoryTestArtifacts({
    repoRoot,
    env: { ...env, CARGO_BUILD_BUILD_DIR: "build/intermediates" },
    processSnapshot: { available: true, entries: [] },
  });
  assert.ok(splitBuildInventory.entries[0].protectedReasons.includes("cargo-lock-unavailable"));

  const completed = runArtifactCommand(parseArtifactCommand([
    "workspace-complete",
    "--repo-root",
    repoRoot,
  ]), { currentRepoRoot: repoRoot, env });
  assert.equal(completed.ok, true);
  assert.equal(completed.removed, true);
  assert.deepEqual(completed.relativeTargets, ["build/crates/licoup-native/target"]);
  assert.equal(existsSync(compiled), false);
  assert.equal(JSON.stringify(completed).includes(repoRoot), false);
});
