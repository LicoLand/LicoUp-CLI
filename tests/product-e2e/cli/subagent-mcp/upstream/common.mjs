import { execFile } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

export const STARTUP_RESULTS = Object.freeze([
  "passed",
  "unavailable",
  "failed",
  "installer_configuration_required",
]);

const SAFE_REASON = /^[a-z][a-z0-9_]{0,63}$/u;
const SAFE_VERSION = /^(?:unresolved|\d+(?:\.\d+)+(?:-[0-9A-Za-z.-]+)?)$/u;
export const SERVER_KEY = "land.lico.licoup.subagents";
const SERVER_KEY_PATTERN = new RegExp(
  `(?:^|[^A-Za-z0-9_.-])${SERVER_KEY.replaceAll(".", "\\.")}(?:$|[^A-Za-z0-9_.-])`,
  "u",
);

export function startupReceipt(agent, result, reason, version = "unresolved") {
  if (!["codex", "cursor", "antigravity"].includes(agent)) {
    throw new TypeError("startup_agent_invalid");
  }
  if (!STARTUP_RESULTS.includes(result)) throw new TypeError("startup_result_invalid");
  if (!SAFE_REASON.test(reason)) throw new TypeError("startup_reason_invalid");
  const safeVersion = SAFE_VERSION.test(String(version || "")) ? String(version) : "unresolved";
  return Object.freeze({ agent, version: safeVersion, result, reason });
}

export function recognizeRegistry(agent, value) {
  if (!value || typeof value !== "object") {
    return startupReceipt(agent, "failed", "startup_registry_invalid");
  }
  const version = value.version ?? value.agentVersion ?? "unresolved";
  const entries = Array.isArray(value)
    ? value
    : value.servers ?? value.mcpServers ?? value.items ?? value.entries ?? [];
  const matchingEntries = Array.isArray(entries)
    ? entries.filter((entry) => typeof entry === "string"
      ? entry === SERVER_KEY
      : entry?.name === SERVER_KEY || entry?.id === SERVER_KEY || entry?.key === SERVER_KEY)
    : [];
  const mappedEntryPresent = !Array.isArray(entries)
    && entries !== null
    && (typeof entries === "object" || typeof entries === "function")
    && Object.hasOwn(entries, SERVER_KEY);
  const present = matchingEntries.length > 0 || mappedEntryPresent || value.serverPresent === true;
  const listed = matchingEntries.some((entry) => typeof entry === "string"
    || registryEntryEnabled(entry))
    || (mappedEntryPresent && registryEntryEnabled(entries[SERVER_KEY]));
  if (listed) return startupReceipt(agent, "passed", "startup_server_recognized", version);
  if (!present && value.registrationMode === "installer-only") {
    return startupReceipt(
      agent,
      "installer_configuration_required",
      "installer_configuration_required",
      version,
    );
  }
  return startupReceipt(agent, "failed", "startup_server_not_recognized", version);
}

function registryEntryEnabled(entry) {
  if (entry === true) return true;
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) return false;
  if (entry.enabled === false || entry.connected === false
    || entry.disabledReason || entry.disabled_reason) return false;
  const state = String(entry.status ?? entry.state ?? "").toLowerCase();
  return !/^(?:disabled|inactive|disconnected|failed|error|unavailable)$/u.test(state);
}

export function parseReadOnlyRegistryOutput(stdout, format) {
  if (format === "json") return JSON.parse(stdout);
  if (format !== "text") throw new TypeError("startup_surface_format_invalid");
  let present = false;
  const listed = String(stdout).split(/\r?\n/u).some((line) => {
    if (!SERVER_KEY_PATTERN.test(line)) return false;
    present = true;
    const status = line.toLowerCase();
    if (/\b(?:disabled|inactive|disconnected|failed|error|unavailable)\b/u.test(status)) return false;
    return /\b(?:connected|ready|enabled|active|running)\b/u.test(status);
  });
  return {
    servers: listed ? [SERVER_KEY] : [],
    serverPresent: present,
  };
}

export async function runReadOnlyList(executable, args, format = "json") {
  if (typeof executable !== "string" || executable.trim() === "") {
    return { unavailable: true };
  }
  try {
    const { stdout } = await execFileAsync(executable, args, {
      encoding: "utf8",
      maxBuffer: 64 * 1024,
      windowsHide: true,
    });
    return { value: parseReadOnlyRegistryOutput(stdout, format) };
  } catch (error) {
    if (error?.code === "ENOENT") return { unavailable: true };
    return { failed: true };
  }
}

export async function executeProbe({
  agent,
  executable,
  args,
  format = "json",
  installerOnly = false,
  inspect = runReadOnlyList,
}) {
  const observation = await inspect(executable, args, format);
  if (observation?.unavailable) {
    return startupReceipt(agent, "unavailable", "startup_surface_unavailable");
  }
  if (observation?.failed) {
    return startupReceipt(agent, "failed", "startup_surface_failed");
  }
  const value = observation?.value;
  if (installerOnly && value && typeof value === "object") {
    return recognizeRegistry(agent, { ...value, registrationMode: "installer-only" });
  }
  return recognizeRegistry(agent, value);
}

export function isDirectExecution(metaUrl) {
  return Boolean(process.argv[1]) && fileURLToPath(metaUrl) === resolve(process.argv[1]);
}

export function printReceipt(receipt) {
  process.stdout.write(`${JSON.stringify(receipt)}\n`);
  process.exitCode = receipt.result === "passed" ? 0 : 1;
}
