import {
  loadCapabilityCatalog,
  reduceCapabilityFacts,
  validateCapabilityReport
} from "./secure-mesh-capability-report.mjs";

const FACT_KEYS = [
  "capability",
  "state",
  "evidenceKind",
  "measuredAtUnixSeconds",
  "reasonCode"
];
const PROBE_KEYS = ["schemaVersion", "facts"];
const MEASUREMENT_KEYS = [
  "schemaVersion",
  "keyStoreAvailable",
  "custodyStrategy",
  "restartSemantics",
  "keyPresent",
  "keyMaterialNonExportable",
  "securityLevelMeasured",
  "securityLevel",
  "insideSecureHardware",
  "userAuthenticationRequested",
  "userAuthenticationRequired",
  "userAuthenticationTypeMeasured",
  "userAuthenticationType",
  "deviceCredentialAvailable",
  "deviceCredentialAllowed",
  "strongBiometricAvailable",
  "strongBiometricAvailabilityMeasured",
  "strongBiometricAllowed",
  "userAuthenticationValiditySeconds",
  "userAuthenticationHardwareEnforced",
  "invalidatedByBiometricEnrollment",
  "biometricEnrollmentInvalidationNotApplicableBecauseDeviceCredentialAllowed",
  "unlockedDeviceRequiredRequested",
  "unlockedDeviceRequired",
  "strongBoxRequested",
  "keyGenerationAttemptCount",
  "bodyRedacted"
];
const FACT_STATES = new Set([
  "supported",
  "unsupported",
  "temporarily_unavailable",
  "unverified"
]);
const EVIDENCE_KINDS = new Set([
  "source_contract",
  "runtime_operation",
  "generated_key_inspection",
  "os_authorization",
  "test_fixture",
  "not_measured"
]);
const SECURITY_LEVELS = new Set([
  "software",
  "unknown_secure",
  "trusted_environment",
  "strongbox",
  "unverified"
]);
const AUTHENTICATION_TYPES = new Set([
  "none",
  "unverified",
  "device_credential",
  "strong_biometric",
  "device_credential_or_strong_biometric"
]);
const FORBIDDEN_REPORT_KEYS = new Set([
  "serial",
  "manufacturer",
  "model",
  "deviceId",
  "androidId",
  "keyAlias",
  "attestation",
  "attestationChain",
  "privatePath",
  "secretValue",
  "credentialValue",
  "ciphertext"
]);

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

function requireExactKeys(value, expected, label) {
  requireValue(isPlainObject(value), `${label} must be an object`);
  const keys = Object.keys(value).sort();
  const canonical = [...expected].sort();
  requireValue(keys.length === canonical.length &&
    keys.every((key, index) => key === canonical[index]), `${label} fields are not exact`);
}

function scanForbiddenKeys(value) {
  if (Array.isArray(value)) {
    value.forEach(scanForbiddenKeys);
    return;
  }
  if (!isPlainObject(value)) return;
  for (const [key, nested] of Object.entries(value)) {
    requireValue(!FORBIDDEN_REPORT_KEYS.has(key),
      "Android capability evidence contains a forbidden disclosure field");
    scanForbiddenKeys(nested);
  }
}

function androidFactIds(catalog) {
  return catalog.order.filter((id) => {
    const definition = catalog.byId.get(id);
    return definition.scope === "local_custody" && definition.derived === false;
  });
}

function mandatoryProtocolFacts(catalog) {
  return catalog.order
    .filter((id) => {
      const definition = catalog.byId.get(id);
      return definition.mandatory && definition.derived === false;
    })
    .map((capability) => ({
      capability,
      state: "supported",
      evidenceKind: "source_contract",
      measuredAtUnixSeconds: null,
      reasonCode: null
    }));
}

