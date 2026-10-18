import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
// The kernel's composition, which names this package's driver directly. The
// host keeps no DeepSeek Harness module of its own.
const compositionPath = "crates/licoup-native/src/platform/runtime_adapters/drivers.rs";
// The wire half, the process half and the session-log reader, all owned by the
// DeepSeek adapter package since DEEPSEEK-PACKAGE and VENDOR-CODE-REMOVAL moved
// them out of the kernel. One package carries one Agent's protocol; the client
// names the package for its vocabulary instead of keeping a second copy.
const packageRoot = "crates/licoup-agent-deepseek/src";
const packageLeaves = Object.freeze([
  "lib.rs",
  "parser.rs",
  "registration.rs",
  "replay.rs",
  "session_store.rs",
  "port/mod.rs",
  "port/usage.rs",
]);
// The process half, which does start the vendor's own executable: it is read
// separately because "spawns no process" is a wire-half claim and not this one's.
const processLeaves = Object.freeze([
  "driver.rs",
  "port/launch_environment.rs",
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

async function packageSources() {
  return Object.fromEntries(await Promise.all(
    packageLeaves.map(async (leaf) => [leaf, await read(`${packageRoot}/${leaf}`)]),
  ));
}

test("the client names the package for the protocol and keeps no Harness driver module", async () => {
  // The kernel has no DeepSeek Harness driver module at all: the host reaches
  // the Agent through the package and cannot hold a second owner of its
  // protocol.
  assert.equal(
    await exists("crates/licoup-native/src/platform/deepseek_harness_driver.rs"),
    false,
    "the host still declares a DeepSeek Harness driver module",
  );
  assert.equal(
    await exists("crates/licoup-native/src/platform/deepseek_harness_driver"),
    false,
    "the host still declares a DeepSeek Harness driver tree",
  );
  const platform = await read("crates/licoup-native/src/platform/mod.rs");
  assert.doesNotMatch(platform, /mod deepseek_harness_driver;/u,
    "the host module tree still declares a DeepSeek Harness driver module");

  const composition = await read(compositionPath);
  // The composition reads the package's driver, so the SDK transport, the
  // bounded frame reading and the turn's projection are the package's.
  assert.match(composition, /use licoup_agent_deepseek::driver as deepseek_harness_driver;/u,
    "the composition does not read the package's driver");
  // The composition keeps no second copy of the vendor fact: every Harness
  // protocol decision is the package's.
  for (const forbidden of [
    "struct FrameParser",
    "struct TurnParser",
    "fn initialize_request",
    "fn prompt_request",
    "fn shutdown_request",
    "fn assistant_response",
    "mod deepseek_harness",
    "deepseek_reader.mjs",
    "include_str!",
  ]) {
    assert.equal(composition.includes(forbidden), false,
      `the client composition keeps a copy of the package's protocol: ${forbidden}`);
  }
});

test("the process half runs the vendor's own executable and still declares no frame", async () => {
  const sources = Object.fromEntries(await Promise.all(
    processLeaves.map(async (leaf) => [leaf, await read(`${packageRoot}/${leaf}`)]),
  ));
  const driver = sources["driver.rs"];
  // The process half reads the one protocol owner rather than restating it, so
  // a request builder or a frame rule cannot be re-declared beside it.
  assert.ok(driver.includes("crate::parser"));
  for (const forbidden of [
    "struct FrameParser",
    "struct TurnParser",
    "fn initialize_request",
    "fn prompt_request",
    "fn shutdown_request",
    "fn assistant_response",
    "mod deepseek_harness",
    "deepseek_reader.mjs",
    "include_str!",
  ]) {
    assert.equal(driver.includes(forbidden), false,
      `the package's process half re-declares the protocol: ${forbidden}`);
  }
  // Nothing in either leaf reaches a client crate, and the one host fact the
  // launch needs crosses the package's own port rather than a client path.
  const joined = Object.values(sources).join("\n");
  for (const clientPath of ["crate::platform", "licoup_native", "licoup_native::"]) {
    assert.equal(joined.includes(clientPath), false, clientPath);
  }
  assert.ok(driver.includes("crate::port::launch_environment::apply_to_command"),
    "the launch environment is read through the package's own port");
});

test("the Harness protocol, its session-log reader, and its registration have single owners", async () => {
  const sources = await packageSources();
  const parser = sources["parser.rs"];
  const reader = sources["session_store.rs"];
  const registration = sources["registration.rs"];

  // Framing, request builders and turn attribution are the package's own.
  assert.ok(parser.includes('AdapterContract::new("deepseek-harness", "lf-jsonl-jsonrpc")'));
  assert.ok(parser.includes("fn parse_line"));
  assert.ok(parser.includes("fn initialize_request"));
  assert.ok(parser.includes("fn prompt_request"));
  assert.ok(parser.includes("fn shutdown_request"));
  assert.ok(parser.includes("fn take_completed_messages"));
  assert.ok(parser.includes("agent/inbox/spliced"));
  assert.ok(parser.includes("session.status"));
  assert.ok(parser.includes("assistant/message"));

  // The registration composition reads is this package's own, and it answers the
  // SDK's declared queries rather than leaving them to a second owner.
  assert.ok(registration.includes('pub const ADAPTER_ID: &str = "deepseek-harness"'));
  assert.ok(registration.includes('pub const FRAMING: &str = "lf-jsonl-jsonrpc"'));
  assert.ok(registration.includes("ParserRegistration::unanswered(CONTRACT)"));

  // The session-log reader implements the generation it declares and refuses any
  // other by name, so an unread generation can never be folded as if it were
  // this one.
  assert.ok(reader.includes("pub const CURRENT_FORMAT_VERSION: u64 = 4"));
  assert.ok(reader.includes("SessionReadError::UnsupportedFormatVersion"));
  assert.ok(reader.includes('"request/header"'));
  assert.ok(reader.includes('"llm/retry-started"'));
  assert.ok(reader.includes('"assistant/message"'));
  assert.ok(reader.includes('"assistant/attempt"'));
  assert.ok(reader.includes("inheritedEventCount"));
  assert.ok(reader.includes("zstd::stream::read::Decoder"));

  // The package owns the protocol and no part of the client: a client path in
  // the package's sources would be the kernel reaching back in through it. The
  // boundary claim a doc comment states may *name* the client crate it refuses;
  // a code path may not cross into it.
  const joined = Object.values(sources)
    .join("\n")
    .split("\n")
    .filter((line) => !line.trimStart().startsWith("//"))
    .join("\n");
  for (const clientPath of ["crate::platform", "licoup_native", "licoup_native::"]) {
    assert.equal(joined.includes(clientPath), false, clientPath);
  }
  // No Node worker is started for the session log any more, and no vendor
  // library is resolved at run time.
  for (const retiredWorker of [
    "Command::new",
    "createRequire",
    "@deepseek-ai/dsh-session-persistence-jsonl",
    "find_binary",
  ]) {
    assert.equal(joined.includes(retiredWorker), false, retiredWorker);
  }
});

test("the package declares its external dependency instead of hiding it", async () => {
  const manifest = JSON.parse(await read("crates/licoup-agent-deepseek/package/manifest.json"));
  const release = JSON.parse(
    await read("crates/licoup-agent-deepseek/package/package-release.json"),
  );
  // The vendor artifact the reader reads is named as an external dependency, and
  // the package states that it owns no persisted data of its own.
  assert.equal(manifest.extensions["org.licoland.adapter.deepseek/persistentData"], "none");
  assert.ok(manifest.extensions["org.licoland.adapter.deepseek/persistentDataReason"]);
  assert.match(
    manifest.extensions["org.licoland.adapter.deepseek/externalDependency"],
    /DeepSeek Harness/,
  );
  // The declared converter is the native entry, so no Node runtime is required
  // to read the vendor artifact.
  assert.equal(release.converter.kind, "native-executable");
  assert.equal(release.converter.entry, manifest.runtime.entry);
  assert.equal(release.converter.entry, "bin/lico-agent-deepseek");
  assert.equal(release.converter.sourceFormat, "deepseek-harness-session-jsonl");
});
