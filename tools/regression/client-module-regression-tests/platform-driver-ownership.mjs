import {
  assert,
  path,
  process,
  test,
  CLIENT_MODULE_CATALOG,
  selectModulesForChangedPaths,
  main,
  ids,
  sourceFiles,
} from "./support.mjs";

function ownsInput(inputs, file) {
  return [...inputs].some((input) => input.endsWith("/**")
    ? file.startsWith(input.slice(0, -2)) || file === input.slice(0, -3)
    : input === file);
}

test("layer, FFI, bridge, packaging, and release paths select dedicated modules", () => {
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/domain/mobile_relay/config.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.domain.mobile-relay.configuration"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-targets/src/domain/targets/binaries.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "regression.agent-scan-paths",
    "architecture.client-boundaries",
    "rust.domain.targets.binaries",
    "rust.domain.targets.platform-integration",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/ffi/android_ffi.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.ffi.android-secret-store-tristate",
    "bridge.android",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/ffi/commands/mcp.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "regression.cli-command-admission-source-bundle",
    "architecture.client-boundaries",
    "rust.ffi.cli-command-admission",
    "bridge.native-mcp-command",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/platform/mcp_streamable_http.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.mcp-streamable-http"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/platform/mcp_approval_plan_store.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.mcp-approval-plan-store"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/bin/licoup.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.ffi.cli-command-admission",
    "rust.bin.licoup",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/bin/licoup/stdio_rpc.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.bin.licoup.rpc",
    "bridge.native-mcp-rpc-guard",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/bin/licoup/stdio_rpc/request.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.ffi.client-state-contract",
    "rust.ffi.cli-command-admission",
    "rust.bin.licoup.rpc",
    "bridge.native-mcp-rpc-guard",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/bin/licoup/tests/rpc/request.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.bin.licoup.rpc"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/bin/licoup/tests/skill_commands.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.bin.licoup.skill-commands"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/ios/Runner/SecureMeshIosBridge.swift",
  ])), ["architecture.client-boundaries", "bridge.ios"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/scripts/build-android-apk.mjs",
  ])), ["packaging.android"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "tools/apple-release/macos-direct-arm64.json",
  ])), ["release.contracts", "release.workflows"]);
});

test("Android Secure Mesh leaves select boundary tests and Kotlin compile independently", () => {
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/MainActivity.kt",
  ])), [
    "architecture.client-boundaries",
    "bridge.android.secure-mesh-boundaries",
    "bridge.android.kotlin-compile",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/android/app/src/test/kotlin/land/lico/licoup/SecureMeshAndroidBridgeBoundaryTest.kt",
  ])), [
    "architecture.client-boundaries",
    "bridge.android.secure-mesh-boundaries",
  ]);
  for (const source of [
    "SecureMeshAndroidAtomicRecordWriter.kt",
    "SecureMeshAndroidNativeDispatchQueue.kt",
  ]) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `apps/desktop/android/app/src/main/kotlin/land/lico/licoup/${source}`,
    ])), [
      "architecture.client-boundaries",
      "flutter.feature.mobile-relay.scenario.android-bridge",
    ]);
  }

  const boundaries = CLIENT_MODULE_CATALOG.find(
    (module) => module.id === "bridge.android.secure-mesh-boundaries",
  );
  const compile = CLIENT_MODULE_CATALOG.find(
    (module) => module.id === "bridge.android.kotlin-compile",
  );
  assert.equal(boundaries.command.args.includes(":app:testDebugUnitTest"), true);
  for (const className of [
    "land.lico.licoup.SecureMeshAndroidBridgeBoundaryTest",
    "land.lico.licoup.SecureMeshAndroidSecretStoreBoundaryTest",
    "land.lico.licoup.SecureMeshAndroidSecretContractTest",
  ]) {
    assert.equal(boundaries.command.args.includes(className), true);
  }
  assert.equal(compile.command.args.includes(":app:compileDebugKotlin"), true);
});

