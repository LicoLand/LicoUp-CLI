#!/usr/bin/env node
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { loadCapabilityCatalog } from "./lib/secure-mesh-capability-report.mjs";
import {
  summarizeAndroidCapabilityStore,
  validateAndroidCapabilityMeasurements,
  validateAndroidCapabilityProbe
} from "./lib/secure-mesh-android-capabilities.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function expectRejected(action, message) {
  let rejected = false;
  try {
    action();
  } catch {
    rejected = true;
  }
  requireValue(rejected, message);
}

function fact(capability, state, reasonCode = null, evidenceKind = "test_fixture") {
  return {
    capability,
    state,
    evidenceKind,
    measuredAtUnixSeconds: null,
    reasonCode
  };
}

function probe(overrides = {}) {
  const catalog = loadCapabilityCatalog();
  const facts = catalog.order
    .filter((id) => {
      const definition = catalog.byId.get(id);
      return definition.scope === "local_custody" && definition.derived === false;
    })
    .map((id) => overrides[id] || fact(id, "unsupported", "fixture_not_supported"));
  return { schemaVersion: 1, facts };
}

function measurements(overrides = {}) {
  return {
    schemaVersion: 1,
    keyStoreAvailable: true,
    custodyStrategy: "os_secure_store",
    restartSemantics: "persistent_state_available",
    keyPresent: true,
    keyMaterialNonExportable: true,
    securityLevelMeasured: true,
    securityLevel: "software",
    insideSecureHardware: false,
    userAuthenticationRequested: true,
    userAuthenticationRequired: true,
    userAuthenticationTypeMeasured: true,
    userAuthenticationType: "device_credential",
    deviceCredentialAvailable: true,
    deviceCredentialAllowed: true,
    strongBiometricAvailable: false,
    strongBiometricAvailabilityMeasured: true,
    strongBiometricAllowed: false,
    userAuthenticationValiditySeconds: 300,
    userAuthenticationHardwareEnforced: false,
    invalidatedByBiometricEnrollment: null,
    biometricEnrollmentInvalidationNotApplicableBecauseDeviceCredentialAllowed: false,
    unlockedDeviceRequiredRequested: true,
    unlockedDeviceRequired: true,
    strongBoxRequested: false,
    keyGenerationAttemptCount: 1,
    bodyRedacted: true,
    ...overrides
  };
}

function supportedBaseFacts(extra = {}) {
  return {
    "custody.os_secure_store": fact("custody.os_secure_store", "supported"),
    "custody.software_backed": fact("custody.software_backed", "supported"),
    "custody.non_exportable": fact("custody.non_exportable", "supported"),
    "custody.device_bound": fact("custody.device_bound", "supported"),
    "custody.unlocked_device_required":
      fact("custody.unlocked_device_required", "supported"),
    "custody.android_keystore": fact("custody.android_keystore", "supported"),
    ...extra
  };
}

function store(capabilityProbe, capabilityMeasurements) {
  return {
    provider: capabilityMeasurements.custodyStrategy === "os_secure_store"
      ? "AndroidKeyStore"
      : "process-memory",
    ffiBoundary: "jni",
    secretTransport: "jni_callback_in_process_secret_bytes",
    secretStoreBackend: capabilityMeasurements.custodyStrategy === "os_secure_store"
      ? "android-keystore"
      : "memory-only-ephemeral",
    secretStoreContract: "rust_secure_mesh_secret_store_handle_v1",
    secretStoreAccountPrefix: "mobileRelayE2ee",
    secretStoreNamespace: "mobileRelayRuntime",
    sharedRustSecretStoreHandleContract: true,
    rawJsonSecretOverridesUsed: false,
    rawJsonSecretOverridesProvenAbsent: true,
    portableConfigAuthority: "rust_generation_cas",
    kotlinConfigReadWrite: false,
    statusProbeSideEffectFree: true,
    androidKeyMaterialExported: false,
    decryptedSecretCrossesJniInProcess: true,
    getNotFoundSeparatedFromFailure: true,
    applicationAuthorizationGrantRequired: true,
    capabilityProbe,
    measurements: capabilityMeasurements
  };
}

const softwareProbe = probe(supportedBaseFacts());
const softwareReport = validateAndroidCapabilityProbe(softwareProbe);
requireValue(softwareReport.custody.strategy === "os_secure_store",
  "safe software AndroidKeyStore was rejected");
requireValue(!softwareReport.enabled.includes("custody.hardware_backed"),
  "software AndroidKeyStore falsely implied hardware backing");
const softwareSummary = summarizeAndroidCapabilityStore(
  store(softwareProbe, measurements())
);
requireValue(softwareSummary.userAuthenticationSelected === true,
  "Android persistent custody omitted mandatory user authentication");

