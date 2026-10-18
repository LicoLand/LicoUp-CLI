import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath =
  "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency.rs";
const moduleRoot =
  "crates/licoup-native/src/domain/mobile_relay/endpoint_trust/directory_transparency";
const productionLeaves = Object.freeze([
  "authority.rs",
  "authorization.rs",
  "authorization/exact.rs",
  "authorization/local.rs",
  "authorization/peer.rs",
  "claim.rs",
  "clock.rs",
  "config.rs",
  "ensure.rs",
  "freshness.rs",
  "verifier.rs",
]);
const testOnlyLeaves = Object.freeze(["test_support.rs"]);
const testLeaves = Object.freeze([
  "authority.rs",
  "claim.rs",
  "clock.rs",
  "config.rs",
  "exact_authorization.rs",
  "freshness.rs",
  "local_authorization.rs",
  "peer_authorization.rs",
  "support.rs",
  "test_support.rs",
  "verifier.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources(leaves = productionLeaves) {
  return Object.fromEntries(await Promise.all(leaves.map(async (leaf) => [
    leaf,
    await read(`${moduleRoot}/${leaf}`),
  ])));
}

test("directory transparency uses a thin facade and ordinary owned leaves", async () => {
  const facade = await read(facadePath);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((name) => name !== "tests")
      .sort(),
    [
      "authority", "authorization", "claim", "clock", "config", "ensure", "freshness",
      "test_support", "verifier",
    ],
  );
  for (const retiredImplementation of [
    "fn ensure_mobile_relay_key_transparency",
    "fn configured_kt_verifier",
    "fn authorize_peer_pairwise_directory",
    "fn authorize_exact_local_directory_response",
    "struct PairwiseDirectoryFreshness",
  ]) {
    assert.equal(facade.includes(retiredImplementation), false);
  }
});

test("claim config pin purpose clock and freshness have distinct owners", async () => {
  const source = await sources();
  const ownership = new Map([
    ["claim.rs", "fn build_local_directory_claim"],
    ["config.rs", "fn configured_kt_verifier"],
    ["config.rs", "fn configured_kt_pin"],
    ["config.rs", "fn parse_local_directory_authorization_purpose"],
    ["clock.rs", "fn current_secure_mesh_kt_gate_epoch_seconds"],
    ["freshness.rs", "fn require_current_pairwise_directory_authority"],
    ["authority.rs", "fn open_mobile_relay_directory_authority"],
  ]);
  for (const [owner, token] of ownership) {
    assert.ok(source[owner].includes(token));
    for (const [other, otherSource] of Object.entries(source)) {
      if (other !== owner) assert.equal(otherSource.includes(token), false);
    }
  }
  assert.ok(source["config.rs"].includes("ensure_no_kt_authority_reset_in_progress"));
  assert.ok(source["config.rs"].includes("canonical lowercase SHA-256 hex"));
  assert.ok(source["authority.rs"].includes("KtFreshnessPolicy::strict"));
  assert.equal(source["clock.rs"].includes("SecureMeshDirectoryAuthority"), false);
});

test("peer local and exact authorization remain separate fail-closed leaves", async () => {
  const source = await sources();
  const owners = new Map([
    ["authorization/peer.rs", "fn authorize_peer_pairwise_directory"],
    ["authorization/local.rs", "fn authorize_local_pairwise_directory"],
    ["authorization/exact.rs", "fn authorize_exact_local_directory_response"],
  ]);
  for (const [owner, token] of owners) {
    assert.ok(source[owner].includes(token));
    for (const [other, otherSource] of Object.entries(source)) {
      if (other !== owner) assert.equal(otherSource.includes(token), false);
    }
  }
  assert.ok(source["authorization/peer.rs"].includes(
    "peer key transparency response is missing"));
  assert.ok(source["authorization/local.rs"].includes(
    "key transparency response is missing"));
  assert.ok(source["authorization/exact.rs"].includes(
    "authorize_exact_directory_response"));
});

test("KT verifier centralizes response parsing gossip refresh and exact request binding", async () => {
  const source = await sources();
  for (const token of [
    "PreparedDirectoryResponse",
    "open_mobile_relay_directory_authority",
    "UntrustedDirectoryResponse",
    "observe_response_gossip_for_test",
    "DirectoryAuthorizationRequest::for_pairwise",
    "DirectoryAuthorizationRequest::for_exact_claim",
    "DirectoryAuthorizationRequest::for_mls",
  ]) {
    assert.ok(source["verifier.rs"].includes(token), token);
  }
  assert.ok(source["ensure.rs"].includes("PairwiseSignedPrekey"));
  assert.ok(source["ensure.rs"].includes("PairwiseOneTimePrekey"));
  assert.ok(source["ensure.rs"].includes("productionAuthority"));
  assert.equal(source["ensure.rs"].includes("SecureMeshDirectoryAuthority::open"), false);
});

test("test authority stays test-only and product leaves have no state-store or egress", async () => {
  const source = await sources();
  const testSource = await sources(testOnlyLeaves);
  const joined = Object.values(source).join("\n");
  assert.ok(testSource["test_support.rs"].includes("ClientStateStore::portable"));
  assert.ok(testSource["test_support.rs"].includes("local-acceptance-mock"));
  for (const forbidden of [
    "ClientStateStore", "SecureMeshKtLog", "local-acceptance-mock", "ureq::", "reqwest::",
    "TcpStream", "Command::new", "upload", "http://", "https://",
  ]) {
    assert.equal(joined.includes(forbidden), false, forbidden);
  }
  for (const hiddenImplementation of ["use super::*", "use crate::*", "::*;", "#[path", "include!(", "mod tests {"]) {
    assert.equal(`${joined}\n${Object.values(testSource).join("\n")}`.includes(hiddenImplementation), false,
      hiddenImplementation);
  }
});

test("directory transparency regressions are split into selectable leaves", async () => {
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
