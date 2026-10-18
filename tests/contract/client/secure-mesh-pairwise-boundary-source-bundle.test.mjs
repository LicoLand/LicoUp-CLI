import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const pairwiseRoot = "crates/licoup-native/src/core/secure_mesh_pairwise";
const negotiationFacade = `${pairwiseRoot}/session_negotiation.rs`;
const negotiationRoot = `${pairwiseRoot}/session_negotiation`;
const ratchetCore = `${pairwiseRoot}/key_ratchet.rs`;
const ratchetRoot = `${pairwiseRoot}/key_ratchet`;
const negotiationLeaves = Object.freeze([
  "capability_binding.rs",
  "handshake_machine.rs",
  "input_validation.rs",
  "key_schedule.rs",
  "transcript_codec.rs",
]);
const ratchetAdapters = Object.freeze(["payload_adapter.rs", "relay_codec.rs"]);

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

test("session negotiation is an exact facade over one atomic machine and four leaves", async () => {
  const facade = await read(negotiationFacade);
  for (const leaf of negotiationLeaves) {
    assert.match(facade, new RegExp(`mod ${leaf.replace(".rs", "")};`, "u"));
    await fs.access(path.join(repoRoot, negotiationRoot, leaf));
  }
  const entries = await fs.readdir(path.join(repoRoot, negotiationRoot), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    [...negotiationLeaves].sort(),
  );
  for (const forbidden of ["impl SecureMeshPairwiseSession", "Hkdf", "SigningKey", "OsRng"])
    assert.equal(facade.includes(forbidden), false, forbidden);
});

test("handshake machine remains the single atomic session state transition owner", async () => {
  const machine = await read(`${negotiationRoot}/handshake_machine.rs`);
  assert.equal((machine.match(/impl SecureMeshPairwiseSession/gu) ?? []).length, 1);
  for (const token of [
    "pub fn initiate(", "pub fn accept(", "complete_initiator_handshake",
    "complete_responder_handshake", "capability_negotiation", "initiator_key_confirmed",
  ]) assert.equal(machine.includes(token), true, token);
  for (const forbidden of [
    "pub struct SecureMeshPairwiseSessionIntro", "Hkdf", "fn derive_initial_keys",
    "fn intro_signature_payload", "fn ensure_intro",
  ]) assert.equal(machine.includes(forbidden), false, forbidden);
});

test("capability key schedule transcript and validation leaves stay role-pure", async () => {
  const sources = Object.fromEntries(await Promise.all([
    "capability_binding.rs", "key_schedule.rs", "transcript_codec.rs", "input_validation.rs",
  ].map(async (leaf) => [leaf, await read(`${negotiationRoot}/${leaf}`)])));

  for (const token of [
    "capability_proof_request", "capability_verification_context",
    "secure_mesh_pairwise_build_protocol_digest",
  ]) assert.equal(sources["capability_binding.rs"].includes(token), true, token);
  assert.equal(sources["capability_binding.rs"].includes("SecureMeshPairwiseSession"), false);

  for (const token of [
    "InitialPairwiseKeys", "derive_pqxdh_classical_initiator_secret", "derive_initial_keys",
    "derive_capability_bound_initial_keys", "Hkdf", "out.zeroize()",
  ]) assert.equal(sources["key_schedule.rs"].includes(token), true, token);
  assert.equal(sources["key_schedule.rs"].includes("impl SecureMeshPairwiseSession"), false);
  assert.equal(sources["key_schedule.rs"].includes("SignedCapabilityProof"), false);

  for (const token of [
    "SecureMeshPairwiseSessionIntro", "SecureMeshPairwiseSessionAccepted",
    "intro_signature_payload", "accept_signature_payload", "handshake_transcript_hash",
    "decode_fixed_base64url", "derive_session_id",
  ]) assert.equal(sources["transcript_codec.rs"].includes(token), true, token);
  assert.equal(sources["transcript_codec.rs"].includes("impl SecureMeshPairwiseSession"), false);
  assert.equal(sources["transcript_codec.rs"].includes("OsRng"), false);

  for (const token of ["ensure_local_identity_key_material", "ensure_intro"])
    assert.equal(sources["input_validation.rs"].includes(token), true, token);
  for (const forbidden of ["Hkdf", "OsRng", "impl SecureMeshPairwiseSession"])
    assert.equal(sources["input_validation.rs"].includes(forbidden), false, forbidden);
});

