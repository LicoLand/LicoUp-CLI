import assert from "node:assert/strict";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { once } from "node:events";
import { spawn, spawnSync, execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  clearCargoProfiles,
  isCargoProfileEntryReclaimable,
} from "./cargo-lock-fence.mjs";
import {
  inspectRunningExecutables,
  parseMacOSProcessSnapshot,
  targetProcessUsage,
} from "./process-use.mjs";

const helperPath = fileURLToPath(new URL("./cargo-lock-fence.py", import.meta.url));
const pythonCheck = spawnSync("python3", ["-c", "import fcntl"], {
  stdio: "ignore",
});
const pythonAvailable = !pythonCheck.error && pythonCheck.status === 0;

function temporaryDirectory(t) {
  const directory = mkdtempSync(path.join(os.tmpdir(), "licoup-process-fence-"));
  const canonicalDirectory = realpathSync(directory);
  t.after(() => rmSync(canonicalDirectory, { force: true, recursive: true }));
  return canonicalDirectory;
}

function importHelper(code, args, input = "") {
  return execFileSync("python3", ["-c", code, helperPath, ...args], {
    encoding: "utf8",
    env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
    input,
  }).trim();
}

test("macOS process parsing preserves every text executable and rejects visibility warnings", () => {
  const repoRoot = "/fixture/repo";
  const targetRoot = path.join(repoRoot, "target");
  const executable = path.join(targetRoot, "debug", "client-fixture");
  const lsof = {
    status: 0,
    stderr: "",
    stdout: [
      "p421",
      "cclient-fixture",
      "ftxt",
      "n" + executable,
      "ftxt",
      "n/usr/lib/dyld",
      "fcwd",
      "n" + repoRoot,
    ].join("\n"),
  };
  const processList = {
    status: 0,
    stderr: "",
    stdout: "421 " + executable + " --hold\n",
  };
  const snapshot = parseMacOSProcessSnapshot(lsof, processList);

  assert.deepEqual(snapshot[0].paths, [executable, "/usr/lib/dyld"]);
  assert.deepEqual(targetProcessUsage(targetRoot, { available: true, entries: snapshot }, repoRoot).binaryPids, [421]);
  assert.equal(parseMacOSProcessSnapshot({ ...lsof, stderr: "lsof: visibility warning" }, processList), null);
  assert.equal(parseMacOSProcessSnapshot(lsof, { ...processList, stderr: "ps: visibility warning" }), null);
});

test("effect-time macOS process parsing retains all executable mappings and fails closed on stderr", {
  skip: !pythonAvailable,
}, (t) => {
  const repoRoot = temporaryDirectory(t);
  const targetRoot = path.join(repoRoot, "target");
  mkdirSync(targetRoot);
  const executable = path.join(targetRoot, "debug", "client-fixture");
  const request = {
    targetRoot,
    repoRoot,
    processStdout: "422 " + executable + " --hold\n",
    processStderr: "",
    processStatus: 0,
    openStdout: [
      "p422",
      "cclient-fixture",
      "ftxt",
      "n" + executable,
      "ftxt",
      "n/usr/lib/dyld",
      "fcwd",
      "n" + repoRoot,
    ].join("\n"),
    openStderr: "",
    openStatus: 0,
  };
  const code = [
    "import importlib.util,json,sys",
    "spec=importlib.util.spec_from_file_location('fence',sys.argv[1])",
    "module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)",
    "request=json.load(sys.stdin)",
    "print(module.parse_process_usage_macos(request['targetRoot'],request['repoRoot'],request['processStdout'],request['processStderr'],request['processStatus'],request['openStdout'],request['openStderr'],request['openStatus']))",
  ].join(";");

  assert.equal(importHelper(code, [], JSON.stringify(request)), "running-binary");
  request.openStderr = "lsof: visibility warning";
  assert.equal(importHelper(code, [], JSON.stringify(request)), "probe-unavailable");
});

test("a running executable copied into the target is protected by the actual process probe", {
  skip: process.platform === "win32" || !existsSync("/bin/cat"),
}, async (t) => {
  const repoRoot = temporaryDirectory(t);
  const targetRoot = path.join(repoRoot, "target");
  const profile = path.join(targetRoot, "debug");
  mkdirSync(profile, { recursive: true });
  const executable = path.join(profile, "cat-fixture");
  copyFileSync("/bin/cat", executable);
  chmodSync(executable, 0o755);

  const child = spawn(executable, [], { stdio: ["pipe", "ignore", "ignore"] });
  t.after(() => {
    if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
  });
  await once(child, "spawn");
  try {
    const usage = targetProcessUsage(targetRoot, inspectRunningExecutables(), repoRoot);
    assert.ok(usage.available, "process visibility must be available to establish safety");
    assert.ok(usage.binaryPids.includes(child.pid), "the live copied executable must be found");
  } finally {
    child.stdin.end();
    child.kill("SIGTERM");
    if (child.exitCode === null && child.signalCode === null) await once(child, "close");
  }
});

