import fs from "node:fs/promises";
import { normalizeSourceCheckFiles } from "./source-check-bundle.mjs";

const configUrl = new URL("../config/secure-mesh-trust-ux.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-trust-ux.json";
const schemaVersion = "licomesh.secure-mesh.trust-ux-config.v2";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["adb_public_key", /AAAA[0-9A-Za-z+/]{40,}={0,2}/u],
  ["device_identifier", /\b(?:UDID|ECID|Serial(?:Number)?|DeviceIdentifier)\s*[:=]\s*[A-Za-z0-9-]{8,}\b/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey|rawSecret|secretMaterial)"\s*:\s*"(?!redacted|\[redacted\])[^"]{8,}"/u]
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
    throw new Error(`Invalid Secure Mesh trust UX ${label}: ${id || "<empty>"}`);
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
    throw new Error(`Invalid Secure Mesh trust UX ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function dedupeTokens(tokens) {
  const seen = new Set();
  const deduped = [];
  for (const token of tokens) {
    if (!seen.has(token)) {
      seen.add(token);
      deduped.push(token);
    }
  }
  return deduped;
}

function normalizeTokenList(value, label, { required = true, maxLength = 260 } = {}) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = dedupeTokens(tokens.filter(Boolean));
  if (required && normalized.length === 0) {
    throw new Error(`Secure Mesh trust UX config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > maxLength) {
      throw new Error(`Invalid Secure Mesh trust UX ${label}`);
    }
    assertNoLeak(token, `secure mesh trust UX ${label}`);
  }
  return normalized;
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh trust UX config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    const files = normalizeSourceCheckFiles(
      check,
      normalizeSafeSourceRef,
      `Secure Mesh trust UX source check ${index + 1}`
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
    throw new Error("Secure Mesh trust UX config source checks must have unique ids");
  }
  return normalized;
}

function normalizeNativeTestFilters(value) {
  const filters = normalizeTokenList(value, "native test filters");
  for (const filter of filters) {
    if (!/^[a-z0-9_]+$/u.test(filter)) {
      throw new Error(`Invalid Secure Mesh trust UX native test filter: ${filter}`);
    }
  }
  return filters;
}

function normalizeExpectedMobileNativeTrustActions(value) {
  const actions = normalizeTokenList(value, "expected mobile native trust actions");
  for (const action of actions) {
    if (!/^secure_mesh\.[A-Za-z0-9_.]{3,120}$/u.test(action)) {
      throw new Error(`Invalid Secure Mesh trust UX mobile native trust action: ${action}`);
    }
  }
  return actions;
}

function normalizeProductTestTargets(value) {
  const targets = normalizeTokenList(value, "product test targets")
    .map((target) => normalizeSafeSourceRef(target, "product test target"));
  for (const target of targets) {
    if (!target.startsWith("apps/desktop/test/") || !target.endsWith(".dart")) {
      throw new Error(`Invalid Secure Mesh trust UX product test target: ${target}`);
    }
  }
  return targets;
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh trust UX config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh trust UX config");
  cachedConfig = {
    configRef,
    schemaVersion,
    sourceChecks: normalizeSourceChecks(payload.sourceChecks),
    nativeTestFilters: normalizeNativeTestFilters(payload.nativeTestFilters),
    productTestTargets: normalizeProductTestTargets(payload.productTestTargets),
    expectedMobileNativeTrustActions: normalizeExpectedMobileNativeTrustActions(
      payload.expectedMobileNativeTrustActions
    )
  };
  assertNoLeak(cachedConfig, "secure mesh trust UX normalized config");
  return cachedConfig;
}

export async function loadSecureMeshTrustUxConfig() {
  return loadRawConfig();
}