export function validateAndroidCapabilityProbe(
  probe,
  catalog = loadCapabilityCatalog()
) {
  scanForbiddenKeys(probe);
  requireExactKeys(probe, PROBE_KEYS, "Android capability probe");
  requireValue(probe.schemaVersion === 1, "Android capability probe schema is unsupported");
  requireValue(Array.isArray(probe.facts), "Android capability facts must be an array");
  const expectedIds = androidFactIds(catalog);
  const seen = new Set();
  for (const fact of probe.facts) {
    requireExactKeys(fact, FACT_KEYS, "Android capability fact");
    requireValue(expectedIds.includes(fact.capability),
      "Android capability fact is not an Android custody fact");
    requireValue(!seen.has(fact.capability), "Android capability fact is duplicated");
    requireValue(FACT_STATES.has(fact.state), "Android capability fact state is invalid");
    requireValue(EVIDENCE_KINDS.has(fact.evidenceKind),
      "Android capability evidence kind is invalid");
    requireValue(fact.measuredAtUnixSeconds === null,
      "Android capability report must omit runtime timestamps");
    requireValue(
      fact.reasonCode === null || /^[a-z0-9._-]{1,96}$/u.test(fact.reasonCode),
      "Android capability reason code is invalid"
    );
    requireValue(
      fact.state === "supported" ? fact.reasonCode === null : typeof fact.reasonCode === "string",
      "Android capability reason does not match its fact state"
    );
    seen.add(fact.capability);
  }
  requireValue(expectedIds.length === seen.size && expectedIds.every((id) => seen.has(id)),
    "Android capability probe does not classify every Android custody fact");

  const report = reduceCapabilityFacts(
    [...mandatoryProtocolFacts(catalog), ...probe.facts],
    catalog
  );
  validateCapabilityReport(report, catalog);
  requireValue(report.mandatoryFoundationComplete === true,
    "Android capability report lost the mandatory protocol foundation");
  return report;
}

export function validateAndroidCapabilityMeasurements(measurements) {
  scanForbiddenKeys(measurements);
  requireExactKeys(measurements, MEASUREMENT_KEYS, "Android capability measurements");
  requireValue(measurements.schemaVersion === 1,
    "Android capability measurement schema is unsupported");
  requireValue(typeof measurements.keyStoreAvailable === "boolean",
    "Android KeyStore availability was not measured");
  requireValue(["os_secure_store", "memory_only_ephemeral"].includes(
    measurements.custodyStrategy
  ), "Android custody strategy is invalid");
  requireValue(["persistent_state_available", "re_pair_rekey_after_restart"].includes(
    measurements.restartSemantics
  ), "Android restart semantics are invalid");
  requireValue(SECURITY_LEVELS.has(measurements.securityLevel),
    "Android key security level is invalid");
  requireValue(AUTHENTICATION_TYPES.has(measurements.userAuthenticationType),
    "Android user authentication type is invalid");
  requireValue(Number.isInteger(measurements.keyGenerationAttemptCount) &&
    measurements.keyGenerationAttemptCount >= 0,
  "Android key generation attempt count is invalid");
  requireValue(measurements.bodyRedacted === true,
    "Android capability measurements are not redacted");

  if (measurements.custodyStrategy === "os_secure_store") {
    requireValue(measurements.keyStoreAvailable === true && measurements.keyPresent === true,
      "Android persistent custody lacks a generated KeyStore key");
    requireValue(measurements.keyMaterialNonExportable === true,
      "Android persistent custody key is exportable or unmeasured");
    requireValue(measurements.restartSemantics === "persistent_state_available",
      "Android persistent custody restart semantics are inconsistent");
  } else {
    requireValue(measurements.keyPresent === false,
      "Android memory-only custody claims a persistent key");
    requireValue(measurements.restartSemantics === "re_pair_rekey_after_restart",
      "Android memory-only custody omitted restart re-pair/rekey");
  }
  if (measurements.userAuthenticationRequired === false) {
    requireValue(measurements.userAuthenticationType === "none",
      "Android non-auth custody claims an authentication type");
  }
  if (measurements.securityLevel === "strongbox") {
    requireValue(measurements.securityLevelMeasured === true,
      "Android StrongBox claim is not measured");
  }
  return measurements;
}

