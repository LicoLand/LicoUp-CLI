import fs from "node:fs/promises";
import { normalizeSourceCheckFiles } from "./source-check-bundle.mjs";

const configUrl = new URL("../config/secure-mesh-release-proof.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-release-proof.json";
const schemaVersion = "licomesh.secure-mesh.release-proof-config.v1";
const requiredInputReportKeys = Object.freeze([
  "updateRelease",
  "physicalMatrix",
  "androidPhysicalInstallLaunch",
  "physicalEvidenceManifest",
  "windowsImplementation",
  "reportRedaction",
  "stationAcceptance",
  "rustCrypto",
  "platformCrypto",
  "androidPlatformCrypto"
]);
const requiredVerifierCommandKeys = Object.freeze([
  "updateRelease",
  "physicalEvidenceManifest",
  "reportRedaction"
]);
const requiredFreshnessWindowKeys = Object.freeze([
  "updateReleaseSeconds",
  "physicalMatrixSeconds",
  "androidPhysicalInstallLaunchSeconds",
  "physicalEvidenceManifestSeconds"
]);
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
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeIdentifier(value, label) {
  const identifier = String(value || "").trim();
  if (!/^[A-Z0-9_]+$/u.test(identifier)) {
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${identifier || "<empty>"}`);
  }
  return identifier;
}

function normalizeCommandId(value, label) {
  const id = String(value || "").trim();
  if (!/^[a-z0-9:_-]+$/u.test(id) || id.includes("..")) {
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${id || "<empty>"}`);
  }
  return id;
}

function normalizeCheckId(value, label) {
  const id = String(value || "").trim();
  if (!/^[a-z0-9][a-z0-9_-]{2,120}$/u.test(id)) {
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${id || "<empty>"}`);
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
    !/^(?:apps|crates|docs|tests|tools)\/.+\.(?:rs|mjs|dart|swift|kt|json|md|entitlements)$/u.test(ref)) {
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokenList(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()) : [];
  const normalized = tokens.filter(Boolean);
  if (normalized.length === 0) {
    throw new Error(`Secure Mesh release proof config must define ${label}`);
  }
  for (const token of normalized) {
    if (/[\r\n]/u.test(token) || token.length > 260) {
      throw new Error(`Invalid Secure Mesh release proof ${label}`);
    }
    assertNoLeak(token, `secure mesh release proof ${label}`);
  }
  if (new Set(normalized).size !== normalized.length) {
    throw new Error(`Secure Mesh release proof ${label} must be unique`);
  }
  return normalized;
}

function normalizeVerifierScript(value, label) {
  const script = String(value || "").trim().replaceAll("\\", "/");
  if (!script ||
    script.startsWith("/") ||
    script.startsWith("file:") ||
    /^https?:\/\//iu.test(script) ||
    script.split("/").includes("..") ||
    !script.endsWith(".mjs") ||
    !(script.startsWith("tests/") || script.startsWith("tools/scripts/"))) {
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${script || "<empty>"}`);
  }
  return script;
}

function normalizeVerifierArg(value, label) {
  const arg = String(value || "").trim();
  if (!/^--[a-z0-9:-]+$/u.test(arg)) {
    throw new Error(`Invalid Secure Mesh release proof ${label}: ${arg || "<empty>"}`);
  }
  return arg;
}

function renderVerifierCommand(command) {
  return ["node", command.script, ...command.args].join(" ");
}

function normalizeVerifierCommand(value, key) {
  const command = asRecord(value);
  if (String(command.runner || "").trim() !== "node") {
    throw new Error(`Invalid Secure Mesh release proof ${key} verifier runner`);
  }
  const normalized = {
    id: normalizeCommandId(command.id, `${key} verifier id`),
    runner: "node",
    script: normalizeVerifierScript(command.script, `${key} verifier script`),
    args: [].concat(command.args || []).map((arg) => normalizeVerifierArg(arg, `${key} verifier arg`))
  };
  if (Object.prototype.hasOwnProperty.call(command, "runIdEnv")) {
    normalized.runIdEnv = normalizeIdentifier(command.runIdEnv, `${key} verifier run id env`);
  }
  normalized.command = renderVerifierCommand(normalized);
  assertNoLeak(normalized, `${key} verifier command`);
  return normalized;
}