test("foundation adapters and architecture scripts have explicit changed-path owners", () => {
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/scripts/verify-client-architecture.mjs",
  ])), [
    "regression.client-architecture-modules",
    "regression.client-architecture-ratchet",
    "architecture.client-boundaries",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/core/acp.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.acp.composition"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/core/acp/requests.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.acp.requests"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/core/acp/responses.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.acp.responses"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/core/acp/codec.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.acp.codec"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/core/task_queue.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.task-queue"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/platform/ansi_stripper.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.ansi-stripper"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/platform/url_security.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.url-security"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/authorized_secure_record.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.secure-mesh.secret-custody-port",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/platform/authorized_secure_record/ledger.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.platform.secure-mesh-secret-store.authorization",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/platform/user_presence.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.platform.secure-mesh-secret-store.authorization",
  ]);
  for (const source of [
    "authority.rs",
    "runner_signature.rs",
    "test_support.rs",
    "transaction.rs",
  ]) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `crates/licoup-native/src/domain/collaboration_plugin/${source}`,
    ])), [
      "regression.rust-format",
      "regression.rust-clippy",
      "architecture.client-boundaries",
      "rust.domain.optional-collaboration",
    ]);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/domain/collaboration_plugin/workflow/mcp_transaction.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.domain.optional-collaboration.workflow-operations.apply-mcp",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/mcp.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.mcp.composition"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/mcp/wire.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.mcp.wire"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/mcp/transfer.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.core.mcp.transfer"]);
  for (const source of [
    "crates/licoup-foundation/src/core/safe_archive.rs",
    "crates/licoup-foundation/src/core/safe_archive/zip_structure.rs",
  ]) {
    assert.deepEqual(ids(selectModulesForChangedPaths([source])), [
      "regression.rust-format",
      "regression.rust-clippy",
      "architecture.client-boundaries",
      "rust.domain.adaptive-flywheel",
      "rust.domain.optional-collaboration",
      "rust.core.safe-archive",
      "rust.core.full-data-root-archive",
      "rust.platform.extension-packages.artifact",
    ]);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-foundation/src/platform/paths.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "regression.agent-scan-paths",
    "architecture.client-boundaries",
    "rust.domain.targets.scan-paths",
    "rust.domain.targets.platform-paths",
    "rust.platform.paths",
    "rust.platform",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/platform/extension_packages/artifact.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.platform.extension-packages",
    "rust.platform.extension-packages.artifact",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/tests/package_lifecycle/main.rs",
  ])), ["regression.rust-format","regression.rust-clippy","rust.platform.extension-packages"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "tests/integration/package_lifecycle/verify_component_lifecycle.py",
  ])), ["rust.platform.extension-packages"]);
  assert.deepEqual(
    CLIENT_MODULE_CATALOG.find((candidate) =>
      candidate.id === "rust.platform.extension-packages").command.args,
    [
      "test",
      "--manifest-path",
      "crates/licoup-native/Cargo.toml",
      "--test",
      "package_lifecycle",
    ],
  );
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/secure_mesh_pairwise.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.secure-mesh.pairwise-codec",
    "rust.core.secure-mesh.pairwise-key-ratchet.core",
    "rust.core.secure-mesh.pairwise-manager-fanout",
    "rust.core.secure-mesh.pairwise-persistence",
    "rust.core.secure-mesh.pairwise-runtime-self-test",
    "rust.core.secure-mesh.pairwise-session-negotiation.handshake-machine",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/secure_mesh_pairwise/codec.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.secure-mesh.pairwise-codec",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-protocol-bindings/src/licoarc_relay.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.protocol-bindings",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-protocol-bindings/src/licoarc_relay/mailbox/schedule.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.protocol-bindings",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-protocol-bindings/src/licoarc_relay/private_header.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.protocol-bindings",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/secure_mesh_command.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.secure-mesh.command",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/secure_mesh_command/schema.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.secure-mesh.command.schema",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-native/src/core/secure_mesh_directory/authority.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.secure-mesh.directory",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/skill_hub_controller_test.dart",
  ])), ["flutter.feature.skill-hub"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/target_controller_test.dart",
  ])), ["flutter.feature.targets"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/client_navigation_controller_test.dart",
  ])), ["flutter.layer.shell"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/native_cli_runtime_context_test.dart",
  ])), ["bridge.flutter-native-client"]);
  for (const source of [
    "native_conversation_command_policy.dart",
    "native_conversation_port.dart",
  ]) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      `apps/desktop/lib/src/platform/native_client/${source}`,
    ])), [
      "architecture.client-boundaries",
      "bridge.flutter-native-client.stdio-transport",
      "regression.native-stdio-rpc-source-bundle",
    ]);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/lib/src/platform/native_client/agent_service_stdio_rpc/client.dart",
  ])), [
    "architecture.client-boundaries",
    "bridge.flutter-native-client.stdio-transport",
    "bridge.flutter-native-client.stdio-integration",
    "regression.native-stdio-rpc-source-bundle",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/lib/src/platform/native_client/agent_service_stdio_rpc/protocol.dart",
  ])), [
    "architecture.client-boundaries",
    "bridge.flutter-native-client.stdio-codec",
    "bridge.flutter-native-client.stdio-integration",
    "regression.native-stdio-rpc-source-bundle",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/native_stdio_rpc_protocol_test.dart",
  ])), [
    "bridge.flutter-native-client.stdio-codec",
    "regression.native-stdio-rpc-source-bundle",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/native_stdio_rpc_client_test.dart",
  ])), [
    "bridge.flutter-native-client.stdio-transport",
    "bridge.flutter-native-client.stdio-integration",
    "regression.native-stdio-rpc-source-bundle",
  ]);
  const stdioCodec = CLIENT_MODULE_CATALOG.find((module) =>
    module.id === "bridge.flutter-native-client.stdio-codec");
  const stdioTransport = CLIENT_MODULE_CATALOG.find((module) =>
    module.id === "bridge.flutter-native-client.stdio-transport");
  const stdioIntegration = CLIENT_MODULE_CATALOG.find((module) =>
    module.id === "bridge.flutter-native-client.stdio-integration");
  const stdioSourceBundle = CLIENT_MODULE_CATALOG.find((module) =>
    module.id === "regression.native-stdio-rpc-source-bundle");
  assert.deepEqual(stdioCodec.command.args.slice(-2), [
    "test/native_stdio_rpc_line_framer_test.dart",
    "test/native_stdio_rpc_protocol_test.dart",
  ]);
  assert.deepEqual(stdioTransport.command.args.slice(-10), [
    "test/stdio_rpc_method_policy_test.dart",
    "test/native_stdio_rpc_client_test.dart",
    "test/native_stdio_rpc_read_pool_test.dart",
    "test/native_stdio_rpc_decoding_test.dart",
    "test/native_stdio_rpc_operation_pending_queue_test.dart",
    "test/conversation_execution_transport_test.dart",
    "test/native_conversation_port_test.dart",
    "test/stdio_rpc_operation_queue_test.dart",
    "test/stdio_transport/transport_control_priority_test.dart",
    "test/stdio_transport/transport_decode_lane_test.dart",
  ]);
  assert.deepEqual(stdioIntegration.command.args.slice(-2), ["--name", "RPC"]);
  assert.deepEqual(stdioSourceBundle.command.args, [
    "--test",
    "tests/contract/client/native-stdio-rpc-source-bundle.test.mjs",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "apps/desktop/test/directory_path_controller_test.dart",
  ])), ["flutter.feature.settings"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "tools/scripts/client-module-regression.mjs",
  ])), ["regression.infrastructure", "architecture.client-boundaries"]);
});

