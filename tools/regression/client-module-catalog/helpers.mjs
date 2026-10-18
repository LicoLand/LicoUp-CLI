import { classifyClientModule } from "../client-regression-metadata.mjs";

const REPO_ROOT = ".";
export const CLIENT_MODULE_RUNNABLE_HOSTS = Object.freeze([
  "darwin",
  "linux",
  "win32",
]);
export const NATIVE_MANIFEST = "crates/licoup-native/Cargo.toml";
// The shared Agent adapter contract lives in this crate, so the modules that
// moved there run their tests against its manifest rather than against the host.
export const ADAPTER_SDK_MANIFEST = "crates/licoup-agent-adapter-sdk/Cargo.toml";
export const FOUNDATION_MANIFEST = "crates/licoup-foundation/Cargo.toml";
export const GATEWAY_CORE_MANIFEST = "crates/licoup-private-gateway/Cargo.toml";
export const GATEWAY_MANIFEST = "crates/licoup-private-gateway/Cargo.toml";
// The Gateway package owns its two programs, so the modules that check them run
// against that crate's manifest rather than against the host's.
export const GATEWAY_ENTRY_MANIFEST = "crates/licoup-private-gateway/entry/Cargo.toml";
export const STATISTICS_MANIFEST = "crates/licoup-statistics/Cargo.toml";
// The Telegram channel package's own program builds from the public gateway's
// entry crate, mirroring the private gateway's entry crate.
export const PUBLIC_GATEWAY_ENTRY_MANIFEST = "crates/licoup-public-gateway/entry/Cargo.toml";
export const CLIENT_STATE_MANIFEST = "crates/licoup-client-state/Cargo.toml";
export const AGENT_TARGETS_MANIFEST = "crates/licoup-agent-targets/Cargo.toml";
export const MODEL_CATALOG_MANIFEST = "crates/licoup-model-catalog/Cargo.toml";

export function rustCrateLayer(crate, filter, harnessArgs = []) {
  return command("cargo", [
    "test", "--manifest-path", `crates/${crate}/Cargo.toml`, "--lib", filter,
    ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
  ], 10 * 60_000);
}

// An Agent adapter package is its own crate and program, so the modules that
// own its protocol and its document run against its manifest rather than the
// host's. The path follows the package's own directory name, so no per-package
// constant is restated here when another Agent's package lands.
export function agentPackageManifest(name) {
  return `crates/licoup-agent-${name}/Cargo.toml`;
}

export const FLUTTER_COMPOSITION_INPUTS = Object.freeze([
  "apps/desktop/analysis_options.yaml",
  "apps/desktop/test/flutter_test_config.dart",
  "apps/desktop/pubspec.lock",
  "apps/desktop/pubspec.yaml",
]);

export const RUST_COMPOSITION_INPUTS = Object.freeze([
  "Cargo.lock",
  "Cargo.toml",
  FOUNDATION_MANIFEST,
  CLIENT_STATE_MANIFEST,
  "crates/licoup-client-state/src/lib.rs",
  AGENT_TARGETS_MANIFEST,
  "crates/licoup-agent-targets/src/lib.rs",
  MODEL_CATALOG_MANIFEST,
  "crates/licoup-model-catalog/src/lib.rs",
  "crates/licoup-model-catalog/src/port.rs",
  "crates/licoup-agent-targets/src/domain/mod.rs",
  "crates/licoup-agent-targets/src/platform/mod.rs",
  "crates/licoup-agent-targets/src/port.rs",
  "crates/licoup-foundation/src/lib.rs",
  "crates/licoup-foundation/src/core/mod.rs",
  "crates/licoup-foundation/src/core/secret_bytes.rs",
  "crates/licoup-foundation/src/platform/mod.rs",
  GATEWAY_CORE_MANIFEST,
  "crates/licoup-private-gateway/src/lib.rs",
  "crates/licoup-private-gateway/src/ports/mod.rs",
  GATEWAY_ENTRY_MANIFEST,
  GATEWAY_MANIFEST,
  "crates/licoup-private-gateway/src/lib.rs",
  "crates/licoup-public-gateway/src/lib.rs",
  "crates/licoup-public-gateway/src/lane.rs",
  PUBLIC_GATEWAY_ENTRY_MANIFEST,
  "crates/licoup-vault/src/lib.rs",
  "crates/licoup-vault/Cargo.toml",
  STATISTICS_MANIFEST,
  "crates/licoup-statistics/src/lib.rs",
  "crates/licoup-statistics/src/host.rs",
  NATIVE_MANIFEST,
  "crates/licoup-native/src/core/mod.rs",
  "crates/licoup-native/src/domain/mod.rs",
  "crates/licoup-native/src/domain/integration_state.rs",
  "crates/licoup-native/src/ffi/commands/mod.rs",
  "crates/licoup-native/src/ffi/mod.rs",
  "crates/licoup-native/src/lib.rs",
  "crates/licoup-native/src/model_catalog_port.rs",
  "crates/licoup-native/src/target_port.rs",
  "crates/licoup-native/src/platform/mod.rs",
]);

