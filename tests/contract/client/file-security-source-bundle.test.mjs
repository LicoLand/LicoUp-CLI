import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-foundation/src/platform/file_security.rs";
const root = "crates/licoup-foundation/src/platform/file_security";
const productionLeaves = Object.freeze([
  "append_lock.rs",
  "atomic_replace.rs",
  "hardening.rs",
  "marker.rs",
  "policy.rs",
  "sync.rs",
  "unix_hardening.rs",
  "validation.rs",
  "windows_acl.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sourceFiles(relativeRoot) {
  const found = [];
  async function visit(relativeDirectory) {
    for (const entry of await fs.readdir(path.join(repoRoot, relativeDirectory), {
      withFileTypes: true,
    })) {
      const relativePath = path.posix.join(relativeDirectory, entry.name);
      if (entry.isDirectory()) await visit(relativePath);
      else if (entry.isFile() && relativePath.endsWith(".rs")) found.push(relativePath);
    }
  }
  await visit(relativeRoot);
  return found.sort();
}

test("file security root is an exact thin facade over dedicated production leaves", async () => {
  const facade = await read(facadePath);
  for (const leaf of productionLeaves) {
    assert.match(facade, new RegExp(`mod ${leaf.replace(".rs", "")};`, "u"));
    await fs.access(path.join(repoRoot, root, leaf));
  }
  const entries = await fs.readdir(path.join(repoRoot, root), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    [...productionLeaves].sort(),
  );
  for (const forbidden of ["fn ", "OpenOptions", "fs::", "include!(", "#[path"])
    assert.equal(facade.includes(forbidden), false, forbidden);
});

test("append lock and atomic replacement preserve bounded no-follow durable semantics", async () => {
  const append = await read(`${root}/append_lock.rs`);
  const atomic = await read(`${root}/atomic_replace.rs`);
  for (const token of [
    "PRIVATE_APPEND_LINE_MAX_BYTES", "PRIVATE_APPEND_FILE_MAX_BYTES", "openat(",
    "O_NOFOLLOW", "FlockArg::LockExclusive", "fstat(", "fsync(",
  ]) assert.match(append, new RegExp(token.replace(/[()]/gu, "\\$&"), "u"));
  for (const token of [
    "create_new(true)", "validate_regular_file_or_missing_no_follow",
    "ErrorKind::CrossesDevices", "copy_cross_device_then_atomic_replace",
    "validate_private_path_ancestors", "sync::file", "sync::parent",
  ]) assert.equal(atomic.includes(token), true, token);
  assert.equal(atomic.includes("fs::copy("), false);
});

test("marker validation sync and hardening keep fail-closed security ownership", async () => {
  const marker = await read(`${root}/marker.rs`);
  const validation = await read(`${root}/validation.rs`);
  const sync = await read(`${root}/sync.rs`);
  const hardening = await read(`${root}/hardening.rs`);
  const unix = await read(`${root}/unix_hardening.rs`);
  const windows = await read(`${root}/windows_acl.rs`);
  assert.match(marker, /PRIVATE_STATE_FILE_MAX_BYTES/u);
  assert.match(marker, /\.take\(\(max_bytes as u64\)\.saturating_add\(1\)\)/u);
  assert.match(marker, /validate_open_state_marker/u);
  assert.match(validation, /symlink_metadata/u);
  assert.match(validation, /Component::ParentDir/u);
  assert.match(validation, /ensure_same_file/u);
  assert.match(validation, /O_NOFOLLOW/u);
  assert.match(sync, /sync_all\(\)/u);
  assert.match(hardening, /private tree contains a symbolic link/u);
  assert.match(hardening, /validate_private_path_ancestors/u);
  for (const token of ["O_NOFOLLOW", "fchmod", "Uid::effective", "0o700", "0o600"])
    assert.equal(unix.includes(token), true, token);
  assert.match(windows, /stdout\(Stdio::null\(\)\)/u);
  assert.match(windows, /stderr\(Stdio::null\(\)\)/u);
  assert.equal(windows.includes(".output()"), false);
});

test("platform policy leaves stay independent and production emits no sensitive path detail", async () => {
  const sources = Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${root}/${leaf}`),
  ])));
  assert.equal(sources["unix_hardening.rs"].includes("windows_acl"), false);
  assert.equal(sources["windows_acl.rs"].includes("unix_hardening"), false);
  for (const leaf of ["policy.rs", "sync.rs", "unix_hardening.rs", "windows_acl.rs"])
    assert.equal(sources[leaf].includes("super::"), false, leaf);
  for (const forbidden of [
    "super::atomic_replace", "super::append_lock", "super::hardening", "super::marker",
  ])
    assert.equal(sources["validation.rs"].includes(forbidden), false, forbidden);
  const joined = Object.values(sources).join("\n");
  for (const forbidden of ["unsafe {", "include!(", "#[path", ".display()", "output.stderr"])
    assert.equal(joined.includes(forbidden), false, forbidden);
});

test("all consumers use only the stable file security facade", async () => {
  const internalModules = "append_lock|atomic_replace|hardening|marker|policy|sync|unix_hardening|validation|windows_acl";
  const internalPath = new RegExp(`file_security::(?:${internalModules})`, "u");
  const consumers = await sourceFiles("crates/licoup-native/src");
  for (const relativePath of consumers) {
    const source = await read(relativePath);
    assert.equal(internalPath.test(source), false, relativePath);
  }
});

test("every security responsibility owns a dedicated narrow regression", async () => {
  const entries = (await fs.readdir(path.join(repoRoot, root, "tests"))).sort();
  assert.deepEqual(entries, [
    "append_lock.rs", "atomic_private_file.rs", "atomic_replace.rs", "composition.rs", "hardening.rs", "marker.rs",
    "mod.rs", "policy.rs", "support.rs", "sync.rs", "unix_hardening.rs", "validation.rs",
    "windows_acl.rs",
  ]);
});
