import { existsSync, readFileSync } from "node:fs";
import { spawn } from "node:child_process";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { agentConfigs } from "../../../tests/product-e2e/cli/agent-conversations/support/parity/constants.mjs";

export const regressionRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const driverInventory = JSON.parse(readFileSync(path.resolve(
  regressionRoot,
  "crates/licoup-native/resources/agent-conversation-drivers.json",
), "utf8"));
const readinessInventory = JSON.parse(readFileSync(path.resolve(
  regressionRoot,
  "crates/licoup-native/resources/agent-conversation-readiness.json",
), "utf8"));

function executableCandidates(name) {
  const suffixes = process.platform === "win32" ? [".exe", ".cmd", ".bat", ""] : [""];
  return String(process.env.PATH || "")
    .split(path.delimiter)
    .filter(Boolean)
    .flatMap((directory) => suffixes.map((suffix) => path.join(directory, `${name}${suffix}`)));
}

export function commandAvailable(name) {
  return executableCandidates(name).some((candidate) => existsSync(candidate));
}

function explicitConfiguredExecutable(config) {
  return config.binaryEnvironment.some((key) => {
    const candidate = process.env[key];
    return typeof candidate === "string" && candidate.length > 0 && existsSync(candidate);
  });
}

let cachedSidecarPath;

function sidecarPath() {
  if (cachedSidecarPath !== undefined) return cachedSidecarPath;
  const candidates = [
    process.env.LICO_CLIENT_PATH,
    "build/crates/licoup-native/target/debug/licoup-cli",
    "build/crates/licoup-native/target/release/licoup-cli",
    "crates/licoup-native/target/debug/licoup-cli",
    "crates/licoup-native/target/release/licoup-cli",
    "target/debug/licoup-cli",
    "target/release/licoup-cli",
    "build/apps/desktop/runnable/macos/release/LicoUp.app/Contents/Helpers/LicoUpCustody.app/Contents/MacOS/licoup-cli",
    "build/apps/desktop/runnable/linux/release/licoup-cli",
    "build/apps/desktop/runnable/windows/release/licoup-cli.exe",
  ].filter(Boolean);
  cachedSidecarPath = candidates
    .map((candidate) => path.resolve(regressionRoot, candidate))
    .find((candidate) => existsSync(candidate)) || "";
  return cachedSidecarPath;
}

const nativeAgentInventoryBySidecar = new Map();

async function nativeAgentBinaryInventory(sidecar) {
  if (nativeAgentInventoryBySidecar.has(sidecar)) {
    return nativeAgentInventoryBySidecar.get(sidecar);
  }
  const pending = new Promise((resolve) => {
    let output = "";
    let overflow = false;
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    let child;
    try {
      child = spawn(sidecar, [
        "targets", "scan",
        "--stdin-json", "true",
        "--include-accessible-environments", "false",
        "--include-history-model-catalog", "false",
        "--enable-agent-cli-model-lookup", "false",
      ], {
        cwd: regressionRoot,
        env: process.env,
        shell: false,
        stdio: ["pipe", "pipe", "ignore"],
        windowsHide: true,
      });
    } catch {
      finish(new Set());
      return;
    }
    child.once("error", () => finish(new Set()));
    child.stdout.on("data", (chunk) => {
      if (overflow) return;
      output += chunk.toString("utf8");
      if (output.length > 1024 * 1024) {
        output = "";
        overflow = true;
      }
    });
    child.once("close", (code) => {
      if (code !== 0 || overflow) {
        finish(new Set());
        return;
      }
      try {
        const scan = JSON.parse(output);
        output = "";
        const available = new Set((Array.isArray(scan?.candidates) ? scan.candidates : [])
          .filter((row) => typeof row?.binaryPath === "string" && row.binaryPath.length > 0)
          .map((row) => row.target)
          .filter((id) => typeof id === "string"));
        finish(available);
      } catch {
        finish(new Set());
      }
    });
    child.stdin.end(`${JSON.stringify({ targetIds: Object.keys(agentConfigs) })}\n`);
  });
  nativeAgentInventoryBySidecar.set(sidecar, pending);
  return pending;
}

