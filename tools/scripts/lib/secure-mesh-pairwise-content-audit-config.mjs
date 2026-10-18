import fs from "node:fs/promises";
import { normalizeSourceCheckFiles } from "./source-check-bundle.mjs";

const configUrl = new URL("../config/secure-mesh-pairwise-content-audit.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-pairwise-content-audit.json";
const schemaVersion = "licomesh.secure-mesh.pairwise-content-audit-config.v2";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_field", /(?:privateKey|sessionKey|rootKey|chainKey|messageKey|rawSecret|secretMaterial)/u]
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

function normalizeSafeReportRef(value, label) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref ||
    !ref.endsWith(".json") ||
    ref.startsWith("/") ||
    ref.startsWith("file:") ||
    /^https?:\/\//iu.test(ref) ||
    ref.split("/").includes("..") ||
    !(ref.startsWith("build/reports/") || ref.startsWith("build/client-cli-vm/"))) {
    throw new Error(`Invalid Secure Mesh pairwise/content audit ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeSafeSourceRef(value, label) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref ||
    ref.startsWith("/") ||
    ref.startsWith("file:") ||
    /^https?:\/\//iu.test(ref) ||
    ref.split("/").includes("..") ||
    !/^(?:apps|crates|tools)\/.+\.(?:rs|mjs|dart|swift|kt|json)$/u.test(ref)) {
    throw new Error(`Invalid Secure Mesh pairwise/content audit ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeEnvKeys(value) {
  const envKeys = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = envKeys.filter(Boolean);
  if (normalized.length === 0) {
    throw new Error("Secure Mesh pairwise/content audit config must define at least one review signoff env key");
  }
  for (const envKey of normalized) {
    if (!/^LICO_[A-Z0-9_]+$/u.test(envKey)) {
      throw new Error(`Invalid Secure Mesh pairwise/content audit env key: ${envKey}`);
    }
  }
  return [...new Set(normalized)];
}

function normalizeCheckId(value, label) {
  const id = String(value || "").trim();
  if (!/^[a-z0-9][a-z0-9_-]{2,120}$/u.test(id)) {
    throw new Error(`Invalid Secure Mesh pairwise/content audit ${label}: ${id || "<empty>"}`);
  }
  return id;
}

function normalizeTokenList(value, label, { required = true } = {}) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = tokens.filter(Boolean);
  if (required && normalized.length === 0) {
    throw new Error(`Secure Mesh pairwise/content audit config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 240) {
      throw new Error(`Invalid Secure Mesh pairwise/content audit ${label}`);
    }
    assertNoLeak(token, `secure mesh pairwise/content audit ${label}`);
  }
  return normalized;
}

function normalizeFunctionName(value, label) {
  const name = String(value || "").trim();
  if (!name) {
    return "";
  }
  if (!/^[A-Za-z_][A-Za-z0-9_]{1,120}$/u.test(name)) {
    throw new Error(`Invalid Secure Mesh pairwise/content audit ${label}: ${name}`);
  }
  return name;
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh pairwise/content audit config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    const files = normalizeSourceCheckFiles(
      check,
      normalizeSafeSourceRef,
      `Secure Mesh pairwise/content audit source check ${index + 1}`
    );
    return {
      id: normalizeCheckId(check.id, `source check ${index + 1} id`),
      file: files[0],
      files,
      ...(normalizeFunctionName(check.functionName, `source check ${index + 1} function`) ?
        { functionName: normalizeFunctionName(check.functionName, `source check ${index + 1} function`) } :
        {}),
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
    throw new Error("Secure Mesh pairwise/content audit config source checks must have unique ids");
  }
  return normalized;
}

function normalizeNativeTestFilters(value) {
  const filters = normalizeTokenList(value, "native test filters");
  for (const filter of filters) {
    if (!/^[a-z0-9_]+$/u.test(filter)) {
      throw new Error(`Invalid Secure Mesh pairwise/content audit native test filter: ${filter}`);
    }
  }
  if (new Set(filters).size !== filters.length) {
    throw new Error("Secure Mesh pairwise/content audit config native test filters must be unique");
  }
  return filters;
}

function configuredReviewSignoffRef(reviewSignoff) {
  for (const envKey of reviewSignoff.envKeys) {
    const value = process.env[envKey];
    if (value && String(value).trim()) {
      return {
        ref: normalizeSafeReportRef(value, `${envKey} review signoff override ref`),
        source: "env",
        envKey
      };
    }
  }
  return {
    ref: reviewSignoff.ref,
    source: "config",
    envKey: ""
  };
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh pairwise/content audit config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh pairwise/content audit config");
  const reportOutput = normalizeSafeReportRef(payload.reportOutput, "report output ref");
  const vectorCorpusOutput = normalizeSafeReportRef(payload.vectorCorpusOutput, "vector corpus output ref");
  const reviewSignoffRaw = asRecord(payload.reviewSignoff);
  const reviewSignoff = {
    envKeys: normalizeEnvKeys(reviewSignoffRaw.envKeys),
    ref: normalizeSafeReportRef(reviewSignoffRaw.ref, "review signoff default ref")
  };
  const sourceChecks = normalizeSourceChecks(payload.sourceChecks);
  const nativeTestFilters = normalizeNativeTestFilters(payload.nativeTestFilters);
  const refs = [reportOutput, vectorCorpusOutput, reviewSignoff.ref];
  if (new Set(refs).size !== refs.length) {
    throw new Error("Secure Mesh pairwise/content audit config must use distinct report refs");
  }
  const reviewSignoffResolved = configuredReviewSignoffRef(reviewSignoff);
  if ([reportOutput, vectorCorpusOutput].includes(reviewSignoffResolved.ref)) {
    throw new Error("Secure Mesh pairwise/content audit review signoff must not overwrite verifier outputs");
  }
  cachedConfig = {
    configRef,
    schemaVersion,
    reportOutput,
    vectorCorpusOutput,
    reviewSignoff,
    reviewSignoffRef: reviewSignoffResolved.ref,
    reviewSignoffRefSource: reviewSignoffResolved.source,
    reviewSignoffEnvKey: reviewSignoffResolved.envKey,
    sourceChecks,
    nativeTestFilters
  };
  return cachedConfig;
}

export async function loadSecureMeshPairwiseContentAuditConfig() {
  return loadRawConfig();
}
