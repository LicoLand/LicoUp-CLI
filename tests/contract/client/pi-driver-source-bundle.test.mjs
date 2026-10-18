import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// The Pi adapter package owns the whole Pi driver: the RPC JSONL vocabulary,
// the parser that classifies one frame exactly once, and the process half that
// runs a turn. One package carries one Agent's protocol and its program.
const packageRoot = "crates/licoup-agent-pi/src";
const driverRoot = `${packageRoot}/driver`;
// The kernel's composition, which names this package's driver directly. The
// host declares no Pi driver module and keeps no Pi protocol of its own.
const compositionPath = "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
const platformModulePath = "crates/licoup-native/src/platform/mod.rs";
const retiredModulePath = "crates/licoup-native/src/platform/pi_driver.rs";
const retiredTreePath = "crates/licoup-native/src/platform/pi_driver";
// The parser tree that used to re-export this package's parser under the
// kernel's former `pi` name.
const parserAdaptersPath =
  "crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs";
// The one Pi claim the host keeps, because the host owns the turn-event
// consumer: it drives the package's driver and observes what reached it.
const hostConsumerSuitePath =
  "crates/licoup-native/src/platform/runtime_adapters/tests/pi_turn_events.rs";

// The driver half the package owns: the launch and supervision, the bounded IO,
// the active-turn control, the protocol loop, the probe and the vocabulary.
const driverLeaves = Object.freeze([
  "active_control.rs",
  "errors.rs",
  "execution.rs",
  "io.rs",
  "model.rs",
  "params.rs",
  "probe.rs",
  "sessions.rs",
  "supervision.rs",
]);
// The claims that drive those leaves, moved beside them from the kernel tree.
const driverTestLeaves = Object.freeze([
  "composition.rs",
  "errors.rs",
  "execution.rs",
  "interaction.rs",
  "io.rs",
  "model.rs",
  "params.rs",
  "parser_events.rs",
  "parser_protocol.rs",
  "probe.rs",
  "sessions.rs",
  "settings.rs",
  "supervision.rs",
  "support.rs",
]);
const parserLeaves = Object.freeze(["events.rs", "protocol.rs"]);

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

function sources() {
  return Object.fromEntries([
    ["parser/pi.rs", read(`${packageRoot}/parser.rs`)],
    ...driverLeaves.map((leaf) => [`driver/${leaf}`, read(`${driverRoot}/${leaf}`)]),
    ...parserLeaves.map((leaf) => [`parser/${leaf}`, read(`${packageRoot}/parser/${leaf}`)]),
  ]);
}

