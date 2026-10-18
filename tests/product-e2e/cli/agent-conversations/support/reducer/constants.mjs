import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const CONTRACT_VERSION = "CL-06";
export const EVIDENCE_SCHEMA_VERSION =
  "v0.0.1:client-agent-conversation-parity-evidence-1";
export const READINESS_SCHEMA_VERSION =
  "v0.0.1:client-agent-conversation-readiness-1";
export const INVENTORY_SCHEMA_VERSION =
  "v0.0.1:client-agent-conversation-drivers-1";
export const MINIMUM_CONSECUTIVE_PASSES = 1;

export const CORE_CHECK_IDS = Object.freeze([
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

export const CONDITIONAL_CHECK_IDS = Object.freeze([
  "C-01",
  "C-02",
  "C-03",
  "C-04",
  "C-05",
  "C-06",
]);

const SCRIPT_DIRECTORY = dirname(fileURLToPath(import.meta.url));
export const REPOSITORY_ROOT = resolve(SCRIPT_DIRECTORY, "../../../../../..");
export const PACKAGING_REGISTRY_FILE = resolve(
  REPOSITORY_ROOT,
  "apps/desktop/packaging.modules.json",
);
export const DRIVER_INVENTORY_FILE = resolve(
  REPOSITORY_ROOT,
  "crates/licoup-native/resources/agent-conversation-drivers.json",
);
export const READINESS_FILE = resolve(
  REPOSITORY_ROOT,
  "crates/licoup-native/resources/agent-conversation-readiness.json",
);
export const CANONICAL_EVIDENCE_FILE = resolve(
  REPOSITORY_ROOT,
  "crates/licoup-native/resources/agent-conversation-evidence.json",
);
export const ADAPTER_MANIFEST_DIRECTORY = resolve(
  REPOSITORY_ROOT,
  "packages/contracts/client/fixtures/agent-conversation-adapter/manifests",
);

export const SAFE_CODE = /^[a-z0-9][a-z0-9._:+-]{0,127}$/;
export const SHA256_DIGEST = /^sha256:[a-f0-9]{64}$/;
export const CORE_RESULTS = new Set(["pass", "fail", "unverified"]);
export const NATIVE_SUPPORT_RESULTS = new Set(["supported", "unsupported", "unknown"]);
export const CONDITIONAL_RESULTS = new Set([
  "pass",
  "fail",
  "gap",
  "unverified",
  "unsupported-by-native",
]);
export const DRIVER_MODES = new Set(["conversation", "blocked", "history-only"]);
export const EVIDENCE_BLOCKING_CODES = new Set([
  "authorized_test_environment_missing",
  "canonical_driver_missing",
  "official_native_lane_missing",
  "safe_cleanup_unavailable",
  "exact_session_resume_unavailable",
]);
export const INVENTORY_BLOCKING_CODES = new Set([
  ...EVIDENCE_BLOCKING_CODES,
  "antigravity_cli_structured_transport_unavailable",
]);

export const SENSITIVE_KEY_FRAGMENTS = Object.freeze([
  "prompt",
  "response",
  "path",
  "session",
  "thread",
  "argv",
  "account",
  "credential",
  "stderr",
  "stdout",
  "message",
  "content",
  "payload",
  "attachment",
  "secret",
  "token",
  "cookie",
  "password",
  "passwd",
  "privatekey",
  "authorization",
  "username",
  "hostname",
  "rawlog",
  "logtext",
  "conversationid",
  "turnid",
  "workingdirectory",
  "cwd",
]);

export const EVIDENCE_TOP_LEVEL_FIELDS = new Set([
  "schemaVersion",
  "contractVersion",
  "harnessVersion",
  "toolVersionClass",
  "generatedAt",
  "adapters",
]);
export const ADAPTER_EVIDENCE_FIELDS = new Set([
  "agentId",
  "driverId",
  "runtimeProtocol",
  "harnessVersion",
  "runtimeVersionClass",
  "runtimeVersionDigest",
  "capabilitySnapshotDigest",
  "adapterManifestDigest",
  "releaseArtifactDigest",
  "releaseSidecarDigest",
  "productContinuityBindingDigest",
  "runtimeSourceClass",
  "registryDigest",
  "driverInventoryDigest",
  "evidenceDigest",
  "officialNativeLane",
  "consecutivePasses",
  "conversationGatePassed",
  "cleanupPassed",
  "privacyPassed",
  "coreChecks",
  "conditionalChecks",
  "blockingCode",
]);
export const CONDITIONAL_EVIDENCE_FIELDS = new Set(["nativeSupport", "result"]);
export const INVENTORY_TOP_LEVEL_FIELDS = new Set([
  "schemaVersion",
  "contractVersion",
  "evidenceContract",
  "drivers",
]);
export const INVENTORY_CONTRACT_FIELDS = new Set([
  "minimumConsecutivePasses",
  "coreChecks",
  "conditionalChecks",
  "requiredBooleans",
  "requiredCounts",
  "requiredDigests",
  "requiredBindings",
]);
export const INVENTORY_DRIVER_FIELDS = new Set([
  "agentId",
  "driverId",
  "runtimeProtocol",
  "officialNativeLaneKind",
  "historyReadable",
  "driverMode",
  "blockerCodes",
  "capabilityMatrix",
  "lifecycleEvidence",
]);

export const LIFECYCLE_EVIDENCE_FIELDS = new Set([
  "accepted",
  "processing",
  "responding",
  "completed",
]);

export const CAPABILITY_MATRIX_FIELDS = new Set([
  "laneFamily",
  "openNew",
  "exactResume",
  "streaming",
  "cancel",
  "interruptSteer",
  "structuredEvents",
  "approvals",
  "multimodal",
  "usageStatus",
  "officialLane",
  "processLocalContinuation",
  "hostSurvivesGuiDisconnect",
  "activeTurnReattach",
  "orderedCursorReplay",
  "permissionModes",
]);

export const LANE_FAMILIES = new Set([
  "acp",
  "app-server",
  "stream-json",
  "serve-http",
  "cli",
  "rpc",
  "unavailable",
]);
