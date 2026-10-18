import fs from "node:fs/promises";
import { normalizeSourceCheckFiles } from "./source-check-bundle.mjs";

const configUrl = new URL("../config/secure-mesh-platform-secret-store-matrix.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-platform-secret-store-matrix.json";
const schemaVersion = "licomesh.secure-mesh.platform-secret-store-matrix-config.v2";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKey|sessionKey|rootKey|chainKey|messageKey|rawSecret|secretMaterial)"\s*:\s*"(?!redacted|\[redacted\])[^"]{8,}"/u]
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

function normalizeCheckId(value, label) {
  const id = String(value || "").trim();
  if (!/^[a-z0-9][a-z0-9_-]{2,120}$/u.test(id)) {
    throw new Error(`Invalid Secure Mesh platform secret-store ${label}: ${id || "<empty>"}`);
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
    !/^(?:apps|crates|tools)\/.+\.(?:rs|mjs|dart|swift|kt|json|entitlements)$/u.test(ref)) {
    throw new Error(`Invalid Secure Mesh platform secret-store ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokenList(value, label, { required = true } = {}) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = tokens.filter(Boolean);
  if (required && normalized.length === 0) {
    throw new Error(`Secure Mesh platform secret-store config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 240) {
      throw new Error(`Invalid Secure Mesh platform secret-store ${label}`);
    }
    assertNoLeak(token, `secure mesh platform secret-store ${label}`);
  }
  return normalized;
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh platform secret-store config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    const files = normalizeSourceCheckFiles(
      check,
      normalizeSafeSourceRef,
      `Secure Mesh platform secret-store source check ${index + 1}`,
    );
    return {
      id: normalizeCheckId(check.id, `source check ${index + 1} id`),
      file: files[0],
      files,
      tokens: normalizeTokenList(check.tokens, `source check ${index + 1} tokens`, {
        required: !Array.isArray(check.forbiddenTokens) || check.forbiddenTokens.length === 0
      }),
      forbiddenTokens: normalizeTokenList(
        check.forbiddenTokens,
        `source check ${index + 1} forbidden tokens`,
        { required: false }
      )
    };
  });
  const ids = normalized.map((check) => check.id);
  if (new Set(ids).size !== ids.length) {
    throw new Error("Secure Mesh platform secret-store config source checks must have unique ids");
  }
  return normalized.map((check) => ({
    ...check,
    files: Object.freeze(check.files),
    tokens: Object.freeze(check.tokens),
    forbiddenTokens: Object.freeze(check.forbiddenTokens),
  }));
}

function normalizeNativeTestFilters(value) {
  const filters = normalizeTokenList(value, "native test filters");
  for (const filter of filters) {
    if (!/^[a-z0-9_]+$/u.test(filter)) {
      throw new Error(`Invalid Secure Mesh platform secret-store native test filter: ${filter}`);
    }
  }
  if (new Set(filters).size !== filters.length) {
    throw new Error("Secure Mesh platform secret-store config native test filters must be unique");
  }
  return filters;
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh platform secret-store matrix config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh platform secret-store matrix config");
  cachedConfig = {
    configRef,
    schemaVersion,
    sourceChecks: normalizeSourceChecks(payload.sourceChecks),
    nativeTestFilters: normalizeNativeTestFilters(payload.nativeTestFilters)
  };
  return cachedConfig;
}

export async function loadSecureMeshPlatformSecretStoreMatrixConfig() {
  return loadRawConfig();
}
