import fs from "node:fs/promises";

const configUrl = new URL("../config/secure-mesh-windows-implementation.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-windows-implementation.json";
const schemaVersion = "licomesh.secure-mesh.windows-implementation-config.v1";
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
    throw new Error(`Invalid Secure Mesh Windows implementation ${label}: ${id || "<empty>"}`);
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
    !/^(?:(?:apps|crates|docs|tests|tools)\/.+\.(?:rs|mjs|dart|swift|kt|json|md)|\.github\/workflows\/.+\.ya?ml)$/u.test(ref)) {
    throw new Error(`Invalid Secure Mesh Windows implementation ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokenList(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = tokens.filter(Boolean);
  if (normalized.length === 0) {
    throw new Error(`Secure Mesh Windows implementation config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 260) {
      throw new Error(`Invalid Secure Mesh Windows implementation ${label}`);
    }
    assertNoLeak(token, `secure mesh Windows implementation ${label}`);
  }
  if (new Set(normalized).size !== normalized.length) {
    throw new Error(`Secure Mesh Windows implementation ${label} must be unique`);
  }
  return normalized;
}

function normalizeOptionalTokenList(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = tokens.filter(Boolean);
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 260) {
      throw new Error(`Invalid Secure Mesh Windows implementation ${label}`);
    }
    assertNoLeak(token, `secure mesh Windows implementation ${label}`);
  }
  if (new Set(normalized).size !== normalized.length) {
    throw new Error(`Secure Mesh Windows implementation ${label} must be unique`);
  }
  return normalized;
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh Windows implementation config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    return {
      id: normalizeCheckId(check.id, `source check ${index + 1} id`),
      file: normalizeSafeSourceRef(check.file, `source check ${index + 1} file`),
      tokens: normalizeTokenList(check.tokens, `source check ${index + 1} tokens`),
      forbiddenTokens: normalizeOptionalTokenList(
        check.forbiddenTokens,
        `source check ${index + 1} forbidden tokens`
      )
    };
  });
  const ids = normalized.map((check) => check.id);
  if (new Set(ids).size !== ids.length) {
    throw new Error("Secure Mesh Windows implementation config source checks must have unique ids");
  }
  return normalized;
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh Windows implementation config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh Windows implementation config");
  cachedConfig = {
    configRef,
    schemaVersion,
    sourceChecks: normalizeSourceChecks(payload.sourceChecks)
  };
  return cachedConfig;
}

export async function loadSecureMeshWindowsImplementationConfig() {
  return loadRawConfig();
}