export async function readOnlyCommandOutput(program, args) {
  return new Promise((resolve) => {
    let output = "";
    let overflow = false;
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    let child;
    try {
      child = spawn(program, args, {
        cwd: regressionRoot,
        env: process.env,
        shell: false,
        stdio: ["ignore", "pipe", "ignore"],
        windowsHide: true,
      });
    } catch {
      finish(Object.freeze({ ok: false, output: "" }));
      return;
    }
    child.once("error", () => finish(Object.freeze({ ok: false, output: "" })));
    child.stdout.on("data", (chunk) => {
      if (overflow) return;
      output += chunk.toString("utf8");
      if (output.length > 1024 * 1024) {
        output = "";
        overflow = true;
      }
    });
    child.once("close", (code) => finish(Object.freeze({
      ok: code === 0 && !overflow,
      output: code === 0 && !overflow ? output : "",
    })));
  });
}

export function definePlatformEntry({ id, hosts, tools = [], artifacts = [], liveCommand = null,
  resources = [], inputs = [], unverifiedInputs = [],
  unavailableReason = "platform_runtime_unavailable", capabilityProbe = null }) {
  return Object.freeze({
    id,
    kind: "platform",
    stage: "compatibility",
    lane: `platform:${id}`,
    // Different platform branches must not serialize behind one synthetic
    // device lock. A concrete platform still has one exclusive runtime lane.
    resources: Object.freeze([`platform-runtime:${id}`, ...resources]),
    inputs: Object.freeze([...inputs]),
    unverifiedInputs: Object.freeze([...unverifiedInputs]),
    liveCommand,
    async probe() {
      if (hosts.length > 0 && !hosts.includes(process.platform)) {
        return Object.freeze({ eligible: false, reason: "platform_host_unavailable" });
      }
      if (tools.some((tool) => !commandAvailable(tool))) {
        return Object.freeze({ eligible: false, reason: "platform_toolchain_unavailable" });
      }
      if (artifacts.some((artifact) => !existsSync(path.resolve(regressionRoot, artifact)))) {
        return Object.freeze({ eligible: false, reason: unavailableReason });
      }
      if (!liveCommand) {
        return Object.freeze({ eligible: false, reason: "platform_live_verifier_unavailable" });
      }
      if (capabilityProbe) {
        const capability = await capabilityProbe();
        if (!capability.eligible) return Object.freeze(capability);
      }
      return Object.freeze({ eligible: true, reason: null });
    },
  });
}

export function defineAgentEntry(id) {
  const driver = driverInventory.drivers.find((candidate) => candidate.agentId === id);
  const readinessRow = readinessInventory.adapters.find((candidate) => candidate.agentId === id);
  if (!driver || !readinessRow) throw new Error(`agent regression inventory is incomplete: ${id}`);
  const config = agentConfigs[id];
  return Object.freeze({
    id,
    kind: "agent",
    stage: "compatibility",
    lane: `agent:${id}`,
    resources: Object.freeze([`agent-runtime:${id}`]),
    inputs: Object.freeze([
      `tests/product-e2e/cli/agent-conversations/${id}/conversation.test.mjs`,
    ]),
    unverifiedInputs: Object.freeze([]),
    liveCommand: config
      ? Object.freeze({
        program: "node",
        args: Object.freeze([
          "tests/product-e2e/cli/agent-conversations/support/conversation.mjs",
          "--agent", id,
          "--timeout-ms", "180000",
        ]),
        cwd: ".",
        timeoutMs: 15 * 60_000,
      })
      : null,
    async probe() {
      if (!config) {
        return Object.freeze({
          eligible: false,
          reason: "agent_live_harness_unavailable",
        });
      }
      const sidecar = sidecarPath();
      if (!sidecar) {
        return Object.freeze({ eligible: false, reason: "lico_client_executable_unavailable" });
      }
      const inventory = explicitConfiguredExecutable(config)
        ? null
        : await nativeAgentBinaryInventory(sidecar);
      if (inventory && !inventory.has(id)) {
        return Object.freeze({ eligible: false, reason: "agent_executable_unavailable" });
      }
      return Object.freeze({ eligible: true, reason: null });
    },
  });
}
