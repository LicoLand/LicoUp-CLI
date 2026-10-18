import fs from "node:fs/promises";
import { normalizeSourceCheckFiles } from "./source-check-bundle.mjs";

const configUrl = new URL("../config/secure-mesh-encrypted-file-handoff.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-encrypted-file-handoff.json";
const schemaVersion = "licomesh.secure-mesh.encrypted-file-handoff-config.v1";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"(?!redacted|\[redacted\])[^"]{8,}"/u],
  ["plaintext_file_canary", /(?:private-file-canary|private-relative-canary|file-body-plaintext-secret-canary-content|settlement-private-file-canary|mobile-ffi-private-file-canary|private-cli-file-canary)/u]
]);
let cachedConfig;

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function dedupePreservingOrder(values) {
  return [...new Set(values)];
}

function normalizeCheckId(value, label) {
  const id = String(value || "").trim();
  if (!/^[a-z0-9][a-z0-9_-]{2,120}$/u.test(id)) {
    throw new Error(`Invalid Secure Mesh encrypted file handoff ${label}: ${id || "<empty>"}`);
  }
  return id;
}

function normalizeSafeSourceRef(value, label) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref ||
    ref.startsWith("/") ||
    ref.startsWith("file:") ||
    /^https?:\/\//iu.test(ref) ||
    ref.split("/").includes("..") ||
    !/^(?:apps|crates|docs|tools)\/.+\.(?:rs|mjs|dart|swift|kt|json|md)$/u.test(ref)) {
    throw new Error(`Invalid Secure Mesh encrypted file handoff ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokenList(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = dedupePreservingOrder(tokens.filter(Boolean));
  if (normalized.length === 0) {
    throw new Error(`Secure Mesh encrypted file handoff config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 260) {
      throw new Error(`Invalid Secure Mesh encrypted file handoff ${label}`);
    }
    assertNoLeak(token, `secure mesh encrypted file handoff ${label}`);
  }
  return normalized;
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh encrypted file handoff config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    const files = normalizeSourceCheckFiles(
      check,
      normalizeSafeSourceRef,
      `Secure Mesh encrypted file handoff source check ${index + 1}`
    );
    return {
      id: normalizeCheckId(check.id, `source check ${index + 1} id`),
      file: files[0],
      files,
      tokens: normalizeTokenList(check.tokens, `source check ${index + 1} tokens`)
    };
  });
  const ids = normalized.map((check) => check.id);
  if (new Set(ids).size !== ids.length) {
    throw new Error("Secure Mesh encrypted file handoff config source checks must have unique ids");
  }
  return normalized.map((check) => Object.freeze({
    ...check,
    files: Object.freeze(check.files),
    tokens: Object.freeze(check.tokens)
  }));
}

function normalizeNativeTestFilters(value) {
  const filters = normalizeTokenList(value, "native test filters");
  for (const filter of filters) {
    if (!/^[a-z0-9_]+$/u.test(filter)) {
      throw new Error(`Invalid Secure Mesh encrypted file handoff native test filter: ${filter}`);
    }
  }
  return Object.freeze(filters);
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh encrypted file handoff config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh encrypted file handoff config");
  cachedConfig = Object.freeze({
    configRef,
    schemaVersion,
    sourceChecks: Object.freeze(normalizeSourceChecks(payload.sourceChecks)),
    nativeTestFilters: normalizeNativeTestFilters(payload.nativeTestFilters)
  });
  return cachedConfig;
}

export async function loadSecureMeshEncryptedFileHandoffConfig() {
  return loadRawConfig();
}
