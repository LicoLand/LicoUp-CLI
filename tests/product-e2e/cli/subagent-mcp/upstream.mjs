#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { probeCodexStartup } from "./upstream/codex-startup-recognition.mjs";
import { probeCursorStartup } from "./upstream/cursor-startup-recognition.mjs";
import { probeAntigravityStartup } from "./upstream/antigravity-startup-recognition.mjs";
import { isDirectExecution, startupReceipt } from "./upstream/common.mjs";
import { admitDiscoveryDocument, DirectMcpClient, verifyServiceHealth } from "./streamable-http.mjs";

const probes = Object.freeze([
  probeCodexStartup,
  probeCursorStartup,
  probeAntigravityStartup,
]);
const providerAgents = Object.freeze(["codex", "cursor", "antigravity"]);

export async function runUpstream(options = {}) {
  const health = await (options.verifyHealth ?? defaultHealth)(options);
  if (health?.result !== "passed") {
    return { route: "upstream", service: health, providers: [] };
  }
  const selected = options.probes ?? probes;
  const providers = await Promise.all(providerAgents.map(async (agent, index) => {
    try {
      if (typeof selected[index] !== "function") throw new TypeError("startup_probe_missing");
      const observed = await selected[index](options.providerOptions ?? {});
      return startupReceipt(agent, observed?.result, observed?.reason, observed?.version);
    } catch {
      return startupReceipt(agent, "failed", "startup_surface_failed");
    }
  }));
  return { route: "upstream", service: health, providers };
}

async function defaultHealth(options) {
  const root = options.portableRoot ?? process.env.LICOUP_HOME;
  if (!root) return { result: "unavailable", reason: "service_discovery_unavailable" };
  try {
    const discovery = admitDiscoveryDocument(JSON.parse(await readFile(
      join(root, "client-state", "subagent-mcp", "discovery.json"),
      "utf8",
    )));
    const token = discovery?.tokens?.codex;
    return await verifyServiceHealth(new DirectMcpClient({ endpoint: discovery?.endpoint, token }));
  } catch {
    return { result: "failed", reason: "service_health_failed" };
  }
}

if (isDirectExecution(import.meta.url)) {
  const receipt = await runUpstream();
  process.stdout.write(`${JSON.stringify(receipt)}\n`);
  process.exitCode = receipt.service.result === "passed"
    && receipt.providers.every((provider) => provider.result === "passed") ? 0 : 1;
}
