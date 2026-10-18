import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/core/secure_mesh_prekey.rs";
const root = "crates/licoup-native/src/core/secure_mesh_prekey";
const productionLeaves = Object.freeze([
  "inventory.rs",
  "key_package.rs",
  "pairwise.rs",
  "validation.rs",
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

test("secure mesh prekey root is an exact restricted stable facade", async () => {
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
  for (const forbidden of [
    "struct ", "enum ", "impl ", "fn ", "SigningKey", "OffsetDateTime", "SecureMeshKtLog",
  ]) assert.equal(facade.includes(forbidden), false, forbidden);
});

test("pairwise leaf exclusively owns PQXDH records authorization and signed digests", async () => {
  const pairwise = await read(`${root}/pairwise.rs`);
  for (const token of [
    "SecureMeshPairwisePreKeyBundle", "SecureMeshPreKeyRecord", "PREKEY_MAGIC",
    "validate_pairwise_prekey_bundle", "signed_prekey_bundle_digest",
    "one_time_prekey_batch_digest", "ML_KEM_1024_PUBLIC_KEY_BYTES",
    "DirectoryAuthorizationPurpose::PairwiseSessionBootstrap",
  ]) assert.equal(pairwise.includes(token), true, token);
  for (const foreign of [
    "SecureMeshKeyPackageRecord", "SecureMeshInventoryStatus", "SecureMeshKtLog", "OsRng",
  ]) assert.equal(pairwise.includes(foreign), false, foreign);
});

test("KeyPackage leaf exclusively owns MLS record suite shape and transcript", async () => {
  const keyPackage = await read(`${root}/key_package.rs`);
  for (const token of [
    "SecureMeshKeyPackageRecord", "KEYPACKAGE_MAGIC", "SECURE_MESH_MLS_CIPHER_SUITE",
    "SECURE_MESH_KEYPACKAGE_WIRE_CIPHER_SUITE", "key_package_signature_payload",
  ]) assert.equal(keyPackage.includes(token), true, token);
  for (const foreign of [
    "SecureMeshPairwisePreKeyBundle", "SecureMeshInventoryStatus",
    "AuthorizedDirectoryLeaf", "ML_KEM_1024_PUBLIC_KEY_BYTES",
  ]) assert.equal(keyPackage.includes(foreign), false, foreign);
});

test("inventory is a pure local low-water projection with no transfer authority", async () => {
  const inventory = await read(`${root}/inventory.rs`);
  assert.match(inventory, /one_time_prekey_replenishment_required/u);
  assert.match(inventory, /key_package_replenishment_required/u);
  assert.match(inventory, /available_one_time_prekeys\s*<=\s*one_time_prekey_low_watermark/u);
  assert.match(inventory, /available_key_packages <= key_package_low_watermark/u);
  for (const forbidden of [
    "reqwest", "TcpStream", "SigningKey", "DeviceTrustPublicIdentity",
  ]) assert.equal(inventory.toLowerCase().includes(forbidden.toLowerCase()), false, forbidden);
});

test("shared validation owns bounded trust time signature and framing primitives", async () => {
  const validation = await read(`${root}/validation.rs`);
  for (const token of [
    "MAX_SIGNATURE_B64_LEN", "MAX_PREKEY_CLOCK_SKEW_SECONDS", "ensure_active_trust_state",
    "ensure_not_expired", "ensure_signature_shape", "verify_signature", "sign_payload",
    "append_len_prefixed_bytes", "hex_sha256", "String::with_capacity",
  ]) assert.equal(validation.includes(token), true, token);
  for (const foreign of [
    "SecureMeshPreKeyRecord", "SecureMeshKeyPackageRecord", "AuthorizedDirectoryLeaf",
    "SECURE_MESH_MLS_CIPHER_SUITE", "ML_KEM_1024_PUBLIC_KEY_BYTES",
  ]) assert.equal(validation.includes(foreign), false, foreign);
});

test("stable signature vectors and exact low-water boundaries have dedicated tests", async () => {
  const pairwise = await read(`${root}/tests/pairwise.rs`);
  const keyPackage = await read(`${root}/tests/key_package.rs`);
  const inventory = await read(`${root}/tests/inventory.rs`);
  assert.match(pairwise, /pairwise_signature_payload_and_signature_match_the_stable_vector/u);
  assert.match(pairwise, /ba74fb2e1dc52166cad3b43010b84ef762ba3f4ae2684430b3fc10698da4f021/u);
  assert.match(keyPackage, /key_package_signature_payload_and_signature_match_the_stable_vector/u);
  assert.match(keyPackage, /8b4f63cb8c740677136f73e55a20c5efad984d9b7066824ab60e193384bb916d/u);
  assert.match(inventory, /low_water_equality_requests_local_replenishment/u);
});

test("directory authority fixtures remain test-only", async () => {
  const production = (await Promise.all(
    productionLeaves.map((leaf) => read(`${root}/${leaf}`)),
  )).join("\n");
  const support = await read(`${root}/tests/support.rs`);
  for (const token of ["SecureMeshKtLog", "UntrustedDirectoryResponse", "OsRng"])
    assert.equal(production.includes(token), false, token);
  assert.match(support, /authorize_test_pairwise_prekey_bundle/u);
  assert.match(support, /SecureMeshKtLog/u);
});

test("external consumers cannot depend on prekey implementation leaves", async () => {
  const internalModules = "inventory|key_package|pairwise|validation";
  const internalPath = new RegExp(`secure_mesh_prekey::(?:${internalModules})::`, "u");
  const consumers = (await sourceFiles("crates/licoup-native/src"))
    .filter((relativePath) => relativePath !== facadePath && !relativePath.startsWith(`${root}/`));
  for (const relativePath of consumers) {
    const source = await read(relativePath);
    assert.equal(internalPath.test(source), false, relativePath);
  }
});

test("prekey production leaves contain no egress or unsafe runtime authority", async () => {
  const production = (await Promise.all(
    productionLeaves.map((leaf) => read(`${root}/${leaf}`)),
  )).join("\n");
  for (const forbidden of [
    "ureq::", "reqwest::", "TcpStream", "UdpSocket", "unsafe {", "Command::new",
  ]) assert.equal(production.includes(forbidden), false, forbidden);
});

test("every prekey responsibility owns a dedicated narrow regression", async () => {
  const entries = (await fs.readdir(path.join(repoRoot, root, "tests"))).sort();
  assert.deepEqual(entries, [
    "composition.rs", "inventory.rs", "key_package.rs", "mod.rs", "pairwise.rs", "support.rs",
    "validation.rs",
  ]);
});