test("Cursor and OpenAgent leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.domain.agent-conversations.parser-cursor-openagent.composition",
      "domain::conversation::history::cursor_openagent::tests::composition::"],
    ["rust.domain.agent-conversations.parser-cursor-openagent.codec",
      "domain::conversation::history::cursor_openagent::tests::codec::"],
    ["rust.domain.agent-conversations.parser-cursor-openagent.cursor",
      "domain::conversation::history::cursor_openagent::tests::cursor::"],
    ["rust.domain.agent-conversations.parser-cursor-openagent.cursor-projection",
      "domain::conversation::history::cursor_openagent::tests::cursor_projection::"],
    ["rust.domain.agent-conversations.parser-cursor-openagent.openagent",
      "domain::conversation::history::cursor_openagent::tests::openagent::"],
    ["rust.domain.agent-conversations.parser-cursor-openagent.fallback",
      "domain::conversation::history::cursor_openagent::tests::fallback::"],
    ["rust.domain.agent-conversations.parser-cursor-openagent.integration",
      "domain::conversation::history::tests::cursor_openagent"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.domain.agent-conversations.parser-cursor-openagent."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
  }
  assert.equal(CLIENT_MODULE_CATALOG.some((candidate) =>
    candidate.id === "rust.domain.agent-conversations.parser-cursor-openagent"), false);

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.cursor-openagent-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/domain/conversation/history/cursor_openagent",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-native/src/domain/conversation/history/cursor_openagent.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Cursor/OpenAgent source must have a focused regression owner: ${relativePath}`);
  }
});

test("neutral ACP runtime and session transport retain bounded ownership", async () => {
  const filters = new Map([
    ["rust.platform.acp-runtime.composition",
      "acp_driver_runtime::tests::composition::"],
    ["rust.platform.acp-runtime.test-support",
      "acp_driver_runtime::tests::"],
    ["rust.platform.acp-runtime.continuity",
      "acp_driver_runtime::tests::continuity::"],
    ["rust.platform.acp-runtime.errors",
      "acp_driver_runtime::tests::errors::"],
    ["rust.platform.acp-runtime.events",
      "acp_driver_runtime::tests::events::"],
    ["rust.platform.acp-runtime.interaction",
      "acp_driver_runtime::tests::interaction::"],
    ["rust.platform.acp-runtime.io",
      "acp_driver_runtime::tests::io::"],
    ["rust.platform.acp-runtime.model",
      "acp_driver_runtime::tests::model::"],
    ["rust.platform.acp-runtime.params",
      "acp_driver_runtime::tests::params::"],
    ["rust.platform.acp-runtime.probe",
      "acp_driver_runtime::tests::probe::"],
    ["rust.platform.acp-runtime.protocol",
      "acp_driver_runtime::tests::protocol::"],
    ["rust.platform.acp-runtime.settings",
      "acp_driver_runtime::tests::settings::"],
    ["rust.platform.acp-runtime.stdio-transport",
      "acp_driver_runtime::tests::stdio_transport::"],
    ["rust.platform.acp-runtime.supervision",
      "acp_driver_runtime::tests::supervision::"],
    ["rust.platform.acp-runtime.replay",
      "acp_driver_runtime::replay::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.acp-runtime."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    assert.equal(CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id)
      .command.args.at(-1), filter);
  }
  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const sources = await sourceFiles(
    "crates/licoup-agent-drivers/src/acp_driver_runtime", ".rs");
  for (const relativePath of [
    "crates/licoup-agent-drivers/src/acp_driver_runtime.rs",
    ...sources,
    "crates/licoup-agent-copilot/src/parser.rs",
    "crates/licoup-agent-kimi/src/parser.rs",
    "crates/licoup-agent-copilot/src/dialect.rs",
    "crates/licoup-agent-copilot/src/dialect.rs",
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `neutral ACP runtime source must have a precise regression owner: ${relativePath}`);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-drivers/src/acp_driver_runtime/session_plan.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.acp-runtime.continuity"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-drivers/src/acp_driver_runtime/params.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.acp-runtime.params"]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-copilot/src/dialect.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.core.agent-copilot-package",
    "rust.platform.acp-runtime.events",
    "rust.platform.acp-runtime.interaction",
    "rust.platform.acp-runtime.protocol",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-drivers/src/acp_driver_runtime/stdio_transport.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.platform.acp-runtime.stdio-transport",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-drivers/src/acp_session_transport/execution.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.platform.acp-session-transport.collaboration-mcp",
  ]);
  assert.equal(CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "rust.platform.acp-session-transport").command.args.at(-1),
  "acp_session_transport::tests::");
  const sessionMcp = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "rust.platform.acp-session-transport.collaboration-mcp");
  assert.equal(sessionMcp.command.args.at(-1),
    "acp_session_transport::tests::");
  const sessionModules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id === "rust.platform.acp-session-transport"
      || candidate.id === "rust.platform.acp-session-transport.collaboration-mcp");
  const sessionInputs = new Set(sessionModules.flatMap((module) => module.inputs));
  for (const relativePath of [
    "crates/licoup-agent-drivers/src/acp_session_transport.rs",
    ...await sourceFiles(
      "crates/licoup-agent-drivers/src/acp_session_transport",
      ".rs",
    ),
    "crates/licoup-agent-hermes/src/parser.rs",
    "crates/licoup-agent-hermes/src/dialect.rs",
    "crates/licoup-agent-hermes/src/dialect.rs",
  ]) {
    assert.equal(sessionInputs.has(relativePath), true,
      `neutral ACP session source must have a precise regression owner: ${relativePath}`);
  }
});

