import { functionBody } from "./source-check.mjs";
import { sha256Json, sha256Text } from "./hash.mjs";

export function extractContentStableVector(source) {
  const body = functionBody(source, "secure_mesh_content_crypto_has_stable_vectors_for_all_payload_kinds");
  const vectorBlocks = [...body.matchAll(/ContentCryptoStableVector\s*\{([\s\S]*?)\n\s*\}/gu)]
    .map((match) => match[1]);
  const vectors = vectorBlocks
    .map((block) => {
      const label = fieldString(block, "label");
      const encryptedHeader = fieldString(block, "encrypted_header");
      const ciphertextSha256 = fieldString(block, "ciphertext_sha256") ||
        (fieldString(block, "ciphertext") ? sha256Text(fieldString(block, "ciphertext")) : "");
      const ciphertextSize = Number(block.match(/ciphertext_size:\s*(\d+)/u)?.[1] || 0);
      return {
        label,
        encryptedHeaderSha256: encryptedHeader ? sha256Text(encryptedHeader) : "",
        ciphertextSha256,
        ciphertextSize,
        vectorDigest: encryptedHeader && ciphertextSha256 && ciphertextSize > 0
          ? sha256Json({ label, encryptedHeader, ciphertextSha256, ciphertextSize })
          : ""
      };
    })
    .filter((vector) => vector.label);
  const labels = new Set(vectors.map((vector) => vector.label).filter(Boolean));
  const requiredLabels = ["command", "result", "error", "file_chunk", "file_manifest"];
  const ok = requiredLabels.every((label) => labels.has(label)) &&
    vectors.every((vector) =>
      Boolean(vector.encryptedHeaderSha256 && vector.ciphertextSha256 && vector.ciphertextSize > 0 && vector.vectorDigest)
    );
  return {
    id: "content-aead-stable-vectors-all-payload-kinds",
    kind: "deterministic-content-crypto-vector",
    ok,
    sourceFile: "crates/licoup-native/src/core/secure_mesh_crypto.rs",
    sourceTest: "secure_mesh_content_crypto_has_stable_vectors_for_all_payload_kinds",
    cipherSuite: "licomesh.secure-payload.v1.chacha20poly1305-hkdfsha256",
    payloadKinds: requiredLabels,
    vectorCount: vectors.length,
    deterministic: true,
    redacted: true,
    rawPlaintextIncluded: false,
    rawContentKeyIncluded: false,
    rawCiphertextIncluded: false,
    vectors,
    vectorDigest: ok ? sha256Json(vectors) : ""
  };
}