test("key ratchet core retains state KDF skipped-key and replay ownership only", async () => {
  const core = await read(ratchetCore);
  for (const adapter of ratchetAdapters) {
    assert.match(core, new RegExp(`mod ${adapter.replace(".rs", "")};`, "u"));
    await fs.access(path.join(repoRoot, ratchetRoot, adapter));
  }
  const entries = await fs.readdir(path.join(repoRoot, ratchetRoot), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    [...ratchetAdapters].sort(),
  );
  for (const token of [
    "struct SecureMeshPairwiseSession", "struct SkippedMessageKey", "skipped_keys",
    "received_message_ids", "derive_ratchet_root", "advance_chain", "Hkdf",
    "MAX_SKIPPED_KEYS", "MAX_REPLAY_IDS",
  ]) assert.equal(core.includes(token), true, token);
  for (const forbidden of [
    "pub fn seal_payload(", "pub fn open_payload(", "LicoArcRelayEnvelopeDraft",
    "SecureMeshPairwisePrivateRelayHeader", "seal_private_relay_header",
  ]) assert.equal(core.includes(forbidden), false, forbidden);
});

test("payload adapter and relay codec have exact non-overlapping authorities", async () => {
  const payload = await read(`${ratchetRoot}/payload_adapter.rs`);
  const relay = await read(`${ratchetRoot}/relay_codec.rs`);
  for (const token of [
    "pub fn seal_payload(", "pub fn open_payload(", "ContentKey",
    "combine_pairwise_and_extra_aad", "ensure_message_for_session",
  ]) assert.equal(payload.includes(token), true, token);
  for (const forbidden of [
    "LicoArcRelayEnvelope", "SecureMeshPairwisePrivateRelayHeader", "derive_ratchet_root",
    "struct SkippedMessageKey",
  ]) assert.equal(payload.includes(forbidden), false, forbidden);

  for (const token of [
    "pub fn seal_payload_envelope(", "pub fn open_payload_envelope(",
    "relay_envelope_from_pairwise_message", "pairwise_message_from_relay_envelope",
    "SecureMeshPairwisePrivateRelayHeader", "seal_private_relay_header",
  ]) assert.equal(relay.includes(token), true, token);
  for (const forbidden of [
    "Hkdf", "advance_chain", "SkippedMessageKey", "derive_hybrid_message_key",
  ]) assert.equal(relay.includes(forbidden), false, forbidden);
});

test("external consumers use negotiation and ratchet facades only", async () => {
  const internalPath = /(?:session_negotiation|key_ratchet)::(?:capability_binding|handshake_machine|input_validation|key_schedule|transcript_codec|payload_adapter|relay_codec)::/u;
  const consumers = (await sourceFiles("crates/licoup-native/src"))
    .filter((relativePath) =>
      !relativePath.startsWith(`${negotiationRoot}/`) &&
      !relativePath.startsWith(`${ratchetRoot}/`));
  for (const relativePath of consumers) {
    const source = await read(relativePath);
    assert.equal(internalPath.test(source), false, relativePath);
  }
});

test("every extracted boundary has a dedicated narrow regression", async () => {
  const tests = new Set((await fs.readdir(path.join(repoRoot, pairwiseRoot, "tests"))).sort());
  for (const expected of [
    "session_negotiation.rs", "session_negotiation_capability_binding.rs",
    "session_negotiation_input_validation.rs", "session_negotiation_key_schedule.rs",
    "session_negotiation_transcript_codec.rs", "key_ratchet.rs",
    "key_ratchet_payload_adapter.rs", "key_ratchet_relay_codec.rs",
  ]) assert.equal(tests.has(expected), true, expected);
});

test("pairwise boundary leaves retain no egress or unsafe runtime authority", async () => {
  const joined = (await Promise.all([
    ...negotiationLeaves.map((leaf) => read(`${negotiationRoot}/${leaf}`)),
    read(ratchetCore),
    ...ratchetAdapters.map((leaf) => read(`${ratchetRoot}/${leaf}`)),
  ])).join("\n");
  for (const forbidden of [
    "ureq::", "reqwest::", "TcpStream", "UdpSocket", "unsafe {", "Command::new",
  ]) assert.equal(joined.includes(forbidden), false, forbidden);
});