test("Kilo Code adapter leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.kilo-code-driver.composition",
      "driver::tests::composition::"],
    ["rust.platform.kilo-code-driver.config",
      "driver::config::tests::"],
    ["rust.platform.kilo-code-driver.execution",
      "driver::tests::execution::"],
    ["rust.platform.kilo-code-driver.probe",
      "driver::tests::probe::"],
    ["rust.platform.kilo-code-driver.projection",
      "driver::projection::tests::"],
    ["rust.platform.kilo-code-driver.transport",
      "driver::tests::execution::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.kilo-code-driver."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    assert.equal(CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id)
      .command.args.at(-1), filter);
  }
  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const sources = await sourceFiles(
    "crates/licoup-agent-kilo/src/driver", ".rs");
  for (const relativePath of [
    "crates/licoup-agent-kilo/src/driver/mod.rs",
    ...sources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Kilo Code adapter source must have a precise regression owner: ${relativePath}`);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-kilo/src/port/serve.rs",
  ])), ["regression.rust-format","regression.rust-clippy","architecture.client-boundaries","rust.platform.kilo-code-driver.transport","rust.core.agent-kilo-package"]);
});

test("runtime adapter modules retain leaf-owned inputs and exact command filters", () => {
  const filters = new Map([
    ["rust.platform.runtime-adapters.registry",
      "platform::runtime_adapters::tests::registry::"],
    ["rust.platform.runtime-adapters.dispatch",
      "platform::runtime_adapters::tests::adapter_dispatch::"],
    ["rust.platform.runtime-adapters.artifact",
      "platform::runtime_adapters::tests::artifact::"],
    ["rust.platform.runtime-adapters.normalization",
      "platform::runtime_adapters::tests::normalization::"],
    ["rust.platform.runtime-adapters.probe",
      "platform::runtime_adapters::tests::probe::"],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    assert.equal(module.inputs.includes(
      "crates/licoup-native/src/platform/runtime_adapters.rs"), false);
  }
});

test("Codex app-server leaves retain exact narrow regression ownership", async () => {
  const sourceBundleId = "regression.codex-app-server-source-bundle";
  const selections = new Map([
    ["crates/licoup-agent-codex/src/app_server/config.rs",
      "rust.platform.codex-app-server.config"],
    ["crates/licoup-agent-codex/src/parser/session.rs",
      "rust.platform.codex-app-server.session"],
    ["crates/licoup-agent-codex/src/parser/events.rs",
      "rust.platform.codex-app-server.events"],
    ["crates/licoup-agent-codex/src/parser/control.rs",
      "rust.platform.codex-app-server.control"],
    ["crates/licoup-agent-codex/src/app_server/driver/io.rs",
      "rust.platform.codex-app-server.io"],
    ["crates/licoup-agent-codex/src/app_server/driver/launch.rs",
      "rust.platform.codex-app-server.launch"],
    ["crates/licoup-agent-codex/src/app_server/driver/supervision.rs",
      "rust.platform.codex-app-server.transport"],
    ["crates/licoup-agent-codex/src/app_server/driver/transport.rs",
      "rust.platform.codex-app-server.transport"],
  ]);
  for (const [source, moduleId] of selections) {
    assert.deepEqual(ids(selectModulesForChangedPaths([
      source,
    ])), [
      "regression.rust-format", "regression.rust-clippy", sourceBundleId,
      "architecture.client-boundaries", "rust.core.agent-codex-package", moduleId,
    ]);
  }

  const filters = new Map([
    ["rust.platform.codex-app-server", "app_server::driver::tests::"],
    ["rust.platform.codex-app-server.config",
      "app_server::driver::tests::config::"],
    ["rust.platform.codex-app-server.session",
      "app_server::driver::tests::session::"],
    ["rust.platform.codex-app-server.events",
      "app_server::driver::tests::events::"],
    ["rust.platform.codex-app-server.control",
      "app_server::driver::tests::control::"],
    ["rust.platform.codex-app-server.io",
      "app_server::driver::tests::io::"],
    ["rust.platform.codex-app-server.launch",
      "app_server::driver::tests::launch::"],
    ["rust.platform.codex-app-server.transport",
      "app_server::driver::tests::transport::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.codex-app-server"));
  assert.equal(modules.length, filters.size);
  for (const [moduleId, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === moduleId);
    assert.equal(module.command.args.at(-1), filter);
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-agent-codex/src/app_server",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-agent-codex/src/app_server/driver.rs",
    ...splitSources,
    "crates/licoup-agent-codex/src/parser.rs",
    ...await sourceFiles(
      "crates/licoup-agent-codex/src/parser",
      ".rs",
    ),
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Codex app-server source must have a precise regression owner: ${relativePath}`);
  }

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/codex-app-server-source-bundle.test.mjs",
  ]);
});

