import { sha256Json } from "./hash.mjs";
import {
  extractContentStableVector,
  extractPairwiseStableVector,
} from "./vectors.mjs";

export async function buildVectorCorpus({
  readText,
  nativeTestFilters,
  sourceOfTruth,
  signoffBindingForCorpus,
}) {
  const readBundle = async (refs) => (
    await Promise.all(refs.map((ref) => readText(ref)))
  ).join("\n");
  const contentCryptoSource = await readBundle([
    "crates/licoup-native/src/core/secure_mesh_crypto.rs",
    "crates/licoup-native/src/core/secure_mesh_crypto/tests/stable_vector.rs",
  ]);
  const pqxdhSource = await readText(
    "crates/licoup-native/src/core/secure_mesh_pqxdh.rs",
  );
  const braidSource = await readBundle([
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/constants.rs",
    "crates/licoup-native/src/core/secure_mesh_mlkem_braid/tests/authenticator.rs",
  ]);
  const pairwiseSource = await readBundle([
    "crates/licoup-native/src/core/secure_mesh_pairwise.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/support.rs",
    "crates/licoup-native/src/core/secure_mesh_pairwise/tests/session_negotiation.rs",
  ]);
  const contentVector = extractContentStableVector(contentCryptoSource);
  const pairwiseVector = extractPairwiseStableVector(
    pqxdhSource,
    braidSource,
    pairwiseSource,
  );
  const corpus = {
    ok: contentVector.ok === true && pairwiseVector.ok === true,
    schemaVersion: "licomesh.secure-mesh.pairwise-content-vector-corpus.v1",
    generatedAt: new Date().toISOString(),
    sourceOfTruth,
    redacted: true,
    rawPrivateMaterialIncluded: false,
    rawPlaintextIncluded: false,
    rawPublicWireBytesIncluded: false,
    externalCryptographicReviewComplete: false,
    releaseOwnerSignoffComplete: false,
    entries: [
      contentVector,
      pairwiseVector,
      {
        id: "pairwise-ratchet-command-result-coverage",
        kind: "native-test-coverage",
        ok: true,
        redacted: true,
        sourceTests: nativeTestFilters.filter(
          (filter) =>
            filter.includes("pairwise") ||
            filter.includes("mobile_relay") ||
            filter.includes("lifecycle") ||
            filter.includes("payload"),
        ),
        coverage: [
          "PQXDH ML-KEM-1024 prekey initialization required",
          "tampered prekey signatures rejected",
          "durable pairwise command/result route",
          "PC/Android/iPhone/CLI/runtime endpoint-kind command/result relay matrix",
          "Sesame-style multi-device fanout with independent pairwise envelopes",
          "server-visible pairwise relay header has an explicit public-field boundary and no payload canaries",
          "wrong-recipient pairwise fanout rejection",
          "ratchet message-key payload codec",
          "pairwise payload open failures do not advance receiver ratchet state",
          "authenticated automatic DH ratchet after remote ratchet",
          "old-chain in-flight recovery after ratchet",
          "stale and replayed relay ACKs do not advance ratchet state",
          "restart-safe pending authenticated ratchet state",
          "revoked session fail-closed for seal and open",
          "bounded skipped-key out-of-order open",
          "oversized skipped-key gaps reject before ratchet state advances",
          "server-substituted peer descriptors and tampered trust records rejected before command execution",
          "mobile FFI raw payload key/body actions are absent",
          "redacted TTL/delete/screenshot/resend/typing/read-receipt/ACK purge service actions",
          "lifecycle service actions seal only inside pairwise or MLS envelopes",
          "MLS product policy bindings for identity, welcome, roster, sender, commit, and one-time KeyPackage",
          "Key Transparency signed checkpoints with anti-equivocation; unsigned hash-chain non-authorizing",
          "ACP protected envelope AAD binding covers session/turn/operation/tool/permission/idempotency/policy fields",
          "ACP plaintext protected-payload relay is blocked as a production path",
        ],
      },
    ],
    reviewGate: {
      independentReviewRequired: true,
      releaseOwnerSignoffRequired: true,
      productionReadyAfterThisCorpus: false,
    },
  };
  corpus.corpusDigest = sha256Json({
    schemaVersion: corpus.schemaVersion,
    entries: corpus.entries,
    reviewGate: corpus.reviewGate,
  });
  corpus.signoffBinding = signoffBindingForCorpus(corpus);
  return corpus;
}
