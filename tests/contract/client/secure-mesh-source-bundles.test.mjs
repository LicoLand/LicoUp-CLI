import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { loadSecureMeshEncryptedFileHandoffConfig } from
  "../../../tools/scripts/lib/secure-mesh-encrypted-file-handoff-config.mjs";
import { loadSecureMeshPairwiseContentAuditConfig } from
  "../../../tools/scripts/lib/secure-mesh-pairwise-content-audit-config.mjs";
import { loadSecureMeshTrustUxConfig } from
  "../../../tools/scripts/lib/secure-mesh-trust-ux-config.mjs";
import {
  normalizeSourceCheckFiles,
  readSourceCheckBundle,
} from "../../../tools/scripts/lib/source-check-bundle.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));

function functionBody(source, name) {
  const start = source.indexOf(`fn ${name}`);
  assert.notEqual(start, -1, `missing checked function: ${name}`);
  const braceStart = source.indexOf("{", start);
  assert.notEqual(braceStart, -1, `missing checked function body: ${name}`);
  let depth = 0;
  for (let index = braceStart; index < source.length; index += 1) {
    if (source[index] === "{") depth += 1;
    if (source[index] !== "}") continue;
    depth -= 1;
    if (depth === 0) return source.slice(braceStart, index + 1);
  }
  assert.fail(`unterminated checked function body: ${name}`);
}

