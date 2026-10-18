import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");
const exists = (path) => existsSync(path);
const common = read("crates/licoup-native/src/platform/provider_mcp_registration.rs");
const manager = read("crates/licoup-native/src/platform/cursor_subagent_mcp_manager.rs");
// Cursor's turn — the launch, the stream it reads and the active turn it
// cancels — is the Cursor adapter package's own driver; the client composes it
// and keeps no copy of it.
const driver = read("crates/licoup-agent-cursor/src/driver/execution.rs");
// The client's composition of that driver, and the host module tree that
// declares no Cursor module of its own.
const composition = read("crates/licoup-native/src/platform/runtime_adapters/drivers.rs");
const platform = read("crates/licoup-native/src/platform/mod.rs");
// Cursor's fixed launch arguments and capability surface are the package's own
// vocabulary, read where the package declares them.
const model = read("crates/licoup-agent-cursor/src/model.rs");
// Cursor's wire dialect — including the application error codes an installed
// Cursor client returns for a delegated MCP turn — is the Cursor adapter
// package's own, parsed once below the port (ADR-0008).
const parser = read("crates/licoup-agent-cursor/src/parser.rs");
const registration = read("crates/licoup-agent-cursor/src/registration.rs");
const runtime = read("crates/licoup-agent-drivers/src/runtime_adapters/subagent_mesh.rs");
const adapters = read("crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs");
const startup = read("tests/product-e2e/cli/subagent-mcp/upstream/cursor-startup-recognition.mjs");

test("Cursor registration is namespaced, digest-bound, owned, and ambiguity-closed", () => {
  assert.match(manager, /ProviderConfigKind::Cursor/u);
  assert.match(common, /land\.lico\.licoup\.subagents/u);
  assert.match(common, /managedBy/u);
  assert.match(common, /config_digest/u);
  assert.match(common, /OwnedEntryAmbiguous/u);
  assert.match(common, /DuplicateConnectorEntry/u);
  assert.match(common, /ApprovalConsumed/u);
  assert.match(common, /pub fn remove/u);
  assert.match(
    common,
    /const SKILL_SOURCE: &str = crate::domain::client_conversation::LICOUP_GUIDE_SKILL_SOURCE;/u,
  );
  assert.match(common, /\.cursor.*skills/su);
  assert.match(common, /publish_shared_skill/u);
  assert.match(common, /\.agents.*skills/su);
});

test("Cursor target keeps exact create/resume, workspace, PTY, acknowledgement and cancel", () => {
  assert.match(driver, /create_chat_session/u);
  assert.match(driver, /\.arg\("--resume"\)/u);
  assert.match(driver, /\.arg\("--workspace"\)/u);
  assert.match(model, /--approve-mcps/u);
  assert.match(driver, /spawn_turn_transport/u);
  assert.match(driver, /PromptAcknowledgementMissing/u);
  assert.match(driver, /register_active_turn/u);
  assert.match(driver, /apply_mcp_runtime_root/u);
  // Exact resume asks the Agent's own parser through the shared port, and the
  // host's Cursor registration answers with Cursor's session-id rule.
  assert.match(runtime, /parser\.valid_identity/u);
  // The host composes the package's own registration for Cursor, which answers
  // exact-resume identity with the same session-id rule the parser binds with.
  assert.match(adapters, /licoup_agent_cursor::registration::REGISTRATION/u);
  assert.match(registration, /pub fn valid_identity/u);
  assert.match(registration, /parser::safe_session_id/u);
  assert.match(runtime, /active_cancel: true/u);
  assert.match(parser, /fn safe_session_id/u);
});

test("the client declares no Cursor driver module and re-states no Cursor protocol", () => {
  // The host has no Cursor driver module or tree: the launch, the stream
  // classification and the turn are the package's, and the path the move
  // retired must not come back as a second owner.
  assert.equal(exists("crates/licoup-native/src/platform/cursor_driver.rs"), false,
    "the host still declares a Cursor driver module");
  assert.equal(exists("crates/licoup-native/src/platform/cursor_driver"), false,
    "the host still declares a Cursor driver tree");
  assert.doesNotMatch(platform, /mod cursor_driver;/u,
    "the host module tree still declares a Cursor driver module");

  // The composition names the package's driver the way the Codex arm names its
  // package, and keeps no Cursor protocol decision of its own.
  assert.match(composition, /use licoup_agent_cursor::driver as cursor_driver;/u);
  for (const forbidden of [
    "create-chat",
    "--output-format",
    "--stream-partial-output",
    "--approve-mcps",
    "cursor_cli_",
    "safe_session_id",
  ]) {
    assert.equal(composition.includes(forbidden), false,
      `the client composition keeps a copy of the package's protocol: ${forbidden}`);
  }

  // The turn launches on the shared pty primitive rather than on a second
  // terminal of its own, so a Cursor turn runs on the same terminal every other
  // CLI lane does, and no half keeps a private copy of that mechanism.
  assert.match(driver, /licoup_foundation::platform::pty_transport::spawn/u);
});

test("Cursor generated guidance is one ordinary unmarked wire prefix", () => {
  const policy = read("crates/licoup-agent-drivers/src/runtime_adapters.rs");
  assert.match(policy, /RuntimeAdapter::Cursor \| RuntimeAdapter::Antigravity/u);
  assert.match(policy, /OrdinaryWirePrefix/u);
  assert.match(driver, /cursor_cli_private_instructions_unsupported/u);
});

test("Cursor startup recognition uses a read-only standard MCP list surface", () => {
  assert.match(startup, /\["mcp", "list"\]/u);
  assert.match(startup, /format: "text"/u);
  assert.match(startup, /installerOnly: true/u);
  assert.match(startup, /probeCursorStartup/u);
});
