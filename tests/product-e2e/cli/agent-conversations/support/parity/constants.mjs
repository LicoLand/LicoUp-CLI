import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const workspaceRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../../..");

export const packagingRegistryPath = join(workspaceRoot, "apps", "desktop", "packaging.modules.json");

export const driversInventoryPath = join(
  workspaceRoot,
  "crates",
  "licoup-native",
  "resources",
  "agent-conversation-drivers.json",
);

export const evidenceManifestPath = join(
  workspaceRoot,
  "crates",
  "licoup-native",
  "resources",
  "agent-conversation-evidence.json",
);

export const packagedReleaseAppPath = join(
  workspaceRoot,
  "build",
  "apps",
  "desktop",
  "runnable",
  "macos",
  "release",
  "LicoUp.app",
);

export const sidecarArgs = ["agent", "conversation", "send", "--stdin-json", "true"];

export const dispatchLaneHarnessVersion = "dispatch-lane-unified-1";

export const coreProbeIds = Object.freeze([
  "P-01",
  "P-02",
  "P-03",
  "P-04",
  "P-05",
  "P-06",
  "P-07",
  "P-08",
  "P-09",
  "P-10",
]);

export const defaultTimeoutMs = 180_000;

export const defaultMaxOutputBytes = 4 * 1024 * 1024;

export const acceptanceMode = "dispatch-lane-unified-1";

// One successful live workflow is sufficient. The workflow itself performs
// both required user behaviors: create one conversation, then resume that
// exact native conversation for one continuation turn.
export const strictRoundCount = 1;

export const verificationTurnCount = 2;

export const disposableProfileSeedEntries = Object.freeze([
  "config.toml",
  "credentials",
  "oauth",
  "device_id",
]);

export const disposableProfileSeedMaxFiles = 128;

export const disposableProfileSeedMaxBytes = 4 * 1024 * 1024;

export const disposableProfileSeedMaxDepth = 8;

