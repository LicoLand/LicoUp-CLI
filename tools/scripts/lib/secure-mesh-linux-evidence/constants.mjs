export const linuxVmPackageReceiptSchema = "licomesh.secure-mesh.linux-vm-package-receipt";
export const linuxNodeMatrixSchema = "licomesh.secure-mesh.linux-node-matrix";
export const linuxVmPackageReceiptSchemaVersion = 2;
export const linuxEvidenceSchemaVersion = 1;

export const VM_KEYS = Object.freeze([
  "schema",
  "schemaVersion",
  "ok",
  "producer",
  "generatedAt",
  "closureChallengeDigest",
  "invocationNonceDigest",
  "productVersion",
  "buildNumber",
  "artifactKind",
  "target",
  "redacted",
  "reportLeakScan",
  "rawPrivateMaterialIncluded",
  "rawPlaintextIncluded",
  "rawPublicWireBytesIncluded",
  "sourceBinding",
  "package",
  "session",
  "smoke",
  "capabilityReport",
  "privacy",
  "nonBlockingDistributionGuidance",
  "summary"
]);

export const NODE_KEYS = Object.freeze([
  "schema",
  "schemaVersion",
  "ok",
  "producer",
  "artifactKind",
  "target",
  "redacted",
  "reportLeakScan",
  "rawPrivateMaterialIncluded",
  "rawPlaintextIncluded",
  "rawPublicWireBytesIncluded",
  "sourceBinding",
  "runtime",
  "isolation",
  "pairwise",
  "restart",
  "teardown",
  "capabilityReport",
  "privacy",
  "summary"
]);

export const FORBIDDEN_KEYS = new Set([
  "host",
  "hostname",
  "username",
  "userName",
  "containerId",
  "runtimeId",
  "processId",
  "pid",
  "port",
  "dbusAddress",
  "objectPath",
  "itemPath",
  "localPath",
  "absolutePath",
  "stateRoot",
  "installRoot",
  "archivePath",
  "bundlePath",
  "rawLog",
  "stdout",
  "stderr",
  "plaintextSample",
  "ciphertextSample",
  "secretValue",
  "tokenValue"
]);

export const FORBIDDEN_VALUES = Object.freeze([
  /(?:^|["'\s])unix:(?:path|abstract)=/iu,
  /\/org\/freedesktop\/(?:DBus|secrets)(?:\/|$)/u,
  /\/(?:Users|home|private|tmp|run|var\/folders)\//u,
  /[A-Za-z]:\\/u,
  /-----BEGIN|-----END/u,
  /Bearer\s+(?!\[redacted\])\S+/u,
  /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u
]);

export const LINUX_EVIDENCE_FAILURE_CATEGORIES = new Set([
  "artifact",
  "binding",
  "capability",
  "privacy",
  "producer",
  "readiness",
  "schema",
  "session",
]);

export const LINUX_EVIDENCE_RULE_ID = /^[a-z][a-z0-9_]{2,95}$/u;

export const LINUX_VM_FAILURE_PHASES = new Set([
  "input_validation",
  "archive_binding",
  "archive_install",
  "cli_smoke",
  "gui_session",
  "gui_display",
  "gui_process",
  "gui_window",
  "gui_interaction",
  "gui_shutdown",
  "gui_stderr",
  "receipt_validation",
  "receipt_write",
]);