test("the host declares no Pi driver module or tree and reads the package instead", () => {
  // The kernel keeps no Pi driver at all: the host reaches the Agent through the
  // package and cannot hold a second owner of its launch, its frames or its
  // turn phases.
  assert.equal(exists(retiredModulePath), false,
    "the host still declares a Pi driver module");
  assert.equal(exists(retiredTreePath), false,
    "the host still declares a Pi driver tree");
  assert.doesNotMatch(read(platformModulePath), /mod pi_driver;/u,
    "the host module tree still declares a Pi driver module");

  const composition = read(compositionPath);
  assert.match(composition, /^use licoup_agent_pi::driver as pi_driver;$/mu,
    "the composition does not read the package's driver");

  // The host's platform tree is the one route a Pi driver could still take into
  // this composition, so every import that reaches it is read as written: a
  // grouped `use crate::platform::{...};` and a single `use crate::platform::x;`
  // say the same thing, and neither shape may hide the name. The premise is
  // checked first, because a composition that no longer names the host's tree at
  // all would make the check below pass without inspecting anything.
  assert.match(composition, /crate\s*::[^;]*\bplatform\b/u,
    "the composition does not name the host's platform tree");
  const hostPlatformImports = [...composition.matchAll(/^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?use [^;]*;/gmu)]
    .map((match) => match[0])
    .filter((declaration) => /crate\s*::[^;]*\bplatform\b/u.test(declaration));
  for (const declaration of hostPlatformImports) {
    assert.doesNotMatch(declaration, /\bpi_driver\b/u,
      `the composition still reaches a Pi driver through the host's platform tree: ${declaration.trim()}`);
  }
  // A platform path used outside an import names the same retired owner, so the
  // whole composition is checked for it rather than only its import statements.
  assert.doesNotMatch(composition,
    /crate\s*::\s*platform\s*::\s*(?:\{[^}]*)?\bpi_driver\b/u,
    "the composition still names a Pi driver inside the host's platform tree");

  // The parser tree re-exports no Pi alias either: with the driver in the
  // package, no host leaf reads Pi's frames through the kernel's former name.
  const parserAdapters = read(parserAdaptersPath);
  assert.doesNotMatch(parserAdapters, /use licoup_agent_pi::parser as pi;/u,
    "the host parser tree still re-exports the package's parser as its own");
  assert.equal(parserAdapters.includes("pi_driver leaves read Pi's frames"), false,
    "the host parser tree still explains a retired Pi driver reader");

  // The host's own consumer suite names the package for the driver it drives and
  // for the parser whose frames it observes.
  const hostConsumerSuite = read(hostConsumerSuitePath);
  assert.match(hostConsumerSuite, /use licoup_agent_pi::driver::/u,
    "the host consumer suite does not name the package's driver");
  assert.match(hostConsumerSuite, /use licoup_agent_pi::parser::/u,
    "the host consumer suite does not name the package's parser");
});