export const ANDROID_SECURE_MESH_LEAF_INPUTS = Object.freeze([
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/MainActivity.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidBridgeContract.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidNativeRuntime.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidCommandRouter.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidJsonCodec.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidRuntimeStatusStore.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidSecretStore.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidSecretContract.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidCustodyManager.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidEncryptedRecordStore.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidMobileRelaySecretBridge.kt",
  "apps/desktop/android/app/src/debug/kotlin/land/lico/licoup/DebugMainActivity.kt",
  "apps/desktop/android/app/src/debug/kotlin/land/lico/licoup/ReleaseAcceptanceChannel.kt",
  "apps/desktop/android/app/src/debug/kotlin/land/lico/licoup/ReleaseAcceptanceDebugCodec.kt",
  "apps/desktop/android/app/src/debug/kotlin/land/lico/licoup/ReleaseAcceptanceDebugContract.kt",
  "apps/desktop/android/app/src/debug/kotlin/land/lico/licoup/ReleaseAcceptanceIngress.kt",
  "apps/desktop/android/app/src/debug/kotlin/land/lico/licoup/SecureMeshAndroidReleaseAcceptanceCoordinator.kt",
  "apps/desktop/android/app/src/main/AndroidManifest.xml",
  "apps/desktop/android/app/src/debug/AndroidManifest.xml",
  "apps/desktop/android/app/src/main/res/xml/backup_rules.xml",
  "apps/desktop/android/app/src/main/res/xml/backup_rules_legacy.xml",
]);

export const FAKE_AGENT_SERVICE_INPUTS = Object.freeze([
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_service.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_state_support.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_conversation_support.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_conversation_fixture.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_archive_support.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_archive_job_fixture.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_usage_support.dart",
  "apps/desktop/test/fixtures/client_controller/support/fake_agent_runtime_support.dart",
]);

export function command(program, args, timeoutMs) {
  const singleRustTarget = args.filter((arg) =>
    ["--lib", "--bin", "--test", "--bench", "--doc"].includes(arg)).length === 1;
  const commandArgs = program === "cargo" && args[0] === "test" &&
    !singleRustTarget && !args.includes("--no-fail-fast")
    ? ["test", "--no-fail-fast", ...args.slice(1)]
    : args;
  return Object.freeze({
    program,
    args: Object.freeze([...commandArgs]),
    cwd: REPO_ROOT,
    timeoutMs,
  });
}

export function node(script, args = [], timeoutMs = 120_000) {
  return command("node", [script, ...args], timeoutMs);
}

export function flutterTests(testPaths) {
  return node(
    "tools/scripts/client-toolchain-runner.mjs",
    [
      "--check",
      "flutter",
      "--cwd",
      "apps/desktop",
      "--",
      "flutter",
      "test",
      "--no-pub",
      ...testPaths,
    ],
    5 * 60_000,
  );
}

export function flutterPackageTests(packageRoot, testPaths = ["test"]) {
  return node(
    "tools/scripts/client-toolchain-runner.mjs",
    [
      "--check",
      "flutter",
      "--cwd",
      packageRoot,
      "--",
      "flutter",
      "test",
      ...testPaths,
    ],
    5 * 60_000,
  );
}

export function flutterTestsMatching(testPaths, namePattern) {
  return node(
    "tools/scripts/client-toolchain-runner.mjs",
    [
      "--check",
      "flutter",
      "--cwd",
      "apps/desktop",
      "--",
      "flutter",
      "test",
      "--no-pub",
      ...testPaths,
      "--name",
      namePattern,
    ],
    5 * 60_000,
  );
}

export function flutterAnalyze() {
  return node(
    "tools/scripts/client-toolchain-runner.mjs",
    [
      "--check",
      "flutter",
      "--cwd",
      "apps/desktop",
      "--",
      "flutter",
      "analyze",
      "--no-pub",
    ],
    5 * 60_000,
  );
}

export function androidGradle(args, timeoutMs = 5 * 60_000) {
  return node(
    "tools/scripts/client-toolchain-runner.mjs",
    [
      "--cwd",
      "apps/desktop/android",
      "--",
      "./gradlew",
      ...args,
      ...(args.includes("--offline") ? [] : ["--offline"]),
    ],
    timeoutMs,
  );
}

export function rustLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      NATIVE_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function rustAdapterSdkLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      ADAPTER_SDK_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

/// One module of an Agent adapter package's library. The package is its own
/// crate and program, so its leaves run against its own manifest rather than
/// against the host that composes it. The manifest is named per call because
/// there is more than one package: a second Agent's leaves must not silently run
/// against the first Agent's crate. The default stays the first Agent's package
/// so the existing call sites keep their meaning.
export function rustAgentPackageLayer(
  filter,
  harnessArgs = [],
  manifest = agentPackageManifest("codex"),
) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      manifest,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

