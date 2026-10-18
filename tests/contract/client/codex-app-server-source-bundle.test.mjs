import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const moduleRoot = "crates/licoup-agent-codex/src/app_server";
const facadePath = `${moduleRoot}.rs`;
const driverRoot = `${moduleRoot}/driver`;
const parserRoot = "crates/licoup-agent-codex/src/parser";
const productionLeaves = Object.freeze([
  "driver/active_control.rs",
  "config.rs",
  "contract.rs",
  "failure.rs",
  "driver/io.rs",
  "driver/launch.rs",
  "limits.rs",
  "model.rs",
  "driver/model_catalog.rs",
  "reserve.rs",
  "driver/supervision.rs",
  "driver/transport.rs",
]);
const parserLeaves = Object.freeze([
  "control.rs",
  "events.rs",
  "helpers.rs",
  "session.rs",
]);
const testLeaves = Object.freeze([
  "config.rs",
  "control.rs",
  "events.rs",
  "io.rs",
  "launch.rs",
  "model_catalog.rs",
  "session.rs",
  "support.rs",
  "transport.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function readLeaves(leaves) {
  return Object.fromEntries(await Promise.all([
    ...leaves.map(async (leaf) => [leaf, await read(`${moduleRoot}/${leaf}`)]),
    ["parser.rs", await read(`${parserRoot}.rs`)],
    ...parserLeaves.map(async (leaf) => [`parser/${leaf}`, await read(`${parserRoot}/${leaf}`)]),
  ]));
}

test("Codex app-server uses a thin facade with no retired monolith", async () => {
  const facade = `${await read(facadePath)}\n${await read(`${driverRoot}.rs`)}`;
  assert.deepEqual(
    [...facade.matchAll(/^(?:pub )?mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((name) => name !== "tests")
      .sort(),
    ["active_control", "config", "contract", "driver", "failure", "io", "launch", "limits", "model", "model_catalog", "reserve", "supervision", "transport"],
  );
  for (const implementationToken of [
    "struct CodexProtocol",
    "struct ProtocolConfig",
    "Command::new",
    "fn run_protocol_loop",
    "fn read_protocol_messages",
  ]) {
    assert.equal(facade.includes(implementationToken), false);
  }
});

test("Codex protocol, state, events, and approval control have single owners", async () => {
  const sources = await readLeaves(productionLeaves);
  const joined = Object.values(sources).join("\n");
  const protocolFacade = sources["parser.rs"];

  for (const moduleName of ["control", "events", "helpers", "session"]) {
    assert.ok(new RegExp(`(?:pub )?mod ${moduleName};`).test(protocolFacade));
  }
  assert.ok(sources["contract.rs"].includes('"codex-app-server-stdio-jsonrpc"'));
  assert.ok(sources["config.rs"].includes("ProtocolConfig::from_params") === false);
  assert.ok(sources["config.rs"].includes("fn from_params"));
  assert.ok(sources["parser/session.rs"].includes('"thread/start"'));
  assert.ok(sources["parser/session.rs"].includes('"thread/resume"'));
  assert.ok(sources["parser/session.rs"].includes('"turn/start"'));
  assert.ok(sources["parser/events.rs"].includes('"turn/completed"'));
  assert.ok(sources["parser/events.rs"].includes("matches_current_ids"));
  assert.ok(sources["parser/control.rs"].includes("fn decline_server_request"));
  assert.ok(sources["parser/control.rs"].includes("ProtocolEffect::Send"));
  assert.equal(sources["parser/control.rs"].includes("ProtocolEffect::Fail"), false);
  assert.equal(sources["parser/control.rs"].includes("ProtocolFailure::user_interaction"), false);
  assert.ok(sources["failure.rs"].includes('message: &\'static str'));
  assert.ok(sources["parser/session.rs"].includes("self.session_id = Some(thread_id.to_string())"));
  assert.ok(sources["parser.rs"].includes("fn parse_line"));
  assert.ok(sources["reserve.rs"].includes("authorized_luna_reserve_model"));
  assert.ok(sources["reserve.rs"].includes("ordinaryUsageAllowed"));
  assert.equal(sources["driver/io.rs"].includes("serde_json::from"), false);

  for (const duplicatedCodec of ["AcpProtocol", "AcpSessionPlan", '"session/new"']) {
    assert.equal(joined.includes(duplicatedCodec), false);
  }
  for (const retiredModulePattern of ["#[path", "include!("]) {
    assert.equal(joined.includes(retiredModulePattern), false);
  }
});

test("Codex transport stays bounded, supervised, and redacted", async () => {
  const sources = await readLeaves(productionLeaves);
  const ioSource = sources["driver/io.rs"];
  const supervision = sources["driver/supervision.rs"];
  const transport = sources["driver/transport.rs"];
  const launch = sources["driver/launch.rs"];

  for (const token of [
    "max_bytes",
    "StdoutLimitExceeded",
    "drain_stderr",
    "AtomicBool",
  ]) {
    assert.ok(ioSource.includes(token), `missing bounded IO token: ${token}`);
  }
  assert.ok(transport.includes("finish_protocol_transport"));
  for (const token of ["PROCESS_POLL_INTERVAL", "contextualize", "terminate_tree"]) {
    assert.ok(supervision.includes(token), `missing supervision token: ${token}`);
  }
  assert.ok(launch.includes('"app-server"'));
  assert.ok(launch.includes('"--stdio"'));
  assert.ok(launch.includes("apply_subagent_caller_context"));
  assert.equal(launch.includes("apply_mcp_runtime_root"), false);
  for (const rawProjection of [
    "stderr: String",
    "stderr: Vec",
    "String::from_utf8_lossy(&stderr",
    "read_to_string",
    "eprintln!",
  ]) {
    assert.equal(ioSource.includes(rawProjection), false);
    assert.equal(supervision.includes(rawProjection), false);
    assert.equal(transport.includes(rawProjection), false);
  }
});

test("Codex regressions remain independently selectable ordinary leaves", async () => {
  const entries = await fs.readdir(path.join(repoRoot, driverRoot, "tests"), {
    withFileTypes: true,
  });
  assert.deepEqual(
    entries
      .filter((entry) => entry.isFile() && entry.name.endsWith(".rs"))
      .map((entry) => entry.name)
      .sort(),
    [...testLeaves].sort(),
  );
  const testFacade = await read(`${driverRoot}/tests.rs`);
  assert.equal(testFacade.includes("mod tests {"), false);
  assert.equal(testFacade.includes("#[path"), false);
});