test("the package's driver facade declares the whole driver, process half included", () => {
  const facade = read(`${driverRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    ["active_control.rs", "execution.rs", "io.rs", "probe.rs", "supervision.rs"],
  );
  assert.deepEqual(
    [...facade.matchAll(/^pub mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    ["errors", "model", "params", "sessions"],
  );
  // The entries the host's composition alias reads are the package's own
  // re-exports, so one Pi execution is described in exactly one place.
  for (const entry of [
    "pub use active_control::{ControlDisposition, steer};",
    "pub use execution::execute;",
    "pub use model::{CapabilityProbe, EffectiveSettings, RUNTIME_PROTOCOL, RunResult};",
    "pub use probe::probe;",
  ]) {
    assert.ok(facade.includes(entry), `the facade does not export ${entry}`);
  }
  for (const implementationToken of [
    "struct PiProtocol",
    "struct ProtocolConfig",
    "Command::new",
    "fn run_protocol_loop",
    "include!(",
    "#[path",
  ]) {
    assert.equal(facade.includes(implementationToken), false, implementationToken);
  }
  for (const leaf of driverLeaves) {
    assert.ok(exists(`${driverRoot}/${leaf}`), `the package does not carry driver/${leaf}`);
  }
  assert.ok(exists(`${driverRoot}/tests/mod.rs`), "the driver suite did not move");
  for (const leaf of driverTestLeaves) {
    assert.ok(exists(`${driverRoot}/tests/${leaf}`), `the driver suite lost ${leaf}`);
  }
});

test("Pi keeps the official fixed RPC JSONL lane without shell fallback", () => {
  const source = sources();
  const joined = Object.values(source).join("\n");
  assert.ok(source["driver/model.rs"].includes('RUNTIME_PROTOCOL: &str = "pi-rpc-stdio-jsonl"'));
  assert.ok(source["driver/supervision.rs"].includes(
    'LAUNCH_ARGS: &[&str] = &["--mode", "rpc", "--offline"]',
  ));
  assert.ok(source["driver/supervision.rs"].includes("Command::new(&self.executable)"));
  assert.ok(source["parser/protocol.rs"].includes('"type": "switch_session"'));
  assert.ok(source["parser/protocol.rs"].includes('"type": "prompt"'));
  assert.ok(source["driver/probe.rs"].includes('"--version"'));
  assert.ok(source["driver/probe.rs"].includes('"--help"'));
  assert.ok(source["driver/probe.rs"].includes(".stdout(Stdio::null())"));
  assert.ok(source["driver/probe.rs"].includes(".stderr(Stdio::null())"));

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

test("Pi exact-session resolution is complete and header parsing fails closed", () => {
  const source = sources();
  const activeControl = source["driver/active_control.rs"];
  const sessions = source["driver/sessions.rs"];
  const protocol = source["parser/protocol.rs"];
  for (const token of [
    "MAX_SESSION_HEADER_BYTES",
    "resolve_session_path_in_roots",
    "session_roots_from_sources",
    "pi_session_identity_ambiguous",
    "pi_session_not_found",
    "pi_session_header_line_too_large",
  ]) {
    assert.ok(sessions.includes(token), `missing Pi session boundary: ${token}`);
  }
  assert.equal(sessions.includes("MAX_SESSION_SCAN_FILES"), false);
  for (const token of [
    "switch_session",
    "pi_session_identity_mismatch",
    "pi_session_id_missing",
  ]) {
    assert.ok(protocol.includes(token), `missing Pi continuity boundary: ${token}`);
  }
  assert.ok(source["driver/params.rs"].includes("resolve_session_path(&requested_session_id)"));
  for (const token of [
    "MAX_ACTIVE_TURNS",
    "ACK_TIMEOUT",
    "expected_turn_id",
    "ControlDisposition::NoActiveTurn",
    "recv_timeout",
    "steer_is_bound_to_the_exact_active_turn",
  ]) {
    assert.ok(activeControl.includes(token), `missing Pi active-turn boundary: ${token}`);
  }
});

test("Pi IO, cleanup, events, and errors remain bounded and non-projecting", () => {
  const source = sources();
  const joined = Object.values(source).join("\n");
  for (const token of [
    "BoundedStdinWriter",
    "StdoutLimitExceeded",
    "max_stdout",
    "max_stderr",
    "finish_protocol_transport",
    "TransportFinishFailure::Lifecycle",
    "finish_or_terminate_tree",
    "pi_rpc_timeout",
    "PROCESS_POLL_INTERVAL",
  ]) {
    assert.ok(joined.includes(token), `missing Pi bounded lifecycle token: ${token}`);
  }
  assert.ok(source["driver/errors.rs"].includes("message: &'static str"));
  assert.ok(source["parser/events.rs"].includes("sanitized_event"));
  assert.ok(source["parser/pi.rs"].includes("decode_jsonl_line"));
  assert.ok(source["parser/pi.rs"].includes("classify_steer_response"));
  assert.ok(source["parser/pi.rs"].includes("session_header_has_id"));
  assert.ok(source["driver/params.rs"].includes("pi_private_instructions_unsupported"));
  assert.equal(source["parser/protocol.rs"].includes("privateInstructions"), false);
  assert.equal(source["driver/io.rs"].includes("serde_json::from_str"), false);
  assert.equal(source["driver/sessions.rs"].includes("serde_json::from_str"), false);
  assert.equal(source["driver/execution.rs"].includes('.get("type")'), false);
  for (const rawProjection of [
    "stderr: String",
    "stderr: Vec",
    "String::from_utf8_lossy(&stderr",
    '"arguments": message',
    '"message": message',
  ]) {
    assert.equal(joined.includes(rawProjection), false);
  }
});

test("Pi split contains no production unsafe or hidden compatibility include", () => {
  const source = sources();
  const joined = Object.values(source).join("\n");
  assert.equal(joined.includes("unsafe {"), false);
  assert.equal(joined.includes("include!("), false);
  assert.equal(joined.includes("#[path"), false);

  // The package owns the protocol, the process and no part of the client: a
  // client path in the package's sources would be the kernel reaching back in
  // through it.
  const packageSources = [
    source["parser/pi.rs"],
    ...parserLeaves.map((leaf) => source[`parser/${leaf}`]),
    ...driverLeaves.map((leaf) => source[`driver/${leaf}`]),
  ].join("\n");
  for (const clientPath of ["crate::platform", "licoup_native", "licoup-native"]) {
    assert.equal(packageSources.includes(clientPath), false, clientPath);
  }
});
