import fs from "node:fs/promises";

const configUrl = new URL("../config/secure-mesh-physical-evidence.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-physical-evidence.json";
const schemaVersion = "licomesh.secure-mesh.physical-evidence-config.v2";
const requiredLinkedReportKeys = Object.freeze([
  "androidPlatformCrypto",
  "androidInstallLaunch",
  "stationAcceptance",
  "ubuntuVmSecretStore",
  "macosReleaseCliProof",
  "macosUserPresenceProof",
  "ubuntuReleaseCliProof",
  "ubuntuLinuxAdaptiveCustodyProof",
  "ubuntuLinuxPackageUpdateProof",
  "ubuntuLinuxVmPackageReceipt",
  "ubuntuLinuxNodeMatrix",
  "platformSecretStore",
  "physicalDeviceMatrix",
  "encryptedFileHandoff",
  "trustUx",
  "windowsImplementation",
  "updateReleaseChannel"
]);
const requiredEvidenceCommandKeys = Object.freeze([
  "android",
  "ios",
  "macos",
  "windows",
  "linux"
]);
const requiredFreshnessWindowKeys = Object.freeze([
  "androidPlatformCryptoSeconds"
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
    throw new Error(`Invalid Secure Mesh physical evidence ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeEvidenceCommand(value, label) {
  const command = String(value || "").trim();
  if (!command ||
    /[\r\n]/u.test(command) ||
    command.length > 240 ||
    !(
      /^npm run [a-z0-9:_-]+(?: -- --[a-z0-9-]+ [a-z0-9._/-]+)?$/u.test(command) ||
      /^node [a-z0-9_./-]+\.mjs(?: --[a-z0-9-]+(?: [a-z0-9_./-]+)?)*$/u.test(command)
    )) {
    throw new Error(`Invalid Secure Mesh physical evidence ${label}`);
  }
  assertNoLeak(command, label);
  return command;
}

function normalizeEvidenceCommands(value) {
  const evidenceCommands = asRecord(value);
  const unknown = Object.keys(evidenceCommands).filter((key) => !requiredEvidenceCommandKeys.includes(key));
  if (unknown.length > 0) {
    throw new Error(`Secure Mesh physical evidence config contains unknown evidence command keys: ${unknown.join(", ")}`);
  }
  const missing = requiredEvidenceCommandKeys.filter((key) => !Object.prototype.hasOwnProperty.call(evidenceCommands, key));
  if (missing.length > 0) {
    throw new Error(`Secure Mesh physical evidence config is missing evidence command keys: ${missing.join(", ")}`);
  }
  const normalized = Object.fromEntries(requiredEvidenceCommandKeys.map((key) => {
    const commands = [].concat(evidenceCommands[key] || [])
      .map((command) => normalizeEvidenceCommand(command, `${key} evidence command`));
    if (commands.length === 0) {
      throw new Error(`Secure Mesh physical evidence config ${key} evidence command list must not be empty`);
    }
    return [key, Object.freeze(commands)];
  }));
  return Object.freeze(normalized);
}

function normalizeLinkedReports(value) {
  const linkedReports = asRecord(value);
  const unknown = Object.keys(linkedReports).filter((key) => !requiredLinkedReportKeys.includes(key));
  if (unknown.length > 0) {
    throw new Error(`Secure Mesh physical evidence config contains unknown linked report keys: ${unknown.join(", ")}`);
  }
  const missing = requiredLinkedReportKeys.filter((key) => !Object.prototype.hasOwnProperty.call(linkedReports, key));
  if (missing.length > 0) {
    throw new Error(`Secure Mesh physical evidence config is missing linked report keys: ${missing.join(", ")}`);
  }
  const normalized = Object.fromEntries(requiredLinkedReportKeys.map((key) => [
    key,
    normalizeSafeReportRef(linkedReports[key], `${key} linked report ref`)
  ]));
  const duplicateRefs = [];
  const refsByValue = new Map();
  for (const [key, ref] of Object.entries(normalized)) {
    if (refsByValue.has(ref)) {
      duplicateRefs.push(`${key}:${ref}`);
      continue;
    }
    refsByValue.set(ref, key);
  }
  if (duplicateRefs.length > 0) {
    throw new Error(`Duplicate Secure Mesh physical evidence linked report refs: ${duplicateRefs.join(", ")}`);
  }
  return normalized;
}

function normalizeFreshnessWindowSeconds(value, label) {
  const seconds = Number(value);
  if (!Number.isInteger(seconds) || seconds < 60 || seconds > 7 * 24 * 60 * 60) {
    throw new Error(`Invalid Secure Mesh physical evidence ${label}; expected 60..604800 seconds`);
  }
  return seconds;
}

function normalizeFreshnessWindows(value) {
  const windows = asRecord(value);
  const unknown = Object.keys(windows).filter((key) => !requiredFreshnessWindowKeys.includes(key));
  if (unknown.length > 0) {
    throw new Error(`Secure Mesh physical evidence config contains unknown freshness window keys: ${unknown.join(", ")}`);
  }
  const missing = requiredFreshnessWindowKeys.filter((key) => !Object.prototype.hasOwnProperty.call(windows, key));
  if (missing.length > 0) {
    throw new Error(`Secure Mesh physical evidence config is missing freshness window keys: ${missing.join(", ")}`);
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
    throw new Error("Secure Mesh physical evidence config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh physical evidence config");
  const reportOutput = normalizeSafeReportRef(payload.reportOutput, "report output ref");
  const linkedReports = normalizeLinkedReports(payload.linkedReports);
  const evidenceCommands = normalizeEvidenceCommands(payload.evidenceCommands);
  const freshnessWindows = normalizeFreshnessWindows(payload.freshnessWindows);
  if (Object.values(linkedReports).includes(reportOutput)) {
    throw new Error("Secure Mesh physical evidence config must not link its own output as an input report");
  }
  cachedConfig = {
    configRef,
    schemaVersion,
    reportOutput,
    linkedReports,
    evidenceCommands,
    freshnessWindows,
    requiredLinkedReportKeys,
    requiredEvidenceCommandKeys,
    requiredFreshnessWindowKeys
  };
  return cachedConfig;
}

export async function loadSecureMeshPhysicalEvidenceConfig() {
  return loadRawConfig();
}