test("Antigravity adapter package leaves retain exact regression ownership", async () => {
  const sourceBundleId = "regression.antigravity-adapter-package-source-bundle";
  const packageModuleId = "rust.core.agent-antigravity-package";
  // The process half is still composed by the client; the wire half moved into
  // the Antigravity adapter package. A source that moved selects the host's
  // bundle and the package's own module, and the package module owns the crate.
  const moved = new Map([
    ["crates/licoup-agent-antigravity/src/parser.rs", packageModuleId],
    ["crates/licoup-agent-antigravity/src/hook.rs", packageModuleId],
    ["crates/licoup-agent-antigravity/src/registration.rs", packageModuleId],
    ["crates/licoup-agent-antigravity/src/replay.rs", packageModuleId],
  ]);
  for (const [source, owner] of moved) {
    assert.equal(
      ids(selectModulesForChangedPaths([source])).includes(owner),
      true,
      `${source} must select ${owner}`,
    );
    assert.equal(
      ids(selectModulesForChangedPaths([source])).includes(sourceBundleId),
      true,
      `${source} must select ${sourceBundleId}`,
    );
  }
  // The host keeps the driver leaves, and they keep a precise owner rather than
  // the host's whole-directory fallback.
  const driverSources = [
    "crates/licoup-agent-antigravity/src/driver.rs",
    ...await sourceFiles("crates/licoup-agent-antigravity/src/driver", ".rs"),
  ];
  assert.ok(driverSources.length > 0);
  const owns = (relativePath) => CLIENT_MODULE_CATALOG.some((module) =>
    module.inputs.some((input) => input.endsWith("/**")
      ? relativePath.startsWith(input.slice(0, -2))
      : input === relativePath));
  for (const relativePath of driverSources) {
    assert.equal(owns(relativePath), true,
      `Antigravity driver source must have a regression owner: ${relativePath}`);
  }
  // Every source the package ships is owned by the package's own module.
  const packageSources = await sourceFiles("crates/licoup-agent-antigravity/src", ".rs");
  assert.ok(packageSources.length > 0);
  for (const relativePath of packageSources) {
    assert.equal(owns(relativePath), true,
      `Antigravity package source must have a regression owner: ${relativePath}`);
  }
  const packageModule = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === packageModuleId);
  assert.deepEqual(packageModule.inputs, ["crates/licoup-agent-antigravity/**"]);

  const sourceBundle = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === sourceBundleId);
  assert.deepEqual(sourceBundle.command.args, [
    "--test",
    "tests/contract/client/antigravity-adapter-package-source-bundle.test.mjs",
  ]);
});

test("local service leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.local-service.composition", "local_service::tests::composition::"],
    ["rust.platform.local-service.bounds", "local_service::tests::bounds::"],
    ["rust.platform.local-service.concurrency", "local_service::tests::concurrency::"],
    ["rust.platform.local-service.endpoint", "local_service::tests::endpoint::"],
    ["rust.platform.local-service.executable", "local_service::tests::executable::"],
    ["rust.platform.local-service.http", "local_service::tests::http::"],
    ["rust.platform.local-service.params", "local_service::tests::params::"],
    ["rust.platform.local-service.port", "local_service::tests::port::"],
    ["rust.platform.local-service.process", "local_service::tests::process::"],
    ["rust.platform.local-service.serve", "local_service::tests::serve::"],
    ["rust.platform.local-service.sse", "local_service::tests::sse::"],
    ["rust.platform.local-service.state", "local_service::tests::state::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.local-service."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-agent-drivers/src/local_service.rs"), false);
    }
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.local-service-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/local-service-source-bundle.test.mjs"]);
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-agent-drivers/src/local_service", ".rs");
  for (const relativePath of [
    "crates/licoup-agent-drivers/src/local_service.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `local service source must have a precise regression owner: ${relativePath}`);
  }
});

