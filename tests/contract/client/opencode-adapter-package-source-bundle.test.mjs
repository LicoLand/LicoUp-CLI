import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// The OpenCode adapter package: the `opencode serve` HTTP and SSE protocol the
// Agent answers with, the parser that classifies it once, the Agent's own half
// of one turn, the endpoint contract that turn runs on, the registration
// composition injects, the arm that replays a recorded transcript, and the
// ports the client answers. One package carries one Agent.
const packageRoot = "crates/licoup-agent-opencode";
const sourceRoot = `${packageRoot}/src`;
const packageFiles = Object.freeze([
  "bin/lico-agent-opencode",
  "contributions/adapter-status.json",
  "manifest.json",
  "package-release.json",
]);
// The engine half the client still composes: the serve facade, its endpoint
// specification and the answer for the package's ports. They read the package
// and own no vendor protocol of their own.
const processHalfFiles = Object.freeze([
  "crates/licoup-native/src/platform/opencode_serve.rs",
  "crates/licoup-native/src/platform/opencode_serve/policy.rs",
  "crates/licoup-native/src/platform/opencode_host.rs",
]);
// Every path the retired kernel-side driver used to occupy. The host declares
// none of them any more: the driver is the package's.
const retiredDriverPaths = Object.freeze([
  "crates/licoup-native/src/platform/opencode_driver.rs",
  "crates/licoup-native/src/platform/opencode_driver",
  "crates/licoup-native/src/platform/opencode_driver/serve_transport.rs",
  "crates/licoup-native/src/platform/opencode_driver/continuity.rs",
  "crates/licoup-native/src/platform/opencode_driver/probe.rs",
  "crates/licoup-native/src/platform/opencode_driver/control.rs",
]);
const compositionPath =
  "crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs";
const replayCompositionPath =
  "crates/licoup-native/src/platform/native_agent_parser/replay/adapters/mod.rs";
const driverCompositionPath =
  "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
const movedParserPath =
  "crates/licoup-native/src/platform/native_agent_parser/adapters/opencode.rs";
const admissionGate = "crate::port::execution::admits_execution()";
const attachCall = "serve::ensure_attachment(executable)";

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

