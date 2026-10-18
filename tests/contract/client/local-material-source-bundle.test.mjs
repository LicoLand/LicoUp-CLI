import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath =
  "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material.rs";
const moduleRoot =
  "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/local_material";
const productionLeaves = Object.freeze([
  "accessors.rs",
  "composition.rs",
  "descriptor.rs",
  "identity_generation.rs",
  "material_mutation.rs",
  "prekey_generation.rs",
  "prekey_inventory.rs",
  "protocol_reset.rs",
  "rotation.rs",
  "state.rs",
  "state_codec.rs",
]);
const testLeaves = Object.freeze([
  "descriptor.rs",
  "generation.rs",
  "inventory.rs",
  "protocol_reset.rs",
  "rotation.rs",
  "state_codec.rs",
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

test("local material uses a thin facade and ordinary owned leaves", async () => {
  const facade = await read(facadePath);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((name) => name !== "tests")
      .sort(),
    [
      "accessors", "composition", "descriptor", "identity_generation",
      "material_mutation", "prekey_generation", "prekey_inventory",
      "protocol_reset", "rotation", "state", "state_codec",
    ],
  );
  for (const retiredImplementation of [
    "fn ensure_mobile_relay_endpoint_material",
    "struct LocalEndpointState",
    "fn local_endpoint_state",
    "fn reset_incompatible_local_pairwise_protocol",
  ]) {
    assert.equal(facade.includes(retiredImplementation), false);
  }
});

test("generation is separate from configuration mutation", async () => {
  const source = await sources();
  const generationLeaves = ["identity_generation.rs", "prekey_generation.rs"];
  const mutationLeaves = [
    "material_mutation.rs", "prekey_inventory.rs", "rotation.rs",
  ];
  for (const leaf of generationLeaves) {
    for (const mutationToken of ["&mut Value", ".insert(", ".remove(", "as_object_mut"]) {
      assert.equal(source[leaf].includes(mutationToken), false, `${leaf}: ${mutationToken}`);
    }
  }
  for (const leaf of mutationLeaves) {
    assert.ok(
      source[leaf].includes("&mut Value") || source[leaf].includes("&mut Map"),
      `${leaf} must own an explicit mutation boundary`,
    );
  }
  assert.ok(source["identity_generation.rs"].includes("generate_identity_material"));
  assert.ok(source["prekey_generation.rs"].includes("mlkem_prekey_material"));
  assert.ok(source["prekey_inventory.rs"].includes("prekeyPublicationVersion"));
});

test("protocol compatibility and state codec fail closed on exact protocol-bound state", async () => {
  const source = await sources();
  for (const token of [
    "MOBILE_RELAY_E2EE_PROTOCOL_VERSION",
    "ensure_local_pairwise_protocol_compatible",
    "config: &Value",
    "requires explicit startup migration",
  ]) {
    assert.ok(source["protocol_reset.rs"].includes(token), token);
  }
  for (const forbiddenMutation of ["&mut Value", ".insert(", ".remove(", "as_object_mut"]) {
    assert.equal(source["protocol_reset.rs"].includes(forbiddenMutation), false);
  }
  for (const requiredSecretField of [
    "PrivateKey",
    "SigningKey",
    "SignedPrekeyPrivateKey",
    "OneTimePrekeyPrivateKey",
    "OneTimeMlKem1024PrekeySeed",
  ]) {
    assert.ok(
      source["state_codec.rs"].includes(
        `MobileRelayE2eeSecretField::${requiredSecretField}`,
      ),
      requiredSecretField,
    );
  }
  for (const requiredStateField of [
    "mailboxRotationEpoch",
    "prekeyPublicationVersion",
    "sessionId",
  ]) {
    assert.ok(source["state_codec.rs"].includes(requiredStateField), requiredStateField);
  }
  for (const retiredPrivateStateField of [
    "privateKeyBase64url",
    "signingKeyBase64url",
    "signedPrekeyPrivateKeyBase64url",
    "oneTimePrekeyPrivateKeyBase64url",
    "oneTimeMlKem1024PrekeySeedBase64url",
  ]) {
    assert.equal(
      source["state_codec.rs"].includes(`\"${retiredPrivateStateField}\"`),
      false,
      retiredPrivateStateField,
    );
  }
  assert.ok(source["state_codec.rs"].includes("ok_or_else"));
});

test("descriptor and accessors remain the sole projection owners", async () => {
  const source = await sources();
  const ownership = new Map([
    ["accessors.rs", "fn pairwise_prekey_bundle"],
    ["descriptor.rs", "fn public_descriptor"],
    ["descriptor.rs", "fn local_endpoint_public_descriptor"],
    ["state_codec.rs", "fn local_endpoint_state"],
    ["rotation.rs", "fn rotate_mobile_relay_one_time_prekeys"],
  ]);
  for (const [owner, functionToken] of ownership) {
    assert.ok(source[owner].includes(functionToken));
    for (const [other, otherSource] of Object.entries(source)) {
      if (other !== owner) assert.equal(otherSource.includes(functionToken), false);
    }
  }
  assert.ok(source["descriptor.rs"].includes("key transparency response is missing"));
  for (const privateField of [
    "privateKeyBase64url", "signingKeyBase64url", "PrekeySeedBase64url",
  ]) {
    assert.equal(source["descriptor.rs"].includes(`\"${privateField}\":`), false);
  }
});

test("local material has no wildcard, hidden implementation, or egress dependency", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  for (const forbidden of [
    "use super::*", "use crate::*", "::*;", "#[path", "include!(", "mod tests {",
    "ureq::", "reqwest::", "TcpStream", "Command::new", "upload", "http://", "https://",
  ]) {
    assert.equal(joined.includes(forbidden), false, forbidden);
  }
});

test("local material regressions are split into selectable leaves", async () => {
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