test("file security leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.file-security.composition", "platform::file_security::tests::composition::"],
    ["rust.platform.file-security.policy", "platform::file_security::tests::policy::"],
    ["rust.platform.file-security.append-lock", "platform::file_security::tests::append_lock::"],
    ["rust.platform.file-security.atomic-replace", "platform::file_security::tests::atomic_"],
    ["rust.platform.file-security.marker", "platform::file_security::tests::marker::"],
    ["rust.platform.file-security.validation", "platform::file_security::tests::validation::"],
    ["rust.platform.file-security.sync", "platform::file_security::tests::sync::"],
    ["rust.platform.file-security.hardening", "platform::file_security::tests::hardening::"],
    ["rust.platform.file-security.unix-hardening", "platform::file_security::tests::unix_hardening::"],
    ["rust.platform.file-security.windows-acl", "platform::file_security::tests::windows_acl::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.file-security."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-foundation/src/platform/file_security.rs"), false);
    }
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.file-security-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/file-security-source-bundle.test.mjs"]);
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-foundation/src/platform/file_security", ".rs");
  for (const relativePath of [
    "crates/licoup-foundation/src/platform/file_security.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `file security source must have a precise regression owner: ${relativePath}`);
  }
});

test("client state leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.client-state.composition", "platform::client_state::tests::composition::"],
    ["rust.platform.client-state.policy", "tests::policy::"],
    ["rust.platform.client-state.collections", "tests::collections::"],
    ["rust.platform.client-state.activity", "tests::activity::"],
    ["rust.platform.client-state.snapshots", "tests::snapshots::"],
    ["rust.platform.client-state.redaction", "tests::redaction::"],
    ["rust.platform.client-state.serialization", "tests::serialization::"],
    ["rust.platform.client-state.paths", "tests::paths::"],
    ["rust.platform.client-state.accessors", "tests::accessors::"],
    ["rust.platform.client-state.operations", "platform::client_state::tests::operations::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.client-state."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/platform/client_state.rs"), false);
    }
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.client-state-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/client-state-source-bundle.test.mjs"]);
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/platform/client_state", ".rs");
  for (const relativePath of [
    "crates/licoup-native/src/platform/client_state.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `client state source must have a precise regression owner: ${relativePath}`);
  }
});

test("OpenCode serve leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.opencode-serve.composition", "platform::opencode_serve::tests::composition::"],
    ["rust.platform.opencode-serve.policy", "platform::opencode_serve::tests::policy::"],
    ["rust.platform.opencode-serve.events", "platform::opencode_serve::tests::events::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.opencode-serve."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.opencode-serve-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/platform/opencode_serve", ".rs");
  for (const relativePath of [
    "crates/licoup-native/src/platform/opencode_serve.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `OpenCode serve source must have a precise regression owner: ${relativePath}`);
  }
});

test("Kilo Code serve leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.kilo-code-serve.composition", "platform::kilo_code_host::tests::"],
    ["rust.platform.kilo-code-serve.policy", "policy::tests::"],
    ["rust.platform.kilo-code-serve.events", "parser::serve::tests::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.kilo-code-serve."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.kilo-code-serve-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/platform/kilo_code_host", ".rs");
  for (const relativePath of [
    "crates/licoup-native/src/platform/kilo_code_host.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Kilo Code serve source must have a precise regression owner: ${relativePath}`);
  }
});

test("OpenClaw Gateway leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.openclaw-gateway.composition", "platform::openclaw_gateway::tests::composition::"],
    ["rust.platform.openclaw-gateway.command", "platform::openclaw_gateway::tests::command::"],
    ["rust.platform.openclaw-gateway.config", "platform::openclaw_gateway::tests::config::"],
    ["rust.platform.openclaw-gateway.health", "platform::openclaw_gateway::tests::health::"],
    ["rust.platform.openclaw-gateway.lifecycle", "platform::openclaw_gateway::tests::lifecycle::"],
    ["rust.platform.openclaw-gateway.model", "platform::openclaw_gateway::tests::model::"],
    ["rust.platform.openclaw-gateway.policy", "platform::openclaw_gateway::tests::policy::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.openclaw-gateway."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
  }
  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.openclaw-gateway-source-bundle");
  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-native/src/platform/openclaw_gateway", ".rs");
  for (const relativePath of [
    "crates/licoup-native/src/platform/openclaw_gateway.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `OpenClaw Gateway source must have a precise regression owner: ${relativePath}`);
  }
});

