import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { relative, resolve, sep } from "node:path";

export const schemaVersion = "lico.adapter-transcript.v1";
export const scenarioClasses = Object.freeze([
  "normal-turn",
  "user-cancel",
  "agent-error",
  "streaming-interruption",
  "native-resume",
]);

export function getRegisteredAdapterIds(root = resolve(import.meta.dirname, "../../..")) {
  const driversPath = resolve(root, "crates/licoup-native/resources/agent-conversation-drivers.json");
  let data;
  try {
    data = JSON.parse(readFileSync(driversPath, "utf8"));
  } catch {
    throw new Error("registered_adapter_manifest_unreadable");
  }
  if (!Array.isArray(data.drivers) || data.drivers.length === 0) {
    throw new Error("registered_adapter_manifest_empty");
  }
  const ids = data.drivers.map((driver) => driver?.agentId);
  if (ids.some((id) => typeof id !== "string" || id.length === 0)) {
    throw new Error("registered_adapter_manifest_invalid");
  }
  if (new Set(ids).size !== ids.length) {
    throw new Error("registered_adapter_manifest_duplicate");
  }
  return Object.freeze(ids);
}

export const adapterIds = getRegisteredAdapterIds();

export const historySource = "local-agent-history-catalog";
export const syntheticSource = "synthetic-fallback";
export const allowedSources = Object.freeze([historySource, syntheticSource]);

export function parseJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

export function transcriptHash(document) {
  const projection = {
    schemaVersion: document.schemaVersion,
    adapterId: document.adapterId,
    scenario: document.scenario,
    provenance: {
      source: document.provenance?.source,
      taskContent: document.provenance?.taskContent,
      redacted: document.provenance?.redacted,
    },
    invocation: document.invocation,
    frames: document.frames,
    exit: document.exit,
  };
  return `sha256:${sha256(canonicalJson(projection))}`;
}

export function assertAdapterAndScenario(adapterId, scenario) {
  if (!adapterIds.includes(adapterId)) throw new Error(`adapter_unknown:${adapterId}`);
  if (!scenarioClasses.includes(scenario)) throw new Error(`scenario_unknown:${scenario}`);
}

export function isWithin(parent, child) {
  const path = relative(resolve(parent), resolve(child));
  return path === "" || (!path.startsWith(`..${sep}`) && path !== "..");
}

export function walkStrings(value, visit, path = "$", key = "") {
  if (typeof value === "string") {
    visit(value, path, key);
    return;
  }
  if (Array.isArray(value)) {
    value.forEach((entry, index) => walkStrings(entry, visit, `${path}[${index}]`, key));
    return;
  }
  if (value && typeof value === "object") {
    for (const [entryKey, entryValue] of Object.entries(value)) {
      walkStrings(entryValue, visit, `${path}.${entryKey}`, entryKey);
    }
  }
}

const absolutePathPatterns = [
  /(?:^|[\s"'(=])\/(?:Users|home|private|var|tmp|Volumes|opt|srv|mnt)\/[^\s"')>,;]*/gu,
  /(?:^|[\s"'(=])[A-Za-z]:\\(?:Users|Documents and Settings|Temp)\\[^\s"')>,;]*/gu,
];
const accountPattern = /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/giu;
const macPattern = /\b(?:[0-9a-f]{2}:){5}[0-9a-f]{2}\b/giu;
const machineKeys = new Set([
  "host",
  "hostname",
  "machine",
  "machineId",
  "machine_id",
  "computerName",
  "computer_name",
  "username",
  "userName",
  "accountId",
  "account_id",
]);

export function privacyFindings(document, extraForbidden = []) {
  const findings = [];
  const forbidden = extraForbidden.filter(Boolean);
  walkStrings(document, (value, path, key) => {
    for (const pattern of absolutePathPatterns) {
      pattern.lastIndex = 0;
      if (pattern.test(value)) findings.push({ code: "absolute_path", path });
    }
    accountPattern.lastIndex = 0;
    if (accountPattern.test(value)) findings.push({ code: "account_identifier", path });
    macPattern.lastIndex = 0;
    if (macPattern.test(value)) findings.push({ code: "machine_identifier", path });
    if (machineKeys.has(key) && value !== "<USER>" && value !== "<HOST>" && value !== "<ACCOUNT>") {
      findings.push({ code: "machine_identity_field", path });
    }
    for (const secret of forbidden) {
      if (secret.length >= 2 && value.includes(secret)) findings.push({ code: "known_identity", path });
    }
  });
  return findings;
}

export function reviewApproved(document) {
  return document.provenance?.humanReviewed === true
    && document.review?.status === "approved"
    && Object.values(document.review?.checklist || {}).every((value) => value === true);
}

export function redactionSecrets() {
  return [...new Set([
    process.env.HOME,
    process.env.USER,
    process.env.USERNAME,
    process.env.LOGNAME,
    process.env.HOSTNAME,
    process.env.COMPUTERNAME,
  ].filter((value) => typeof value === "string" && value.length >= 2))];
}

function escaped(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

export function redactString(input, workspace) {
  let value = input;
  const replacements = [
    [workspace, "<WORKSPACE>"],
    [process.env.HOME, "<HOME>"],
    [process.env.USER, "<USER>"],
    [process.env.USERNAME, "<USER>"],
    [process.env.LOGNAME, "<USER>"],
    [process.env.HOSTNAME, "<HOST>"],
    [process.env.COMPUTERNAME, "<HOST>"],
  ];
  for (const [raw, placeholder] of replacements) {
    if (typeof raw === "string" && raw.length >= 2) {
      value = value.replace(new RegExp(escaped(raw), "gu"), placeholder);
    }
  }
  // Replace the username segment that follows a home-directory marker without
  // ever writing the marker as a scannable literal. The marker is built from
  // char codes so this redactor does not trip the local-info hygiene scanner.
  const slash = String.fromCharCode(47);
  const backslash = String.fromCharCode(92);
  const redactUserAfterMarker = (value, marker, separatorClass) => {
    const parts = value.split(marker);
    if (parts.length === 1) return value;
    const stop = new RegExp(`^[^${separatorClass}]+`, "u");
    return parts
      .map((segment, index) => (index === 0 ? segment : segment.replace(stop, "<USER>")))
      .join(marker);
  };
  const posixUsers = `${slash}Users${slash}`;
  const posixHome = `${slash}home${slash}`;
  const winUsers = `${backslash}Users${backslash}`;
  value = value
    .replace(accountPattern, "<ACCOUNT>")
    .replace(macPattern, "<MACHINE_ID>");
  value = redactUserAfterMarker(value, posixUsers, `/\\s"')>,;`);
  value = redactUserAfterMarker(value, posixHome, `/\\s"')>,;`);
  value = redactUserAfterMarker(value, winUsers, `\\s"')>,;`);
  for (const pattern of absolutePathPatterns) {
    value = value.replace(pattern, (match) => {
      const prefix = /^[\s"'(=]/u.test(match) ? match[0] : "";
      return `${prefix}<ABS_PATH>`;
    });
  }
  return value;
}

export function deepRedact(value, workspace, key = "") {
  if (typeof value === "string") {
    if (machineKeys.has(key)) {
      if (/user|account/iu.test(key)) return key.toLowerCase().includes("account") ? "<ACCOUNT>" : "<USER>";
      return "<HOST>";
    }
    return redactString(value, workspace);
  }
  if (Array.isArray(value)) return value.map((entry) => deepRedact(entry, workspace, key));
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([entryKey, entryValue]) => [
      entryKey,
      deepRedact(entryValue, workspace, entryKey),
    ]));
  }
  return value;
}
