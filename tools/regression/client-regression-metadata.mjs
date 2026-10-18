import os from "node:os";

export const CLIENT_REGRESSION_STAGES = Object.freeze([
  "foundation",
  "frontend",
  "backend",
  "integration",
  "scenarios",
  "compatibility",
]);

export const CLIENT_REGRESSION_STAGE_DEPENDENCIES = Object.freeze({
  foundation: Object.freeze([]),
  frontend: Object.freeze(["foundation"]),
  backend: Object.freeze(["foundation"]),
  integration: Object.freeze(["frontend", "backend"]),
  scenarios: Object.freeze(["integration"]),
  // Compatibility starts after every core stage is terminal, not only after
  // every core stage passed. Missing optional capabilities therefore cannot
  // hide independent evidence or fail the core regression.
  compatibility: Object.freeze([]),
});

const FOUNDATION_KINDS = new Set([
  "regression-infrastructure",
  "architecture",
  "flutter-composition",
  "rust-composition",
  "rust-crate",
]);
const FRONTEND_KINDS = new Set([
  "flutter-feature",
  "flutter-layer",
  "flutter-controller",
]);
const INTEGRATION_KINDS = new Set([
  "flutter-contract",
  "platform-bridge",
]);
const SCENARIO_KINDS = new Set(["packaging", "release"]);

export function regressionStageForKind(kind) {
  if (FOUNDATION_KINDS.has(kind)) return "foundation";
  if (FRONTEND_KINDS.has(kind)) return "frontend";
  if (INTEGRATION_KINDS.has(kind)) return "integration";
  if (SCENARIO_KINDS.has(kind)) return "scenarios";
  if (["rust-domain", "rust-core", "rust-platform", "rust-ffi"].includes(kind)) return "backend";
  throw new Error(`client regression kind has no stage: ${kind}`);
}

function innerToolchain(args) {
  if (args[0] !== "tools/scripts/client-toolchain-runner.mjs") return null;
  const separator = args.indexOf("--");
  if (separator < 0) return null;
  const executable = args[separator + 1] || "";
  if (["flutter", "dart"].includes(executable)) return "flutter";
  if (["./gradlew", "gradlew.bat"].includes(executable)) return "gradle";
  return null;
}

const BRIDGE_FORMAT_CONTRACT_FILES = Object.freeze([
  "tests/contract/client/client-bridge-generation.test.mjs",
  "tests/contract/client/continuous-assistant-contract.test.mjs",
]);
const RUST_SDK_CONTRACT_FILES = Object.freeze([
  ...BRIDGE_FORMAT_CONTRACT_FILES,
  "tests/contract/client/continuous-assistant-integration.test.mjs",
  "tests/contract/client/continuous-assistant-scenarios.test.mjs",
  "tests/contract/client/continuous-assistant-evaluation.test.mjs",
  "tests/contract/client/continuous-assistant-adoption.test.mjs",
]);
const FLUTTER_SDK_CONTRACT_FILES = Object.freeze([
  "tests/contract/client/continuous-assistant-ux.test.mjs",
]);

function normalizeContractFile(file) {
  return String(file).replaceAll("\\", "/");
}

export function nodeTestFileToolchain(file) {
  const normalized = normalizeContractFile(file);
  if (FLUTTER_SDK_CONTRACT_FILES.includes(normalized)) return "flutter";
  if (RUST_SDK_CONTRACT_FILES.includes(normalized)) return "rust";
  return "node-test";
}

function nodeTestFiles(args) {
  return args.slice(1).filter((value) => !String(value).startsWith("-") && String(value).endsWith(".mjs"));
}

export function regressionToolchain(command) {
  if (command.program === "cargo" ||
      (command.program === "node" && command.args[0] === "tools/scripts/migration-crate-tests.mjs")) {
    return "rust";
  }
  const nested = innerToolchain(command.args);
  if (nested) return nested;
  if (command.program === "node" && command.args[0] === "--test") {
    const files = nodeTestFiles(command.args);
    const toolchains = [...new Set(files.map(nodeTestFileToolchain))];
    if (toolchains.length === 1) return toolchains[0];
    if (toolchains.some((toolchain) => toolchain !== "node-test")) {
      throw new Error("mixed node-test and SDK-owned contract files in one command");
    }
    return "node-test";
  }
  if (command.program === "node") return "node";
  throw new Error("client regression command has no toolchain");
}

const TOOLCHAIN_WEIGHT = Object.freeze({
  rust: 3,
  flutter: 3,
  gradle: 4,
  "node-test": 2,
  node: 1,
});

const TOOLCHAIN_RESOURCES = Object.freeze({
  rust: Object.freeze(["cargo-target"]),
  flutter: Object.freeze(["flutter-cache"]),
  gradle: Object.freeze(["gradle-cache"]),
  "node-test": Object.freeze([]),
  node: Object.freeze([]),
});

function wrapperResources(command) {
  if (command.program === "node" &&
      command.args[0] === "tests/smoke/native-client-smoke.mjs") {
    return ["cargo-target"];
  }
  if (command.program === "node" &&
      command.args[0] === "tools/scripts/client-android-native-tests.mjs") {
    // This bounded wrapper owns a Gradle test run, two Cargo FFI filters, and
    // may consult Flutter Doctor while resolving Java. Charge every shared
    // toolchain resource instead of disguising it as a pure Node leaf.
    return ["cargo-target", "flutter-cache", "gradle-cache"];
  }
  if (command.program === "node" && command.args[0] === "--test") {
    const resources = [];
    for (const file of nodeTestFiles(command.args)) {
      const toolchain = nodeTestFileToolchain(file);
      if (toolchain === "rust") resources.push("cargo-target");
      if (toolchain === "flutter") resources.push("flutter-cache");
      if (BRIDGE_FORMAT_CONTRACT_FILES.includes(normalizeContractFile(file))) {
        resources.push("flutter-cache");
      }
    }
    return resources;
  }
  return [];
}

export function classifyClientModule({ id, kind, command }) {
  const stage = regressionStageForKind(kind);
  const toolchain = regressionToolchain(command);
  return Object.freeze({
    stage,
    lane: stage,
    environment: toolchain,
    toolchain,
    weight: TOOLCHAIN_WEIGHT[toolchain],
    resources: Object.freeze([
      ...new Set([...TOOLCHAIN_RESOURCES[toolchain], ...wrapperResources(command)]),
    ]),
    internalParallelism: ["rust", "flutter", "gradle", "node-test"].includes(toolchain),
    batchKey: `${toolchain}:${id}`,
  });
}

export function defaultRegressionCapacities(available = os.availableParallelism()) {
  const global = Math.max(4, available);
  return Object.freeze({
    global,
    pools: Object.freeze({
      rust: Math.max(4, Math.floor(global * 0.75)),
      flutter: Math.max(3, Math.floor(global * 0.5)),
      gradle: Math.max(4, Math.floor(global * 0.5)),
      "node-test": Math.max(2, Math.floor(global * 0.75)),
      node: Math.max(2, global - 1),
      compatibility: Math.max(2, Math.floor(global * 0.5)),
    }),
    resources: Object.freeze({
      // Cargo/libtest and Flutter already provide native internal parallelism.
      // Keep their shared target/cache single-owner so hybrid platform
      // wrappers cannot create a second hidden toolchain tree.
      "cargo-target": 1,
      "flutter-cache": 1,
      "gradle-cache": 1,
    }),
  });
}