/// One Agent adapter package's whole crate. The package is its own crate and
/// program, so its tree is one test target that runs against its own manifest.
/// The module id and the manifest both follow the package's own directory name,
/// so adding an Agent's package is one call rather than a copied block, and two
/// packages cannot silently run against one crate.
export function agentPackageCrateModule(name, summary) {
  return defineModule({
    id: `rust.core.agent-${name}-package`,
    kind: "rust-core",
    summary,
    inputs: [`crates/licoup-agent-${name}/**`],
    command: command(
      "cargo",
      ["test", "--manifest-path", agentPackageManifest(name)],
      10 * 60_000,
    ),
  });
}

/// One module of the Codex adapter package's library.
export function codexAgentPackageLayer(filter, harnessArgs = []) {
  return rustAgentPackageLayer(filter, harnessArgs, agentPackageManifest("codex"));
}

/// One module of the Claude Code adapter package's library.
export function claudeCodeAgentPackageLayer(filter, harnessArgs = []) {
  return rustAgentPackageLayer(filter, harnessArgs, agentPackageManifest("claude-code"));
}

export function gatewayCoreLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      GATEWAY_CORE_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function gatewayLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      GATEWAY_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function gatewayIntegrationTest(target, filter) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      GATEWAY_MANIFEST,
      "--test",
      target,
      ...(filter ? [filter] : []),
    ],
    10 * 60_000,
  );
}

export function statisticsLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      STATISTICS_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function foundationLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      FOUNDATION_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function clientStateLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      CLIENT_STATE_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function agentTargetsLayer(filter, harnessArgs = []) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      AGENT_TARGETS_MANIFEST,
      "--lib",
      filter,
      ...(harnessArgs.length > 0 ? ["--", ...harnessArgs] : []),
    ],
    10 * 60_000,
  );
}

export function rustCrateIntegrationTest(crate, target, features = []) {
  return command(
    "cargo",
    [
      "test",
      "-p",
      crate,
      ...(features.length > 0 ? ["--features", features.join(",")] : []),
      "--test",
      target,
    ],
    10 * 60_000,
  );
}

export function rustIntegrationTest(target, filter) {
  return command(
    "cargo",
    [
      "test",
      "--manifest-path",
      NATIVE_MANIFEST,
      "--test",
      target,
      ...(filter ? [filter] : []),
    ],
    10 * 60_000,
  );
}

export function rustBinaryTests(binary, filter, features = []) {
  return command(
    "cargo",
    [
      "test",
      "-p",
      "licoup-native",
      ...(features.length > 0 ? ["--features", features.join(",")] : []),
      "--bin",
      binary,
      filter,
    ],
    10 * 60_000,
  );
}

export function defineModule({
  id,
  kind,
  summary,
  inputs,
  command: moduleCommand,
  runnableHosts = CLIENT_MODULE_RUNNABLE_HOSTS,
  targetEvidenceHosts = [],
}) {
  const regression = classifyClientModule({ id, kind, command: moduleCommand });
  return Object.freeze({
    id,
    kind,
    summary,
    inputs: Object.freeze([...new Set(inputs)]),
    command: moduleCommand,
    regression: Object.freeze({
      ...regression,
      runnableHosts: Object.freeze([...runnableHosts]),
      targetEvidenceHosts: Object.freeze([...targetEvidenceHosts]),
    }),
  });
}

export function secureMeshModule({
  id,
  summary,
  source,
  resources = [],
  testInputs = [],
}) {
  return defineModule({
    id,
    kind: "rust-core",
    summary,
    inputs: [
      `crates/licoup-native/src/core/${source}.rs`,
      ...testInputs,
      ...resources,
    ],
    command: rustLayer(`core::${source}::tests`),
  });
}

export function assembleClientModuleCatalog(idOrder, moduleGroups) {
  if (!Array.isArray(idOrder) || !Array.isArray(moduleGroups)) {
    throw new Error("client module catalog assembly inputs are invalid");
  }
  const expectedIds = new Set();
  for (const id of idOrder) {
    if (expectedIds.has(id)) {
      throw new Error(`duplicate client module order id: ${id}`);
    }
    expectedIds.add(id);
  }

  const moduleById = new Map();
  for (const group of moduleGroups) {
    if (!Array.isArray(group)) {
      throw new Error("client module catalog group must be an array");
    }
    for (const module of group) {
      if (moduleById.has(module.id)) {
        throw new Error(`duplicate client module definition: ${module.id}`);
      }
      moduleById.set(module.id, module);
    }
  }

  const missing = idOrder.filter((id) => !moduleById.has(id));
  if (missing.length > 0) {
    throw new Error(`missing client module definitions: ${missing.join(", ")}`);
  }
  const unexpected = [...moduleById.keys()].filter((id) => !expectedIds.has(id));
  if (unexpected.length > 0) {
    throw new Error(`unexpected client module definitions: ${unexpected.join(", ")}`);
  }
  return Object.freeze(idOrder.map((id) => moduleById.get(id)));
}
