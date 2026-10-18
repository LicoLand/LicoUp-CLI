// The OpenClaw adapter's source split, across the client and its package.
//
// Both halves of OpenClaw live in `crates/licoup-agent-openclaw` now: the
// Gateway ACP state machine, the byte-line codec, the allowlisted update
// projection, the run and failure vocabulary, the session continuity binding,
// the request validation, the endpoint policy, and the reviewed process half —
// the bounded capability probe, the Gateway attach, the supervised bridge, the
// bounded transport and the cleanup.
//
// This contract states the split: the host declares no OpenClaw driver module or
// tree, the composition reads the package without keeping a copy of its
// protocol, the host answers the package's Gateway port from its own Gateway
// engine, the package reaches no client crate, and the moved process half still
// carries the fixed Gateway ACP lane, exact continuity, bounded IO, cleanup and
// privacy the client has always required.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
/// The driver module the host used to declare, and must not declare again.
const retiredDriverRoot = "crates/licoup-native/src/platform/openclaw_driver";
/// The kernel's composition, which names this package's driver directly.
const compositionPath = "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
/// The kernel's answers for this package's ports.
const hostPortsPath = "crates/licoup-native/src/platform/openclaw_host.rs";
/// The client's Gateway engine, which the port answer reads and the package
/// never reaches.
const gatewayRoot = "crates/licoup-native/src/platform/openclaw_gateway";
const packageRoot = "crates/licoup-agent-openclaw/src";

/// The process half this package now owns.
const driverLeaves = Object.freeze([
  "execution.rs",
  "io.rs",
  "probe.rs",
  "supervision.rs",
]);
const parserLeaves = Object.freeze(["codec.rs", "events.rs", "protocol.rs"]);
const vocabularyLeaves = Object.freeze([
  "continuity.rs",
  "errors.rs",
  "model.rs",
  "params.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function exists(relativePath) {
  try {
    await fs.access(path.join(repoRoot, relativePath));
    return true;
  } catch {
    return false;
  }
}

async function sources() {
  return Object.fromEntries(await Promise.all([
    ...driverLeaves.map(async (leaf) => [
      `driver/${leaf}`,
      await read(`${packageRoot}/driver/${leaf}`),
    ]),
    ["driver.rs", await read(`${packageRoot}/driver.rs`)],
    ...parserLeaves.map(async (leaf) => [
      `parser/${leaf}`,
      await read(`${packageRoot}/parser/${leaf}`),
    ]),
    ["parser/root.rs", await read(`${packageRoot}/parser.rs`)],
    ...vocabularyLeaves.map(async (leaf) => [
      `vocabulary/${leaf}`,
      await read(`${packageRoot}/gateway_acp/${leaf}`),
    ]),
    ["gateway_acp.rs", await read(`${packageRoot}/gateway_acp.rs`)],
    ["gateway_acp/contract.rs", await read(`${packageRoot}/gateway_acp/contract.rs`)],
    ["gateway.rs", await read(`${packageRoot}/gateway.rs`)],
    ["policy.rs", await read(`${packageRoot}/policy.rs`)],
    ["port/gateway.rs", await read(`${packageRoot}/port/gateway.rs`)],
    ["port/turn_event.rs", await read(`${packageRoot}/port/turn_event.rs`)],
  ]));
}

test("the host declares no OpenClaw driver and the composition names the package", async () => {
  // The kernel has no OpenClaw driver module at all: the host reaches the Agent
  // through the package and cannot hold a second owner of its protocol or of the
  // reviewed process sites that moved with it.
  assert.equal(
    await exists(`${retiredDriverRoot}.rs`),
    false,
    "the host still declares an OpenClaw driver module",
  );
  assert.equal(
    await exists(retiredDriverRoot),
    false,
    "the host still declares an OpenClaw driver tree",
  );
  const platform = await read("crates/licoup-native/src/platform/mod.rs");
  assert.doesNotMatch(platform, /mod openclaw_driver;/u,
    "the host module tree still declares an OpenClaw driver module");

  const composition = await read(compositionPath);
  assert.match(composition, /use licoup_agent_openclaw::driver as openclaw_driver;/u,
    "the composition does not read the package's driver");
  // The composition keeps no second copy of the vendor fact: every OpenClaw
  // protocol decision and every reviewed process site is the package's.
  for (const forbidden of [
    "struct OpenClawProtocol",
    "impl ProtocolConfig",
    "struct SessionBinding",
    "fn run_protocol_loop",
    "Command::new",
    "include!(",
    "#[path",
  ]) {
    assert.equal(composition.includes(forbidden), false,
      `the client composition keeps a copy of the package's driver: ${forbidden}`);
  }

  // The host still answers the package's Gateway port, and it answers it over
  // the client's own reviewed Gateway engine rather than over a copy of it.
  const hostPorts = await read(hostPortsPath);
  assert.ok(
    hostPorts.includes("openclaw_gateway::ensure_attach_endpoint"),
    "the host port answer must read the client's own Gateway engine",
  );
  assert.equal(
    await exists(`${gatewayRoot}.rs`),
    true,
    "the client's Gateway engine must stay in the kernel",
  );
  assert.equal(
    await exists(`${gatewayRoot}/command.rs`),
    true,
    "the client's Gateway engine must stay in the kernel",
  );
});

test("the package's driver leaves read the protocol and restate none of it", async () => {
  const source = await sources();
  for (const leaf of driverLeaves) {
    const implementation = source[`driver/${leaf}`];
    for (const owned of [
      "struct OpenClawProtocol",
      "impl ProtocolConfig",
      "struct SessionBinding",
      "fn projected_event",
      "include!(",
      "#[path",
    ]) {
      assert.equal(
        implementation.includes(owned),
        false,
        `driver/${leaf} must not keep a second implementation of ${owned}`,
      );
    }
    assert.equal(
      implementation.includes("licoup_native"),
      false,
      `driver/${leaf} must not reach the client crate`,
    );
  }
  // One parse stays one parse: the driver names the package's own ingress paths.
  assert.ok(source["driver/execution.rs"].includes("crate::parser::protocol"));
  assert.ok(source["driver/io.rs"].includes("crate::parser::codec"));
  assert.ok(source["driver.rs"].includes("crate::gateway_acp::model"));
});

test("OpenClaw retains one fixed Gateway ACP lane without shell fallback", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  assert.ok(source["vocabulary/model.rs"].includes(
    'RUNTIME_PROTOCOL: &str = "openclaw-acp-stdio-jsonrpc"',
  ));
  // The published format identity the release declaration names is declared
  // once, in the package, beside the protocol it describes.
  assert.ok(source["gateway_acp/contract.rs"].includes(
    'PROTOCOL_FORMAT: &str = "openclaw.gateway-acp.v1"',
  ));
  assert.ok(source["driver/supervision.rs"].includes(
    'ATTACH_ARGS_PREFIX: &[&str] = &["acp", "--url"]',
  ));
  assert.ok(source["driver/supervision.rs"].includes("Command::new(&self.executable)"));
  assert.ok(source["driver/probe.rs"].includes('&["acp", "--help"]'));
  assert.ok(source["driver/probe.rs"].includes('&["--version"]'));
  assert.ok(source["driver/probe.rs"].includes(".stderr(Stdio::null())"));
  // The endpoint policy the attach is resolved against is OpenClaw's own, and
  // the client's engine reads it rather than keeping a second copy.
  assert.ok(source["policy.rs"].includes("VENDOR_DEFAULT_PORT: u16 = 18_789"));
  assert.ok(source["policy.rs"].includes("DEFAULT_PORT: u16 = 24_189"));
  assert.ok(source["policy.rs"].includes('"vendor-default"'));
  assert.ok(source["port/gateway.rs"].includes("ensure_attach_endpoint"));
  for (const fallback of [
    'Command::new("sh")',
    'Command::new("bash")',
    'Command::new("cmd")',
    'Command::new("powershell")',
    'args: ["run"]',
    'args: ["chat"]',
  ]) {
    assert.equal(joined.includes(fallback), false);
  }
});

