import { createHash, createPublicKey, verify } from "node:crypto";

const SHA256 = /^sha256:[a-f0-9]{64}$/u;
const KEY_ID = /^[a-z0-9][a-z0-9._-]{2,63}$/u;

function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(bytes) {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

function decodeCanonicalBase64(value, maximumBytes = 16 * 1024) {
  const encoded = String(value || "").trim();
  if (!encoded || encoded.length > maximumBytes ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(encoded)) {
    return Buffer.alloc(0);
  }
  const bytes = Buffer.from(encoded, "base64");
  return bytes.length > 0 && bytes.toString("base64") === encoded
    ? bytes
    : Buffer.alloc(0);
}

function isoMilliseconds(value) {
  const milliseconds = Date.parse(String(value || ""));
  return Number.isFinite(milliseconds) ? milliseconds : Number.NaN;
}

function trustedKeyRecord(records, signer) {
  if (!Array.isArray(records) || !signer || typeof signer !== "object") return null;
  const keyId = String(signer.keyId || "").trim();
  const authority = String(signer.authority || "").trim();
  if (!KEY_ID.test(keyId) || !authority) return null;
  const record = records.find((candidate) =>
    candidate?.keyId === keyId && candidate?.authority === authority);
  if (!record) return null;
  const publicKeyDer = decodeCanonicalBase64(record.publicKeySpkiBase64);
  if (!publicKeyDer.length || !SHA256.test(String(record.publicKeyFingerprint || "")) ||
    sha256(publicKeyDer) !== record.publicKeyFingerprint) return null;
  try {
    const publicKey = createPublicKey({ key: publicKeyDer, type: "spki", format: "der" });
    return publicKey.asymmetricKeyType === "ed25519"
      ? { ...record, publicKey, publicKeyDer }
      : null;
  } catch {
    return null;
  }
}

function decisionBinding(binding, signoff) {
  return {
    schemaVersion: "licomesh.secure-mesh.pairwise-content-review-signoff.v2",
    sourceOfTruth: String(signoff?.sourceOfTruth || ""),
    corpusSchemaVersion: String(binding?.corpusSchemaVersion || ""),
    corpusDigest: String(binding?.corpusDigest || ""),
    corpusEntryCount: Number(binding?.corpusEntryCount || 0),
    corpusEntryIdsDigest: String(binding?.corpusEntryIdsDigest || ""),
    sourceCheckCount: Number(binding?.sourceCheckCount || 0),
    sourceCheckIdsDigest: String(binding?.sourceCheckIdsDigest || ""),
    nativeTestFilterCount: Number(binding?.nativeTestFilterCount || 0),
    nativeTestFiltersDigest: String(binding?.nativeTestFiltersDigest || ""),
    sourceStateDigest: String(binding?.sourceStateDigest || ""),
    producerSourceDigest: String(binding?.producerSourceDigest || ""),
    independentCryptographicReviewComplete:
      signoff?.independentCryptographicReviewComplete === true,
    releaseOwnerSignoffComplete: signoff?.releaseOwnerSignoffComplete === true,
    releaseDecision: String(signoff?.releaseDecision || ""),
    productionReadyClaimed: signoff?.productionReadyClaimed === true,
  };
}

export function pairwiseReviewStatementBytes({
  binding,
  signoff,
  role,
  signer,
  reviewerSignatureDigest = "",
}) {
  const statement = {
    ...decisionBinding(binding, signoff),
    role,
    authority: String(signer?.authority || ""),
    keyId: String(signer?.keyId || ""),
    signedAt: String(signer?.signedAt || ""),
    ...(role === "release-owner" ? { reviewerSignatureDigest } : {}),
  };
  return Buffer.from(canonicalJson(statement), "utf8");
}

function bindingReady(binding) {
  return binding?.schemaVersion ===
      "licomesh.secure-mesh.pairwise-content-review-signoff-binding.v2" &&
    binding?.corpusSchemaVersion ===
      "licomesh.secure-mesh.pairwise-content-vector-corpus.v1" &&
    SHA256.test(String(binding?.corpusDigest || "")) &&
    Number.isInteger(binding?.corpusEntryCount) && binding.corpusEntryCount > 0 &&
    SHA256.test(String(binding?.corpusEntryIdsDigest || "")) &&
    Number.isInteger(binding?.sourceCheckCount) && binding.sourceCheckCount > 0 &&
    SHA256.test(String(binding?.sourceCheckIdsDigest || "")) &&
    Number.isInteger(binding?.nativeTestFilterCount) &&
      binding.nativeTestFilterCount > 0 &&
    SHA256.test(String(binding?.nativeTestFiltersDigest || "")) &&
    SHA256.test(String(binding?.sourceStateDigest || "")) &&
    SHA256.test(String(binding?.producerSourceDigest || ""));
}

export function verifyPairwiseReviewSignoff({ binding, signoff, authorities }) {
  const base = {
    bindingReady: false,
    decisionReady: false,
    authoritiesDistinct: false,
    reviewerSignatureVerified: false,
    releaseOwnerSignatureVerified: false,
    sourceStateDigestBound: false,
    producerSourceDigestBound: false,
    ready: false,
  };
  try {
    base.bindingReady = bindingReady(binding);
    base.sourceStateDigestBound = SHA256.test(String(binding?.sourceStateDigest || ""));
    base.producerSourceDigestBound = SHA256.test(String(binding?.producerSourceDigest || ""));
    base.decisionReady = signoff?.schemaVersion ===
        "licomesh.secure-mesh.pairwise-content-review-signoff.v2" &&
      signoff?.sourceOfTruth &&
      signoff?.independentCryptographicReviewComplete === true &&
      signoff?.releaseOwnerSignoffComplete === true &&
      signoff?.releaseDecision === "approved_for_release_gate" &&
      signoff?.productionReadyClaimed === false;
    const reviewer = trustedKeyRecord(authorities?.reviewerKeys, signoff?.reviewer);
    const owner = trustedKeyRecord(authorities?.releaseOwnerKeys, signoff?.releaseOwner);
    const reviewerSignedAt = isoMilliseconds(signoff?.reviewer?.signedAt);
    const ownerSignedAt = isoMilliseconds(signoff?.releaseOwner?.signedAt);
    const reviewerSignature = decodeCanonicalBase64(signoff?.reviewer?.signatureBase64);
    const ownerSignature = decodeCanonicalBase64(signoff?.releaseOwner?.signatureBase64);
    base.authoritiesDistinct = Boolean(reviewer && owner &&
      reviewer.keyId !== owner.keyId && reviewer.authority !== owner.authority &&
      !reviewer.publicKeyDer.equals(owner.publicKeyDer));
    base.reviewerSignatureVerified = base.bindingReady && base.decisionReady &&
      Boolean(reviewer) && reviewerSignature.length === 64 &&
      Number.isFinite(reviewerSignedAt) && verify(
        null,
        pairwiseReviewStatementBytes({
          binding,
          signoff,
          role: "independent-reviewer",
          signer: signoff.reviewer,
        }),
        reviewer.publicKey,
        reviewerSignature,
      );
    const reviewerSignatureDigest = reviewerSignature.length === 64
      ? sha256(reviewerSignature)
      : "";
    base.releaseOwnerSignatureVerified = base.reviewerSignatureVerified &&
      base.authoritiesDistinct && Boolean(owner) && ownerSignature.length === 64 &&
      Number.isFinite(ownerSignedAt) && ownerSignedAt >= reviewerSignedAt && verify(
        null,
        pairwiseReviewStatementBytes({
          binding,
          signoff,
          role: "release-owner",
          signer: signoff.releaseOwner,
          reviewerSignatureDigest,
        }),
        owner.publicKey,
        ownerSignature,
      );
    base.ready = base.bindingReady && base.decisionReady &&
      base.authoritiesDistinct && base.reviewerSignatureVerified &&
      base.releaseOwnerSignatureVerified && base.sourceStateDigestBound &&
      base.producerSourceDigestBound;
    return Object.freeze(base);
  } catch {
    return Object.freeze(base);
  }
}

export function reviewSignatureDigest(signatureBase64) {
  const signature = decodeCanonicalBase64(signatureBase64);
  return signature.length === 64 ? sha256(signature) : "";
}
