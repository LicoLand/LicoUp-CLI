import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// The kernel's composition, which names this package's driver directly. The host
// keeps no Antigravity driver module and no Antigravity driver tree of its own.
const compositionPath = "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
// The wire half and the process half, both owned by the Antigravity adapter
// package: the protocol moved there first and the driver followed it. One
// package carries one Agent's program; the client names the package for it
// instead of keeping a second copy.
const packageRoot = "crates/licoup-agent-antigravity/src";
const driverLeaf = `${packageRoot}/driver.rs`;
const driverRoot = `${packageRoot}/driver`;
const parserPath = `${packageRoot}/parser.rs`;
const hookPath = `${packageRoot}/hook.rs`;
const registrationPath = `${packageRoot}/registration.rs`;
const replayPath = `${packageRoot}/replay.rs`;
const parserLeafFacade = `${packageRoot}/parser`;
const portLifecycleTest = "crates/licoup-agent-antigravity/tests/port_lifecycle.rs";
const closedAdmissionTest = "crates/licoup-agent-antigravity/tests/closed_admission.rs";
const packageManifest = "crates/licoup-agent-antigravity/Cargo.toml";
const packageRelease = "crates/licoup-agent-antigravity/package";
const retiredGeneratedScript = "session-receipt-hook.sh";

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function exists(relativePath) {
  return fs.stat(path.join(repoRoot, relativePath)).then(() => true, () => false);
}

test("the kernel carries no second copy of the Antigravity protocol", async () => {
  // The parser tree is gone from the host: a `mod antigravity;` there would be a
  // second copy of the Agent's protocol, and the composition names the package.
  assert.equal(
    await exists("crates/licoup-native/src/platform/native_agent_parser/adapters/antigravity.rs"),
    false,
  );
  assert.equal(
    await exists(
      "crates/licoup-native/src/platform/native_agent_parser/replay/adapters/antigravity.rs",
    ),
    false,
  );
  const composition = await read(
    "crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs",
  );
  assert.ok(composition.includes("licoup_agent_antigravity::registration::REGISTRATION"));
  assert.equal(composition.includes("mod antigravity;"), false);
  // No parser alias stays here either: the driver that used to read the protocol
  // through this tree is the package's now and reaches the package's own
  // `parser`, so an alias nothing reads would be a forwarding shell over a
  // protocol the host does not own.
  assert.equal(composition.includes("licoup_agent_antigravity::parser"), false);
  assert.equal(composition.includes("as antigravity;"), false);
  const replay = await read(
    "crates/licoup-native/src/platform/native_agent_parser/replay/adapters/mod.rs",
  );
  assert.ok(replay.includes("licoup_agent_antigravity::replay::replay_arm"));
  assert.equal(replay.includes("mod antigravity;"), false);

  // The vendor protocol names live in the package, not in the host, and the
  // driver the host used to keep beside them is the package's now.
  const driverLeaves = await Promise.all(
    [driverLeaf, ...["model", "control", "errors", "probe", "auth", "hooks", "execution", "tests"]
      .map((leaf) => `${driverRoot}/${leaf}.rs`)].map((source) => read(source)),
  );
  const implementation = driverLeaves.join("\n");
  for (const duplicatedProtocol of [
    'AdapterContract::new("antigravity"',
    "PtyOutputParser {",
    "struct TerminalFacts",
    "fn classify_terminal",
    "fn parse_hook_receipt",
  ]) {
    assert.equal(
      implementation.includes(duplicatedProtocol),
      false,
      `the package's driver must not declare ${duplicatedProtocol}`,
    );
  }
});

