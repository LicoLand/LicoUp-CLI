import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/domain/mobile_relay/key_transparency.rs";
const moduleRoot = "crates/licoup-native/src/domain/mobile_relay/key_transparency";
const productionLeaves = Object.freeze([
  "authority.rs",
  "authority/challenge.rs",
  "authority/proposal.rs",
  "authority/reset.rs",
  "authority/transaction.rs",
  "config.rs",
  "contract.rs",
  "dispatcher.rs",
  "gossip.rs",
  "persistence.rs",
  "projection.rs",
  "provision.rs",
  "publication.rs",
  "revocation.rs",
  "self_monitor.rs",
  "status.rs",
]);
const testLeaves = Object.freeze([
  "authority.rs",
  "dispatcher.rs",
  "monitor_gossip.rs",
  "provision.rs",
  "publication.rs",
  "revocation.rs",
  "status.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${moduleRoot}/${leaf}`),
  ])));
}

test("key transparency uses a thin facade and ordinary owned leaves", async () => {
  const facade = await read(facadePath);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((name) => name !== "tests")
      .sort(),
    [
      "authority", "config", "contract", "dispatcher", "gossip", "persistence",
      "projection", "provision", "publication", "revocation", "self_monitor", "status",
    ],
  );
  for (const token of [
    "fn key_transparency_provision",
    "fn key_transparency_self_monitor",
    "fn key_transparency_status",
    "ClientStateStore",
  ]) {
    assert.equal(facade.includes(token), false);
  }
});

test("authority challenge transaction and reset remain fail closed", async () => {
  const source = await sources();
  for (const moduleName of ["challenge", "proposal", "reset", "transaction"]) {
    assert.ok(source["authority.rs"].includes(`mod ${moduleName};`));
  }
  for (const token of [
    "proposalDigest",
    "configGeneration",
    "authorityGeneration",
    "expiresAtEpochSeconds",
    "generation is stale",
  ]) {
    assert.ok(source["authority/challenge.rs"].includes(token));
  }
  for (const token of [
    "RESET_KEY_TRANSPARENCY_AUTHORITY",
    "after_guard_persisted",
    "after_mls_selected_custody_reset",
    "after_kt_authority_state_reset",
    "clear_mobile_relay_pairing_state",
  ]) {
    assert.ok(source["authority/reset.rs"].includes(token));
  }
  assert.ok(source["authority/transaction.rs"].includes("requires_security_reset =="));
  assert.ok(source["authority/transaction.rs"].includes("complete_kt_authority_challenge"));
});

test("persistence secret context and projection dependencies are one way", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  assert.deepEqual(
    productionLeaves.filter((leaf) => source[leaf].includes("secret_custody")),
    ["config.rs"],
  );
  assert.deepEqual(
    productionLeaves.filter((leaf) =>
      source[leaf].includes("ClientStateStore") || source[leaf].includes("file_security")),
    ["persistence.rs"],
  );
  for (const forbidden of ["secret_custody", "ClientStateStore", "file_security", "save_config"]){
    assert.equal(source["projection.rs"].includes(forbidden), false);
  }
  assert.equal(source["projection.rs"].includes('"privateKeyMaterial"'), false);
  assert.equal(joined.includes("use super::*"), false);
  assert.equal(joined.includes("use crate::*"), false);
  for (const retired of ["#[path", "include!(", "mod tests {"]) {
    assert.equal(joined.includes(retired), false);
  }
});

test("publication revocation provision monitoring gossip and status have distinct owners", async () => {
  const source = await sources();
  const ownership = new Map([
    ["publication.rs", "key_transparency_publication_request"],
    ["revocation.rs", "key_transparency_revocation_request"],
    ["provision.rs", "key_transparency_provision"],
    ["self_monitor.rs", "key_transparency_self_monitor"],
    ["gossip.rs", "key_transparency_gossip"],
    ["status.rs", "key_transparency_status"],
  ]);
  for (const [owner, functionName] of ownership) {
    assert.ok(source[owner].includes(`fn ${functionName}`));
    for (const [other, otherSource] of Object.entries(source)) {
      if (other !== owner) assert.equal(otherSource.includes(`fn ${functionName}`), false);
    }
  }
  assert.ok(source["provision.rs"].includes("does not match the exact pending local claim"));
  assert.ok(source["status.rs"].includes("unwrap_or(true)"));
  assert.ok(source["status.rs"].includes('"guardValid"'));
  for (const forbiddenEgress of ["ureq::", "reqwest::", "TcpStream", "Command::new"]){
    assert.equal(Object.values(source).some((value) => value.includes(forbiddenEgress)), false);
  }
});

test("key transparency regressions are split into selectable leaves", async () => {
  const entries = await fs.readdir(path.join(repoRoot, moduleRoot, "tests"), {
    withFileTypes: true,
  });
  assert.deepEqual(
    entries
      .filter((entry) => entry.isFile() && entry.name.endsWith(".rs"))
      .map((entry) => entry.name)
      .sort(),
    [...testLeaves].sort(),
  );
  const testFacade = await read(`${moduleRoot}/tests.rs`);
  assert.equal(testFacade.includes("mod tests {"), false);
  assert.equal(testFacade.includes("#[path"), false);
});
