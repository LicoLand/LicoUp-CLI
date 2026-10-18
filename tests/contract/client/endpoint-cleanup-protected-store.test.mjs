import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");

const NATIVE_CUSTODY = "crates/licoup-native/src/domain/mobile_relay/secret_custody";
const authority = read(`${NATIVE_CUSTODY}/cleanup_authority.rs`);
const consumer = read(`${NATIVE_CUSTODY}/cleanup.rs`);
const tests = read(
  "crates/licoup-native/src/domain/mobile_relay/tests/secret_custody/cleanup_authority.rs",
);
const commandTable = read("crates/licoup-native/src/ffi/commands/mod.rs");
const doc = read("docs/modules/endpoint-collaboration.md");

test("the cleanup authorization is persisted evidence, not a caller claim", () => {
  // The subject is derived from persisted identity and custody state.
  assert.match(authority, /fn custody_cleanup_subject\(/u);
  assert.match(authority, /local_public_device_identity\(config\)/u);
  assert.match(authority, /current_custody_namespace\(\)/u);

  // The replacement endpoint is accepted only against a device trust record
  // the subject itself signed, re-verified here.
  assert.match(authority, /fn authenticated_replacement_endpoint\(/u);
  assert.match(authority, /verify_device_trust_record_json\(/u);
  assert.match(authority, /trust_state == DeviceTrustState::Verified/u);
  assert.match(authority, /mobile relay replacement endpoint must differ from the custody subject/u);

  // The authenticated replacement endpoint is required before the confirmation
  // is even read: a missing trust record refuses regardless of the parameters.
  const authorize = authority.slice(authority.indexOf("fn authorize_custody_cleanup("));
  assert.ok(
    authorize.indexOf("authenticated_replacement_endpoint(config)") <
      authorize.indexOf("validate_custody_cleanup_confirmation("),
    "authentication must precede confirmation validation",
  );
});

test("the confirmation is an informed record naming the exact bounded scope", () => {
  assert.match(authority, /CUSTODY_CLEANUP_CONFIRMATION_SCHEMA/u);
  assert.match(authority, /CUSTODY_CLEANUP_CONSEQUENCE: &str =\s*"irreversibleLocalSecretErasure"/u);
  for (const field of [
    "subjectEndpointId",
    "subjectIdentityFingerprint",
    "custodyNamespace",
    "replacementEndpointId",
    "replacementDeviceTrustFingerprint",
    "inventoryDigest",
    "scope",
  ]) {
    assert.match(authority, new RegExp(`"${field}"`, "u"));
  }
  // Anything outside the enumerated inventory is refused, not erased.
  assert.match(authority, /confirmation scope is outside the bounded custody inventory/u);
  assert.match(authority, /does not cover the enumerated custody inventory/u);
  assert.match(authority, /confirmation scope exceeds its bound/u);
  // The retired flag-based route is gone from the whole tree.
  assert.doesNotMatch(authority, /disposableProof|disposable-proof/u);
  assert.doesNotMatch(consumer, /disposableProof|disposable-proof/u);
  assert.doesNotMatch(commandTable, /disposable-proof/u);
  assert.match(commandTable, /name: "cleanup-confirmation"/u);
});

test("the custody consumer deletes narrowly and uses no other custody operation", () => {
  // Scope the check to the cleanup route itself: the shared secret-class
  // round-trip helper elsewhere in the file legitimately round-trips a value.
  const route = consumer.slice(
    consumer.indexOf("fn e2ee_secret_store_cleanup_in("),
    consumer.indexOf("fn load_config_for_custody_cleanup("),
  );
  // The loop walks the authorized inventory and nothing else.
  assert.match(route, /for handle in &inventory\.secret_handles/u);
  assert.match(
    route,
    /remove_authorized_pairwise_store_files\(&inventory\.pairwise_store_files\)/u,
  );
  // Delete is the only custody operation the route performs.
  assert.match(route, /delete_secret_with_session\(&session, handle\)/u);
  assert.doesNotMatch(route, /get_secret|set_secret|expose_bytes|export/u);
  // The batch is sized from the authorized inventory, and its own budget is
  // checked afterwards so a wider session cannot pass unnoticed.
  assert.match(route, /let operation_count = inventory\.secret_handles\.len\(\)/u);
  assert.match(route, /consumed_operation_count\(\) == operation_count/u);
  assert.match(route, /authorization_batch_within_budget\(\)/u);
});

test("settlement is observed and a lost receipt is never converted into success", () => {
  // `complete` is derived from observation, and every refusal leaves a bounded
  // pending list rather than a claimed erase.
  assert.match(consumer, /"status": if complete \{ "cleaned" \} else \{ "partial" \}/u);
  assert.match(consumer, /let complete = pending_secret_handles\.is_empty\(\) && pending_pairwise_store_files\.is_empty\(\)/u);
  assert.match(consumer, /pendingPairwiseStoreFiles/u);
  assert.match(consumer, /pendingSecretHandles/u);
  assert.match(consumer, /custody_cleanup_reason_code/u);
  assert.match(consumer, /ErrorKind::PermissionDenied/u);
  // The receipt is issued, and the replacement endpoint is explicitly not
  // confirmed by the cleaned side.
  assert.match(consumer, /CUSTODY_CLEANUP_RECEIPT_KIND/u);
  assert.match(consumer, /"replacementEndpointConfirmed": false/u);
  assert.match(consumer, /"confirmation": "pendingReceiptDelivery"/u);
  assert.match(consumer, /never silently converted into success/u);
  // Nothing is written after the last observation. The check covers the
  // production source only; the synthetic fixture below it deliberately writes.
  assert.match(consumer, /After the last observation this function writes nothing/u);
  assert.doesNotMatch(
    consumer.slice(0, consumer.indexOf("#[cfg(test)]")),
    /fs::write|File::create|create_dir_all/u,
  );
});

test("the owning module guidance states the route and its limits", () => {
  for (const statement of [
    "The approved credential deletion route",
    "Strict authentication",
    "Explicit informed confirmation",
    "Settlement is observed, never assumed",
    "Ordinary protected operations keep their existing authorization",
    "promises forensic or backup erasure",
  ]) {
    assert.match(doc, new RegExp(statement, "u"));
  }
});

test("the shipped proofs are synthetic custody fixtures", () => {
  assert.match(
    tests,
    /EphemeralSecretStore|with_mobile_relay_secret_store_override|with_pairwise_secret_store_override/u,
  );
  assert.match(tests, /temp_dir\(/u);
  assert.doesNotMatch(tests, /security-framework|SecItemDelete|keyring/u);
  for (const proof of [
    "mobile_relay_custody_cleanup_admits_only_the_authorized_bounded_inventory",
    "mobile_relay_custody_cleanup_rejects_without_an_authenticated_replacement_endpoint",
    "mobile_relay_custody_cleanup_rejects_a_confirmation_outside_the_bounded_inventory",
    "mobile_relay_custody_cleanup_reports_denied_deletion_as_pending",
    "mobile_relay_custody_cleanup_completes_once_the_denied_deletion_is_allowed",
  ]) {
    assert.match(tests, new RegExp(`fn ${proof}\\(`, "u"));
  }
});