test("the host declares no Antigravity driver and reads the package's", async () => {
  // A kernel module or tree for this Agent would be a second owner of its
  // program; both are gone, and the module tree declares neither.
  assert.equal(
    await exists("crates/licoup-native/src/platform/antigravity_driver.rs"),
    false,
    "the host still declares an Antigravity driver module",
  );
  assert.equal(
    await exists("crates/licoup-native/src/platform/antigravity_driver"),
    false,
    "the host still declares an Antigravity driver tree",
  );
  const platform = await read("crates/licoup-native/src/platform/mod.rs");
  assert.doesNotMatch(platform, /mod antigravity_driver;/u,
    "the host module tree still declares an Antigravity driver module");

  // The composition names the package's driver, so the launch metadata, the
  // runtime protocol, the receipt rule and the terminal classification are the
  // package's.
  const composition = await read(compositionPath);
  assert.match(composition, /use licoup_agent_antigravity::driver as antigravity_driver;/u,
    "the composition does not read the package's driver");
  assert.ok(composition.includes("antigravity_driver::execute("),
    "the composition does not run the package's own turn");
  assert.ok(composition.includes("antigravity_driver::probe("),
    "the composition does not run the package's own probe");
  // The composition keeps no second copy of the vendor fact: every Antigravity
  // protocol decision is the package's.
  for (const forbidden of [
    "PtyOutputParser",
    "classify_terminal",
    "parse_hook_receipt",
    "DANGEROUS_SKIP_MODE",
    "antigravity-cli-argv-hook-v1",
    "antigravity-cli",
    "--print=",
    "--conversation=",
    "--dangerously-skip-permissions",
    "RECEIPT_ENV",
    "hook_bridge",
  ]) {
    assert.equal(composition.includes(forbidden), false,
      `the client composition keeps a copy of the package's protocol: ${forbidden}`);
  }
  // The cancel and cleanup arms reach the package's own control entries, so the
  // host holds no Antigravity control plane either.
  const lane = await read("crates/licoup-native/src/platform/conversation_lane.rs");
  assert.ok(lane.includes("licoup_agent_antigravity::driver::cancel(&session_id)"));
  assert.ok(lane.includes("licoup_agent_antigravity::driver::cleanup_session(&session_id)"));
  assert.ok(
    lane.includes("licoup_agent_antigravity::driver::ControlDisposition::Accepted"),
  );
});

test("the package owns the protocol and no part of the client", async () => {
  const driverRust = ["driver.rs", "driver/model.rs", "driver/control.rs", "driver/errors.rs",
    "driver/probe.rs", "driver/auth.rs", "driver/hooks.rs", "driver/execution.rs"];
  const sources = (
    await Promise.all(
      [parserPath, hookPath, registrationPath, replayPath,
        ...driverRust.map((source) => `${packageRoot}/${source}`)].map((source) => read(source)),
    )
  ).join("\n");
  // A client path in the package's sources would be the kernel reaching back in
  // through it: what the package needs from its host arrives through its ports.
  for (const clientPath of ["crate::platform", "licoup_native", "licoup-native"]) {
    assert.equal(sources.includes(clientPath), false, clientPath);
  }
  // The package is a program of its own: its manifest names the shared adapter
  // contract, the shared engine and inventory crates below every Agent, and the
  // foundation primitives it reuses — and no client crate. The driver reads the
  // Subagent-mesh bindings and the login-shell snapshot from those shared crates
  // rather than keeping a second copy of either rule.
  const manifest = await read(packageManifest);
  assert.ok(manifest.includes("licoup-agent-adapter-sdk"));
  assert.equal(manifest.includes("licoup-native"), false, "licoup-native");
  for (const sharedCrate of ["licoup-agent-drivers", "licoup-agent-targets"]) {
    assert.ok(manifest.includes(sharedCrate), sharedCrate);
  }
  assert.equal(manifest.includes("trait FrameReplay"), false);
});

