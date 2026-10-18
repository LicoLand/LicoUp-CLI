import { FORBIDDEN_KEYS, FORBIDDEN_VALUES } from "./constants.mjs";

export function assert(condition, message) {
  if (!condition) throw new Error(message);
}

export function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

export function assertExactKeys(value, expected, label) {
  assert(isPlainObject(value), `${label} must be an object`);
  const keys = Object.keys(value);
  assert(
    keys.length === expected.length && expected.every((key) => keys.includes(key)),
    `${label} fields are not exact`
  );
}

export function assertDigest(value, label) {
  assert(/^sha256:[a-f0-9]{64}$/u.test(String(value || "")), `${label} is invalid`);
}

export function scanPrivacy(value) {
  if (Array.isArray(value)) {
    for (const item of value) scanPrivacy(item);
    return;
  }
  if (isPlainObject(value)) {
    for (const [key, nested] of Object.entries(value)) {
      assert(!FORBIDDEN_KEYS.has(key), "Linux evidence contains a forbidden runtime field");
      scanPrivacy(nested);
    }
    return;
  }
  if (typeof value === "string") {
    assert(
      FORBIDDEN_VALUES.every((pattern) => !pattern.test(value)),
      "Linux evidence contains machine-local, runtime, or credential data"
    );
  }
}

export function validatePrivacyRecord(privacy) {
  assertExactKeys(privacy, [
    "redacted",
    "runtimeIdentityIncluded",
    "localPathIncluded",
    "dbusOrObjectDataIncluded",
    "rawLogsIncluded",
    "rawPlaintextIncluded",
    "rawCiphertextIncluded",
    "rawSecretsIncluded"
  ], "Linux evidence privacy record");
  assert(privacy.redacted === true, "Linux evidence is not marked redacted");
  for (const key of Object.keys(privacy).filter((key) => key !== "redacted")) {
    assert(privacy[key] === false, `Linux evidence privacy field ${key} must be false`);
  }
}

export function validateRootRedaction(report) {
  assert(report.redacted === true && report.reportLeakScan === true,
    "Linux evidence root redaction declaration is incomplete");
  for (const key of [
    "rawPrivateMaterialIncluded",
    "rawPlaintextIncluded",
    "rawPublicWireBytesIncluded"
  ]) {
    assert(report[key] === false, `Linux evidence root field ${key} must be false`);
  }
}

export function validateSourceBinding(binding, expectedSourceDigest) {
  assertExactKeys(binding, [
    "sourceStateDigest",
    "sourceStateDigestProvenance",
    "archiveDigest",
    "bundleManifestDigest",
    "nativeClientDigest",
    "stale"
  ], "Linux evidence source binding");
  assertDigest(binding.sourceStateDigest, "Linux evidence source-state digest");
  assertDigest(binding.archiveDigest, "Linux evidence archive digest");
  assertDigest(binding.bundleManifestDigest, "Linux evidence bundle-manifest digest");
  assertDigest(binding.nativeClientDigest, "Linux evidence native-client digest");
  assert(
    ["git-worktree", "vm-orchestrator-verified"].includes(binding.sourceStateDigestProvenance),
    "Linux evidence source-state provenance is invalid"
  );
  assert(binding.stale === false, "Linux evidence is stale");
  if (expectedSourceDigest) {
    assertDigest(expectedSourceDigest, "Expected Linux source-state digest");
    assert(binding.sourceStateDigest === expectedSourceDigest, "Linux evidence source binding is stale");
  }
}

export function linuxEvidencePrivacyRecord() {
  return Object.freeze({
    redacted: true,
    runtimeIdentityIncluded: false,
    localPathIncluded: false,
    dbusOrObjectDataIncluded: false,
    rawLogsIncluded: false,
    rawPlaintextIncluded: false,
    rawCiphertextIncluded: false,
    rawSecretsIncluded: false
  });
}