export function extractPairwiseStableVector(pqxdhSource, braidSource, pairwiseSource) {
  const pqxdhBody = functionBody(
    pqxdhSource,
    "pqxdh_schedule_is_deterministic_domain_separated_and_context_bound",
  );
  const braidBody = functionBody(braidSource, "authenticator_known_answer");
  const tripleRatchetBody = functionBody(
    pairwiseSource,
    "secure_mesh_pairwise_triple_ratchet_combines_ec_and_sparse_pq_messages",
  );
  const pqxdhVectors = [
    ["ec-secret", /hex\(first\.ec_secret\(\)\),\s*"([a-f0-9]{64})"/u],
    ["scka-secret", /hex\(first\.scka_secret\(\)\),\s*"([a-f0-9]{64})"/u],
    [
      "associated-data",
      /hex\(&libcrux_sha3::sha256\(first\.associated_data\(\)\)\),\s*"([a-f0-9]{64})"/u,
    ],
  ].map(([label, pattern]) => redactedKnownAnswer(label, pqxdhBody, pattern));
  const braidVectors = [
    ["authenticator-root", /hex\(auth\.root_key\.as_slice\(\)\),\s*"([a-f0-9]{64})"/u],
    ["authenticator-mac", /hex\(auth\.mac_key\.as_slice\(\)\),\s*"([a-f0-9]{64})"/u],
    [
      "header-mac",
      /hex\(&auth\.mac_header\(1,\s*&header\)\.unwrap\(\)\),\s*"([a-f0-9]{64})"/u,
    ],
  ].map(([label, pattern]) => redactedKnownAnswer(label, braidBody, pattern));
  const parameterSizes = {
    headerBytes: numericConstant(braidSource, "ML_KEM_BRAID_HEADER_BYTES"),
    encapsulationKeyBytes: numericConstant(braidSource, "ML_KEM_BRAID_EK_BYTES"),
    ciphertext1Bytes: numericConstant(braidSource, "ML_KEM_BRAID_CT1_BYTES"),
    ciphertext2Bytes: numericConstant(braidSource, "ML_KEM_BRAID_CT2_BYTES"),
    transitionCount: numericConstant(braidSource, "ML_KEM_BRAID_TRANSITION_COUNT"),
  };
  const cipherSuite = String(
    pairwiseSource.match(/SECURE_MESH_PAIRWISE_CIPHER_SUITE:\s*&str\s*=\s*"([^"]+)"/u)?.[1] || "",
  );
  const tripleRatchetIntegrationCovered = [
    "seal_message_with_nonce",
    "sparse_pq_header.message_number",
    "rotate_sending_ratchet_with_secret",
    "open_message",
  ].every((token) => tripleRatchetBody.includes(token));
  const expectedParameterSizes = {
    headerBytes: 64,
    encapsulationKeyBytes: 1536,
    ciphertext1Bytes: 1408,
    ciphertext2Bytes: 160,
    transitionCount: 13,
  };
  const ok = pqxdhVectors.every((vector) => vector.expectedValueSha256.startsWith("sha256:")) &&
    braidVectors.every((vector) => vector.expectedValueSha256.startsWith("sha256:")) &&
    JSON.stringify(parameterSizes) === JSON.stringify(expectedParameterSizes) &&
    cipherSuite ===
      "licomesh.pqxdh-triple-ratchet.v1.x25519-ed25519-mlkem1024-hkdfsha256-chacha20poly1305" &&
    tripleRatchetIntegrationCovered;
  return {
    id: "pairwise-pqxdh-mlkem1024-triple-ratchet-stable-vectors",
    kind: "deterministic-pairwise-pqxdh-mlkem1024-triple-ratchet-vector",
    ok,
    sourceFiles: [
      "crates/licoup-native/src/core/secure_mesh_pqxdh.rs",
      "crates/licoup-native/src/core/secure_mesh_mlkem_braid.rs",
      "crates/licoup-native/src/core/secure_mesh_pairwise.rs",
    ],
    sourceTests: [
      "pqxdh_schedule_is_deterministic_domain_separated_and_context_bound",
      "authenticator_known_answer",
      "secure_mesh_pairwise_triple_ratchet_combines_ec_and_sparse_pq_messages",
    ],
    cipherSuite,
    parameterSet: "ML-KEM-1024",
    parameterSizes,
    deterministic: true,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    pqxdhVectorCount: pqxdhVectors.length,
    braidVectorCount: braidVectors.length,
    tripleRatchetIntegrationCovered,
    pqxdhVectors,
    braidVectors,
    vectorDigest: ok
      ? sha256Json({ cipherSuite, parameterSizes, pqxdhVectors, braidVectors })
      : "",
  };
}

export function redactedKnownAnswer(label, body, pattern) {
  const expectedValue = String(body.match(pattern)?.[1] || "");
  return {
    label,
    expectedValueSha256: expectedValue ? sha256Text(expectedValue) : "",
  };
}

export function numericConstant(source, name) {
  const raw = String(
    source.match(new RegExp(`const ${name}: usize = ([0-9_]+);`, "u"))?.[1] || "0",
  );
  return Number(raw.replaceAll("_", ""));
}

export function fieldString(source, fieldName) {
  return source.match(new RegExp(`${fieldName}:\\s*"([^"]+)"`, "u"))?.[1] || "";
}