test("Claude Code driver leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.claude-code-driver.composition",
      "driver::tests::composition::"],
    ["rust.platform.claude-code-driver.test-support",
      "driver::tests::"],
    ["rust.platform.claude-code-driver.model",
      "driver::tests::model::"],
    ["rust.platform.claude-code-driver.errors",
      "driver::tests::failure::"],
    ["rust.platform.claude-code-driver.params",
      "driver::tests::command::"],
    ["rust.platform.claude-code-driver.command",
      "driver::tests::command::"],
    ["rust.platform.claude-code-driver.events",
      "driver::tests::events::"],
    ["rust.platform.claude-code-driver.protocol",
      "driver::tests::protocol::"],
    ["rust.platform.claude-code-driver.io",
      "driver::tests::io::"],
    ["rust.platform.claude-code-driver.control",
      "driver::tests::control::"],
    ["rust.platform.claude-code-driver.transport",
      "driver::tests::transport::"],
    ["rust.platform.claude-code-driver.supervision",
      "driver::tests::supervision::"],
    ["rust.platform.claude-code-driver.probe",
      "driver::tests::probe::"],
    ["rust.platform.claude-code-driver.execution",
      "driver::tests::execution::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.claude-code-driver."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-agent-claude-code/src/driver.rs"), false);
    }
  }

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.claude-code-driver-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/claude-code-driver-source-bundle.test.mjs"]);

  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-agent-claude-code/src/driver",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-agent-claude-code/src/driver.rs",
    ...splitSources,
    "crates/licoup-agent-claude-code/src/protocol/parser/adapter.rs",
    ...await sourceFiles(
      "crates/licoup-agent-claude-code/src/protocol/parser",
      ".rs",
    ),
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Claude Code driver source must have a precise regression owner: ${relativePath}`);
  }
});

test("OpenClaw driver leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.openclaw-driver.composition",
      "driver::tests::composition::"],
    ["rust.platform.openclaw-driver.test-support",
      "driver::tests::"],
    ["rust.platform.openclaw-driver.model",
      "driver::tests::model::"],
    ["rust.platform.openclaw-driver.errors",
      "driver::tests::errors::"],
    ["rust.platform.openclaw-driver.params",
      "driver::tests::params::"],
    ["rust.platform.openclaw-driver.codec",
      "driver::tests::codec::"],
    ["rust.platform.openclaw-driver.continuity",
      "driver::tests::continuity::"],
    ["rust.platform.openclaw-driver.events",
      "driver::tests::events::"],
    ["rust.platform.openclaw-driver.protocol",
      "driver::tests::protocol::"],
    ["rust.platform.openclaw-driver.interaction",
      "driver::tests::interaction::"],
    ["rust.platform.openclaw-driver.io",
      "driver::tests::io::"],
    ["rust.platform.openclaw-driver.supervision",
      "driver::tests::supervision::"],
    ["rust.platform.openclaw-driver.probe",
      "driver::tests::probe::"],
    ["rust.platform.openclaw-driver.execution",
      "driver::tests::execution::"],
    ["rust.platform.openclaw-driver.replay",
      "replay::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.openclaw-driver."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-agent-openclaw/src/driver.rs"), false);
    }
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-openclaw/src/gateway_acp/params.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "regression.openclaw-driver-source-bundle",
    "rust.platform.openclaw-driver.params",
    "rust.core.agent-openclaw-package",
  ]);
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-openclaw/src/gateway_acp/continuity.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "regression.openclaw-driver-source-bundle",
    "rust.platform.openclaw-driver.continuity",
    "rust.core.agent-openclaw-package",
  ]);

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.openclaw-driver-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/openclaw-driver-source-bundle.test.mjs"]);

  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-agent-openclaw/src/driver",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-agent-openclaw/src/driver.rs",
    ...splitSources,
    "crates/licoup-agent-openclaw/src/parser.rs",
    ...await sourceFiles(
      "crates/licoup-agent-openclaw/src/parser",
      ".rs",
    ),
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `OpenClaw driver source must have a precise regression owner: ${relativePath}`);
  }
});

test("Pi driver leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.pi-driver.composition",
      "driver::tests::composition::"],
    ["rust.platform.pi-driver.test-support",
      "driver::tests::"],
    ["rust.platform.pi-driver.model",
      "driver::tests::model::"],
    ["rust.platform.pi-driver.errors",
      "driver::tests::errors::"],
    ["rust.platform.pi-driver.params",
      "driver::tests::params::"],
    ["rust.platform.pi-driver.settings",
      "driver::tests::settings::"],
    ["rust.platform.pi-driver.protocol",
      "driver::tests::parser_protocol::"],
    ["rust.platform.pi-driver.interaction",
      "driver::tests::interaction::"],
    ["rust.platform.pi-driver.events",
      "driver::tests::parser_events::"],
    ["rust.platform.pi-driver.sessions",
      "driver::tests::sessions::"],
    ["rust.platform.pi-driver.io",
      "driver::tests::io::"],
    ["rust.platform.pi-driver.supervision",
      "driver::tests::supervision::"],
    ["rust.platform.pi-driver.probe",
      "driver::tests::probe::"],
    ["rust.platform.pi-driver.execution",
      "driver::tests::execution::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.pi-driver."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-agent-pi/src/driver.rs"), false);
    }
  }

  const sourceCheck = CLIENT_MODULE_CATALOG.find((candidate) =>
    candidate.id === "regression.pi-driver-source-bundle");
  assert.deepEqual(sourceCheck.command.args,
    ["--test", "tests/contract/client/pi-driver-source-bundle.test.mjs"]);

  const ownedInputs = new Set([
    ...modules.flatMap((module) => module.inputs),
    ...sourceCheck.inputs,
  ]);
  const splitSources = await sourceFiles(
    "crates/licoup-agent-pi/src/driver",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-agent-pi/src/driver.rs",
    ...splitSources,
    "crates/licoup-agent-pi/src/parser.rs",
    ...await sourceFiles(
      "crates/licoup-agent-pi/src/parser",
      ".rs",
    ),
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Pi driver source must have a precise regression owner: ${relativePath}`);
  }
});