const strongBoxOverrides = supportedBaseFacts({
  "custody.software_backed": fact(
    "custody.software_backed",
    "unsupported",
    "android_keystore_not_software_backed"
  ),
  "custody.hardware_backed": fact("custody.hardware_backed", "supported"),
  "custody.tee": fact("custody.tee", "supported"),
  "custody.strongbox": fact("custody.strongbox", "supported")
});
const strongBoxReport = validateAndroidCapabilityProbe(probe(strongBoxOverrides));
requireValue(strongBoxReport.enabled.includes("custody.strongbox"),
  "StrongBox scenario did not retain its exact capability");

const memoryProbe = probe({});
const memoryMeasurements = measurements({
  keyStoreAvailable: false,
  custodyStrategy: "memory_only_ephemeral",
  restartSemantics: "re_pair_rekey_after_restart",
  keyPresent: false,
  keyMaterialNonExportable: null,
  securityLevelMeasured: false,
  securityLevel: "unverified",
  insideSecureHardware: null,
  unlockedDeviceRequiredRequested: false,
  unlockedDeviceRequired: null,
  keyGenerationAttemptCount: 0,
  userAuthenticationRequested: false,
  userAuthenticationRequired: false,
  userAuthenticationType: "none",
  deviceCredentialAvailable: false,
  deviceCredentialAllowed: false,
  userAuthenticationValiditySeconds: null,
  userAuthenticationHardwareEnforced: null
});
const memorySummary = summarizeAndroidCapabilityStore(
  store(memoryProbe, memoryMeasurements)
);
requireValue(memorySummary.custodyStrategy === "memory_only_ephemeral" &&
  memorySummary.restartSemantics === "re_pair_rekey_after_restart",
"Android memory-only fallback omitted restart re-pair/rekey");

const unknownMeasurement = measurements({
  securityLevelMeasured: false,
  securityLevel: "unverified",
  insideSecureHardware: null
});
validateAndroidCapabilityMeasurements(unknownMeasurement);

const leaked = structuredClone(softwareProbe);
leaked.deviceId = "forbidden";
expectRejected(() => validateAndroidCapabilityProbe(leaked),
  "Android capability schema accepted a device identifier");

const sourceFiles = [
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidSecretStore.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidCapability.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidCapabilityProbe.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidCustodyManager.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidEncryptedRecordStore.kt",
  "apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidKeyPolicy.kt",
  "crates/licoup-native/src/ffi/android_ffi.rs",
  "tools/scripts/client-android-native-tests.mjs",
  "tools/scripts/client-android-physical-install-launch.mjs"
];
const sources = new Map(sourceFiles.map((relativePath) => [
  relativePath,
  readFileSync(path.join(repoRoot, relativePath), "utf8")
]));
const source = [...sources.values()].join("\n");
requireValue(
  sources.get("apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidKeyPolicy.kt")
    .includes("internal object SecureMeshAndroidKeyPolicyStrategy"),
  "Android adaptive custody policy strategy declaration is missing"
);
requireValue(
  sources.get("apps/desktop/android/app/src/main/kotlin/land/lico/licoup/SecureMeshAndroidCustodyManager.kt")
    .includes("SecureMeshAndroidKeyPolicyStrategy.select("),
  "Android custody manager does not invoke the adaptive key policy strategy"
);
for (const requiredImplementation of [
  "SecureMeshAndroidKeyAttemptFailure.STRONGBOX_UNAVAILABLE",
  "SecureMeshAndroidCustodySelection.MemoryOnly",
  "statusProbeSideEffectFree",
  "requires user-approved re-pair",
  "SecureMeshAndroidAtomicRecordWriter",
  "ephemeralStore.put",
  "secureMeshAndroidCapabilityProbeJson",
  "parse_android_capability_facts",
  "SecureMeshAndroidAdaptiveCustodyTest"
]) {
  requireValue(source.includes(requiredImplementation),
    `Android adaptive custody implementation is missing: ${requiredImplementation}`);
}
for (const forbiddenPersistence of [
  "plaintext-secret-store",
  "portable-file-secret-store",
  "shared-preferences-secret-store"
]) {
  requireValue(!source.includes(forbiddenPersistence),
    `Android source includes forbidden persistence: ${forbiddenPersistence}`);
}

console.log(JSON.stringify({
  ok: true,
  scenarioCount: 4,
  softwareCustody: softwareSummary.custodyStrategy,
  memoryRestartSemantics: memorySummary.restartSemantics,
  strongBoxEnabled: strongBoxReport.enabled.includes("custody.strongbox")
}));
