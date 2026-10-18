import { createHash, createPublicKey, sign as signPayload, verify as verifyPayload } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

import {
  SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM,
  SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_FIELD,
  SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_E2EE_AUTHORITY_TRUST_ROOT_SCHEMA_VERSION,
  SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY,
  SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
  canonicalSecureClientMeshAuthorityProofPayload,
  secureClientMeshEvidenceAuthorityForBlocker
} from "./secure-client-mesh-evidence-contract.mjs";

const PEM_BEGIN = "-----BEGIN ";
const PEM_END = "-----";
const SERVER_SCRIPTS_PREFIX = "tools/" + "server" + "-scripts/";
const PRIVATE_PEM_HEADER_PATTERN = new RegExp(
  `${PEM_BEGIN}[A-Z ]*PRIVATE KEY${PEM_END}|${PEM_BEGIN}OPENSSH PRIVATE KEY${PEM_END}`,
  "u"
);

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function pathInsideRoot(root, target) {
  const relative = path.relative(root, target);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

function sha256Digest(text = "") {
  return `sha256:${createHash("sha256").update(String(text), "utf8").digest("hex")}`;
}

function containsPrivateKeyPem(value = "") {
  return PRIVATE_PEM_HEADER_PATTERN.test(String(value || ""));
}

function privateMaterialFieldReasons(value = {}, label = "authority-trust-root") {
  const record = asRecord(value);
  return Object.keys(record)
    .filter((key) => key !== "publicKeyPem")
    .filter((key) => /private|secret|seed|passphrase|signingKey|rawKey|keyMaterial/iu.test(key))
    .map(() => `${label}-private-material-field`);
}

function normalizeTrustRootPublicKeyPem(value = "", algorithm = "") {
  const pem = String(value || "").trim();
  const reasons = [];
  if (!pem) {
    reasons.push("authority-trust-root-public-key-missing");
  }
  if (containsPrivateKeyPem(pem)) {
    reasons.push("authority-trust-root-private-key-material");
  }
  if (pem && (!pem.includes("-----BEGIN PUBLIC KEY-----") || !pem.includes("-----END PUBLIC KEY-----"))) {
    reasons.push("authority-trust-root-public-key-not-spki");
  }
  if (reasons.length > 0) {
    return { accepted: false, publicKeyPem: "", reasons };
  }
  try {
    const publicKey = createPublicKey(pem);
    if (algorithm === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM &&
      publicKey.asymmetricKeyType !== "ed25519") {
      return {
        accepted: false,
        publicKeyPem: "",
        reasons: ["authority-trust-root-public-key-algorithm-mismatch"]
      };
    }
    return {
      accepted: true,
      publicKeyPem: publicKey.export({ type: "spki", format: "pem" }),
      reasons: []
    };
  } catch {
    return {
      accepted: false,
      publicKeyPem: "",
      reasons: ["authority-trust-root-public-key-invalid"]
    };
  }
}

function optionalInstant(value = "", reason = "") {
  const text = String(value || "").trim();
  if (!text) {
    return { value: "", accepted: true, ms: null, reason: "" };
  }
  const ms = Date.parse(text);
  return Number.isFinite(ms)
    ? { value: new Date(ms).toISOString(), accepted: true, ms, reason: "" }
    : { value: text, accepted: false, ms: null, reason };
}

function normalizeKeyLifecycle(key = {}) {
  const notBefore = optionalInstant(key.notBefore, "authority-trust-root-key-not-before-invalid");
  const notAfter = optionalInstant(key.notAfter, "authority-trust-root-key-not-after-invalid");
  const revokedAt = optionalInstant(key.revokedAt, "authority-trust-root-key-revoked-at-invalid");
  const invalidReasons = [notBefore.reason, notAfter.reason, revokedAt.reason].filter(Boolean);
  return {
    accepted: invalidReasons.length === 0,
    notBefore: notBefore.value,
    notAfter: notAfter.value,
    revokedAt: revokedAt.value,
    issuer: String(key.issuer || "").trim(),
    audience: String(key.audience || "").trim(),
    invalidReasons,
    reasons: invalidReasons
  };
}

function trustRootKeyLifecycleReasonsForReport(key = {}, report = {}) {
  const record = asRecord(report);
  const summary = asRecord(record.summary);
  const checkedAt = String(record.checkedAt || summary.checkedAt || "").trim();
  const checkedAtMs = Date.parse(checkedAt);
  const reasons = [];
  if ((key.notBefore || key.notAfter || key.revokedAt) && !Number.isFinite(checkedAtMs)) {
    reasons.push("authority-proof-report-checked-at-invalid");
    return reasons;
  }
  const notBeforeMs = key.notBefore ? Date.parse(key.notBefore) : null;
  const notAfterMs = key.notAfter ? Date.parse(key.notAfter) : null;
  const revokedAtMs = key.revokedAt ? Date.parse(key.revokedAt) : null;
  if (Number.isFinite(notBeforeMs) && checkedAtMs < notBeforeMs) {
    reasons.push("authority-trust-root-key-not-yet-valid");
  }
  if (Number.isFinite(notAfterMs) && checkedAtMs > notAfterMs) {
    reasons.push("authority-trust-root-key-expired");
  }
  if (Number.isFinite(revokedAtMs) && checkedAtMs >= revokedAtMs) {
    reasons.push("authority-trust-root-key-revoked");
  }
  return reasons;
}

function trustRootKeyLifecycleReasonsForVerificationTime(key = {}, verificationNow = "") {
  const checkedAt = String(verificationNow || "").trim();
  const checkedAtMs = Date.parse(checkedAt);
  const reasons = [];
  if ((key.notBefore || key.notAfter || key.revokedAt) && !Number.isFinite(checkedAtMs)) {
    reasons.push("authority-proof-verification-now-invalid");
    return reasons;
  }
  const notBeforeMs = key.notBefore ? Date.parse(key.notBefore) : null;
  const notAfterMs = key.notAfter ? Date.parse(key.notAfter) : null;
  const revokedAtMs = key.revokedAt ? Date.parse(key.revokedAt) : null;
  if (Number.isFinite(notBeforeMs) && checkedAtMs < notBeforeMs) {
    reasons.push("authority-trust-root-key-currently-not-yet-valid");
  }
  if (Number.isFinite(notAfterMs) && checkedAtMs > notAfterMs) {
    reasons.push("authority-trust-root-key-currently-expired");
  }
  if (Number.isFinite(revokedAtMs) && checkedAtMs >= revokedAtMs) {
    reasons.push("authority-trust-root-key-currently-revoked");
  }
  return reasons;
}

function normalizeTrustRoot(root = {}) {
  const record = asRecord(root);
  const keys = Array.isArray(record.authorityKeys) ? record.authorityKeys : [record];
  const schemaAccepted = record.schemaVersion === SECURE_CLIENT_MESH_E2EE_AUTHORITY_TRUST_ROOT_SCHEMA_VERSION;
  const sourceOfTruthAccepted = record.sourceOfTruth === SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH;
  const rootPrivateMaterialReasons = [
    ...privateMaterialFieldReasons(record),
    containsPrivateKeyPem(JSON.stringify(record)) ? "authority-trust-root-private-key-material" : ""
  ].filter(Boolean);
  const normalizedKeys = keys.map((item) => {
    const key = asRecord(item);
    const authority = String(key.authority || "").trim();
    const keyId = String(key.keyId || "").trim();
    const algorithm = String(key.algorithm || "").trim();
    const publicKey = normalizeTrustRootPublicKeyPem(key.publicKeyPem, algorithm);
    const privateMaterialReasons = privateMaterialFieldReasons(key, "authority-trust-root-key");
    const lifecycle = normalizeKeyLifecycle(key);
    const accepted = publicKey.accepted === true &&
      privateMaterialReasons.length === 0 &&
      lifecycle.accepted === true;
    return {
      authority,
      keyId,
      algorithm,
      publicKeyPem: accepted ? publicKey.publicKeyPem : "",
      trusted: key.trusted !== false,
      accepted,
      notBefore: lifecycle.notBefore,
      notAfter: lifecycle.notAfter,
      revokedAt: lifecycle.revokedAt,
      issuer: lifecycle.issuer,
      audience: lifecycle.audience,
      lifecycleAccepted: lifecycle.accepted,
      reasons: [...publicKey.reasons, ...privateMaterialReasons, ...lifecycle.invalidReasons],
      lifecycleReasons: lifecycle.reasons
    };
  });
  const trustedKeyIds = new Set();
  const duplicateTrustedKeys = new Set();
  for (const key of normalizedKeys) {
    if (key.trusted === false) {
      continue;
    }
    const tuple = `${key.authority}\u0000${key.keyId}\u0000${key.algorithm}`;
    if (trustedKeyIds.has(tuple)) {
      duplicateTrustedKeys.add(tuple);
    }
    trustedKeyIds.add(tuple);
  }
  const duplicateReasons = duplicateTrustedKeys.size > 0
    ? ["authority-trust-root-duplicate-trusted-key"]
    : [];
  const keyReasons = normalizedKeys.flatMap((key) => key.reasons);
  return {
    provided: true,
    accepted: schemaAccepted &&
      sourceOfTruthAccepted &&
      rootPrivateMaterialReasons.length === 0 &&
      duplicateReasons.length === 0 &&
      normalizedKeys.every((key) => key.accepted === true || key.trusted === false),
    schemaAccepted,
    sourceOfTruthAccepted,
    keys: normalizedKeys,
    reasons: [
      schemaAccepted ? "" : "authority-trust-root-schema-mismatch",
      sourceOfTruthAccepted ? "" : "authority-trust-root-source-of-truth-mismatch",
      ...rootPrivateMaterialReasons,
      ...duplicateReasons,
      ...keyReasons
    ].filter(Boolean)
  };
}

export async function loadSecureClientMeshAuthorityTrustRoot(filePath = "", { evidenceRoot = "" } = {}) {
  const value = String(filePath || "").trim();
  if (!value) {
    return { provided: false, accepted: false, keys: [], reasons: ["authority-trust-root-not-provided"] };
  }
  const trustRootPath = path.resolve(value);
  if (evidenceRoot) {
    const rootPath = path.resolve(evidenceRoot);
    const realRoot = await fs.realpath(rootPath).catch(() => "");
    const realTrustRoot = await fs.realpath(trustRootPath).catch(() => "");
    if (pathInsideRoot(rootPath, trustRootPath) ||
      (realRoot && (pathInsideRoot(realRoot, trustRootPath) || (realTrustRoot && pathInsideRoot(realRoot, realTrustRoot))))) {
      return { provided: true, accepted: false, keys: [], reasons: ["authority-trust-root-inside-evidence-root"] };
    }
  }
  try {
    return normalizeTrustRoot(JSON.parse(await fs.readFile(trustRootPath, "utf8")));
  } catch {
    return { provided: true, accepted: false, keys: [], reasons: ["authority-trust-root-unreadable"] };
  }
}

function proofRequirement(blocker = "", report = {}) {
  const authority = secureClientMeshEvidenceAuthorityForBlocker(blocker);
  const verifier = String(asRecord(report).verifier || "").trim();
  const generatedBy = String(asRecord(report).generatedBy || "").trim();
  const serverProvenanceLike = verifier.startsWith(SERVER_SCRIPTS_PREFIX) ||
    generatedBy.startsWith(SERVER_SCRIPTS_PREFIX);
  const externalAuthority = authority.evidenceAuthorities.includes(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.externalClient) ||
    authority.evidenceAuthorities.includes(SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY.independentAudit);
  return !serverProvenanceLike && externalAuthority;
}

export function verifySecureClientMeshEvidenceAuthorityProof(report = {}, blocker = "", trustRoot = {}, options = {}) {
  if (!proofRequirement(blocker, report)) {
    return { required: false, accepted: true, trustRootAccepted: false, reasons: [] };
  }
  const rawRoot = asRecord(trustRoot);
  const root = Array.isArray(rawRoot.keys) ? rawRoot : normalizeTrustRoot(rawRoot);
  const payload = canonicalSecureClientMeshAuthorityProofPayload(report);
  const payloadDigest = sha256Digest(payload);
  const rawProof = asRecord(report)[SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_FIELD];
  const proofProvided = rawProof && typeof rawProof === "object" && !Array.isArray(rawProof) &&
    Object.keys(rawProof).length > 0;
  if (!proofProvided) {
    return {
      required: true,
      accepted: false,
      trustRootAccepted: false,
      payloadDigestAccepted: false,
      signatureAccepted: false,
      payloadDigest,
      authority: "",
      keyId: "",
      reasons: ["authority-proof-missing"]
    };
  }
  const proof = asRecord(rawProof);
  const authority = String(proof.authority || "").trim();
  const keyId = String(proof.keyId || "").trim();
  const algorithm = String(proof.algorithm || "").trim();
  const verificationNow = String(options.verificationNow || options.freshnessNow || new Date().toISOString()).trim();
  const matchingKeyAnyState = (Array.isArray(root.keys) ? root.keys : []).find((key) =>
    key.authority === authority &&
    key.keyId === keyId &&
    key.algorithm === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM
  );
  const matchingKeyLifecycleReasons = trustRootKeyLifecycleReasonsForReport(matchingKeyAnyState || {}, report);
  const matchingKeyCurrentLifecycleReasons = trustRootKeyLifecycleReasonsForVerificationTime(
    matchingKeyAnyState || {},
    verificationNow
  );
  const matchingKey = (Array.isArray(root.keys) ? root.keys : []).find((key) =>
    key.trusted !== false &&
    key.authority === authority &&
    key.keyId === keyId &&
    key.algorithm === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM &&
    trustRootKeyLifecycleReasonsForReport(key, report).length === 0 &&
    trustRootKeyLifecycleReasonsForVerificationTime(key, verificationNow).length === 0
  );
  const schemaAccepted = proof.schemaVersion === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_SCHEMA_VERSION;
  const sourceOfTruthAccepted = proof.sourceOfTruth === SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH;
  const algorithmAccepted = algorithm === SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM;
  const trustRootAccepted = root.accepted === true && Boolean(matchingKey?.publicKeyPem);
  const payloadDigestAccepted = String(proof.payloadDigest || "").trim() === payloadDigest;
  let signatureAccepted = false;
  if (schemaAccepted && sourceOfTruthAccepted && algorithmAccepted && trustRootAccepted && payloadDigestAccepted) {
    try {
      const key = createPublicKey(matchingKey.publicKeyPem);
      signatureAccepted = verifyPayload(null, Buffer.from(payload, "utf8"), key, Buffer.from(String(proof.signature || ""), "base64"));
    } catch {
      signatureAccepted = false;
    }
  }
  const accepted = schemaAccepted && sourceOfTruthAccepted && algorithmAccepted &&
    trustRootAccepted && payloadDigestAccepted && signatureAccepted;
  return {
    required: true,
    accepted,
    trustRootAccepted,
    payloadDigestAccepted,
    signatureAccepted,
    payloadDigest,
    authority,
    keyId,
    verificationNow,
    reasons: [
      schemaAccepted ? "" : "authority-proof-schema-mismatch",
      sourceOfTruthAccepted ? "" : "authority-proof-source-of-truth-mismatch",
      algorithmAccepted ? "" : "authority-proof-algorithm-mismatch",
      trustRootAccepted ? "" : "authority-trust-root-key-not-accepted",
      ...(trustRootAccepted ? [] : [].concat(root.reasons || [])),
      ...(trustRootAccepted ? [] : matchingKeyLifecycleReasons),
      ...(trustRootAccepted ? [] : matchingKeyCurrentLifecycleReasons),
      payloadDigestAccepted ? "" : "authority-proof-payload-digest-mismatch",
      signatureAccepted ? "" : "authority-proof-signature-invalid"
    ].filter(Boolean)
  };
}

export function attachSecureClientMeshEvidenceAuthorityProof(report = {}, { privateKeyPem, authority, keyId } = {}) {
  const payload = canonicalSecureClientMeshAuthorityProofPayload(report);
  const signature = signPayload(null, Buffer.from(payload, "utf8"), privateKeyPem).toString("base64");
  return {
    ...report,
    [SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_FIELD]: {
      schemaVersion: SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_SCHEMA_VERSION,
      sourceOfTruth: SECURE_CLIENT_MESH_PRODUCTION_SOURCE_OF_TRUTH,
      algorithm: SECURE_CLIENT_MESH_E2EE_AUTHORITY_PROOF_ALGORITHM,
      authority,
      keyId,
      payloadDigest: sha256Digest(payload),
      signature
    }
  };
}
