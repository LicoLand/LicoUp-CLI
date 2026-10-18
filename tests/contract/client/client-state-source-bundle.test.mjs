import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// `licoup-client-state` owns the store and its journal owners; `licoup-native`
// keeps only the facade and the command surface that depends on the generated
// wire contract.
const facadePath = "crates/licoup-native/src/platform/client_state.rs";
const facadeRoot = "crates/licoup-native/src/platform/client_state";
const root = "crates/licoup-client-state/src";
const productionLeaves = Object.freeze([
  "accessors.rs",
  "activity.rs",
  "collections.rs",
  "migration.rs",
  "paths.rs",
  "policy.rs",
  "redaction.rs",
  "resource_policy.rs",
  "serialization.rs",
  "snapshots.rs",
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

test("client state root is an exact thin stable facade", async () => {
  const facade = await read(facadePath);
  assert.match(facade, /mod operations;/u);
  const entries = await fs.readdir(path.join(repoRoot, facadeRoot), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    ["operations.rs"],
  );
  for (const forbidden of ["struct ", "impl ", "fn ", "fs::", "include!(", "#[path"])
    assert.equal(facade.includes(forbidden), false, forbidden);
});

test("the persistence crate owns every leaf and nothing else", async () => {
  const entries = await fs.readdir(path.join(repoRoot, root), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    ["lib.rs", ...productionLeaves].sort(),
  );
  for (const leaf of productionLeaves) await fs.access(path.join(repoRoot, root, leaf));
});

test("collections activity and snapshots are independent single-path owners", async () => {
  const owners = Object.fromEntries(await Promise.all([
    "collections.rs", "activity.rs", "snapshots.rs",
  ].map(async (leaf) => [leaf, await read(`${root}/${leaf}`)])));
  assert.match(owners["collections.rs"], /struct ClientStateStore \{\s*root: PathBuf/u);
  assert.match(owners["activity.rs"], /struct ActivityLog \{\s*path: PathBuf/u);
  assert.match(owners["snapshots.rs"], /struct SnapshotStore \{\s*root: PathBuf/u);
  for (const [leaf, source] of Object.entries(owners)) {
    for (const foreign of ["ClientStateStore", "ActivityLog", "SnapshotStore"])
      if (!source.includes(`struct ${foreign}`)) assert.equal(source.includes(foreign), false, `${leaf}:${foreign}`);
  }
  const accessors = await read(`${root}/accessors.rs`);
  assert.match(accessors, /impl ClientStateStore/u);
  assert.match(accessors, /ActivityLog::from_state_root/u);
  assert.match(accessors, /SnapshotStore::from_state_root/u);
});

test("activity JSONL is bounded latest-first in memory and privacy projected", async () => {
  const activity = await read(`${root}/activity.rs`);
  const policy = await read(`${root}/policy.rs`);
  for (const token of [
    "MAX_ACTIVITY_FILE_BYTES", "MAX_ACTIVITY_EVENT_BYTES", "MAX_ACTIVITY_EVENTS",
    "MAX_ACTIVITY_TYPE_BYTES",
  ]) {
    assert.match(policy, new RegExp(token, "u"));
    assert.match(activity, new RegExp(token, "u"));
  }
  assert.match(activity, /VecDeque/u);
  assert.match(activity, /pop_front\(\)/u);
  assert.match(activity, /open_private_text_bounded/u);
  assert.match(activity, /validate_private_file_unchanged/u);
  assert.match(activity, /read_line\(/u);
  assert.equal(activity.includes("read_private_text_bounded"), false);
  assert.match(activity, /redact_activity_payload/u);
  assert.match(activity, /internal_state_reference/u);
  assert.equal(activity.includes("BufReader"), false);
  assert.equal(activity.includes("display_path"), false);
});

test("snapshot capture restore and listing remain bounded redacted and traversal safe", async () => {
  const snapshots = await read(`${root}/snapshots.rs`);
  const paths = await read(`${root}/paths.rs`);
  for (const token of [
    "MAX_SNAPSHOT_SOURCE_BYTES", "MAX_SNAPSHOT_RECORD_BYTES", "MAX_SNAPSHOT_FILES",
    "redact_snapshot", "validate_restore_destination", "redacted_local_path",
  ]) assert.equal(snapshots.includes(token), true, token);
  assert.match(paths, /snapshot_id\.starts_with\("snapshot-"\)/u);
  assert.match(paths, /MAX_SNAPSHOT_ID_BYTES/u);
  assert.match(paths, /validate_private_path_ancestors/u);
  assert.match(paths, /validate_path_owner/u);
  assert.match(paths, /O_NOFOLLOW/u);
  assert.match(paths, /ensure_same_file/u);
  assert.equal(snapshots.includes("display_path"), false);
  assert.equal(snapshots.includes('"sourcePath": paths::redacted_local_path()'), true);
});

test("redaction caches compiled patterns and fails closed on depth and evidence bounds", async () => {
  const redaction = await read(`${root}/redaction.rs`);
  const policy = await read(`${root}/policy.rs`);
  assert.match(redaction, /OnceLock<Regex>/u);
  assert.match(redaction, /MAX_REDACTION_DEPTH/u);
  assert.match(redaction, /MAX_REDACTION_PATHS/u);
  assert.match(redaction, /REDACTED_PRIVATE_KEY/u);
  assert.match(redaction, /is_local_path_key/u);
  assert.match(policy, /REDACTED_LOCAL_PATH/u);
  assert.equal((redaction.match(/Regex::new\(/gu) ?? []).length, 3);
  assert.equal(redaction.includes("Regex::new(pattern)"), false);
});

test("serialization and path helpers own all bounded filesystem details", async () => {
  const serialization = await read(`${root}/serialization.rs`);
  const paths = await read(`${root}/paths.rs`);
  assert.match(serialization, /read_private_text_bounded/u);
  assert.match(serialization, /atomic_write_private_text_bounded/u);
  assert.match(serialization, /content\.len\(\) <= max_bytes/u);
  assert.match(paths, /Read::by_ref/u);
  assert.match(paths, /saturating_add\(1\)/u);
  assert.match(paths, /symlink_metadata/u);
  for (const forbidden of ["ureq::", "reqwest::", "TcpStream", "UdpSocket", "unsafe {"])
    assert.equal(`${serialization}\n${paths}`.includes(forbidden), false, forbidden);
});

test("all external consumers use only the restricted client state facade", async () => {
  const internalModules = "accessors|activity|collections|migration|operations|paths|policy|redaction|resource_policy|serialization|snapshots";
  const internalPath = new RegExp(`(?:client_state|licoup_client_state)::(?:${internalModules})::`, "u");
  const consumers = (await sourceFiles("crates/licoup-native/src"))
    .filter((relativePath) => relativePath !== facadePath && !relativePath.startsWith(`${facadeRoot}/`));
  for (const relativePath of consumers) {
    const source = await read(relativePath);
    assert.equal(internalPath.test(source), false, relativePath);
  }
  const production = (await Promise.all(productionLeaves.map((leaf) => read(`${root}/${leaf}`))))
    .join("\n");
  for (const forbidden of [
    "ureq::", "reqwest::", "TcpStream", "UdpSocket", "unsafe {",
  ]) assert.equal(production.includes(forbidden), false, forbidden);
});

test("every client state responsibility owns a dedicated narrow regression", async () => {
  const owners = (await fs.readdir(path.join(repoRoot, root, "tests"))).sort();
  assert.deepEqual(owners, [
    "accessors.rs", "activity.rs", "collections.rs", "mod.rs", "paths.rs",
    "policy.rs", "redaction.rs", "resource_policy.rs", "serialization.rs",
    "snapshots.rs", "support.rs",
  ]);
  const facade = (await fs.readdir(path.join(repoRoot, facadeRoot, "tests"))).sort();
  assert.deepEqual(facade, ["composition.rs", "mod.rs", "operations.rs", "support.rs"]);
});
