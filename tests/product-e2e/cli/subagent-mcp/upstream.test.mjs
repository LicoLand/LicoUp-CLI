import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import test from "node:test";
import { runUpstream } from "./upstream.mjs";
import { probeCodexStartup } from "./upstream/codex-startup-recognition.mjs";
import { probeCursorStartup } from "./upstream/cursor-startup-recognition.mjs";
import { probeAntigravityStartup } from "./upstream/antigravity-startup-recognition.mjs";
import { parseReadOnlyRegistryOutput, recognizeRegistry } from "./upstream/common.mjs";

const recognized = (version = "1.2.3") => async (_executable, args) => ({
  value: { version, mcpServers: [{ name: "land.lico.licoup.subagents" }] },
  args,
});
const execFileAsync = promisify(execFile);

test("service health completes before three provider probes fan out", async () => {
  const events = [];
  let active = 0;
  let peak = 0;
  const provider = (agent) => async () => {
    assert.deepEqual(events, ["health"]);
    active += 1;
    peak = Math.max(peak, active);
    await Promise.resolve();
    active -= 1;
    return { agent, version: "1.0.0", result: "passed", reason: "startup_server_recognized" };
  };
  const receipt = await runUpstream({
    verifyHealth: async () => {
      events.push("health");
      return { result: "passed", reason: "service_healthy" };
    },
    probes: [provider("codex"), provider("cursor"), provider("antigravity")],
  });
  assert.equal(peak, 3);
  assert.deepEqual(receipt.providers.map((item) => item.agent), ["codex", "cursor", "antigravity"]);
});

test("provider startup modules use a transient Codex declaration and supported read-only list surfaces", async () => {
  const calls = [];
  const inspect = async (executable, args) => {
    calls.push({ executable, args });
    return recognized()(executable, args);
  };
  const results = await Promise.all([
    probeCodexStartup({ executable: "fake-codex", inspect }),
    probeCursorStartup({ executable: "fake-cursor", inspect }),
    probeAntigravityStartup({ executable: "fake-antigravity", inspect }),
  ]);
  assert.ok(results.every((item) => item.result === "passed"));
  assert.deepEqual(calls[0].args.slice(-3), ["mcp", "list", "--json"]);
  assert.match(calls[0].args[1], /mcp_servers=.*land\.lico\.licoup\.subagents/u);
  assert.deepEqual(calls[1].args, ["mcp", "list"]);
  assert.deepEqual(calls[2].args, ["mcp", "list"]);
  assert.doesNotMatch(JSON.stringify(calls), /prompt|conversation|tools\/call|plugin|add|remove|install|write/iu);
});

test("standalone startup recognition runs against a hermetic provider process", async () => {
  const fixture = fileURLToPath(new URL("./upstream/fixtures/read-only-mcp-list.mjs", import.meta.url));
  const inspect = async (_executable, args) => {
    const { stdout } = await execFileAsync(process.execPath, [fixture, ...args], { encoding: "utf8" });
    return { value: JSON.parse(stdout) };
  };
  const receipt = await probeCodexStartup({ executable: "fixture", inspect });
  assert.deepEqual(receipt, {
    agent: "codex", version: "1.2.3", result: "passed", reason: "startup_server_recognized",
  });
});

test("startup failures stay typed and privacy safe", async () => {
  const unavailable = await probeCodexStartup({ inspect: async () => ({ unavailable: true }) });
  const installer = await probeCursorStartup({
    inspect: async () => ({ value: { version: "1.0.0", registrationMode: "installer-only", servers: [] } }),
  });
  const unsafe = await probeAntigravityStartup({
    inspect: async () => ({ value: { version: "not-a-version", servers: [] } }),
  });
  assert.equal(unavailable.result, "unavailable");
  assert.equal(installer.result, "installer_configuration_required");
  assert.equal(unsafe.version, "unresolved");
  assert.doesNotMatch(JSON.stringify([unavailable, installer, unsafe]), /not-a-version|Bearer|token/iu);
});

test("text list parsing admits only the exact namespaced server key", () => {
  assert.deepEqual(parseReadOnlyRegistryOutput(
    "✓ land.lico.licoup.subagents connected\n",
    "text",
  ), { servers: ["land.lico.licoup.subagents"], serverPresent: true });
  assert.deepEqual(parseReadOnlyRegistryOutput(
    "land.lico.licoup.subagents-foreign connected\n",
    "text",
  ), { servers: [], serverPresent: false });
  assert.deepEqual(parseReadOnlyRegistryOutput(
    "land.lico.licoup.subagents: disabled\n",
    "text",
  ), { servers: [], serverPresent: true });
});

test("JSON registry recognition rejects null and explicitly unhealthy entries", () => {
  for (const entry of [
    null,
    { enabled: false },
    { connected: false },
    { status: "failed" },
    { state: "disconnected" },
  ]) {
    assert.equal(recognizeRegistry("codex", {
      servers: { "land.lico.licoup.subagents": entry },
    }).result, "failed");
  }
  assert.equal(recognizeRegistry("codex", {
    servers: { "land.lico.licoup.subagents": { enabled: true } },
  }).result, "passed");
  assert.equal(recognizeRegistry("cursor", {
    registrationMode: "installer-only",
    servers: { "land.lico.licoup.subagents": { enabled: false } },
  }).result, "failed");
  assert.equal(recognizeRegistry("cursor", {
    registrationMode: "installer-only", servers: {},
  }).result, "installer_configuration_required");
});

test("provider probes are not invoked when service health fails", async () => {
  let probes = 0;
  const receipt = await runUpstream({
    verifyHealth: async () => ({ result: "failed", reason: "service_health_failed" }),
    probes: [async () => { probes += 1; }],
  });
  assert.equal(probes, 0);
  assert.deepEqual(receipt.providers, []);
});

test("one unexpected provider failure is isolated behind the safe result contract", async () => {
  const receipt = await runUpstream({
    verifyHealth: async () => ({ result: "passed", reason: "service_healthy" }),
    probes: [
      async () => ({ agent: "codex", version: "1.0.0", result: "passed", reason: "startup_server_recognized" }),
      async () => { throw new Error("provider-output-canary"); },
      async () => ({ agent: "antigravity", version: "1.0.0", result: "passed", reason: "startup_server_recognized" }),
    ],
  });
  assert.equal(receipt.providers[1].result, "failed");
  assert.equal(receipt.providers[1].reason, "startup_surface_failed");
  assert.doesNotMatch(JSON.stringify(receipt), /provider-output-canary|Bearer|token/iu);
});