test("OpenClaw continuity keeps protocol and resumable Gateway identities exact", async () => {
  const source = await sources();
  const continuity = source["vocabulary/continuity.rs"];
  const params = source["vocabulary/params.rs"];
  for (const token of [
    "SessionBinding",
    "capture_opening_update",
    "reconcile_open_response",
    "openclaw_acp_session_mismatch",
    "openclaw_acp_native_session_id_missing",
    "AcpSessionMethod::Load",
  ]) {
    assert.ok(continuity.includes(token), `missing OpenClaw continuity token: ${token}`);
  }
  assert.ok(params.includes('meta.insert("sessionKey"'));
  assert.ok(params.includes('meta.insert("requireExisting"'));
  // The one client fact the package may not reach arrives as an explicit lazy
  // argument, so the package names no client crate for it.
  assert.ok(params.includes("local_mcp"));
  assert.equal(params.includes("crate::domain"), false);
  assert.equal(params.includes("acp_servers_for_runtime"), false);
  // The client answers it at its own boundary: the driver composition asks the
  // registered port, and the host's composition installs the answer over its own
  // client state, so the composition names the port entry and no domain owner.
  assert.ok(
    (await read(compositionPath)).includes('collaboration_acp_servers("openclaw")'),
    "the client answers the MCP registration at its own boundary",
  );
});

test("OpenClaw IO, cleanup, events, and errors stay bounded and non-projecting", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  for (const token of [
    "BoundedStdinWriter",
    "StdoutLimitExceeded",
    "max_stdout",
    "max_stderr",
    "finish_protocol_transport",
    "TransportFinishFailure::Lifecycle",
    "openclaw_acp_timeout",
    "PROCESS_POLL_INTERVAL",
  ]) {
    assert.ok(joined.includes(token), `missing OpenClaw lifecycle token: ${token}`);
  }
  assert.ok(source["vocabulary/errors.rs"].includes("message: &'static str"));
  assert.ok(source["parser/events.rs"].includes("projected_event"));
  assert.ok(source["parser/protocol.rs"].includes("handle_frame"));
  assert.equal(source["driver/io.rs"].includes("decode_message"), false);
  assert.equal(source["parser/protocol.rs"].includes("update.payload().clone()"), false);
  for (const rawProjection of [
    "stderr: String",
    "stderr: Vec",
    "combined.extend(stderr",
    "String::from_utf8_lossy(&stderr",
    '"rawInput": update',
    '"_meta": update',
  ]) {
    assert.equal(joined.includes(rawProjection), false);
  }
});

test("OpenClaw split contains no production unsafe or client reach from the package", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  assert.equal(joined.includes("unsafe {"), false);
  assert.equal(joined.includes("include!("), false);
  assert.equal(joined.includes("#[path"), false);
  // The whole package is composed by the client, so none of it may reach back
  // into a client crate — the moved process half included.
  for (const clientReach of ["licoup_native", "crate::platform", "crate::domain"]) {
    assert.equal(
      joined.includes(clientReach),
      false,
      `the OpenClaw adapter package must not reach the client: ${clientReach}`,
    );
  }
});
