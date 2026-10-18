import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// The Kimi Code adapter package: the ACP frame dialect the Agent answers with,
// the parser that classifies it once, the driver half the shared ACP engine
// runs, the registration composition injects and the arm that replays a
// recorded transcript. One package carries one Agent's protocol.
const packageRoot = "crates/licoup-agent-kimi";
const sourceRoot = `${packageRoot}/src`;
const packageFiles = Object.freeze([
  "bin/lico-agent-kimi",
  "contributions/adapter-status.json",
  "manifest.json",
  "package-release.json",
]);
// The kernel's composition, which names this package's driver directly. The
// host keeps no Kimi module of its own.
const compositionPath = "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
// The shared ACP engine, which names no Agent.
const enginePath = "crates/licoup-agent-drivers/src/acp_driver_runtime";

function read(relativePath) {
  return readFileSync(path.join(repoRoot, relativePath), "utf8");
}

function exists(relativePath) {
  try {
    readFileSync(path.join(repoRoot, relativePath));
    return true;
  } catch {
    return false;
  }
}

test("one package carries the Kimi Code dialect, its parser and its program", () => {
  const library = read(`${sourceRoot}/lib.rs`);
  assert.deepEqual(
    [...library.matchAll(/^pub mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    ["dialect", "driver", "parser", "port", "registration", "replay"],
  );
  // The replay arm is a test surface, so it is a feature rather than a
  // production module — and the feature is what the host's test build enables.
  assert.match(library, /#\[cfg\(any\(test, feature = "test-support"\)\)\]\npub mod replay;/u);

  // The parser is the single ingress: the dialect answers the transport port
  // with this package's own functions, and re-implements no ACP semantics.
  const dialect = read(`${sourceRoot}/dialect.rs`);
  for (const member of [
    "decode_frame",
    "is_notification",
    "response_id_matches",
    "response_is_error",
    "session_update",
    "prompt_stop_reason",
    "initialize_response",
    "client_request",
    "permission_request",
    "completed_transitions",
    "failed_transitions",
  ]) {
    assert.match(dialect, new RegExp(`^\\s*${member}[,:]`, "mu"),
      `the dialect does not answer the transport port's ${member}`);
  }
  assert.match(dialect, /AcpParserRegistration/u);
  assert.match(dialect, /ldr_id: DRIVER_ID|driver_id: DRIVER_ID/u);

  // The parser declares the adapter contract exactly once, in this package.
  const parser = read(`${sourceRoot}/parser.rs`);
  assert.equal((parser.match(/AdapterContract::new\(/gu) ?? []).length, 1);
  assert.match(parser, /AdapterContract::new\("kimi-code", "lf-ndjson-acp"\)/u);

  // The driver half declares the launch metadata and delegates the engine, so
  // the package describes a Kimi execution without owning an ACP transport.
  const driver = read(`${sourceRoot}/driver.rs`);
  assert.match(driver, /AcpDriverSpec::new\(RUNTIME_PROTOCOL, &\["acp"\]\)/u);
  assert.match(driver, /with_identity\(crate::dialect::DRIVER_ID, "kimi_code_acp"\)/u);
  assert.match(driver, /execute_acp\(/u);
  assert.match(driver, /probe_acp\(/u);
  for (const forbidden of ["std::process::Command", "TcpStream", "reqwest", "ureq"]) {
    assert.equal(driver.includes(forbidden), false,
      `the package may not open its own route to the Agent: ${forbidden}`);
  }

  // Nothing in the package reaches a client crate.
  for (const file of ["lib.rs", "dialect.rs", "parser.rs", "driver.rs", "registration.rs",
    "replay.rs", "port/mod.rs", "port/execution.rs", "bin/lico-agent-kimi.rs"]) {
    const source = read(`${sourceRoot}/${file}`);
    assert.equal(source.includes("licoup_native"), false,
      `${file} reaches back into the client`);
  }
});

test("the committed package artifact is the one the host reads", () => {
  for (const file of packageFiles) {
    assert.ok(exists(`${packageRoot}/package/${file}`), `the payload is missing ${file}`);
  }
  const manifest = JSON.parse(read(`${packageRoot}/package/manifest.json`));
  assert.equal(manifest.id, "org.licoland.adapter.kimi");
  assert.equal(manifest.runtime.mode, "process");
  assert.equal(manifest.runtime.entry, "bin/lico-agent-kimi");
  assert.equal(manifest.runtime.runtimeRef, undefined);
  assert.deepEqual(manifest.profiles.map((profile) => profile.id), ["agent-execution"]);
  assert.deepEqual(manifest.profiles[0].capabilities, ["agent-execution.v1"]);
  const release = JSON.parse(read(`${packageRoot}/package/package-release.json`));
  assert.equal(release.packageId, manifest.id);
  assert.equal(release.packageVersion, manifest.version);
  assert.equal(release.converter.kind, "native-executable");
  assert.equal(release.converter.entry, manifest.runtime.entry);
  assert.equal(release.converter.sourceFormat, "kimi-code.acp.v1");
  assert.equal(release.converter.targetFormat, "licoup.conversation.v1");

  // The package owns no persisted data, and says so rather than leaving a
  // reader to infer it.
  assert.equal(manifest.conversion, undefined);
  assert.equal(manifest.extensions["org.licoland.adapter.kimi/persistentData"], "none");
  assert.equal(typeof manifest.extensions["org.licoland.adapter.kimi/persistentDataReason"],
    "string");
});

test("the client names the package for the protocol and keeps no Kimi driver module", () => {
  // The kernel has no Kimi driver module at all: the host reaches the Agent
  // through the package and cannot hold a second owner of its protocol.
  assert.equal(
    exists("crates/licoup-native/src/platform/kimi_code_driver.rs"),
    false,
    "the host still declares a Kimi Code driver module",
  );
  assert.equal(
    exists("crates/licoup-native/src/platform/kimi_code_driver"),
    false,
    "the host still declares a Kimi Code driver tree",
  );
  const platform = read("crates/licoup-native/src/platform/mod.rs");
  assert.doesNotMatch(platform, /mod kimi_code_driver;/u,
    "the host module tree still declares a Kimi Code driver module");

  const composition = read(compositionPath);
  // The composition reads the package's driver, so the launch metadata, the
  // runtime protocol and the frame interpretation are the package's.
  assert.match(composition, /use licoup_agent_kimi::driver as kimi_code_driver;/u,
    "the composition does not read the package's driver");
  // The composition keeps no second copy of the vendor fact: every Kimi protocol
  // decision is the package's.
  for (const forbidden of [
    "AcpDriverSpec::new",
    "with_identity",
    "with_launch_settings",
    "with_allow_all_argument",
    "decode_json_line",
    "validate_session_update",
    "validate_prompt_response",
    "AdapterContract::new",
  ]) {
    assert.equal(composition.includes(forbidden), false,
      `the client composition keeps a copy of the package's protocol: ${forbidden}`);
  }
});

test("the shared ACP engine names no Agent and holds no Kimi branch", () => {
  const protocol = read(`${enginePath}/protocol.rs`);
  const port = read(`${enginePath}/parser_port.rs`);
  const replay = read(`${enginePath}/replay.rs`);
  // Kimi Code is named nowhere in the shared engine: the moved dialect reaches
  // the engines through the port and through the package's own arm, so no
  // vendor branch can reappear beside the shared reducer.
  for (const [name, source] of [["protocol", protocol], ["parser_port", port], ["replay", replay]]) {
    for (const agent of ["kimi", "codex", "hermes", "claude"]) {
      assert.equal(source.toLowerCase().includes(agent), false,
        `the shared ACP ${name} names the Agent ${agent}`);
    }
  }
  // The ACP profile's own state machine is a shared resource rather than a
  // vendor branch: the reducer reads the packaged machine and names no Agent.
  assert.match(protocol, /state_machines::copilot_protocol/u);
  // The dialect is resolved by the identity the transport is keyed on, never by
  // an Agent name.
  assert.match(port, /pub fn parser_for\(driver_id: &str\)/u);
  assert.match(protocol, /parser_port::parser_for\(driver_id\)/u);
  // An Agent's own package hands its dialect to the shared reducer rather than
  // the engine learning that Agent's name.
  assert.match(replay, /pub fn with_dialect\(registration: AcpParserRegistration\)/u);
});

test("the moved parser leaves no copy in the host and the inventory keeps thirteen", () => {
  assert.equal(exists("crates/licoup-native/src/platform/native_agent_parser/adapters/kimi_code.rs"),
    false, "the host still carries a copy of the moved Kimi parser");
  const composition = read(
    "crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs",
  );
  // The composition names no Kimi Code parser at all: this host parses none of
  // that Agent's frames, so the package's own dialect registration is what
  // reaches the transport and the parser registration is the package's own
  // value. An alias nothing reads would be a forwarding shell the compiler
  // reports as an unused import, so no path in this tree re-exports it.
  assert.deepEqual(
    composition.match(/^.*licoup_agent_kimi::parser.*$/gmu) ?? [],
    [],
    "the composition names no Kimi Code parser",
  );
  assert.doesNotMatch(composition, /mod kimi_code;/u);
  assert.match(composition, /licoup_agent_kimi::registration::REGISTRATION/u);
  assert.match(composition, /licoup_agent_codex::registration::REGISTRATION/u);
  // One declaration per Agent: the entries the host still answers itself plus
  // one package registration per moved parser. The moved set is derived from the
  // registrations the composition names rather than restated here, so a package
  // whose parser leaves the host cannot leave the count behind.
  const hostedEntries =
    (composition.match(/ParserRegistration::(?:unanswered|new)\(/gu) ?? []).length;
  const movedPackages = [
    ...composition.matchAll(
      /^ {4}licoup_agent_(\w+)::registration::REGISTRATION,$/gmu,
    ),
  ];
  assert.ok(
    movedPackages.some((match) => match[1] === "kimi"),
    "the composition names the Kimi Code package's own registration",
  );
  assert.equal(hostedEntries + movedPackages.length, 13);
  const registrations = composition.slice(
    composition.indexOf("pub(in crate::platform) static REGISTRATIONS"),
    composition.indexOf("/// The parser registrations this host injects"),
  );
  // Every Agent is declared exactly once: the host's own entries plus one
  // package registration each still add up to the thirteen-parser inventory.
  assert.equal((registrations.match(/^\s{4}(?:ParserRegistration::\w+\(|licoup_agent_\w+::registration::REGISTRATION,)/gmu)
    ?? []).length, 13);
});
