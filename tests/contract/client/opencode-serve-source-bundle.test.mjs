import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { openCodeWorkspaceUrl } from "../../product-e2e/cli/agent-conversations/support/gates/opencode-http.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/platform/opencode_serve.rs";
const root = "crates/licoup-native/src/platform/opencode_serve";
// The driver this file used to read from the kernel is the adapter package's
// now; the client keeps the engine facade, the endpoint specification and the
// answer for the package's ports.
const packageDriverRoot = "crates/licoup-agent-opencode/src/driver";
const packagePolicyPath = "crates/licoup-agent-opencode/src/policy.rs";

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

test("OpenCode serve is a thin facade plus one target policy leaf", async () => {
  const facade = await read(facadePath);
  const policy = await read(`${root}/policy.rs`);
  const packagePolicy = await read(packagePolicyPath);
  assert.match(facade, /local_service::serve::ensure\(policy::SPEC/u);
  assert.match(facade, /local_service::sse::watch_frames/u);
  assert.doesNotMatch(facade, /local_service::sse::watch_data/u);
  // The endpoint facts are the package's, and the engine specification reads
  // them field for field rather than restating any of them.
  assert.match(policy, /default_port: vendor::SPEC\.default_port/u);
  assert.match(policy, /default_executable: vendor::SPEC\.default_executable/u);
  assert.match(policy, /health_failed: vendor::SPEC\.errors\.health_failed/u);
  assert.match(policy, /native_agent_parser::adapters::opencode::readiness/u);
  assert.match(packagePolicy, /default_port: 24173/u);
  assert.match(packagePolicy, /default_executable: "opencode"/u);
  assert.match(packagePolicy, /health_failed: "opencode_serve_health_failed"/u);
  assert.doesNotMatch(facade, /adapters::opencode/u);
  for (const forbidden of ["ureq::", "TcpListener", "read_state", "wait_for_health"])
    assert.equal(facade.includes(forbidden), false, forbidden);
});

test("OpenCode target owns dedicated composition policy and event regressions", async () => {
  const entries = (await fs.readdir(path.join(repoRoot, root, "tests"))).sort();
  assert.deepEqual(entries, ["composition.rs", "events.rs", "mod.rs", "policy.rs"]);
  // The event lane's own projection claim moved into the package with the driver
  // that reads the stream; this client-side test binds the client's SSE ingress
  // and byte record to the package's watcher.
  const events = await read(`${root}/tests/events.rs`);
  assert.match(events, /licoup_agent_opencode::driver::\{?ServeStreamFailure, watch_session_events\}?/u);
  assert.match(events, /crate::platform::opencode_host/u);
  assert.match(events, /tool\.updated/u);
  const driverEvents = await read(`${packageDriverRoot}/tests/serve_transport.rs`);
  assert.match(driverEvents, /target_event_lane_projects_only_assistant_text_parts/u);
  assert.match(driverEvents, /ServeEventParser::new\("open-2"\)/u);
});

test("OpenCode facade never projects raw state or local executable paths", async () => {
  const sources = `${await read(facadePath)}\n${await read(`${root}/policy.rs`)}\n${await read(packagePolicyPath)}`;
  assert.equal(sources.includes('"state":'), false);
  assert.equal(sources.includes("stateDir"), false);
  assert.equal(sources.includes("unsafe {"), false);
});

test("OpenCode driver keeps phase-specific first failures", async () => {
  const transport = await read(`${packageDriverRoot}/serve_transport.rs`);
  const probe = await read(`${packageDriverRoot}/probe.rs`);
  for (const code of [
    "opencode_serve_health_failed",
    "opencode_serve_message_failed",
    "opencode_serve_control_failed",
    "opencode_serve_sse_unavailable",
  ]) assert.match(transport, new RegExp(code, "u"));
  assert.equal(transport.includes('"opencode_serve_unavailable"'), false);
  assert.match(probe, /first_health_failure/u);
  assert.match(probe, /endpoint_failure\(&error\)/u);
});

test("OpenCode live regression uses official workspace query routing", () => {
  const url = new URL(openCodeWorkspaceUrl(
    "http://127.0.0.1:24173",
    ["session", "session/with space", "message"],
    "/workspace/with space",
  ));
  assert.equal(url.pathname, "/session/session%2Fwith%20space/message");
  assert.equal(url.searchParams.get("directory"), "/workspace/with space");
});