test("cargo lock effect rejects symlinked targets, lock replacement, and nested repositories", {
  skip: !pythonAvailable,
}, (t) => {
  const repoRoot = temporaryDirectory(t);
  const actualTarget = path.join(repoRoot, "actual-target");
  const aliasTarget = path.join(repoRoot, "target-alias");
  const profile = path.join(actualTarget, "debug");
  const lockPath = path.join(profile, ".cargo-lock");
  const cachePath = path.join(profile, "deps", "libfixture.rlib");
  mkdirSync(path.dirname(cachePath), { recursive: true });
  writeFileSync(lockPath, "");
  writeFileSync(cachePath, "cache");
  symlinkSync(actualTarget, aliasTarget, "dir");

  const profileDescriptor = [{ relativeProfile: "debug", lockNames: [".cargo-lock"] }];
  assert.equal(clearCargoProfiles({
    repoRoot,
    targetRoot: aliasTarget,
    profiles: profileDescriptor,
  }).status, "lock-unavailable");
  assert.equal(existsSync(cachePath), true);

  const raceCode = [
    "import importlib.util,os,sys",
    "spec=importlib.util.spec_from_file_location('fence',sys.argv[1])",
    "module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)",
    "lock,external=sys.argv[2:]",
    "real_open=os.open",
    "def replace_before_open(path,flags,*args,**kwargs):",
    " if path==lock:",
    "  os.unlink(lock);os.symlink(external,lock);os.open=real_open",
    " return real_open(path,flags,*args,**kwargs)",
    "os.open=replace_before_open",
    "handle=module.open_checked_lock(lock)",
    "print('unsafe' if handle else 'rejected')",
    "if handle: handle.close()",
  ].join("\n");
  const externalLock = path.join(repoRoot, "external-lock");
  writeFileSync(lockPath, "inside");
  writeFileSync(externalLock, "outside");
  assert.equal(importHelper(raceCode, [lockPath, externalLock]), "rejected");

  rmSync(lockPath);
  writeFileSync(lockPath, "inside");
  const heldLockCode = [
    "import fcntl,importlib.util,os,sys",
    "spec=importlib.util.spec_from_file_location('fence',sys.argv[1])",
    "module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)",
    "lock,external=sys.argv[2:];handle=module.open_checked_lock(lock)",
    "fcntl.flock(handle.fileno(),fcntl.LOCK_EX)",
    "os.unlink(lock);os.symlink(external,lock)",
    "print('rejected' if not module.lock_path_matches_handle(lock,handle) else 'unsafe')",
    "handle.close()",
  ].join("\n");
  assert.equal(importHelper(heldLockCode, [lockPath, externalLock]), "rejected");

  rmSync(lockPath);
  symlinkSync(externalLock, lockPath);
  assert.equal(clearCargoProfiles({
    repoRoot,
    targetRoot: actualTarget,
    profiles: profileDescriptor,
  }).status, "lock-unavailable");
  assert.equal(existsSync(cachePath), true);

  rmSync(lockPath);
  writeFileSync(lockPath, "");
  const nestedRepository = path.join(profile, "build", "nested", ".git");
  mkdirSync(nestedRepository, { recursive: true });
  const sentinel = path.join(profile, "build", "nested", "worktree-source");
  writeFileSync(sentinel, "preserve");
  const result = clearCargoProfiles({
    repoRoot,
    targetRoot: actualTarget,
    profiles: profileDescriptor,
  });
  assert.equal(result.status, "lock-unavailable");
  assert.equal(existsSync(sentinel), true);
  assert.equal(existsSync(cachePath), true);
});

test("profile path identity rejects a replaced intermediate directory after locking", {
  skip: !pythonAvailable,
}, (t) => {
  const repoRoot = temporaryDirectory(t);
  const targetRoot = path.join(repoRoot, "target");
  const profile = path.join(targetRoot, "debug");
  const lockPath = path.join(profile, ".cargo-lock");
  mkdirSync(profile, { recursive: true });
  writeFileSync(lockPath, "");

  const code = [
    "import fcntl,importlib.util,os,sys",
    "spec=importlib.util.spec_from_file_location('fence',sys.argv[1])",
    "module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)",
    "target,profile,lock=sys.argv[2:]",
    "profiles=[{'relativeProfile':'debug','lockNames':['.cargo-lock']}]",
    "expected=module.profile_directory_identities(target,profiles)",
    "handle=module.open_checked_lock(lock);fcntl.flock(handle.fileno(),fcntl.LOCK_EX)",
    "os.rename(profile,profile+'-saved');os.symlink(profile+'-saved',profile)",
    "print('rejected' if not module.same_profile_directories(target,profiles,expected) else 'unsafe')",
    "handle.close()",
  ].join("\n");

  assert.equal(importHelper(code, [targetRoot, profile, lockPath]), "rejected");
});

test("Cargo reclaimability policy agrees on directories, compiler files, and protected entries", () => {
  assert.equal(isCargoProfileEntryReclaimable({ name: "deps", type: "directory" }), true);
  assert.equal(isCargoProfileEntryReclaimable({ name: "notes", type: "directory" }), false);
  assert.equal(isCargoProfileEntryReclaimable({ name: "libfixture.rlib", type: "file" }), true);
  assert.equal(isCargoProfileEntryReclaimable({ name: ".cargo-lock", type: "file" }), false);
  assert.equal(isCargoProfileEntryReclaimable({ name: "README", type: "file" }), false);
  assert.equal(isCargoProfileEntryReclaimable({ name: "fixture", type: "file", executable: true }), true);
  assert.equal(isCargoProfileEntryReclaimable({ name: "notes.sh", type: "file", executable: true }), false);
  assert.equal(isCargoProfileEntryReclaimable({ name: "release.zip", type: "file", executable: true }), false);
  assert.equal(isCargoProfileEntryReclaimable({ name: "fixture", type: "symlink", executable: true }), false);
});
