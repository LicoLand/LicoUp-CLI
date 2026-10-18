import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const read = (path) => readFileSync(path, "utf8");
const application = read("crates/licoup-native/src/domain/subagents/mod.rs");
const production = read("crates/licoup-native/src/domain/subagents/production.rs");
const engine = read("crates/licoup-public-gateway/src/server.rs");
const library = read("crates/licoup-public-gateway/src/lib.rs");
const core = read("crates/licoup-native/src/core/mcp.rs");
const connector = read("crates/licoup-public-gateway/src/connector.rs");
const transport = read("crates/licoup-public-gateway/src/transport.rs");
const remoteApplication = read("crates/licoup-public-gateway/src/application.rs");
const lifecycle = read("crates/licoup-native/src/platform/mcp_service_process.rs");
const mcpCargo = read("crates/licoup-public-gateway/Cargo.toml");
const runtime = read("crates/licoup-agent-runtime/src/lib.rs");
const adapters = read("crates/licoup-agent-adapters/src/lib.rs");
const claims = read("crates/licoup-conversation/src/store/dispatches.rs");
const providerRuntime = read(
  "crates/licoup-agent-drivers/src/runtime_adapters/subagent_mesh.rs",
);
const agentHubCatalog = read("crates/licoup-native/src/domain/agent_hub/catalog.rs");
const agentHubVersion = read("crates/licoup-native/src/domain/agent_hub/version_check.rs");
const cliSurface = read("crates/licoup-native/src/ffi/commands/subagents.rs");
const commandTable = read("crates/licoup-native/src/ffi/commands/mod.rs");
const applicationPorts = read("crates/licoup-native/src/domain/application_port.rs");
const schema = JSON.parse(read("schemas/subagent_mcp/subagent_mcp.schema.json"));

const TOOLS = [
  "lico_subagents_list",
  "lico_subagent_probe",
  "lico_subagent_delegate",
  "lico_subagent_continue",
  "lico_subagent_cancel",
];