async function assertSourceBundle(check) {
  assert.ok(Array.isArray(check.files));
  assert.ok(check.files.length > 0);
  assert.equal(check.file, check.files[0]);
  assert.equal(new Set(check.files).size, check.files.length);
  const sources = await Promise.all(check.files.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const source = sources.join("\n");
  const scopedSource = check.functionName
    ? functionBody(source, check.functionName)
    : source;
  for (const token of check.tokens || []) {
    assert.ok(scopedSource.includes(token), `${check.id} is missing required source evidence`);
  }
  for (const token of check.forbiddenTokens || []) {
    assert.ok(!scopedSource.includes(token), `${check.id} contains forbidden source evidence`);
  }
}

test("Secure Mesh source checks accept focused multi-file evidence bundles", async () => {
  const configs = await Promise.all([
    loadSecureMeshPairwiseContentAuditConfig(),
    loadSecureMeshTrustUxConfig(),
    loadSecureMeshEncryptedFileHandoffConfig(),
  ]);
  const checks = configs.flatMap((config) => config.sourceChecks);
  assert.ok(checks.some((check) => check.files.length > 1));
  await Promise.all(checks.map(assertSourceBundle));
});

test("source-check bundles preserve legacy files and deduplicate focused files", async () => {
  const normalize = (value, label) => {
    assert.ok(label.includes("file"));
    const normalized = String(value || "").trim();
    if (!normalized) throw new Error("invalid source ref");
    return normalized;
  };
  assert.deepEqual(
    normalizeSourceCheckFiles({ file: "legacy.rs" }, normalize, "legacy check"),
    ["legacy.rs"],
  );
  assert.deepEqual(
    normalizeSourceCheckFiles(
      { files: ["first.rs", "first.rs", "second.rs"] },
      normalize,
      "focused check",
    ),
    ["first.rs", "second.rs"],
  );
  assert.throws(
    () => normalizeSourceCheckFiles({ files: [] }, normalize, "empty check"),
    /must define files/u,
  );
  const reads = [];
  const bundle = await readSourceCheckBundle(
    { file: "legacy.rs" },
    async (sourceRef) => {
      reads.push(sourceRef);
      return "legacy-source";
    },
  );
  assert.deepEqual(bundle, { files: ["legacy.rs"], source: "legacy-source" });
  assert.deepEqual(reads, ["legacy.rs"]);
});

test("core MLS source bundle preserves split cryptographic and durable-state authority", async () => {
  const files = [
    "crates/licoup-native/src/core/secure_mesh_mls.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/runtime_self_test.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/capability_extension.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/provider.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/provider_storage.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/participant.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/key_package.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_create.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_state.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_member.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_join.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_message.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_payload.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/group_commit.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/private_context_codec.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/durable_store.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/codec.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/config.rs",
    "crates/licoup-native/src/core/secure_mesh_mls/constants.rs",
  ];
  const sources = await Promise.all(files.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const source = sources.join("\n");
  for (const token of [
    "runtime_crypto_self_test",
    "SecureMeshMlsCapabilityExtension",
    "SecureMeshOpenMlsProvider",
    "mlkem1024_seed_base64url",
    "validate_ml_kem_1024_public_key",
    "create_mlkem1024_epoch_extension",
    "open_mlkem1024_epoch_extension",
    "derive_group_payload_content_key",
    "open_private_context_payload",
    "secure mesh MLS durable compare-and-swap failed",
    "secure mesh MLS durable revoke epoch rollback detected",
    "checked_add",
    "zeroize",
  ]) {
    assert.ok(source.includes(token), `core MLS source bundle is missing ${token}`);
  }
  assert.ok(!sources[0].includes("impl SecureMeshMlsGroup"));
  assert.ok(!sources[0].includes("mod tests {"));
});

test("ML-KEM Braid source bundle preserves bounded split protocol authority", async () => {
  const files = [
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/constants.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/wire.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/output.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/secret.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/authenticator.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/erasure_gf.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/erasure_encoder.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/erasure_decoder.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/encapsulation_kdf.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/protocol_state.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/transition.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/send_transition.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/receive_transition.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/session.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/persistence.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/validation.rs",
  ];
  const sources = await Promise.all(files.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const source = sources.join("\n");
  for (const token of [
    "ML_KEM_BRAID_TRANSITION_COUNT",
    "MAX_SOURCE_CHUNKS",
    "MAX_PERSISTED_SESSION_BYTES",
    "incremental::encapsulate1",
    "incremental::encapsulate2",
    "decapsulate_compressed_key",
    "batch_inverse_into",
    "checked_next_epoch",
    "PERSISTENCE_REVISION",
    "BTreeMap",
    "Zeroizing",
    "zeroize",
  ]) {
    assert.ok(source.includes(token), `ML-KEM Braid source bundle is missing ${token}`);
  }
  assert.ok(!sources[0].includes("impl MlKemBraidSession"));
  assert.ok(!sources[0].includes("mod tests {"));
  assert.ok(!sources[0].includes("#[path"));
});

test("pairwise persistence source bundle preserves split transactional and secret-store authority", async () => {
  const files = [
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/capability_proof.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/initial_write.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/local_prekey.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/namespace_binding.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/public_snapshot.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/remote_prekey.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/replay_watermark.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/restoration_validation.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/revocation.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/schema.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/secret_cleanup.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/secret_snapshot.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/secret_store_io.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/session_codec.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/session_commit.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/session_read.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/store_model.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/persistence/store_open.rs",
  ];
  const sources = await Promise.all(files.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const source = sources.join("\n");
  for (const token of [
    "prepare_secret_bound_snapshot_with_optional_authorization",
    "consume_local_prekey_use",
    "consume_remote_prekey_use",
    "consume_prepared_capability_proof_pair",
    "advance_pairwise_replay_time_watermark",
    "replay_window_preserved",
    "skipped_keys_not_reintroduced",
    "pairwise_secret_store_key_is_bound",
    "MAX_PERSISTED_SECRET_SNAPSHOT_BYTES",
    "Zeroizing",
    "zeroize",
    "secure_delete",
    "compare-and-swap failed",
  ]) {
    assert.ok(source.includes(token), `pairwise persistence source bundle is missing ${token}`);
  }
  assert.ok(!sources[0].includes("impl SecureMeshPairwiseDurableStore"));
  assert.ok(!sources[0].includes("mod tests {"));
  assert.ok(!sources[0].includes("#[path"));
});

test("content crypto source bundle preserves split AEAD, KDF, codec, and padding authority", async () => {
  const files = [
    "crates/licoup-native/src/core/secure_mesh_crypto.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/aad_binding.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/constants.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/content_key.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/frame_codec.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/header_codec.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/key_derivation.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/length_codec.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/model.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/padding.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/private_context.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/public_payload.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/validation.rs",
  ];
  const sources = await Promise.all(files.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const source = sources.join("\n");
  for (const token of [
    "SECURE_MESH_CONTENT_CIPHER_SUITE",
    "MAX_CONTENT_BYTES",
    "MAX_PADDING_BUCKET_BYTES",
    "AAD_MAGIC",
    "HKDF_SALT_DOMAIN",
    "PRIVATE_CONTEXT_HKDF_SALT_DOMAIN",
    "ChaCha20Poly1305",
    "Hkdf::<Sha256>",
    "Zeroizing",
    "build_aad_with_binding",
    "derive_aead_key",
    "derive_private_context_aead_key",
    "add_bucket_padding",
    "remove_authenticated_padding",
    "encode_private_context_frame",
    "decode_private_context_frame",
    "decode_canonical_base64url",
    "seal_payload_with_aad_binding",
    "open_payload_with_aad_binding",
    "seal_private_context_payload",
    "open_private_context_payload",
  ]) {
    assert.ok(source.includes(token), `content crypto source bundle is missing ${token}`);
  }
  assert.ok(!sources[0].includes("impl ContentKey"));
  assert.ok(!sources[0].includes("mod tests {"));
  assert.ok(!sources[0].includes("#[path"));
});

test("mobile FFI source bundle preserves shared bounded dispatch and redacted fixture authority", async () => {
  const files = [
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/action_catalog.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/dispatch_context.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/dispatch_router.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/feature_status.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/fixture_envelope.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/fixture_file.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/fixture_lifecycle.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/fixture_payload.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/fixture_trust.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/protected_operation.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/redacted_error.rs",
    "crates/licoup-native/src/ffi/secure_mesh_mobile_ffi/request_validation.rs",
  ];
  const sources = await Promise.all(files.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const source = sources.join("\n");
  for (const token of [
    "EXPECTED_FEATURES",
    "runtime_protocol_hash",
    "MOBILE_RELAY_NATIVE_ACTIONS",
    "MAX_FFI_REQUEST_BYTES",
    "MAX_FFI_JSON_DEPTH",
    "MAX_FFI_JSON_NODES",
    "validate_ffi_json_structure",
    "secure_mesh_action_requires_protected_operation_gate",
    "ensure_secure_mesh_protected_operation_allowed",
    "with_pairwise_secret_store_override",
    "with_mobile_relay_secret_store_override",
    "native_envelope_fixture",
    "native_payload_crypto_fixture",
    "native_file_handoff_proof_fixture",
    "native_device_trust_fixture",
    "native_lifecycle_service_action_fixture",
    "unsupported_action_response",
  ]) {
    assert.ok(source.includes(token), `mobile FFI source bundle is missing ${token}`);
  }
  assert.ok(!sources[0].includes("match action"));
  assert.ok(!sources[0].includes("mod tests {"));
  assert.ok(!sources[0].includes("#[path"));
});
