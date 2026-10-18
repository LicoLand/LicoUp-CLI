import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// The Hermes adapter package: the persistent ACP frame dialect the Agent
// answers with, the parser that classifies it once, the permission question it
// asks, the normalized transitions its turns reduce to, the registration
// composition injects and the arm that replays a recorded transcript. One
// package carries one Agent's protocol.
const packageRoot = "crates/licoup-agent-hermes";
const sourceRoot = `${packageRoot}/src`;
const packageFiles = Object.freeze([
  "bin/lico-agent-hermes",
  "contributions/adapter-status.json",
  "manifest.json",
  "package-release.json",
]);
// The composition above the adapter port, and the two places the Hermes dialect
// is installed and replayed from.
const compositionPath =
  "crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs";
const replayCompositionPath =
  "crates/licoup-native/src/platform/native_agent_parser/replay/adapters/mod.rs";
const dialectsPath = "crates/licoup-native/src/platform/runtime_adapters/dialects.rs";
const driversPath = "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
// The parser subtrees this package replaced. The host keeps no copy of any of
// them: the parser is the package's, and the two superseded files that once
// accompanied it were the shared ACP session engine's own copies.
const movedParserPaths = Object.freeze([
  "crates/licoup-native/src/platform/native_agent_parser/adapters/hermes.rs",
  "crates/licoup-native/src/platform/native_agent_parser/adapters/hermes/framing.rs",
  "crates/licoup-native/src/platform/native_agent_parser/adapters/hermes/protocol.rs",
  "crates/licoup-native/src/platform/native_agent_parser/replay/adapters/hermes.rs",
]);
// The Hermes driver module and tree the host composed before the process half
// moved into this package. The host keeps neither.
const retiredDriverPaths = Object.freeze([
  "crates/licoup-native/src/platform/hermes_driver.rs",
  "crates/licoup-native/src/platform/hermes_driver",
]);
// The Gateway lane the host composed until VENDOR-CODE-REMOVAL completed the
// retirement: Hermes' TUI Gateway transport, the turn it drives and the history
// page over it all belong to this package now. The host keeps no copy of any of
// them, and declares no module for them.
const retiredGatewayPaths = Object.freeze([
  "crates/licoup-native/src/platform/hermes_tui_gateway.rs",
  "crates/licoup-native/src/platform/hermes_tui_gateway",
  "crates/licoup-native/src/platform/hermes_tui_gateway_driver.rs",
  "crates/licoup-native/src/platform/remote_hermes_gateway_history.rs",
]);

function read(relativePath) {
  return readFileSync(path.join(repoRoot, relativePath), "utf8");
}

