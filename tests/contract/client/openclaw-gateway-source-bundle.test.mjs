import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/platform/openclaw_gateway.rs";
const root = "crates/licoup-native/src/platform/openclaw_gateway";
const packageRoot = "crates/licoup-agent-openclaw/src";
const leaves = Object.freeze([
  "command.rs", "config.rs", "health.rs", "lifecycle.rs", "model.rs", "policy.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

test("OpenClaw Gateway facade owns every dedicated production leaf", async () => {
  const facade = await read(facadePath);
  for (const leaf of leaves) {
    assert.match(facade, new RegExp(`mod ${leaf.replace(".rs", "")};`, "u"));
    await fs.access(path.join(repoRoot, root, leaf));
  }
  for (const forbidden of ["Command::new", "ureq::", "TcpListener", "read_state"])
    assert.equal(facade.includes(forbidden), false, forbidden);
});

test("OpenClaw keeps vendor attach WebSocket config and owned stop semantics independent", async () => {
  const policy = await read(`${root}/policy.rs`);
  const health = await read(`${root}/health.rs`);
  const lifecycle = await read(`${root}/lifecycle.rs`);
  const config = await read(`${root}/config.rs`);
  // The vendor-default port is OpenClaw's own endpoint fact, so it lives in the
  // adapter package that carries the Agent: the client's engine reads it there
  // rather than keeping a second copy beside its own state and process
  // configuration.
  const packagePolicy = await read(`${packageRoot}/policy.rs`);
  assert.match(packagePolicy, /VENDOR_DEFAULT_PORT: u16 = 18_789/u);
  assert.match(packagePolicy, /DEFAULT_PORT: u16 = 24_189/u);
  assert.match(policy,
    /pub\(super\) use licoup_agent_openclaw::policy::\{DEFAULT_PORT, VENDOR_DEFAULT_PORT\};/u);
  assert.equal(policy.includes("18_789"), false);
  assert.match(health, /attachMode": "vendor-default"/u);
  assert.match(health, /http::probe_status/u);
  assert.match(lifecycle, /stoppedOwnedProcess": false/u);
  assert.match(lifecycle, /process::stop/u);
  assert.equal(config.includes('\\"bind\\": \\"loopback\\"'), true);
  assert.equal(lifecycle.includes('"state": service_state'), false);
  assert.equal(lifecycle.includes("stateDir"), false);
});

test("OpenClaw command uses fixed direct launch and discards raw process output", async () => {
  const command = await read(`${root}/command.rs`);
  assert.match(command, /Command::new\(executable\)/u);
  assert.match(command, /"gateway"/u);
  assert.match(command, /"loopback"/u);
  assert.match(command, /env_remove\("OPENCLAW_GATEWAY_TOKEN"\)/u);
  assert.match(command, /process::spawn_detached/u);
  assert.equal(command.includes('Command::new("sh")'), false);
  assert.equal(command.includes("OpenOptions"), false);
});

test("OpenClaw owns dedicated leaf regressions and no unsafe compatibility include", async () => {
  const entries = (await fs.readdir(path.join(repoRoot, root, "tests"))).sort();
  assert.deepEqual(entries, [
    "command.rs", "composition.rs", "config.rs", "health.rs", "lifecycle.rs", "mod.rs",
    "model.rs", "policy.rs",
  ]);
  const joined = (await Promise.all([
    read(facadePath),
    ...leaves.map((leaf) => read(`${root}/${leaf}`)),
  ])).join("\n");
  assert.equal(joined.includes("unsafe {"), false);
  assert.equal(joined.includes("include!("), false);
  assert.equal(joined.includes("#[path"), false);
});
