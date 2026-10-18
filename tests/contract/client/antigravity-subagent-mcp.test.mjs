import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(path, "utf8");
const common = read("crates/licoup-native/src/platform/provider_mcp_registration.rs");
const manager = read("crates/licoup-native/src/platform/antigravity_subagent_mcp_manager.rs");
// The driver is the Antigravity package's; the host keeps no module of its own.
const driver = "crates/licoup-agent-antigravity/src/driver";
const execution = read(`${driver}/execution.rs`);
const hooks = read(`${driver}/hooks.rs`);
const auth = read(`${driver}/auth.rs`);
const registration = read("crates/licoup-agent-antigravity/src/registration.rs");
const runtime = read("crates/licoup-agent-drivers/src/runtime_adapters/subagent_mesh.rs");
const adapters = read("crates/licoup-native/src/platform/native_agent_parser/adapters/mod.rs");
const startup = read("tests/product-e2e/cli/subagent-mcp/upstream/antigravity-startup-recognition.mjs");

test("Antigravity registration uses one common namespaced ownership contract", () => {
  assert.match(manager, /ProviderConfigKind::Antigravity/u);
  assert.match(manager, /0\.2\.0/u);
  assert.match(common, /ConfigAmbiguous/u);
  assert.match(common, /official\.exists\(\) && legacy\.exists\(\)/u);
  assert.doesNotMatch(manager, /SERVER_KEY/u);
  assert.match(common, /\.gemini.*config.*skills/su);
  assert.match(common, /fn antigravity_context_environment/u);
  assert.match(common, /"\$\{LICOUP_HOME\}"/u);
  assert.doesNotMatch(common, /Antigravity => entry\.get\("env"\)\.is_none\(\)/u);
  const envFn = common.indexOf("fn antigravity_context_environment");
  const envFnEnd = common.indexOf("#[derive", envFn);
  assert.doesNotMatch(common.slice(envFn, envFnEnd), /\$\{env:LICOUP_/u);
});

test("Antigravity target requires hook identity, auth preflight, PTY and exact resume", () => {
  assert.match(execution, /ensure_authorized/u);
  assert.match(execution, /ensure_hook_bridge/u);
  assert.match(execution, /read_conversation_id/u);
  assert.match(execution, /receipt_session/u);
  assert.match(execution, /pty_transport::spawn/u);
  assert.match(execution, /register_active_turn/u);
  assert.match(execution, /control_session_id/u);
  assert.match(execution, /dispatchId/u);
  assert.match(execution, /apply_mcp_runtime_root/u);
  assert.match(execution, /current_dir\(&self\.workspace\)/u);
  assert.match(hooks, /receipt/u);
  assert.match(auth, /oauth|auth/iu);
  // Exact resume asks the Agent's own parser through the shared port, and the
  // host's Antigravity entry is the package's own registration, which answers
  // with Antigravity's receipt rule. The host parser tree keeps no Antigravity
  // alias: the package's driver reads the package's parser directly.
  assert.match(runtime, /parser\.valid_identity/u);
  assert.match(adapters, /licoup_agent_antigravity::registration::REGISTRATION/u);
  assert.doesNotMatch(adapters, /as antigravity;/u);
  assert.match(registration, /fn valid_identity/u);
  assert.match(registration, /parser::valid_session_id/u);
});

test("Antigravity guidance and cancellation are explicit without private field fallback", () => {
  const policy = read("crates/licoup-agent-drivers/src/runtime_adapters.rs");
  const control = read(`${driver}/control.rs`);
  assert.match(policy, /OrdinaryWirePrefix/u);
  assert.match(execution, /antigravity_private_instructions_unsupported/u);
  // The control plane is the package's own, reached by the host at the
  // visibility a package publishes rather than the kernel's former
  // `pub(in crate::platform)`.
  assert.match(control, /pub fn cancel\(session_id: &str\) -> ControlDisposition/u);
  assert.match(runtime, /active_cancel: true/u);
});

test("Antigravity startup recognition uses a read-only standard MCP list surface", () => {
  assert.match(startup, /\["mcp", "list"\]/u);
  assert.match(startup, /\?\? "agy"/u);
  assert.match(startup, /installerOnly: true/u);
  assert.match(startup, /probeAntigravityStartup/u);
});