test("one package carries the Hermes dialect, its parser and its program", () => {
  const library = read(`${sourceRoot}/lib.rs`);
  assert.deepEqual(
    [...library.matchAll(/^pub mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    ["dialect", "driver", "parser", "registration", "remote_gateway_history",
      "replay", "tui_gateway", "tui_gateway_driver"],
  );
  // The replay arm is a test surface, so it is a feature rather than a
  // production module — and the feature is what the host's test build enables.
  assert.match(library, /#\[cfg\(any\(test, feature = "test-support"\)\)\]\npub mod replay;/u);
  // This package owns a protocol and the process half that speaks it, not a host
  // seam: the shared ACP transport, the login-shell launch environment and the
  // target contract are all lower crates, so the package asks its host for
  // nothing and declares no port.
  assert.equal(existsSync(path.join(repoRoot, `${sourceRoot}/port`)), false,
    "the package declares no port because it asks its host for no fact");
  assert.equal(library.includes("pub mod port;"), false);

  // The parser is the single ingress, and the dialect answers the transport port
  // with this package's own functions rather than re-implementing ACP semantics.
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
  assert.match(dialect, /driver_id: DRIVER_ID/u);
  // Hermes is the one Agent whose permission question is projected rather than
  // shared: the summary and the tool list are this package's own facts.
  assert.match(dialect, /parser::permission_request/u);
  assert.match(dialect, /ProtocolPermissionRequest/u);
  // Hermes asks no client request over this profile, and states that rather
  // than borrowing another Agent's reader.
  assert.match(dialect, /pub fn no_client_request\(_: &Value\) -> Option<ProtocolClientRequest>/u);

  // The parser declares the adapter contract exactly once, in this package.
  const parser = read(`${sourceRoot}/parser.rs`);
  assert.equal((parser.match(/AdapterContract::new\(/gu) ?? []).length, 1);
  assert.match(parser, /AdapterContract::new\("hermes", "stdio-jsonrpc-acp"\)/u);
  // The two facts no other Agent owns: the permission question and the
  // transition vocabulary the host reads through the registration.
  assert.match(parser, /pub fn permission_request\(message: &Value\) -> Option<PermissionRequest>/u);
  assert.match(parser, /unit_id: "hermes:reply"/u);
  assert.match(parser, /pub fn completed_transitions\(output: &str\) -> Vec<Transition>/u);
  assert.match(parser, /pub fn failed_transitions\(code: &str, stage: &str, message: &str\)/u);

  // The registration answers the SDK's normalized-transition query with its own
  // builders and keeps the durable-identity query fail-closed. Both facts are
  // the semantics the host read before the parser moved, so a package that
  // answered neither would silently empty every Hermes turn's transitions.
  const registration = read(`${sourceRoot}/registration.rs`);
  assert.match(registration, /ParserRegistration::new\(\s*CONTRACT,\s*execution_transitions,\s*no_identity,?\s*\)/u);
  assert.match(registration, /pub fn execution_transitions\(outcome: &ExecutionOutcome<'_>\)/u);
  assert.match(registration, /pub fn no_identity\(_: &DurableIdentityRequest<'_>\) -> bool \{\s*false\s*\}/u);
  assert.doesNotMatch(registration, /ParserRegistration::unanswered\(/u,
    "Hermes' registration answers its transitions rather than declaring them unanswered");

  // The driver half declares the launch and probe contract and delegates to the
  // shared engine and the lower-crate host facilities, so the package describes
  // a Hermes execution without owning an ACP transport or a second copy of the
  // login-shell environment.
  const driver = read(`${sourceRoot}/driver.rs`);
  assert.match(driver, /AcpSessionDriverSpec::new\("hermes-acp", &\["acp"\]\)/u);
  assert.match(driver, /with_runtime_id\("hermes"\)/u);
  assert.match(driver, /pub use execution::\{cancel, cleanup_session, execute_with_connection\};/u);
  assert.match(driver, /pub use probe::probe;/u);
  for (const forbidden of ["std::process::Command", "TcpStream", "reqwest", "ureq"]) {
    assert.equal(driver.includes(forbidden), false,
      `the package may not open its own route to the Agent: ${forbidden}`);
  }
  const execution = read(`${sourceRoot}/driver/execution.rs`);
  assert.match(execution, /licoup_agent_drivers::acp_session_transport/u);
  assert.match(execution, /licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection/u);
  // The ACP driver leaf is ACP only: the Gateway lane is the package's own too
  // ([`tui_gateway`], [`tui_gateway_driver`]), and the composed choice between
  // the two belongs to the composition that can see the runtime connection.
  assert.equal(execution.includes("hermes_tui_gateway"), false,
    "the ACP driver leaf reaches for the Gateway lane the composition picks");
  const probe = read(`${sourceRoot}/driver/probe.rs`);
  assert.match(probe, /licoup_agent_targets::platform::user_shell_environment::apply_to_command/u);
  assert.match(probe, /licoup_foundation::platform::process_supervisor::configure_untrusted_agent_command/u);

  // Nothing in the package reaches a client crate.
  for (const file of ["lib.rs", "dialect.rs", "driver.rs", "driver/execution.rs",
    "driver/probe.rs", "parser.rs", "registration.rs", "replay.rs",
    "remote_gateway_history.rs", "tui_gateway.rs", "tui_gateway_driver.rs",
    "bin/lico-agent-hermes.rs"]) {
    const source = read(`${sourceRoot}/${file}`);
    assert.equal(source.includes("licoup_native"), false,
      `${file} reaches back into the client`);
  }
});

test("the committed package artifact is the one the host reads", () => {
  for (const file of packageFiles) {
    assert.ok(existsSync(path.join(repoRoot, `${packageRoot}/package/${file}`)),
      `the payload is missing ${file}`);
  }
  const manifest = JSON.parse(read(`${packageRoot}/package/manifest.json`));
  assert.equal(manifest.id, "org.licoland.adapter.hermes");
  assert.equal(manifest.runtime.mode, "process");
  assert.equal(manifest.runtime.entry, "bin/lico-agent-hermes");
  assert.equal(manifest.runtime.runtimeRef, undefined);
  assert.deepEqual(manifest.profiles.map((profile) => profile.id), ["agent-execution"]);
  assert.deepEqual(manifest.profiles[0].capabilities, ["agent-execution.v1"]);
  const release = JSON.parse(read(`${packageRoot}/package/package-release.json`));
  assert.equal(release.packageId, manifest.id);
  assert.equal(release.packageVersion, manifest.version);
  assert.equal(release.converter.kind, "native-executable");
  assert.equal(release.converter.entry, manifest.runtime.entry);
  assert.equal(release.converter.sourceFormat, "hermes.acp.v1");
  assert.equal(release.converter.targetFormat, "licoup.conversation.v1");
  // The release declaration names the format the package's own dialect
  // publishes, so the document and the program cannot disagree.
  assert.match(read(`${sourceRoot}/dialect.rs`),
    /pub const PROTOCOL_FORMAT: &str = "hermes\.acp\.v1";/u);

  // The package owns no persisted data, and says so rather than leaving a
  // reader to infer it.
  assert.equal(manifest.conversion, undefined);
  assert.equal(manifest.extensions["org.licoland.adapter.hermes/persistentData"], "none");
  assert.equal(typeof manifest.extensions["org.licoland.adapter.hermes/persistentDataReason"],
    "string");
});

test("the moved Hermes parser leaves no copy in the host and the inventory keeps thirteen", () => {
  for (const relativePath of movedParserPaths) {
    assert.equal(existsSync(path.join(repoRoot, relativePath)), false,
      `the host still carries a copy of the moved Hermes parser: ${relativePath}`);
  }
  // The driver half moved the same way: the kernel declares no Hermes driver
  // module or tree, and the composition names the package's own driver.
  for (const relativePath of retiredDriverPaths) {
    assert.equal(existsSync(path.join(repoRoot, relativePath)), false,
      `the host still carries the retired Hermes driver: ${relativePath}`);
  }
  // The Gateway lane was the last Hermes half the host still composed. It
  // travelled into the package with the driver, so the host carries neither the
  // transport, its turn, its history page, nor a module declaration for any of
  // them — a kernel copy reappearing fails here.
  for (const relativePath of retiredGatewayPaths) {
    assert.equal(existsSync(path.join(repoRoot, relativePath)), false,
      `the host still carries the retired Hermes Gateway lane: ${relativePath}`);
  }
  const platform = read("crates/licoup-native/src/platform/mod.rs");
  assert.doesNotMatch(platform, /mod hermes_driver;/u,
    "the host module tree still declares a Hermes driver module");
  for (const retiredModule of ["hermes_tui_gateway", "hermes_tui_gateway_driver",
    "remote_hermes_gateway_history"]) {
    assert.doesNotMatch(platform, new RegExp(`mod ${retiredModule};`, "u"),
      `the host module tree still declares ${retiredModule}`);
  }
  const drivers = read(driversPath);
  assert.match(drivers, /use licoup_agent_hermes::driver as hermes_driver;/u,
    "the composition does not read the package's driver");
  // The composition keeps no second copy of the vendor fact: the launch
  // argument, the runtime identity and the probe commands are the package's,
  // and so is the Gateway lane's protocol identity.
  for (const forbidden of [
    '"hermes-acp-stdio-jsonrpc"',
    '"hermes-tui-gateway-stdio-jsonrpc"',
    '"acp", "--check"',
    '"acp", "--version"',
    "hermes_acp_probe_failed",
    "AcpSessionDriverSpec::new",
  ]) {
    assert.equal(drivers.includes(forbidden), false,
      `the client composition keeps a copy of the package's protocol: ${forbidden}`);
  }
  // The one Hermes lane the composition still chooses is the TUI gateway, and
  // the choice is the host's because the runtime connection is in view here.
  // Both halves of it are read from the package: the lane's identity and its
  // turn entry, named through the package's own modules.
  assert.match(drivers, /licoup_agent_hermes::tui_gateway::RUNTIME_PROTOCOL/u);
  assert.match(drivers,
    /use licoup_agent_hermes::tui_gateway_driver as hermes_tui_gateway_driver;/u);
  assert.match(drivers, /hermes_tui_gateway_driver::execute\(/u);

  const composition = read(compositionPath);
  // The composition dispatches the package's own registration and declares no
  // parser path, module or alias of its own for Hermes: nothing above the port
  // reads a host copy, and an alias nothing reads is the forwarding shell the
  // composition's own boundary contract reports as an unused import.
  assert.equal(composition.includes("licoup_agent_hermes::parser as hermes"), false,
    "the composition keeps a Hermes parser alias nothing reads");
  assert.ok(composition.includes("licoup_agent_hermes::registration::REGISTRATION"),
    "the composition does not dispatch the package's own registration");
  assert.doesNotMatch(composition, /mod hermes;/u);
  // The transitions the host reads are the package's answer rather than a second
  // builder kept above the port.
  assert.equal(composition.includes("hermes_transitions"), false,
    "the composition keeps a second copy of Hermes' transition answer");
  assert.equal(composition.includes("hermes::CONTRACT"), false,
    "the composition restates the package's adapter declaration");
  // The inventory is the authority for how many entries the composition holds,
  // so a moved Agent cannot leave a stale count behind: every packaged adapter
  // contributes the package's own registration, and every other adapter one
  // declared entry.
  const adapterIds = JSON.parse(
    read("crates/licoup-native/resources/agent-conversation-drivers.json"))
    .drivers.map((driver) => driver.agentId);
  const packageEntries =
    (composition.match(/licoup_agent_\w+::registration::REGISTRATION/gu) ?? []).length;
  const packagedAdapters = readdirSync(path.join(repoRoot, "crates"), { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.startsWith("licoup-agent-"))
    .map((entry) => `crates/${entry.name}/package/manifest.json`)
    .filter((relativePath) => existsSync(path.join(repoRoot, relativePath)))
    .filter((relativePath) => read(relativePath).includes("agent-execution.v1"));
  assert.equal(packageEntries, packagedAdapters.length,
    "the composition dispatches one package registration per packaged adapter");
  const hostedEntries =
    (composition.match(/ParserRegistration::(?:unanswered|new)\(/gu) ?? []).length;
  assert.equal(hostedEntries + packageEntries, adapterIds.length);
  const registrations = composition.slice(
    composition.indexOf("pub(in crate::platform) static REGISTRATIONS"),
    composition.indexOf("/// The parser registrations this host injects"),
  );
  assert.equal((registrations.match(/^\s{4}(?:ParserRegistration::\w+\(|licoup_agent_\w+::registration::REGISTRATION,)/gmu)
    ?? []).length, adapterIds.length);

  // The replay arm moved with the parser: the composition builds it from the
  // package and declares no arm module for Hermes.
  const replay = read(replayCompositionPath);
  assert.ok(replay.includes("licoup_agent_hermes::replay::replay_arm"),
    "the replay composition does not build the package's Hermes arm");
  assert.doesNotMatch(replay, /mod hermes;/u);
  assert.equal(replay.includes("hermes::Replay"), false,
    "the replay composition keeps a second Hermes arm");
  // One arm per packaged Agent, matched against the same inventory rather than
  // against a literal, and each Agent exactly once.
  const arms = [...replay.matchAll(/^\s{8}"([a-z-]+)" =>/gmu)].map((match) => match[1]);
  assert.deepEqual([...arms].sort(), [...adapterIds].sort(),
    "the replay composition builds exactly one arm per packaged adapter");
  assert.equal(new Set(arms).size, arms.length);
  // The three shared-ACP Agents keep one arm each rather than one per merge.
  assert.equal((replay.match(/"copilot" =>/gu) ?? []).length, 1);
  assert.equal((replay.match(/"cursor" =>/gu) ?? []).length, 1);
  assert.equal((replay.match(/"deepseek-harness" =>/gu) ?? []).length, 1);
});

test("the persistent ACP dialect is installed from the package and named nowhere else", () => {
  const drivers = read(driversPath);
  // The dialect table installs the package's registration whole instead of
  // restating Hermes' members above the port.
  assert.ok(drivers.includes("licoup_agent_hermes::dialect::registration()"),
    "the dialect table does not install the package's Hermes dialect");
  assert.equal(drivers.includes("hermes::decode_frame"), false,
    "the dialect table restates a Hermes parser member");
  // The dialect identity has one authority: the table and the run path read the
  // package's own declaration rather than a second literal that could drift
  // away from the identity the transport resolves frames by.
  assert.equal(drivers.includes('"hermes-acp"'), false,
    "the composition restates the Hermes dialect identity");
  assert.equal((drivers.match(/licoup_agent_hermes::dialect::DRIVER_ID/gu) ?? []).length, 2);

  // The composition above the transport names no Hermes parser function of its
  // own: the projection Hermes' parser owns lives in the package.
  const dialects = read(dialectsPath);
  assert.equal(dialects.includes("hermes"), false,
    "the host still assembles a Hermes dialect projection");
  assert.equal(dialects.includes("native_agent_parser::adapters::hermes"), false);

  // The production caller is real: a Hermes turn's normalized transitions are
  // read from the registration the composition dispatches, which is the
  // package's own answer.
  const hermesRun = read(driversPath);
  assert.match(hermesRun, /registrations_parser\("hermes"\)/u);
  assert.match(hermesRun, /normalize_hermes\(/u);
  const normalization = read(
    "crates/licoup-agent-drivers/src/runtime_adapters/normalization.rs",
  );
  assert.match(normalization, /pub fn normalize_hermes\(/u);
  assert.match(normalization, /\(parser\.execution_transitions\)\(/u);
});