test("OpenCode driver leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.opencode-driver.composition",
      "driver::tests::composition::"],
    ["rust.platform.opencode-driver.test-support",
      "driver::tests::"],
    ["rust.platform.opencode-driver.probe",
      "driver::tests::probe::"],
    ["rust.platform.opencode-driver.serve-transport",
      "driver::tests::serve_transport::"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.opencode-driver."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-agent-opencode/src/driver.rs"), false);
    }
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-agent-opencode/src/driver",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-agent-opencode/src/driver.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `OpenCode driver source must have a precise regression owner: ${relativePath}`);
  }
  assert.deepEqual(ids(selectModulesForChangedPaths([
    "crates/licoup-agent-opencode/src/driver/continuity.rs",
  ])), [
    "regression.rust-format",
    "regression.rust-clippy",
    "architecture.client-boundaries",
    "rust.platform.opencode-driver.serve-transport",
    "rust.core.agent-opencode-package",
    "regression.adapter-package-contracts",
  ]);
});

test("Hermes driver leaves retain exact tests and complete source ownership", async () => {
  const filters = new Map([
    ["rust.platform.hermes-driver.composition",
      "driver::tests::composition::"],
    ["rust.platform.hermes-driver.test-support",
      "driver::tests::"],
    ["rust.platform.hermes-driver.capabilities",
      "driver::tests::capabilities::"],
    ["rust.platform.hermes-driver.command",
      "driver::tests::command::"],
    ["rust.platform.hermes-driver.protocol",
      "driver::tests::protocol::"],
    ["rust.platform.hermes-driver.events",
      "driver::tests::events::"],
    ["rust.platform.hermes-driver.approval",
      "driver::tests::approval::"],
    ["rust.platform.hermes-driver.process-io",
      "driver::tests::process_io::"],
    ["rust.platform.hermes-driver.execution",
      "driver::tests::execution::"],
    ["rust.platform.hermes-driver.continuity",
      "driver::tests::continuity::"],
    ["rust.platform.hermes-driver.probe",
      "driver::tests::probe::"],
    ["rust.platform.hermes-driver.error-normalization",
      "driver::tests::errors::"],
    ["rust.platform.hermes-driver.tui-gateway",
      "tui_gateway"],
  ]);
  const modules = CLIENT_MODULE_CATALOG.filter((candidate) =>
    candidate.id.startsWith("rust.platform.hermes-driver."));
  assert.equal(modules.length, filters.size);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.equal(module.command.args.at(-1), filter);
    if (!id.endsWith(".composition")) {
      assert.equal(module.inputs.includes(
        "crates/licoup-agent-hermes/src/driver.rs"), false);
    }
  }

  const ownedInputs = new Set(modules.flatMap((module) => module.inputs));
  const splitSources = await sourceFiles(
    "crates/licoup-agent-hermes/src/driver",
    ".rs",
  );
  for (const relativePath of [
    "crates/licoup-agent-hermes/src/driver.rs",
    ...splitSources,
  ]) {
    assert.equal(ownsInput(ownedInputs, relativePath), true,
      `Hermes driver source must have a precise regression owner: ${relativePath}`);
  }
});

test("native CLI modules retain exact binary-scoped command filters", () => {
  const filters = new Map([
    ["rust.bin.licoup", ["tests::"]],
    ["rust.bin.licoup.rpc", ["--", "tests::rpc::", "stdio_rpc::server::conversation::",
      "stdio_rpc::server::work_control_routing_tests::",
      "stdio_rpc::request::work_control_routing_tests::"]],
    ["rust.bin.licoup.core-commands", ["tests::core_commands::"]],
    ["rust.bin.licoup.skill-commands", ["tests::skill_commands::"]],
    ["rust.bin.licoup.parsing", ["tests::parsing::"]],
  ]);
  for (const [id, filter] of filters) {
    const module = CLIENT_MODULE_CATALOG.find((candidate) => candidate.id === id);
    assert.deepEqual(module.command.args, [
      "test",
      "-p",
      "licoup-native",
      "--bin",
      "licoup-cli",
      ...filter,
    ]);
    if (id !== "rust.bin.licoup") {
      assert.equal(module.inputs.includes(
        "crates/licoup-native/src/bin/licoup.rs"), false);
    }
  }
});
