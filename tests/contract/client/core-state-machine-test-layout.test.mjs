import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const coreRoot = "crates/licoup-native/src/core";

const physicalTestModules = Object.freeze([
  {
    id: "session-negotiation",
    source: `${coreRoot}/secure_mesh_session_negotiation.rs`,
    tests: `${coreRoot}/secure_mesh_session_negotiation/tests/mod.rs`,
    names: [
      "pairwise_and_mls_transcripts_bind_both_proofs_and_protocol_only_intersection",
      "stable_pairwise_and_mls_transcript_vectors_are_deterministic",
      "binding_tamper_downgrade_scope_injection_and_mismatch_fail_before_acceptance",
      "mls_binding_rejects_proof_digest_and_negotiated_set_tamper",
      "exact_capability_proof_replay_is_rejected_before_second_session_acceptance",
      "missing_mandatory_capability_and_cross_session_binding_fail_closed",
      "challenge_and_policy_mismatch_are_rejected_before_transcript_acceptance",
      "accepted_projection_contains_exact_local_peer_and_negotiated_sets_without_tier",
      "replay_guard_never_evicts_unexpired_proofs_to_accept_new_sessions",
      "proof_digests_are_bound_even_when_builds_differ",
    ],
    vectors: [
      "sha256:QdEF_MBAQpxTB1cuyu6jX80moqptttmXvsb2byh9fGg",
      "sha256:dQTsUOMYHVpn33HVrIvCigfzIFu-2vvQEazzzLY2U-Y",
    ],
  },
  {
    id: "capability-proof",
    source: `${coreRoot}/secure_mesh_capability_proof.rs`,
    tests: `${coreRoot}/secure_mesh_capability_proof/tests/mod.rs`,
    names: [
      "stable_capability_proof_vector_uses_existing_endpoint_identity_signature",
      "canonical_proof_is_independent_of_platform_fact_input_order",
      "proof_verification_rejects_signature_challenge_freshness_identity_build_and_policy_mismatch",
      "proof_verification_rejects_dependency_incomplete_overclaim_and_inexact_sets",
      "proof_json_rejects_unknown_capabilities_unknown_fields_and_noncanonical_encodings",
      "projection_contains_exact_sets_and_redacted_reasons_without_fixed_posture_fields",
      "proof_signing_rejects_wrong_signing_key_and_unbounded_lifetime",
      "exact_sets_remain_authoritative_across_json_round_trip",
      "proof_scope_catalog_distinguishes_protocol_from_local_custody",
    ],
    vectors: [
      "1UZOP7gXoDjrHzIBkFBaT4SVlRYBZ4ovw9G0naGFSSGCH1ev3mLRUC6-i0yMg0Hid0TAzFfaSUxyVJOK3r1bBg",
      "sha256:_GTxCW4cyLrWuGTLMAIXLD9bNWWdUJD_KR2Z80waScQ",
      "37796ae8c68f7b93117928a9702f79536e15e43c4a9baf04cd9f3b2c85cf5688",
    ],
  },
  {
    id: "sparse-pq-ratchet",
    source: `${coreRoot}/secure_mesh_sparse_pq_ratchet.rs`,
    tests: `${coreRoot}/secure_mesh_sparse_pq_ratchet/tests/mod.rs`,
    names: [
      "sparse_pq_ratchet_matches_keys_and_restores_state",
      "sparse_pq_ratchet_supports_bounded_out_of_order_messages",
      "sparse_pq_ratchet_opens_retained_previous_epoch_after_new_epoch",
      "hybrid_message_key_is_bound_to_both_ratchets_and_session",
      "sparse_pq_ratchet_destroy_is_persistent_and_fail_closed",
      "sparse_pq_ratchet_rejects_oversized_persisted_state",
    ],
    vectors: ["[0x31; 32]", "[0x42; 32]", "[0x47; 32]", "[0x53; 32]"],
  },
  {
    id: "secure-mesh-acp",
    source: `${coreRoot}/secure_mesh_acp.rs`,
    tests: `${coreRoot}/secure_mesh_acp/tests/mod.rs`,
    names: [
      "secure_mesh_acp_envelope_aad_has_stable_digest_vector",
      "secure_mesh_acp_envelope_aad_field_mutation_fails_open",
      "secure_mesh_acp_pairwise_protected_payload_round_trip",
      "secure_mesh_acp_plaintext_protected_payload_relay_is_blocked",
      "secure_mesh_acp_sealed_payloads_hide_raw_and_encoded_canaries",
      "secure_mesh_acp_payload_classes_cover_protected_taxonomy",
      "secure_mesh_acp_status_remains_independent_review_blocked",
    ],
    vectors: ["c5dcd95e8ae40644ea4cad031eca4351ac7e095bd10e5e9a219b3c60c1445534"],
  },
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

function testNames(source) {
  return [...source.matchAll(/#\[test\]\s*fn\s+([a-z0-9_]+)\s*\(/gu)]
    .map((match) => match[1]);
}

test("core state-machine regressions use ordinary physical test submodules", async () => {
  for (const module of physicalTestModules) {
    const [production, tests] = await Promise.all([
      read(module.source),
      read(module.tests),
    ]);
    assert.match(production, /#\[cfg\(test\)\]\s*mod tests;/u, module.id);
    assert.equal(production.includes("mod tests {"), false, module.id);
    assert.equal(production.includes("#[test]"), false, module.id);
    for (const forbidden of ["#[path", "include!(", "use super::*", "mod tests {"]) {
      assert.equal(tests.includes(forbidden), false, `${module.id}: ${forbidden}`);
    }
  }
});

test("physical test modules preserve every selectable test name", async () => {
  for (const module of physicalTestModules) {
    assert.deepEqual(testNames(await read(module.tests)), module.names, module.id);
  }
});

test("physical test modules preserve deterministic vectors", async () => {
  for (const module of physicalTestModules) {
    const source = await read(module.tests);
    for (const vector of module.vectors) {
      assert.ok(source.includes(vector), `${module.id}: ${vector}`);
    }
  }
});

test("MLS security-ledger test-only queries have a physical support owner", async () => {
  const productionPath = `${coreRoot}/secure_mesh_mls_product/security_ledger.rs`;
  const supportPath = `${coreRoot}/secure_mesh_mls_product/security_ledger/test_support.rs`;
  const scenariosPath = `${coreRoot}/secure_mesh_mls_product/tests/security_ledger.rs`;
  const [production, support, scenarios] = await Promise.all([
    read(productionPath),
    read(supportPath),
    read(scenariosPath),
  ]);

  assert.match(production, /#\[cfg\(test\)\]\s*mod test_support;/u);
  for (const token of [
    "use rusqlite::OptionalExtension",
    "fn was_key_package_consumed",
    "fn key_package_consumed_at",
  ]) {
    assert.equal(production.includes(token), false, token);
  }
  for (const token of [
    "OptionalExtension",
    "fn was_key_package_consumed",
    "fn key_package_consumed_at",
  ]) {
    assert.ok(support.includes(token), token);
  }
  assert.equal(support.includes("#[path"), false);
  assert.equal(support.includes("include!("), false);
  assert.deepEqual(testNames(scenarios), [
    "secure_mesh_mls_journal_recovers_every_action_at_every_cross_store_boundary",
    "secure_mesh_mls_invalid_prepared_requests_do_not_consume_journal_capacity",
    "secure_mesh_mls_journal_enforces_single_writer_exact_state_and_bounded_gc",
    "secure_mesh_mls_journal_and_replay_ledgers_fail_closed_at_capacity",
    "secure_mesh_mls_product_keypackage_one_time_consumption",
    "secure_mesh_mls_replay_watermark_rejects_expiry_revival_after_clock_rollback",
    "secure_mesh_mls_security_ledger_survives_restart_and_rolls_back_atomically",
  ]);
  assert.ok(scenarios.includes("was_key_package_consumed"));
  assert.ok(scenarios.includes("key_package_consumed_at"));
});