export function summarizeAndroidCapabilityStore(store = {}) {
  requireValue(isPlainObject(store.capabilityProbe),
    "Android secret store capability probe is missing");
  requireValue(isPlainObject(store.measurements),
    "Android secret store capability measurements are missing");
  const report = validateAndroidCapabilityProbe(store.capabilityProbe);
  const measurements = validateAndroidCapabilityMeasurements(store.measurements);
  requireValue(report.custody.strategy === measurements.custodyStrategy,
    "Android capability report and measured custody disagree");
  requireValue(report.custody.restartSemantics === measurements.restartSemantics,
    "Android capability report and measured restart semantics disagree");
  return {
    provider: String(store.provider || ""),
    ffiBoundary: String(store.ffiBoundary || ""),
    secretTransport: String(store.secretTransport || ""),
    secretStoreBackend: String(store.secretStoreBackend || ""),
    secretStoreContract: String(store.secretStoreContract || ""),
    secretStoreAccountPrefix: String(store.secretStoreAccountPrefix || ""),
    secretStoreNamespace: String(store.secretStoreNamespace || ""),
    sharedRustSecretStoreHandleContract:
      store.sharedRustSecretStoreHandleContract === true,
    rawJsonSecretOverridesUsed: store.rawJsonSecretOverridesUsed === true,
    rawJsonSecretOverridesProvenAbsent:
      store.rawJsonSecretOverridesUsed === false &&
      store.rawJsonSecretOverridesProvenAbsent === true,
    portableConfigAuthority: String(store.portableConfigAuthority || ""),
    kotlinConfigReadWrite: store.kotlinConfigReadWrite === true,
    statusProbeSideEffectFree: store.statusProbeSideEffectFree === true,
    androidKeyMaterialExported: store.androidKeyMaterialExported === true,
    decryptedSecretCrossesJniInProcess:
      store.decryptedSecretCrossesJniInProcess === true,
    getNotFoundSeparatedFromFailure:
      store.getNotFoundSeparatedFromFailure === true,
    applicationAuthorizationGrantRequired:
      store.applicationAuthorizationGrantRequired === true,
    custodyStrategy: report.custody.strategy,
    restartSemantics: report.custody.restartSemantics,
    enabledCapabilities: report.enabled,
    unavailableCapabilities: report.unavailable,
    unverifiedCapabilities: report.unverified,
    mandatoryFoundationComplete: report.mandatoryFoundationComplete,
    userAuthenticationSelected:
      measurements.userAuthenticationRequired === true ||
      measurements.userAuthenticationRequested === true,
    deviceCredentialAvailable: measurements.deviceCredentialAvailable === true,
    strongBiometricAvailable: measurements.strongBiometricAvailable === true,
    securityLevel: measurements.securityLevel,
    capabilityReport: report,
    measurements
  };
}

export function assertAndroidCapabilityStoreValid(store = {}, phase = "") {
  const summary = summarizeAndroidCapabilityStore(store);
  requireValue(summary.ffiBoundary === "jni", `Android JNI boundary is missing ${phase}`);
  requireValue(summary.secretStoreContract === "rust_secure_mesh_secret_store_handle_v1",
    `Android shared secret-store contract is missing ${phase}`);
  requireValue(summary.sharedRustSecretStoreHandleContract === true,
    `Android shared secret-store handle is missing ${phase}`);
  requireValue(summary.rawJsonSecretOverridesProvenAbsent === true,
    `Android raw secret overrides are not proven absent ${phase}`);
  requireValue(summary.secretTransport === "jni_callback_in_process_secret_bytes",
    `Android JNI secret transport is not described exactly ${phase}`);
  requireValue(summary.portableConfigAuthority === "rust_generation_cas" &&
    summary.kotlinConfigReadWrite === false,
  `Android portable config has more than one writer ${phase}`);
  requireValue(summary.statusProbeSideEffectFree === true,
    `Android status probe may mutate custody ${phase}`);
  requireValue(summary.androidKeyMaterialExported === false,
    `Android KeyStore key material was exported ${phase}`);
  requireValue(summary.decryptedSecretCrossesJniInProcess === true,
    `Android JNI secret-byte boundary was hidden ${phase}`);
  requireValue(summary.getNotFoundSeparatedFromFailure === true,
    `Android secret read conflates missing data with failure ${phase}`);
  requireValue(summary.mandatoryFoundationComplete === true,
    `Android mandatory Secure Mesh foundation is incomplete ${phase}`);
  return summary;
}
