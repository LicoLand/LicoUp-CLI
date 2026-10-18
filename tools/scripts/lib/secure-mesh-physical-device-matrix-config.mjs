import fs from "node:fs/promises";

const configUrl = new URL("../config/secure-mesh-physical-device-matrix.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-physical-device-matrix.json";
const schemaVersion = "licomesh.secure-mesh.physical-device-matrix-config.v2";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["adb_public_key", /AAAA[0-9A-Za-z+/]{40,}={0,2}/u],
  ["device_identifier", /\b(?:UDID|ECID|Serial(?:Number)?|DeviceIdentifier)\s*[:=]\s*[A-Za-z0-9-]{8,}\b/u],
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
    throw new Error(`Invalid Secure Mesh physical-device matrix ${label}: ${id || "<empty>"}`);
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
    !/^(?:apps|crates|docs|tools)\/.+\.(?:rs|mjs|dart|swift|kt|json|md|entitlements)$/u.test(ref)) {
    throw new Error(`Invalid Secure Mesh physical-device matrix ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokenList(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = tokens.filter(Boolean);
  if (normalized.length === 0) {
    throw new Error(`Secure Mesh physical-device matrix config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 260) {
      throw new Error(`Invalid Secure Mesh physical-device matrix ${label}`);
    }
    assertNoLeak(token, `secure mesh physical-device matrix ${label}`);
  }
  if (new Set(normalized).size !== normalized.length) {
    throw new Error(`Secure Mesh physical-device matrix ${label} must be unique`);
  }
  return normalized;
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh physical-device matrix config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    const refs = Array.isArray(check.files) ? check.files : [check.file];
    if (refs.length === 0) {
      throw new Error(`Secure Mesh physical-device matrix config must define source check ${index + 1} files`);
    }
    const files = refs.map((ref, fileIndex) => normalizeSafeSourceRef(
      ref,
      `source check ${index + 1} file ${fileIndex + 1}`
    ));
    return {
      id: normalizeCheckId(check.id, `source check ${index + 1} id`),
      file: files[0],
      files,
      tokens: normalizeTokenList(check.tokens, `source check ${index + 1} tokens`)
    };
  });
  const ids = normalized.map((check) => check.id);
  if (new Set(ids).size !== ids.length) {
    throw new Error("Secure Mesh physical-device matrix config source checks must have unique ids");
  }
  return normalized;
}

function normalizePhysicalMatrix(value) {
  const entries = Array.isArray(value) ? value : [];
  if (entries.length === 0) {
    throw new Error("Secure Mesh physical-device matrix config must define physical matrix entries");
  }
  const normalized = entries.map((item, index) => {
    const entry = asRecord(item);
    const scenario = normalizeCheckId(entry.scenario, `physical matrix entry ${index + 1} scenario`);
    const requiredPlatforms = normalizeTokenList(
      entry.requiredPlatforms,
      `physical matrix entry ${scenario} required platforms`
    );
    const status = String(entry.status || "").trim();
    if (!["missing", "partial", "blocked", "ready"].includes(status)) {
      throw new Error(`Invalid Secure Mesh physical-device matrix status for ${scenario}: ${status || "<empty>"}`);
    }
    const requiredAssertions = normalizeTokenList(
      entry.requiredAssertions,
      `physical matrix entry ${scenario} required assertions`
    );
    return {
      scenario,
      requiredPlatforms,
      status,
      requiredAssertions
    };
  });
  const scenarios = normalized.map((entry) => entry.scenario);
  if (new Set(scenarios).size !== scenarios.length) {
    throw new Error("Secure Mesh physical-device matrix scenarios must be unique");
  }
  return normalized;
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh physical-device matrix config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh physical-device matrix config");
  cachedConfig = {
    configRef,
    schemaVersion,
    sourceChecks: normalizeSourceChecks(payload.sourceChecks),
    physicalMatrix: normalizePhysicalMatrix(payload.physicalMatrix)
  };
  return cachedConfig;
}

export async function loadSecureMeshPhysicalDeviceMatrixConfig() {
  return loadRawConfig();
}