test("one package carries the OpenCode serve protocol, its parser and its program", () => {
  const library = read(`${sourceRoot}/lib.rs`);
  assert.deepEqual(
    [...library.matchAll(/^pub mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    ["driver", "host", "parser", "policy", "port", "registration", "replay"],
  );
  // The parser declares the adapter contract exactly once, in this package, and
  // every entry point the serve path reads is the package's own.
  const parser = read(`${sourceRoot}/parser.rs`);
  assert.equal((parser.match(/AdapterContract::new\(/gu) ?? []).length, 1);
  assert.match(parser, /AdapterContract::new\("opencode", "http-sse"\)/u);
  for (const member of [
    "struct ServeEventParser",
    "fn session_id",
    "fn session_collection",
    "fn health_ready",
    "fn readiness",
    "fn message",
    "fn completed_transitions",
    "fn failure_transitions",
  ]) {
    assert.ok(parser.includes(member), `the parser does not own ${member}`);
  }
  // The parser is the interpreter, not the engine: it starts no process, opens
  // no socket and carries no HTTP client.
  for (const forbidden of [
    "std::process::Command",
    "TcpStream",
    "TcpListener",
    "reqwest",
    "ureq",
    "std::net",
  ]) {
    assert.equal(parser.includes(forbidden), false,
      `the package may not open its own route to the Agent: ${forbidden}`);
  }
  // Nothing in the package reaches a client crate.
  for (const file of [
    "lib.rs",
    "parser.rs",
    "driver.rs",
    "driver/control.rs",
    "driver/continuity.rs",
    "driver/probe.rs",
    "driver/serve_transport.rs",
    "host.rs",
    "policy.rs",
    "registration.rs",
    "replay.rs",
    "port/mod.rs",
    "port/execution.rs",
    "port/serve.rs",
    "port/turn_event.rs",
    "bin/lico-agent-opencode.rs",
  ]) {
    assert.equal(read(`${sourceRoot}/${file}`).includes("licoup_native"), false,
      `${file} reaches back into the client`);
  }
  // The driver performs a turn through the ports the host answers; the one
  // engine entry it names directly is the shared active-turn registry's control
  // answer, which needs no translation between the two vocabularies.
  const transport = read(`${sourceRoot}/driver/serve_transport.rs`);
  assert.doesNotMatch(transport, /local_service::(?:http|sse|serve)/u,
    "the package's turn opens an engine route of its own");
  assert.doesNotMatch(transport, /turn_control/u,
    "the package's turn reaches the control registry outside its control entry");
  const control = read(`${sourceRoot}/driver/control.rs`);
  assert.match(control, /licoup_agent_drivers::local_service::turn_control/u);
  assert.match(control, /turn_control::cancel\(OPENCODE_DRIVER\.agent_id, session_id\)/u);
});

test("the committed package artifact is the one the host reads", () => {
  for (const file of packageFiles) {
    assert.ok(exists(`${packageRoot}/package/${file}`), `the payload is missing ${file}`);
  }
  const manifest = JSON.parse(read(`${packageRoot}/package/manifest.json`));
  assert.equal(manifest.id, "org.licoland.adapter.opencode");
  assert.equal(manifest.runtime.mode, "process");
  assert.equal(manifest.runtime.entry, "bin/lico-agent-opencode");
  assert.equal(manifest.runtime.runtimeRef, undefined);
  assert.deepEqual(manifest.profiles.map((profile) => profile.id), ["agent-execution"]);
  assert.deepEqual(manifest.profiles[0].capabilities, ["agent-execution.v1"]);
  const release = JSON.parse(read(`${packageRoot}/package/package-release.json`));
  assert.equal(release.packageId, manifest.id);
  assert.equal(release.packageVersion, manifest.version);
  assert.equal(release.converter.kind, "native-executable");
  assert.equal(release.converter.entry, manifest.runtime.entry);
  // The inbound format is the wire the package's parser reads, named by the
  // package rather than retyped in the release document.
  const registration = read(`${sourceRoot}/registration.rs`);
  const declared = registration.match(/PROTOCOL_FORMAT: &str = "([^"]+)"/u);
  assert.ok(declared, "the package declares the protocol it reads");
  assert.equal(release.converter.sourceFormat, declared[1]);
  assert.equal(release.converter.targetFormat, "licoup.conversation.v1");

  // The package owns no persisted data, and says so rather than leaving a
  // reader to infer it.
  assert.equal(manifest.conversion, undefined);
  assert.equal(manifest.extensions["org.licoland.adapter.opencode/persistentData"], "none");
  assert.equal(typeof manifest.extensions["org.licoland.adapter.opencode/persistentDataReason"],
    "string");
});

test("the client keeps the engine half, names the package and declares no driver of its own", () => {
  // The kernel declares no OpenCode driver module and keeps no OpenCode driver
  // tree: the launch declaration, the session-open protocol, the request shape,
  // the stream classification, the projection and their tests are the package's.
  for (const retired of retiredDriverPaths) {
    assert.equal(exists(retired), false,
      `the host still carries the retired OpenCode driver path ${retired}`);
  }
  const platform = read("crates/licoup-native/src/platform/mod.rs");
  assert.doesNotMatch(platform, /mod opencode_driver;/u,
    "the host still declares an OpenCode driver module");
  assert.match(platform, /pub\(crate\) mod opencode_host;/u,
    "the host does not declare the package's port answer");
  // Composition reads the package the way the Codex arm reads its own.
  const driverComposition = read(driverCompositionPath);
  assert.match(driverComposition, /use licoup_agent_opencode::driver as opencode_driver;/u);
  assert.doesNotMatch(driverComposition, /use crate::platform::\{[^}]*opencode_driver/u,
    "the composition still reads a kernel OpenCode driver module");
  assert.doesNotMatch(driverComposition, /opencode-serve-http-v1/u,
    "the composition restates the package's protocol");

  // The engine half is the client's; the frame interpretation is not. No file on
  // this side re-reads a serve frame or keeps a second copy of the parser.
  for (const relativePath of processHalfFiles) {
    const source = read(relativePath);
    assert.ok(source.includes("opencode"),
      `${relativePath} is not part of the OpenCode engine half`);
    for (const forbidden of [
      "message.part.updated",
      "assistant_messages",
      "struct ServeEventParser",
      "AdapterContract::new",
    ]) {
      assert.equal(source.includes(forbidden), false,
        `${relativePath} keeps a copy of the package's protocol: ${forbidden}`);
    }
  }
  // The facade classifies nothing: the readiness reader is the package's, named
  // by the endpoint specification through the composition's own name for that
  // parser, and the facade only frames what the engine hands it.
  const facade = read("crates/licoup-native/src/platform/opencode_serve.rs");
  assert.doesNotMatch(facade, /adapters::opencode/u,
    "the serve facade still classifies the package's frames");
  assert.match(facade, /local_service::sse::watch_frames/u);
  const policy = read("crates/licoup-native/src/platform/opencode_serve/policy.rs");
  assert.match(policy, /native_agent_parser::adapters::opencode::readiness/u);
  // The endpoint facts are the package's, field for field, and the client keeps
  // no second copy of them.
  assert.match(policy, /licoup_agent_opencode::policy as vendor/u);
  for (const field of [
    "identity: vendor::SPEC.identity",
    "default_port: vendor::SPEC.default_port",
    "reserved_ports: vendor::SPEC.reserved_ports",
    "executable_environment: vendor::SPEC.executable_environment",
    "default_executable: vendor::SPEC.default_executable",
    "executable_missing: vendor::SPEC.errors.executable_missing",
    "stop_failed: vendor::SPEC.errors.stop_failed",
  ]) {
    assert.ok(policy.includes(field), `the engine specification does not read ${field}`);
  }
  // The launcher is the engine's own half, so it is stated here and nowhere
  // else the host composes.
  assert.match(policy, /fn configure_command\(command: &mut Command, host: &str, port: u16\)/u);
  // Nothing the package owns is restated beside it.
  for (const restated of ["24173", "opencode-serve", "opencode_executable_missing"]) {
    assert.equal(policy.includes(restated), false,
      `the host policy restates the package's endpoint fact ${restated}`);
  }
});

test("the assignment this package reads is the host's, and the host installs it", () => {
  // The host owns the idle-admission decision and joins it to the package's
  // ports once per process.
  const composition = read("crates/licoup-native/src/lib.rs");
  assert.match(composition, /licoup_agent_opencode::port::execution::install\(/u);
  assert.match(composition, /admits_execution: admits_agent_execution/u);
  assert.match(composition, /licoup_agent_opencode::host::install\(/u);
  assert.match(composition, /platform::opencode_host::host_ports\(\)/u);
  // The package's own turn asks the package's admission port before it attaches,
  // so a turn started under the close-admission barrier is refused rather than
  // run.
  const transport = read(`${sourceRoot}/driver/serve_transport.rs`);
  const gate = transport.indexOf(admissionGate);
  assert.notEqual(gate, -1, "the driver never asks the package's admission port");
  const attach = transport.indexOf(attachCall);
  assert.notEqual(attach, -1, "the driver no longer attaches through the serve port");
  assert.ok(gate < attach,
    "the admission gate must be read before the endpoint is attached");
  assert.match(transport, /opencode_execution_admission_closed/u);
  // The host answers that port from this client's engine, and the answer names
  // no vendor field or failure code of its own: the engine is protocol-agnostic
  // and the vocabulary is the package's.
  const host = read("crates/licoup-native/src/platform/opencode_host.rs");
  assert.match(host, /pub\(crate\) fn serve_port\(\) -> ServePort/u);
  assert.doesNotMatch(host, /opencode_serve_/u,
    "the host answer restates the package's own failure vocabulary");
});

test("the moved protocol leaves no copy in the host and the composition keeps one entry", () => {
  assert.equal(exists(movedParserPath), false,
    "the host still carries a copy of the moved OpenCode parser");
  assert.equal(
    exists("crates/licoup-native/src/platform/native_agent_parser/replay/adapters/opencode.rs"),
    false,
    "the host still carries a copy of the moved OpenCode replay arm");
  const composition = read(compositionPath);
  assert.match(composition, /use licoup_agent_opencode::parser as opencode;/u);
  assert.doesNotMatch(composition, /mod opencode;/u);
  assert.doesNotMatch(composition, /AdapterContract::new\("opencode"/u,
    "the composition restates the package's adapter declaration");
  assert.match(composition, /licoup_agent_opencode::registration::REGISTRATION/u);
  const registrations = composition.slice(
    composition.indexOf("pub(in crate::platform) static REGISTRATIONS"),
    composition.indexOf("/// The parser registrations this host injects"),
  );
  assert.equal(
    (registrations.match(/licoup_agent_opencode::registration::REGISTRATION/gu) ?? []).length,
    1,
    "the composition declares the OpenCode parser more than once",
  );
  const replay = read(replayCompositionPath);
  assert.doesNotMatch(replay, /mod opencode;/u);
  assert.match(replay, /"opencode" => licoup_agent_opencode::replay::replay_arm\(adapter_id\)\?/u);
  // The parity evidence is the package's own: its corpus test drives this
  // package's registration and replay arm through the SDK harness.
  const corpus = read(`${packageRoot}/tests/replay_corpus.rs`);
  assert.match(corpus, /licoup_agent_opencode::registration::parser_set/u);
  assert.match(corpus, /replay_corpus\(/u);
});
