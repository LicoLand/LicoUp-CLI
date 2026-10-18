import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/domain/mobile_relay/pairwise_session.rs";
const moduleRoot = "crates/licoup-native/src/domain/mobile_relay/pairwise_session";
const productionLeaves = Object.freeze([
  "crypto_operation.rs",
  "handshake.rs",
  "payload.rs",
  "response.rs",
  "status_projection.rs",
  "store.rs",
  "transaction.rs",
]);
const testLeaves = Object.freeze([
  "crypto_operation.rs",
  "handshake.rs",
  "payload.rs",
  "response.rs",
  "status_projection.rs",
  "store.rs",
  "transaction.rs",
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

test("pairwise session uses a thin facade and ordinary owned leaves", async () => {
  const facade = await read(facadePath);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((name) => name !== "tests")
      .sort(),
    [
      "crypto_operation", "handshake", "payload", "response", "status_projection",
      "store", "transaction",
    ],
  );
  for (const retiredImplementation of [
    "fn authorized_pairwise_session_status",
    "fn secure_result_response_summary",
    "fn seal_mobile_relay_payload_with_pairwise_operation",
    "fn initialize_mobile_relay_pairwise_session",
    "fn mobile_relay_pairwise_store_path",
  ]) {
    assert.equal(facade.includes(retiredImplementation), false);
  }
});

test("status payload response crypto handshake and store have distinct owners", async () => {
  const source = await sources();
  const ownership = new Map([
    ["status_projection.rs", "fn authorized_pairwise_session_status"],
    ["payload.rs", "fn secure_command_payload"],
    ["response.rs", "fn secure_result_response_summary"],
    ["response.rs", "fn result_envelope_replay_proof_with_pairwise_operation"],
    ["crypto_operation.rs", "fn seal_mobile_relay_payload_with_pairwise_operation"],
    ["crypto_operation.rs", "fn open_mobile_relay_payload_with_pairwise_operation"],
    ["handshake.rs", "fn initialize_mobile_relay_pairwise_session"],
    ["store.rs", "fn mobile_relay_pairwise_store_path"],
  ]);
  for (const [owner, token] of ownership) {
    assert.ok(source[owner].includes(token));
    for (const [other, otherSource] of Object.entries(source)) {
      if (other !== owner) assert.equal(otherSource.includes(token), false);
    }
  }
});

test("response projection and replay proof remain body-redacted", async () => {
  const source = await sources();
  for (const token of [
    '"bodyRedacted": true',
    '"replayErrorRedacted": true',
    "resultEnvelopePresent",
    "is_pairwise_replay_rejection_error",
  ]) {
    assert.ok(source["response.rs"].includes(token), token);
  }
  assert.equal(source["response.rs"].includes('response.get("body")'), false);
  assert.equal(source["response.rs"].includes("seal_payload_envelope"), false);
});

test("seal open and transaction commit preserve one atomic durable boundary", async () => {
  const source = await sources();
  assert.ok(source["crypto_operation.rs"].includes("seal_payload_envelope"));
  assert.ok(source["crypto_operation.rs"].includes("open_payload_envelope"));
  assert.ok(source["crypto_operation.rs"].includes("PairwiseDirectoryGate::Required"));
  assert.ok(source["crypto_operation.rs"].includes("PairwiseDirectoryGate::KtGossipControl"));
  assert.equal(source["crypto_operation.rs"].match(/pairwise_operation\.commit\(\)\?/gu)?.length, 2);
  assert.ok(source["transaction.rs"].includes("struct MobileRelayPairwiseOperation"));
  assert.ok(source["transaction.rs"].includes("fn commit(&mut self)"));
  assert.ok(source["transaction.rs"].includes(
    "self.record = self.store.commit_session_with_authorized_session("));
  assert.equal(source["crypto_operation.rs"].includes(
    "commit_session_with_authorized_session("), false);
});

test("handshake bootstrap keeps all PQXDH transitions and prekey consumption together", async () => {
  const source = await sources();
  for (const token of [
    "complete_responder_handshake",
    "complete_initiator_handshake",
    "SecureMeshPairwiseSession::accept",
    "SecureMeshPairwiseSession::initiate",
    "upsert_initial_with_local_prekey_claim_and_capability_proofs",
    "upsert_initial_with_remote_prekey_claim",
    "rotate_mobile_relay_one_time_prekeys",
    "CapabilityProofReplayGuard",
  ]) {
    assert.ok(source["handshake.rs"].includes(token), token);
  }
});

test("durable store alone owns path creation restart purge and secret-store selection", async () => {
  const source = await sources();
  for (const token of [
    "ClientStateStore::portable",
    "pairwise-pqxdh.sqlite3",
    "purge_unrecoverable_memory_only_sessions",
    "purge_sessions_preserving_prekey_history",
    "pairwise_secret_store_override",
    "selected_mobile_relay_secret_store",
  ]) {
    assert.ok(source["store.rs"].includes(token), token);
  }
  for (const [leaf, leafSource] of Object.entries(source)) {
    if (leaf !== "store.rs") assert.equal(leafSource.includes("ClientStateStore"), false);
  }
});

test("pairwise session has no wildcard hidden implementation or egress", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  for (const forbidden of [
    "use super::*", "use crate::*", "::*;", "#[path", "include!(", "mod tests {",
    "ureq::", "reqwest::", "TcpStream", "Command::new", "upload", "http://", "https://",
  ]) {
    assert.equal(joined.includes(forbidden), false, forbidden);
  }
});

test("pairwise session regressions are split into selectable leaves", async () => {
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