function normalizeVerifierCommands(value) {
  const verifierCommands = asRecord(value);
  const unknown = Object.keys(verifierCommands).filter((key) => !requiredVerifierCommandKeys.includes(key));
  if (unknown.length > 0) {
    throw new Error(`Secure Mesh release proof config contains unknown verifier command keys: ${unknown.join(", ")}`);
  }
  const missing = requiredVerifierCommandKeys.filter((key) => !Object.prototype.hasOwnProperty.call(verifierCommands, key));
  if (missing.length > 0) {
    throw new Error(`Secure Mesh release proof config is missing verifier command keys: ${missing.join(", ")}`);
  }
  const normalized = Object.fromEntries(requiredVerifierCommandKeys.map((key) => [
    key,
    normalizeVerifierCommand(verifierCommands[key], key)
  ]));
  const ids = new Set(Object.values(normalized).map((command) => command.id));
  if (ids.size !== requiredVerifierCommandKeys.length) {
    throw new Error("Secure Mesh release proof config verifier command ids must be unique");
  }
  if (normalized.reportRedaction.runIdEnv !== "LICO_SECURE_MESH_REDACTION_RUN_ID") {
    throw new Error("Secure Mesh release proof report redaction verifier must receive the configured redaction run id env");
  }
  return normalized;
}

function normalizeInputReports(value) {
  const inputReports = asRecord(value);
  const unknown = Object.keys(inputReports).filter((key) => !requiredInputReportKeys.includes(key));
  if (unknown.length > 0) {
    throw new Error(`Secure Mesh release proof config contains unknown input report keys: ${unknown.join(", ")}`);
  }
  const missing = requiredInputReportKeys.filter((key) => !Object.prototype.hasOwnProperty.call(inputReports, key));
  if (missing.length > 0) {
    throw new Error(`Secure Mesh release proof config is missing input report keys: ${missing.join(", ")}`);
  }
  return Object.fromEntries(requiredInputReportKeys.map((key) => [
    key,
    normalizeSafeReportRef(inputReports[key], `${key} input report ref`)
  ]));
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  if (checks.length === 0) {
    throw new Error("Secure Mesh release proof config must define source checks");
  }
  const normalized = checks.map((item, index) => {
    const check = asRecord(item);
    const files = normalizeSourceCheckFiles(
      check,
      normalizeSafeSourceRef,
      `Secure Mesh release proof source check ${index + 1}`,
    );
    return {
      id: normalizeCheckId(check.id, `source check ${index + 1} id`),
      file: files[0],
      files,
      tokens: normalizeTokenList(check.tokens, `source check ${index + 1} tokens`),
    };
  });
  const ids = normalized.map((check) => check.id);
  if (new Set(ids).size !== ids.length) {
    throw new Error("Secure Mesh release proof config source checks must have unique ids");
  }
  return normalized.map((check) => Object.freeze({
    ...check,
    files: Object.freeze(check.files),
    tokens: Object.freeze(check.tokens),
  }));
}

function normalizeFreshnessWindowSeconds(value, label) {
  const seconds = Number(value);
  if (!Number.isInteger(seconds) || seconds < 60 || seconds > 7 * 24 * 60 * 60) {
    throw new Error(`Invalid Secure Mesh release proof ${label}; expected 60..604800 seconds`);
  }
  return seconds;
}

function normalizeFreshnessWindows(value) {
  const windows = asRecord(value);
  const unknown = Object.keys(windows).filter((key) => !requiredFreshnessWindowKeys.includes(key));
  if (unknown.length > 0) {
    throw new Error(`Secure Mesh release proof config contains unknown freshness window keys: ${unknown.join(", ")}`);
  }
  const missing = requiredFreshnessWindowKeys.filter((key) => !Object.prototype.hasOwnProperty.call(windows, key));
  if (missing.length > 0) {
    throw new Error(`Secure Mesh release proof config is missing freshness window keys: ${missing.join(", ")}`);
  }
  return Object.fromEntries(requiredFreshnessWindowKeys.map((key) => [
    key,
    normalizeFreshnessWindowSeconds(windows[key], `${key} freshness window`)
  ]));
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh release proof config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh release proof config");
  const reportOutput = normalizeSafeReportRef(payload.reportOutput, "report output ref");
  const inputReports = normalizeInputReports(payload.inputReports);
  const verifierCommands = normalizeVerifierCommands(payload.verifierCommands);
  const sourceChecks = normalizeSourceChecks(payload.sourceChecks);
  const freshnessWindows = normalizeFreshnessWindows(payload.freshnessWindows);
  if (Object.values(inputReports).includes(reportOutput)) {
    throw new Error("Secure Mesh release proof config must not include its own output as an input report");
  }
  cachedConfig = {
    configRef,
    schemaVersion,
    reportOutput,
    inputReports,
    verifierCommands,
    freshnessWindows,
    sourceChecks,
    requiredInputReportKeys,
    requiredVerifierCommandKeys,
    requiredFreshnessWindowKeys
  };
  return cachedConfig;
}

export async function loadSecureMeshReleaseProofConfig() {
  return loadRawConfig();
}
