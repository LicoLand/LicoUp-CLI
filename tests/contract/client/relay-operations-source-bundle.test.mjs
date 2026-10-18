import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/domain/mobile_relay/relay_operations.rs";
const moduleRoot = "crates/licoup-native/src/domain/mobile_relay/relay_operations";
const productionLeaves = Object.freeze([
  "allow_list.rs",
  "command_handlers.rs",
  "command_handlers/check_in.rs",
  "command_handlers/create.rs",
  "command_handlers/poll.rs",
  "command_handlers/result.rs",
  "delivery.rs",
  "envelope.rs",
  "mailbox.rs",
  "station.rs",
  "status.rs",
]);
const testLeaves = Object.freeze([
  "allow_list.rs",
  "delivery.rs",
  "envelope.rs",
  "mailbox.rs",
  "station.rs",
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

test("relay operations uses a thin facade and ordinary owned leaves", async () => {
  const facade = await read(facadePath);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((name) => name !== "tests")
      .sort(),
    ["allow_list", "command_handlers", "delivery", "envelope", "mailbox", "station", "status"],
  );
  for (const token of [
    "command_create_secure",
    "canonical_mailbox_token",
    "validate_secure_envelope",
    "station_context",
    "e2ee_status",
  ]) {
    assert.ok(facade.includes(token), token);
  }
});

test("command handlers retain ciphertext-only four-operation station calls", async () => {
  const source = await sources();
  const handlers = [
    "command_handlers/check_in.rs",
    "command_handlers/create.rs",
    "command_handlers/poll.rs",
    "command_handlers/result.rs",
  ].map((leaf) => source[leaf]).join("\n");
  for (const token of [
    "lease_mailbox",
    "send_envelope",
    "receive_envelopes",
    "delete_envelope",
  ]) {
    assert.ok(handlers.includes(token), token);
  }
  assert.ok(source["command_handlers/create.rs"].includes("secure_envelope_param(params)"));
  assert.ok(source["command_handlers/create.rs"].includes(
    "seal_mobile_relay_payload_deferred"));
  assert.ok(source["command_handlers/create.rs"].includes("commit_with_pending_delivery"));
  assert.ok(source["command_handlers/create.rs"].includes("recover_pending_secure_command"));
  assert.ok(source["command_handlers/result.rs"].includes(
    "open_mobile_relay_payload_deferred"));
  assert.ok(source["command_handlers/result.rs"].includes("commit_with_received_payload"));
  assert.ok(source["command_handlers/result.rs"].includes('"bodyRedacted": true'));
  assert.ok(handlers.includes("transportHint"));
  assert.equal(handlers.includes("execute_command"), false);
  assert.equal(handlers.includes("plaintext"), false);
});

test("station mailbox envelope delivery status and allow-list have distinct owners", async () => {
  const source = await sources();
  const ownership = new Map([
    ["station.rs", "fn station_context"],
    ["mailbox.rs", "fn canonical_mailbox_token"],
    ["envelope.rs", "fn validate_secure_envelope"],
    ["delivery.rs", "fn relay_envelope_from_delivery"],
    ["status.rs", "fn e2ee_status"],
    ["allow_list.rs", "fn allowed_agent_ids"],
  ]);
  for (const [owner, token] of ownership) {
    assert.ok(source[owner].includes(token));
    for (const [other, otherSource] of Object.entries(source)) {
      if (other !== owner) assert.equal(otherSource.includes(token), false);
    }
  }
});

test("station context is explicit and projects only untrusted transport hints", async () => {
  const source = await sources();
  for (const token of [
    "mobile relay is disabled",
    "effective_station_base_url",
    "validated_station_base_url",
    "BadTowerStationTransport::new",
    "stationReportedLeased",
    "stationReportedAccepted",
    "stationReportedDuplicate",
    "stationReportedAcknowledged",
  ]) {
    assert.ok(source["station.rs"].includes(token), token);
  }
});

test("mailbox envelope and delivery enforce Lico Arc bounded structures", async () => {
  const source = await sources();
  for (const token of [
    "SecureMeshDeliverySecret",
    "SecureMeshMailboxSchedule",
    "SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS",
    "checked_mul",
  ]) {
    assert.ok(source["mailbox.rs"].includes(token), token);
  }
  assert.ok(source["envelope.rs"].includes("LicoArcRelayEnvelope::from_json"));
  assert.ok(source["envelope.rs"].includes("serde_json::to_string"));
  const pairwisePayload = await read(
    "crates/licoup-native/src/domain/mobile_relay/pairwise_session/payload.rs",
  );
  assert.ok(pairwisePayload.includes("MOBILE_RELAY_COMMAND_TTL_SECONDS"));
  assert.ok(pairwisePayload.includes("timestamp_after_seconds"));
  assert.ok(source["delivery.rs"].includes("LicoArcRelayEnvelope"));
  assert.ok(source["delivery.rs"].includes("to_json"));
});

test("status and allow list remain authorization-aware and secret-free", async () => {
  const source = await sources();
  for (const token of [
    "should_authorize_secret_read",
    "authorizationRequiredForFullStatus",
    "pairwise_session_verification_requires_authorization",
    "key_transparency_label_refresh_required",
    "redacted_pairing_invite",
  ]) {
    assert.ok(source["status.rs"].includes(token), token);
  }
  assert.ok(source["allow_list.rs"].includes("PACKAGED_RUNTIME_ADAPTER_IDS"));
  assert.ok(source["allow_list.rs"].includes("runtime.message.send"));
  assert.ok(source["allow_list.rs"].includes("BTreeSet"));
  for (const forbiddenSecret of [
    "privateKeyBase64url", "signingKeyBase64url", "pairingSecretBase64url",
  ]) {
    assert.equal(source["status.rs"].includes(forbiddenSecret), false);
  }
});

test("relay operations has no wildcard hidden implementation or direct HTTP egress", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  for (const forbidden of [
    "use super::*", "use crate::*", "::*;", "#[path", "include!(", "mod tests {",
    "ureq::", "reqwest::", "TcpStream", "Command::new", "upload", "http://", "https://",
  ]) {
    assert.equal(joined.includes(forbidden), false, forbidden);
  }
});

test("relay operations regressions are split into selectable leaves", async () => {
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