test("independent MCP freezes its protocol, identity, and five public operations", () => {
  assert.match(remoteApplication, /PROTOCOL_REVISION: &str = "2025-06-18"/u);
  assert.match(remoteApplication, /SERVER_NAME: &str = "lico-up-subagents"/u);
  assert.match(remoteApplication, /SERVER_VERSION: &str = "0.14.0"/u);
  const list = remoteApplication.slice(
    remoteApplication.indexOf("pub const REMOTE_TOOL_NAMES"),
    remoteApplication.indexOf("pub const TOOL_NAMES"),
  );
  assert.deepEqual(
    [...list.matchAll(/"(lico_[^"]+)"/gu)].map((match) => match[1]),
    TOOLS,
  );
  assert.match(remoteApplication, /REMOTE_TOOL_NAMES\.contains/u);
  assert.match(remoteApplication, /"subagents"\.into\(\).*"execute"\.into\(\)/su);
  assert.match(remoteApplication, /"rpc", "stdio"/u);
  const dependencies = mcpCargo.split("[[bin]]")[0];
  const foundation = /^licoup-foundation = \{ path = "\.\.\/licoup-foundation"(?:, version = "[^"]+")? \}$/mu;
  assert.match(dependencies, foundation);
  assert.doesNotMatch(
    dependencies.replace(foundation, ""),
    /(?:licoup-[\w-]+\s*=|path\s*=|workspace\s*=)/u,
    "only neutral Foundation may be shared; runtime and domain authority stay behind the CLI",
  );
  assert.match(application, /"additionalProperties": false/u);
  assert.equal(schema.properties.protocolRevision.const, "2025-06-18");
  assert.equal(schema.properties.server.properties.version.const, "0.14.0");
  assert.equal(schema.properties.tools.minItems, TOOLS.length);
  assert.equal(schema.properties.tools.maxItems, TOOLS.length);
  assert.deepEqual(schema.properties.tools.prefixItems.map((item) => item.const), TOOLS);
});

test("delegated work is reached through the shared facade and nothing else", () => {
  // One door. The CLI surface decodes its own wire shape and hands the command
  // to the shared facade; a domain owner reached from here would be a second
  // path, and the MCP would stop sharing the CLI's semantics.
  const implementation = cliSurface.slice(0, cliSurface.indexOf("mod tests"));
  assert.match(implementation, /application_port::execute_as/u);
  assert.doesNotMatch(
    implementation,
    /production_application|subagents::local/u,
    "the CLI surface must not reach a domain owner directly",
  );
  // The ports behind the facade are the only place that names the owner, and
  // both delegated families have a real implementation there — a family that
  // only ever answers "unsupported" would be a door that leads nowhere.
  assert.match(applicationPorts, /impl ActorPort for/u);
  assert.match(applicationPorts, /impl SubagentPort for/u);
  assert.match(applicationPorts, /impl AssistantPort for/u);
  assert.match(applicationPorts, /production_application\(\)/u);
  assert.doesNotMatch(applicationPorts, /UnsupportedFamily|application_family_unsupported/u);
});

test("CLI and MCP meet at one argv envelope for the same tool call", () => {
  // The MCP crate deliberately shares no type with `licoup-native`, so the seam
  // the two interfaces meet at is the JSON the MCP puts on the CLI argv and the
  // CLI decodes. If one side renamed a field, the two interfaces would quietly
  // stop asking for the same thing.
  const mcpEnvelope = remoteApplication.slice(
    remoteApplication.indexOf("fn cli_invocation"),
    remoteApplication.indexOf("// Validate the closed scalar schemas"),
  );
  const cliEnvelope = cliSurface.slice(
    cliSurface.indexOf("struct Invocation"),
    cliSurface.indexOf("impl CallerScope"),
  );
  for (const field of ["name", "arguments", "caller"]) {
    assert.ok(mcpEnvelope.includes(`"${field}"`), `the MCP envelope names ${field}`);
    assert.ok(cliEnvelope.includes(field), `the CLI decodes ${field}`);
  }
  for (const field of [
    "provider_id",
    "conversation_id",
    "membership_id",
    "parent_dispatch_id",
  ]) {
    assert.ok(cliEnvelope.includes(field), `the CLI decodes ${field}`);
  }
  assert.match(cliEnvelope, /rename_all = "camelCase"/u);
  assert.match(mcpEnvelope, /"providerId"[\s\S]*caller\.provider_id/u);
  assert.match(mcpEnvelope, /"conversationId"[\s\S]*caller\.conversation_id/u);
  assert.match(mcpEnvelope, /"membershipId"[\s\S]*caller\.membership_id/u);
  assert.match(mcpEnvelope, /"parentDispatchId"[\s\S]*caller\.parent_dispatch_id/u);
  assert.match(mcpEnvelope, /insert_optional/u);
  assert.doesNotMatch(mcpEnvelope, /authenticated/u);
  // Both sides address the operation by the same tool name, and the MCP reaches
  // the CLI through the one route the facade serves.
  assert.match(remoteApplication, /"subagents"\.into\(\).*"execute"\.into\(\)/su);
  assert.match(
    commandTable,
    /path: &\["subagents", "execute"\][\s\S]*?handler: subagents::handle_subagents_execute/u,
  );
});

test("independent engine owns inbound framing, initialization, calls, and cancellation", () => {
  for (const marker of [
    "pub struct McpServerDefinition",
    "pub struct McpServerEngine",
    "notifications/cancelled",
    "tools/list",
    "tools/call",
    "Request cancelled",
  ]) assert.ok(engine.includes(marker), marker);
  assert.match(core, /OUTBOUND_TRANSFER_PROTOCOL_REVISION/u);
  assert.match(lifecycle, /Command::new\(binary\)/u);
  assert.match(lifecycle, /"service",\s*action/u);
});

test("the service process comes from the installed package generation, not a neighbour", () => {
  // The bundled-neighbour resolution is gone. The generation is selected from
  // the package store, so an absent or switched-off package starts nothing.
  assert.doesNotMatch(lifecycle, /packaged_binary_directory/u);
  assert.match(lifecycle, /MCP_PACKAGE_ID: &str = "org\.licoland\.feature\.mcp"/u);
  assert.match(lifecycle, /select_generation/u);
  assert.match(lifecycle, /mcp_package_absent/u);
  assert.match(lifecycle, /mcp_package_disabled/u);
  const selection = read(
    "crates/licoup-native/src/platform/extension_packages/selection.rs",
  );
  assert.match(selection, /pub fn select_generation/u);
  assert.match(selection, /pub fn switched_on/u);
  // The installed manifest's own process entry is what is resolved, and the
  // payload the artifact ships declares exactly that entry.
  const artifact = JSON.parse(read("crates/licoup-public-gateway/package/manifest.json"));
  assert.equal(artifact.runtime.mode, "process");
  assert.equal(artifact.runtime.entry, "bin/lico-subagent-mcp");
  assert.match(selection, /Runtime::Process/u);
});

test("the approved payload is the payload that serves", () => {
  // The launcher measures the generation's executable and hands the digest to
  // the program it starts; the serving process measures itself against it.
  assert.match(
    lifecycle,
    /APPROVED_DIGEST_ENV: &str = "LICOUP_MCP_APPROVED_DIGEST"/u,
  );
  assert.match(lifecycle, /generation\.payload_digest\(\)/u);
  assert.match(lifecycle, /mcp_package_consent_mismatch/u);
  const payloadLifecycle = read("crates/licoup-public-gateway/src/lifecycle.rs");
  assert.match(payloadLifecycle, /mcp_payload_not_approved/u);
  assert.match(payloadLifecycle, /APPROVED_DIGEST_ENV/u);
  assert.match(payloadLifecycle, /fn check_approved_payload/u);
  assert.match(payloadLifecycle, /check_approved_payload\(\)\?/u);
  // The digest is computed from the bytes, never from the path or a name.
  assert.match(payloadLifecycle, /Sha256::new\(\)/u);
});

test("connector performs one authenticated HTTP exchange through the independent service", () => {
  assert.match(connector, /connector_exchange/u);
  assert.match(connector, /load_connector_discovery/u);
  const implementation = connector.slice(0, connector.indexOf("mod tests"));
  assert.doesNotMatch(implementation, /fn tool_catalog|fn call_tool|tools\/list/u);
  assert.doesNotMatch(connector, /thread::sleep|TcpListener/u);
  assert.match(transport, /Ipv4Addr::LOCALHOST/u);
  assert.match(transport, /authorization/u);
  assert.match(transport, /mcp-session-id/u);
  assert.match(transport, /MAX_HTTP_CONNECTIONS/u);
  assert.match(transport, /atomic_write_private_text_bounded/u);
});

test("mesh caller membership comes from the native registry through the CLI catalog", () => {
  assert.match(adapters, /pub fn caller_providers/u);
  assert.match(application, /pub fn caller_providers/u);
  assert.match(remoteApplication, /get\("callers"\)/u);
  assert.match(transport, /\.caller_providers\(\)/u);
  assert.match(connector, /published_callers/u);
  assert.match(production, /adapters: AdapterRegistry/u);
});

// The workspace dependency graph, read from Cargo rather than from a hand-kept
// list. Only normal and build edges are traversed: a dev-dependency is a test's
// own input and enters no binary.
function workspaceEdges() {
  const result = spawnSync("cargo",
    ["metadata", "--offline", "--no-deps", "--format-version", "1"], {
      cwd: repoRoot,
      encoding: "utf8",
      shell: false,
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 60_000,
      maxBuffer: 64 * 1024 * 1024,
    });
  assert.equal(result.status, 0, `cargo metadata must succeed: ${result.stderr}`);
  const metadata = JSON.parse(result.stdout);
  const members = new Set(metadata.workspace_members);
  const workspace = metadata.packages.filter((entry) => members.has(entry.id));
  const byName = new Map(workspace.map((entry) => [entry.name, entry]));
  return {
    workspace,
    byName,
    edges: new Map(workspace.map((entry) => [entry.name, entry.dependencies
      .filter((dependency) => dependency.kind !== "dev" && dependency.kind !== "build")
      .map((dependency) => dependency.name)
      .filter((name) => byName.has(name))])),
  };
}

function reachableFrom(edges, origin) {
  const seen = new Set();
  const queue = [origin];
  while (queue.length > 0) {
    const name = queue.shift();
    for (const next of edges.get(name) || []) {
      if (seen.has(next)) continue;
      seen.add(next);
      queue.push(next);
    }
  }
  return seen;
}

test("no kernel crate reaches the optional MCP payload, and the neutral engine stands alone", () => {
  const { workspace, edges } = workspaceEdges();
  const payload = "licoup-public-gateway";
  assert.ok(edges.has(payload), `${payload} is a workspace member`);

  // The crate is also the neutral library the kernel links for channel custody,
  // so crate-level edges are expected. What stays forbidden is the optional
  // `service` payload: every dependent must opt out of default features, so the
  // gated payload code is never linked into kernel or channel builds.
  for (const member of workspace) {
    if (member.name === payload) continue;
    for (const dependency of member.dependencies) {
      if (dependency.name !== payload || dependency.kind === "dev") continue;
      assert.equal(dependency.uses_default_features, false,
        `${member.name} reaches ${payload} with the default service payload enabled`);
    }
  }

  // The optional payload's own in-repo edges go one way, to the neutral
  // Foundation every crate may share, and nowhere else.
  assert.deepEqual([...edges.get(payload)].sort(), ["licoup-foundation"]);

  // The crate's own boundary is declared, not implied: the neutral engine is
  // built by default with the payload and on its own without it, and the
  // optional binary is not built at all without the feature.
  assert.match(mcpCargo, /\[features\]\n(?:.*\n)*?default = \["service"\]\nservice = \[\]/u);
  assert.match(mcpCargo, /required-features = \["service"\]/u);
  const neutral = ["pub mod wire;", "mod server;"];
  for (const declaration of neutral) {
    assert.ok(library.includes(declaration), declaration);
    assert.doesNotMatch(library,
      new RegExp(`#\\[cfg\\(feature = "service"\\)\\]\\s*${declaration}`, "u"),
      `${declaration} is the neutral layer and is not gated`);
  }
  for (const service of ["pub mod application;", "pub mod lifecycle;",
    "pub mod private_state;", "pub mod transport;"]) {
    assert.match(library,
      new RegExp(`#\\[cfg\\(feature = "service"\\)\\]\\s*${service}`, "u"),
      `${service} is the optional service payload and is gated`);
  }

  // The neutral MCP consumers live in the kernel and keep their own identity:
  // the service-neutral message adapter with its outbound transfer approval
  // gate, the bounded outbound HTTP transport, the approval plan store, and the
  // shared guide resource. None of them names the payload crate.
  const neutralOwners = [
    "crates/licoup-native/src/core/mcp.rs",
    "crates/licoup-native/src/core/mcp/wire.rs",
    "crates/licoup-native/src/core/mcp/transfer.rs",
    "crates/licoup-native/src/platform/mcp_streamable_http.rs",
    "crates/licoup-native/src/platform/mcp_approval_plan_store.rs",
    "crates/licoup-native/resources/licoup-guide/SKILL.md",
  ];
  for (const relative of neutralOwners) {
    const source = read(relative);
    assert.ok(source.length > 0, relative);
    assert.equal(source.includes("licoup_mcp"), false,
      `${relative} is a neutral owner and must not name the optional payload`);
    assert.equal(source.includes("licoup-mcp"), false,
      `${relative} is a neutral owner and must not name the optional payload`);
  }
  assert.match(read("crates/licoup-native/src/core/mcp.rs"), /Service-neutral/u);
  assert.match(read("crates/licoup-native/src/core/mcp.rs"), /McpExternalTransferGate/u);
  assert.match(read("crates/licoup-native/src/core/mcp.rs"), /DEFAULT_TRANSFER_APPROVAL_TTL/u);
});

test("caller and target ports meet only in one registry", () => {
  assert.match(runtime, /pub trait McpCallerIntegration/u);
  assert.match(runtime, /pub trait SubagentRuntimeAdapter/u);
  assert.match(runtime, /pub enum InstructionPolicy/u);
  assert.match(runtime, /pub fn reduce_readiness/u);
  assert.match(runtime, /pub fn reduce_execution_admission/u);
  assert.match(runtime, /pub struct ExecutionAdmissionEvidence/u);
  assert.match(adapters, /register_pair/u);
  assert.match(adapters, /BTreeMap<ProviderId/u);
  assert.match(providerRuntime, /production_subagent_registry/u);
  for (const provider of ["codex", "cursor", "antigravity"]) {
    assert.match(providerRuntime, new RegExp(`"${provider}"`, "u"));
  }
});

test("execution admission is independent of observational readiness", () => {
  assert.match(application, /reduce_execution_admission/u);
  assert.match(providerRuntime, /executable_message_send_route/u);
  assert.match(providerRuntime, /available_runtime_executable/u);
  assert.doesNotMatch(application, /subagent_readiness_rejected/u);
  assert.doesNotMatch(providerRuntime, /permission_ready: transport_ready/u);
});

test("Agent Hub probes the exact private target binding with the existing parser", () => {
  assert.match(agentHubCatalog, /executable_binding/u);
  assert.match(agentHubVersion, /binding_belongs_to_agent/u);
  assert.match(agentHubVersion, /run_probe\(executable_binding, args\)/u);
  assert.match(agentHubVersion, /parse_output/u);
});

test("durable authority rejects unsafe lineage before adapter effects", () => {
  for (const code of [
    "subagent_self_call_rejected",
    "subagent_caller_membership_inactive",
    "subagent_target_membership_inactive",
    "subagent_duplicate_active_edge",
    "subagent_cross_conversation_rejected",
    "subagent_lineage_cycle",
    "subagent_repeated_ancestor",
    "subagent_depth_exceeded",
  ]) assert.match(claims, new RegExp(code, "u"));
  assert.match(claims, /TransactionBehavior::Immediate/u);
  assert.match(application, /claim_dispatch[\s\S]*runtime\.send/u);
  assert.match(application, /ReconciliationRequired/u);
});

test("generated guidance is adapter-declared and old visible markup is retired", () => {
  const dispatch = read(
    "crates/licoup-native/src/bin/licoup/stdio_rpc/server/conversation.rs",
  );
  assert.match(providerRuntime, /NativeDeveloperInstructions/u);
  assert.match(providerRuntime, /OrdinaryWirePrefix/u);
  assert.match(dispatch, /compose_generated_instruction_delivery/u);
  assert.doesNotMatch(application, /privateInstructions/u);
});

test("independent verification routes retain one target-keyed latest-version Manifest", () => {
  const en = read("docs/protocols/subagent-mcp.md");
  const zh = read("docs/protocols/subagent-mcp.zh-CN.md");
  const upstream = read("tests/product-e2e/cli/subagent-mcp/upstream.mjs");
  const downstream = read("tests/product-e2e/cli/subagent-mcp/downstream.mjs");
  const manifest = read("tests/product-e2e/cli/subagent-mcp/interop-manifest.mjs");
  for (const source of [en, zh]) {
    assert.match(source, /tests\/product-e2e\/cli\/subagent-mcp\/interop-manifest\.yaml/u);
    assert.match(source, /upstream\.mjs/u);
    assert.match(source, /downstream\.mjs/u);
  }
  assert.match(upstream, /Promise\.all/u);
  assert.match(downstream, /options\.live !== true/u);
  assert.match(downstream, /lico_subagent_delegate/u);
  assert.match(downstream, /projectStructuredMcpFailure/u);
  assert.match(downstream, /runtimeAvailable/u);
  assert.doesNotMatch(downstream, /conversationReadiness === "ready"/u);
  assert.match(downstream, /conversation\.subagent\.edge|readCanonicalEdge/u);
  assert.match(manifest, /TARGET_AGENTS/u);
  assert.match(manifest, /Results.*Notes/su);
});

// ---------------------------------------------------------------------------
// MCP-OPTIONAL-COMPOSITION — the minimal client carries no MCP payload, nothing
// starts the service on the client's behalf, and the service arrives only as an
// independently released package whose absence is a state rather than a silent
// failure.
// ---------------------------------------------------------------------------

const packaging = JSON.parse(read("apps/desktop/packaging.modules.json"));
const conversationHost = read(
  "crates/licoup-native/src/bin/licoup/conversation_host.rs",
);
const stagedEntry = read("crates/licoup-public-gateway/package/bin/lico-subagent-mcp");
const stageToolPath = "tools/distribution/client-release-package-stage.mjs";
const stageTool = read(stageToolPath);
// The tool exports its own entry-point-scoped gate, so one package's staged
// bytes can be asked about directly instead of whichever one the whole-set
// command reaches first. Importing it runs nothing: the CLI has its own guard.
const { loadStagedPlan, verifyEntries } = await import(
  pathToFileURL(path.join(repoRoot, stageToolPath)).href);

function runStage(args) {
  return spawnSync(process.execPath, [stageToolPath, ...args], {
    cwd: repoRoot,
    encoding: "utf8",
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
    timeout: 120_000,
    maxBuffer: 16 * 1024 * 1024,
  });
}

test("the minimal client carries no MCP payload in any packaging module", () => {
  const service = packaging.modules["subagents-mcp"];
  assert.equal(service.required, false, "the service module is optional");
  assert.equal(service.enabled, false, "the minimal client does not bundle it");
  assert.equal(service.cargoBin, undefined, "the module bundles no binary");
  assert.equal(service.embeddedCargoBin, undefined);

  // Nothing enabled anywhere bundles the connector, directly or embedded.
  for (const [id, module] of Object.entries(packaging.modules)) {
    if (module.enabled === false) continue;
    for (const binary of [module.cargoBin, module.embeddedCargoBin]) {
      assert.notEqual(binary, "lico-subagent-mcp",
        `${id} must not bundle the optional connector`);
    }
  }

  // The Codex plugin is installed from the Codex plugin marketplace: it carries
  // no second copy of the connector and does not depend on the optional module.
  const codex = packaging.modules["codex-plugin"];
  assert.equal(codex.embeddedCargoBin, undefined);
  assert.equal(codex.embeddedCargoTarget, undefined);
  assert.equal((codex.requires || []).includes("subagents-mcp"), false);

  // The declared bundle set is the architecture ratchet's own truth: it derives
  // the set from this config and refuses a module that bundles an optional
  // artifact without declaring it, so re-bundling the payload stops being a
  // quiet packaging edit (`verify-client-architecture.mjs`).
});

test("nothing starts the optional service, and no hidden switch remains", () => {
  assert.equal(conversationHost.includes("LICOUP_MCP_AUTOSTART"), false);
  assert.equal(conversationHost.includes("mcp_service_process"), false,
    "the conversation host does not reach the optional service at all");
  for (const relative of [
    "crates/licoup-public-gateway/src/application.rs",
    "crates/licoup-native/src/bin/licoup/conversation_host.rs",
    "crates/licoup-native/src/platform/mcp_service_process.rs",
    "crates/licoup-native/tests/cli_command_contract_cases.rs",
    "crates/licoup-native/tests/data_home_process.rs",
  ]) {
    assert.equal(read(relative).includes("LICOUP_MCP_AUTOSTART"), false,
      `${relative} must not carry the retired autostart switch`);
  }
  // "The host does not start it" has to stay true of the owning module and the
  // CLI command table too, not only of the one caller that was edited. A
  // host-startup entry point left behind is dead code today and a second
  // activation path tomorrow, and the package's own `on-demand` activation
  // would stop being the truth about when the service runs.
  for (const [label, source] of [
    ["the conversation host", conversationHost],
    ["the package lifecycle module", lifecycle],
    ["the CLI command table", commandTable],
  ]) {
    assert.equal(source.includes("start_on_host_startup"), false,
      `${label} must not carry the retired host-startup entry point`);
  }
});

test("the service is a package, and the package declares when it may run", () => {
  const manifest = JSON.parse(read("crates/licoup-public-gateway/package/manifest.json"));
  assert.equal(manifest.id, "org.licoland.feature.mcp");
  assert.equal(manifest.runtime.mode, "process");
  assert.equal(manifest.runtime.entry, "bin/lico-subagent-mcp");
  // On-demand is why nothing launches it at client startup: the package itself
  // says a caller starts it.
  assert.equal(manifest.activation, "on-demand");
  assert.deepEqual(
    manifest.profiles.flatMap((profile) => profile.capabilities),
    ["mcp-server.v1"],
  );
  assert.deepEqual(manifest.requires, [], "the package requires nothing");
  assert.match(packaging.modules["subagents-mcp"].label,
    /org\.licoland\.feature\.mcp/u,
    "the optional module names the package that delivers it");

  // The capability's declared owner is that package, so absence is a fact about
  // this installation rather than a defect of the client.
  const deployment = read("crates/licoup-extension-contracts/src/deployment.rs");
  assert.match(deployment,
    /"mcp-server\.v1",\s*\n\s*PackOwnership::Optional\("org\.licoland\.feature\.mcp"\),/u);
});

test("the release stage fills the declared native entry before the payload is built", () => {
  // The committed entry is the staged placeholder the package declares, and the
  // payload tooling accepts it so a checkout packages deterministically.
  assert.match(stagedEntry, /staged entry/u);
  assert.match(stagedEntry, /cargo build --bin lico-subagent-mcp/u);
  assert.match(stagedEntry, /tools\/scripts\/client-release-package-index\.mjs/u);

  // The release step builds through the client's own native build owner and
  // refuses an entry that is not a compiled program for the target platform.
  assert.match(stageTool, /buildNativeSidecars/u);
  assert.match(stageTool, /package_stage_entry_not_compiled/u);
  assert.match(stageTool, /compiledProgramHeader/u);
  assert.match(stageTool, /chmodSync\(entry\.stagedPath, 0o755\)/u);
  assert.match(stageTool, /client-release-package-index\.mjs build/u);

  const plan = runStage(["plan"]);
  assert.equal(plan.status, 0, plan.stderr);
  const planned = JSON.parse(plan.stdout);
  const mcp = planned.entries.find((entry) => entry.packageId === "org.licoland.feature.mcp");
  assert.deepEqual(
    { source: mcp.source, entry: mcp.entry, binary: mcp.binary },
    {
      source: "crates/licoup-public-gateway/package",
      entry: "bin/lico-subagent-mcp",
      binary: "lico-subagent-mcp",
    },
  );
  assert.equal(mcp.staged.sha256, `sha256:${createHash("sha256")
    .update(readFileSync("crates/licoup-public-gateway/package/bin/lico-subagent-mcp")).digest("hex")}`,
  "the plan reports the bytes actually staged in the checkout");

  // A checkout cannot pass the release gate: the placeholder is not a program.
  // That refusal is exactly what proves the release step is the only thing that
  // can put an MCP payload on a released machine.
  //
  // The refusal is asked for the MCP entry on its own, because the release set
  // declares several packages and the whole-set command stops at whichever
  // placeholder it reaches first; that first refusal is still asserted below,
  // but it is no longer evidence about this package.
  let refusal;
  try {
    const mcpEntries = loadStagedPlan().filter(
      (entry) => entry.packageId === "org.licoland.feature.mcp");
    assert.equal(mcpEntries.length, 1, "the release set declares exactly one MCP package");
    verifyEntries(mcpEntries);
    assert.fail("a checkout must not pass the release gate");
  } catch (error) {
    refusal = error;
  }
  assert.equal(refusal.code, "package_stage_entry_not_compiled");
  assert.equal(refusal.details?.packageId, "org.licoland.feature.mcp");
  assert.equal(refusal.details?.entry, "bin/lico-subagent-mcp");

  const verify = runStage(["verify"]);
  assert.equal(verify.status, 1, verify.stdout);
  assert.equal(JSON.parse(verify.stderr).code, "package_stage_entry_not_compiled");

  // The gate itself accepts a compiled entry, so it is a real check rather than
  // a rule nothing can satisfy.
  const selfTest = runStage(["--self-test"]);
  assert.equal(selfTest.status, 0, selfTest.stderr);
  assert.equal(JSON.parse(selfTest.stdout).ok, true);
});