test("Antigravity parses one vendor fact once, below the adapter port", async () => {
  const parser = await read(parserPath);
  // The declaration is the same string the corpus records, so a fixture cannot
  // pass against another Agent's channel.
  assert.ok(parser.includes('AdapterContract::new("antigravity", "pty-hook-json")'));
  assert.ok(parser.includes("fn parse_hook_receipt"));
  assert.ok(parser.includes("struct PtyOutputParser"));
  assert.ok(parser.includes("fn classify_terminal"));
  // Every refusal is the protocol's own stable code, and the classification
  // order is the protocol's rather than a caller's.
  for (const code of [
    "antigravity_cli_timeout",
    "antigravity_hook_receipt_missing",
    "antigravity_cli_session_drift",
    "antigravity_cli_turn_failed",
    "antigravity_cli_empty_output",
  ]) {
    assert.ok(parser.includes(code), code);
  }
  const replay = await read(replayPath);
  // The replay arm drives this parser rather than re-interpreting a frame.
  assert.ok(replay.includes("classify_terminal"));
  assert.ok(replay.includes("parse_hook_receipt"));
  assert.ok(replay.includes("CONTRACT.framing"));
  assert.equal(await exists(parserLeafFacade), false, "the parser is one leaf");
});

test("the receipt hook is a native package subcommand, not a generated script", async () => {
  const hook = await read(hookPath);
  // The writer writes one direct object, owner-only, through an atomic rename.
  assert.ok(hook.includes('"conversationId"'));
  assert.ok(hook.includes("0o600"));
  assert.ok(hook.includes("fs::rename"));
  assert.ok(hook.includes("record_with_environment"));
  const program = await read(`${packageRoot}/bin/lico-agent-antigravity.rs`);
  assert.ok(program.includes('Some("receipt") => hook::main()'));
  // Comments may name the interpreter this replaces; the code may not invoke it.
  // Every file in the hook path is read through this one rule, so documenting
  // the removal cannot fail the contract and a real invocation cannot hide.
  const withoutComments = (source) =>
    source
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("//"))
      .join("\n");
  const invoked = [hook, program].map(withoutComments).join("\n");
  for (const interpreter of ["python", "python3", "/bin/sh"]) {
    assert.equal(invoked.includes(interpreter), false, interpreter);
  }

  const bridge = await read(`${driverRoot}/hooks.rs`);
  // The bridge installs the package program and retires the generated script a
  // previous client left; no interpreter is named anywhere in the hook path.
  assert.ok(bridge.includes("PACKAGE_PROGRAM"));
  assert.ok(bridge.includes("RECEIPT_SUBCOMMAND"));
  assert.ok(bridge.includes(retiredGeneratedScript));
  assert.equal(withoutComments(bridge).includes("python3"), false);
  assert.equal(bridge.includes("write_hook_script"), false);
  const failure = await read(`${driverRoot}/tests.rs`);
  assert.equal(
    withoutComments(failure).includes("python3"),
    false,
    "no fixture needs an interpreter",
  );

  // The package document ships the native entry and no interpreter asset.
  const manifest = JSON.parse(await read(`${packageRelease}/manifest.json`));
  assert.equal(manifest.runtime.mode, "process");
  assert.equal(manifest.runtime.entry, "bin/lico-agent-antigravity");
  assert.equal(manifest.runtime.runtimeRef, undefined);
  const release = JSON.parse(await read(`${packageRelease}/package-release.json`));
  assert.equal(release.converter.kind, "native-executable");
  assert.equal(release.converter.entry, "bin/lico-agent-antigravity");
  const entry = await fs.readFile(
    path.join(repoRoot, packageRelease, release.converter.entry),
  );
  assert.equal(entry.subarray(0, 2).toString("utf8") === "#!", false);
});

