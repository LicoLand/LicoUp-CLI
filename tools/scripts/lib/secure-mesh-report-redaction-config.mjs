import fs from "node:fs/promises";

const configUrl = new URL("../config/secure-mesh-report-redaction.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-report-redaction.json";
const schemaVersion = "licomesh.secure-mesh.report-redaction-config.v1";
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
    throw new Error(`Invalid Secure Mesh redaction ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function uniqueRefs(values, label) {
  const refs = [].concat(values || []).map((value) => normalizeSafeReportRef(value, label));
  const seen = new Set();
  const duplicates = [];
  for (const ref of refs) {
    if (seen.has(ref)) {
      duplicates.push(ref);
      continue;
    }
    seen.add(ref);
  }
  if (duplicates.length > 0) {
    throw new Error(`Duplicate Secure Mesh redaction ${label}: ${duplicates.join(", ")}`);
  }
  return refs;
}

function normalizeMode(value, label) {
  const mode = asRecord(value);
  const requiredRefs = uniqueRefs(mode.requiredRefs, `${label} required ref`);
  const optionalRefs = uniqueRefs(mode.optionalRefs, `${label} optional ref`);
  const deferredGraphRefs = uniqueRefs(mode.deferredGraphRefs, `${label} deferred graph ref`);
  if (requiredRefs.length === 0) {
    throw new Error(`Secure Mesh redaction mode ${label} must define required refs`);
  }
  const requiredSet = new Set(requiredRefs);
  const overlappingOptional = optionalRefs.filter((ref) => requiredSet.has(ref));
  if (overlappingOptional.length > 0) {
    throw new Error(`Secure Mesh redaction mode ${label} has optional refs already required: ${overlappingOptional.join(", ")}`);
  }
  const overlappingDeferred = deferredGraphRefs.filter((ref) => requiredSet.has(ref));
  if (overlappingDeferred.length > 0) {
    throw new Error(`Secure Mesh redaction mode ${label} defers refs already required: ${overlappingDeferred.join(", ")}`);
  }
  return {
    requiredRefs,
    optionalRefs,
    deferredGraphRefs
  };
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const config = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (config?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh redaction config schema version mismatch");
  }
  assertNoLeak(config, "secure mesh redaction config");
  const reportOutputs = asRecord(config.reportOutputs);
  const modes = asRecord(config.modes);
  const normalized = {
    configRef,
    schemaVersion,
    reportOutputs: {
      default: normalizeSafeReportRef(reportOutputs.default, "default output ref"),
      releaseProofInputs: normalizeSafeReportRef(reportOutputs.releaseProofInputs, "release proof output ref")
    },
    modes: {
      default: normalizeMode(modes.default, "default"),
      releaseProofInputs: normalizeMode(modes.releaseProofInputs, "releaseProofInputs")
    }
  };
  for (const [modeName, mode] of Object.entries(normalized.modes)) {
    if (mode.requiredRefs.includes(normalized.reportOutputs.default) ||
      mode.requiredRefs.includes(normalized.reportOutputs.releaseProofInputs)) {
      throw new Error(`Secure Mesh redaction mode ${modeName} must not scan verifier output reports as required input`);
    }
  }
  cachedConfig = normalized;
  return cachedConfig;
}

export async function loadSecureMeshReportRedactionConfig() {
  return loadRawConfig();
}