export const agentConfigs = Object.freeze({
  opencode: Object.freeze({
    id: "opencode",
    driverId: "opencode-serve",
    executable: "opencode",
    binaryEnvironment: ["OPENCODE_PATH", "OPENCODE_BIN"],
    acpArgs: ["serve"],
    runtimeProtocol: "opencode-serve-http-v1",
    laneFamily: "serve-http",
    cleanupKind: "openagent-cli",
    listArgs: ["session", "list", "--format", "json"],
    deleteArgs: (sessionId) => ["session", "delete", sessionId],
    exportArgs: (sessionId) => ["export", sessionId, "--sanitize"],
  }),
  copilot: Object.freeze({
    id: "copilot",
    driverId: "copilot-acp",
    executable: "copilot",
    binaryEnvironment: ["COPILOT_PATH", "COPILOT_BIN"],
    acpArgs: ["--acp", "--stdio", "--no-auto-update"],
    runtimeProtocol: "copilot-acp-v1-stdio-ndjson",
    laneFamily: "acp",
    cleanupKind: "copilot-sdk",
  }),
  "kilo-code": Object.freeze({
    id: "kilo-code",
    driverId: "kilo-code-serve",
    executable: "kilo",
    binaryEnvironment: ["KILO_PATH", "KILO_BIN", "KILOCODE_PATH"],
    acpArgs: ["serve"],
    runtimeProtocol: "kilo-code-serve-http-v1",
    laneFamily: "serve-http",
    cleanupKind: "openagent-cli",
    listArgs: ["session", "list", "--format", "json", "--all"],
    deleteArgs: (sessionId) => ["session", "delete", sessionId],
    exportArgs: (sessionId) => ["export", sessionId, "--sanitize"],
  }),
  cursor: Object.freeze({
    id: "cursor",
    driverId: "cursor-cli",
    executable: "cursor-agent",
    binaryEnvironment: ["CURSOR_AGENT_PATH", "CURSOR_PATH", "CURSOR_BIN"],
    acpArgs: [
      "--print",
      "--output-format",
      "stream-json",
      "--trust",
      "--force",
    ],
    runtimeProtocol: "cursor-agent-cli-v1",
    laneFamily: "cli",
    cleanupKind: "cursor-cli-chat-leaf",
    promptInArguments: true,
    continuityInArguments: true,
    // Native argv CLI turn (create-chat / --resume); not sidecar.
    cliTurnKind: "native-cli",
    cliReadbackKind: "native-cli",
    sameSessionGate: true,
  }),
  openclaw: Object.freeze({
    id: "openclaw",
    driverId: "openclaw-acp",
    executable: "openclaw",
    binaryEnvironment: ["OPENCLAW_PATH", "OPENCLAW_BIN"],
    acpArgs: ["acp"],
    runtimeProtocol: "openclaw-acp-stdio-jsonrpc",
    laneFamily: "acp",
    cleanupKind: "openclaw-acp",
  }),
  hermes: Object.freeze({
    id: "hermes",
    driverId: "hermes-acp",
    executable: "hermes",
    binaryEnvironment: ["HERMES_PATH", "HERMES_BIN"],
    acpArgs: ["acp"],
    runtimeProtocol: "hermes-acp-stdio-jsonrpc",
    laneFamily: "acp",
    cleanupKind: "hermes-cli",
    listArgs: ["sessions", "list", "--limit", "10000"],
    deleteArgs: (sessionId) => ["sessions", "delete", sessionId, "--yes"],
  }),
  "kimi-code": Object.freeze({
    id: "kimi-code",
    driverId: "kimi-code-acp",
    executable: "kimi",
    binaryEnvironment: ["KIMI_PATH", "KIMI_BIN", "KIMI_CODE_PATH"],
    acpArgs: ["acp"],
    runtimeProtocol: "kimi-code-acp-v1-stdio-ndjson",
    laneFamily: "acp",
    cleanupKind: "disposable-data-root",
    disposableEnvironmentKey: "KIMI_CODE_HOME",
    sameSessionGate: true,
  }),
  pi: Object.freeze({
    id: "pi",
    driverId: "pi-rpc",
    executable: "pi",
    binaryEnvironment: ["PI_PATH", "PI_BIN", "PI_CODING_AGENT_PATH"],
    acpArgs: ["--mode", "rpc", "--offline"],
    runtimeProtocol: "pi-rpc-stdio-jsonl",
    laneFamily: "rpc",
    cleanupKind: "pi-disposable-session-root",
    disposableEnvironmentKey: "PI_CODING_AGENT_SESSION_DIR",
  }),
  codex: Object.freeze({
    id: "codex",
    driverId: "codex-app-server",
    executable: "codex",
    binaryEnvironment: ["CODEX_PATH", "CODEX_BIN"],
    acpArgs: ["app-server", "--stdio"],
    runtimeProtocol: "codex-app-server-stdio-jsonrpc",
    laneFamily: "app-server",
    cleanupKind: "codex-app-server",
  }),
  "claude-code": Object.freeze({
    id: "claude-code",
    driverId: "claude-code-stream-json",
    executable: "claude",
    binaryEnvironment: ["CLAUDE_PATH", "CLAUDE_BIN"],
    acpArgs: [
      "--print", "--input-format", "stream-json", "--output-format", "stream-json",
      "--verbose", "--include-partial-messages", "--no-session-persistence",
    ],
    runtimeProtocol: "claude-code-cli-stream-json",
    laneFamily: "stream-json",
    cleanupKind: "process-local-rpc",
    continuityScope: "process-local",
    isolatedConfigEnvironmentKey: "CLAUDE_CONFIG_DIR",
    noHistoryEnvironmentKey: "CLAUDE_CODE_SKIP_PROMPT_HISTORY",
    sameSessionGate: true,
  }),
  antigravity: Object.freeze({
    id: "antigravity",
    driverId: "antigravity-cli",
    executable: "agy",
    binaryEnvironment: ["ANTIGRAVITY_PATH", "AGY_PATH"],
    acpArgs: ["--print", "--dangerously-skip-permissions"],
    runtimeProtocol: "antigravity-cli-argv-hook-v1",
    laneFamily: "cli",
    cleanupKind: "antigravity-cli-brain-leaf",
    promptInArguments: true,
    continuityInArguments: true,
    // Hook-bridge session receipt lives inside the native driver; harness
    // exercises the adapter through the sidecar send path only.
    cliTurnKind: "sidecar",
    cliReadbackKind: "none",
    sameSessionGate: true,
    turnViaSidecar: true,
  }),
  "lico-agent": Object.freeze({
    id: "lico-agent",
    driverId: "lico-agent-rpc",
    executable: "lico-agent",
    binaryEnvironment: ["LICO_AGENT_PATH", "LICO_AGENT_BIN"],
    acpArgs: ["--mode", "rpc"],
    runtimeProtocol: "lico-agent-rpc-stdio-jsonl",
    laneFamily: "rpc",
    cleanupKind: "lico-agent-rpc",
  }),
});

/** Agents whose acceptance path is the same-session sequential gate. */
export const sameSessionGateAgentIds = Object.freeze(
  Object.values(agentConfigs)
    .filter((config) => config.sameSessionGate === true)
    .map((config) => config.id),
);

/** Agents whose privacy contract allows prompt/continuity id in argv. */
export const argvPrivacyLaneAgentIds = Object.freeze(
  Object.values(agentConfigs)
    .filter(
      (config) =>
        config.promptInArguments === true && config.continuityInArguments === true,
    )
    .map((config) => config.id),
);