test("the package declares the ports its host answers", async () => {
  const execution = await read(`${packageRoot}/port/execution.rs`);
  const turnEvent = await read(`${packageRoot}/port/turn_event.rs`);
  for (const port of ["ExecutionPort", "admits_execution", "subagent_caller_context"]) {
    assert.ok(execution.includes(port), port);
  }
  for (const sink of [
    "emit_turn_event",
    "emit_agent_message_chunk",
    "emit_agent_message_completed",
    "emit_agent_processing",
  ]) {
    assert.ok(turnEvent.includes(sink), sink);
  }
  // Fail-closed before installation: a package running outside the client
  // emits nothing and admits nothing.
  assert.ok(execution.includes("HostEffect::Uninstalled"));
  assert.ok(execution.includes(".ok_or(HostEffect::Uninstalled)"));
  assert.ok(turnEvent.includes("if let Some(port) = PORT.get()"));
  // That claim is about the *uninstalled* process state, so it is asserted in a
  // process that installs nothing: the crate's lib suite installs the ports to
  // drive a real turn, and an assertion there would be an assertion about test
  // order rather than about the port.
  const lifecycle = await read(portLifecycleTest);
  assert.ok(lifecycle.includes("assert!(!execution::installed())"));
  assert.ok(lifecycle.includes("Err(execution::HostEffect::Uninstalled)"));
  assert.ok(lifecycle.includes('Err("the agent-execution port is already installed")'));
  assert.ok(lifecycle.includes("assert!(!turn_event::installed())"));
  assert.ok(lifecycle.includes("an uninstalled port has no sink to reach"));
  // What a *closed* answer does to a turn is a different process's claim, so the
  // gate is exercised in both directions rather than only open.
  const refused = await read(closedAdmissionTest);
  assert.ok(refused.includes('Some("antigravity_execution_admission_closed")'));
  assert.ok(refused.includes('Some("turn/execute")'));
  assert.ok(refused.includes("admits_execution: || false"));

  // The host answers both ports from its own facts. The turn-event answer is the
  // platform layer's; the execution admission answer is the crate root's,
  // because composing the host's close-admission barrier is what the root owns
  // rather than a second policy inside `platform`.
  const platform = await read("crates/licoup-native/src/platform/mod.rs");
  assert.ok(platform.includes("antigravity_turn_event_port"));
  const composition = await read("crates/licoup-native/src/lib.rs");
  assert.ok(composition.includes("licoup_agent_antigravity::port::turn_event::install("));
  assert.ok(composition.includes("licoup_agent_antigravity::port::execution::install("));
  // The execution port's admission field carries this host's own answer, read
  // inside the Antigravity install call rather than another package's.
  const [, antigravityInstall] = composition.split(
    "licoup_agent_antigravity::port::execution::install(",
  );
  assert.ok(
    antigravityInstall
      ?.split("::port::execution::install(")[0]
      .includes("admits_execution: admits_agent_execution,"),
    "the Antigravity execution port carries the host's own admission answer",
  );
  // ... and that answer is the close-admission barrier, not a second policy.
  const admissionAnswer = composition
    .split("fn admits_agent_execution() -> bool {")[1]
    ?.split("\n}\n")[0];
  assert.ok(
    admissionAnswer?.includes("WorkAdmission::open(") &&
      admissionAnswer.includes(".barrier()"),
    "the host's admission answer reads the close-admission barrier",
  );

  // The driver reads admission before it starts any process, so the package
  // cannot bypass the host's idle-update admission.
  const driver = await read(`${driverRoot}/execution.rs`);
  assert.ok(driver.includes("admits_execution"));
});

test("the package declares its own release and replay evidence", async () => {
  const cargo = await read(packageManifest);
  assert.ok(cargo.includes("licoup-agent-adapter-sdk"));
  const corpusTest = await read("crates/licoup-agent-antigravity/tests/replay_corpus.rs");
  assert.ok(corpusTest.includes("replay_corpus"));
  assert.ok(corpusTest.includes("parser_set()"));
  const artifactTest = await read("crates/licoup-agent-antigravity/tests/package_artifact.rs");
  for (const assertion of [
    "org.licoland.adapter.antigravity",
    "bin/lico-agent-antigravity",
    "PackageManifest::from_value",
    "capability_owner",
  ]) {
    assert.ok(artifactTest.includes(assertion), assertion);
  }
  // The registered adapter id and the manifest's declared adapter are one.
  const declaration = JSON.parse(await read(`${packageRelease}/manifest.json`));
  const registration = await read(registrationPath);
  assert.ok(registration.includes('pub const ADAPTER_ID: &str = "antigravity"'));
  assert.equal(declaration.id, "org.licoland.adapter.antigravity");
});
