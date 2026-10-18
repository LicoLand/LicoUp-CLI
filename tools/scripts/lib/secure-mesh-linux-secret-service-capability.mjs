import {
  loadCapabilityCatalog,
  reduceCapabilityFacts,
  validateCapabilityReport
} from "./secure-mesh-capability-report.mjs";

export const linuxSecretServiceProbeSchemaVersion = 1;

const EXACT_PROBE_KEYS = Object.freeze([
  "schemaVersion",
  "interaction",
  "api",
  "session",
  "defaultCollection",
  "collection",
  "prompt",
  "read",
  "write",
  "delete",
  "service",
  "ordinaryFilePersistence"
]);
const ENUMS = Object.freeze({
  interaction: new Set(["noninteractive", "user_initiated"]),
  api: new Set(["available", "absent", "unverified"]),
  session: new Set(["established", "failed", "unverified"]),
  defaultCollection: new Set(["available", "absent", "unverified"]),
  collection: new Set(["unlocked", "locked", "unverified"]),
  prompt: new Set(["not_required", "required", "not_attempted", "unverified"]),
  read: new Set(["supported", "unsupported", "unverified"]),
  write: new Set(["supported", "unsupported", "unverified"]),
  delete: new Set(["supported", "unsupported", "unverified"]),
  service: new Set(["stable", "disappeared", "temporarily_unavailable", "unverified"]),
  ordinaryFilePersistence: new Set(["absent", "detected", "unverified"])
});
const FORBIDDEN_PROBE_KEYS = new Set([
  "address",
  "busAddress",
  "containerId",
  "dbusAddress",
  "hostname",
  "itemPath",
  "localPath",
  "objectPath",
  "rawLog",
  "username"
]);
const SENSITIVE_VALUE_PATTERNS = Object.freeze([
  /(?:^|["'\s])unix:(?:path|abstract)=/iu,
  /\/org\/freedesktop\/(?:DBus|secrets)(?:\/|$)/u,
  /\/(?:Users|home|private|tmp|var\/folders)\//u,
  /[A-Za-z]:\\/u,
  /-----BEGIN|-----END/u,
  /Bearer\s+(?!\[redacted\])\S+/u
]);

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

function canonicalJsonValue(value) {
  if (Array.isArray(value)) return value.map(canonicalJsonValue);
  if (!isPlainObject(value)) return value;
  return Object.fromEntries(
    Object.keys(value)
      .sort()
      .map((key) => [key, canonicalJsonValue(value[key])])
  );
}

function scanNoRuntimeIdentity(value) {
  if (Array.isArray(value)) {
    for (const item of value) scanNoRuntimeIdentity(item);
    return;
  }
  if (isPlainObject(value)) {
    for (const [key, nested] of Object.entries(value)) {
      requireValue(!FORBIDDEN_PROBE_KEYS.has(key), "Linux Secret Service probe contains a forbidden runtime field");
      scanNoRuntimeIdentity(nested);
    }
    return;
  }
  if (typeof value === "string") {
    requireValue(
      SENSITIVE_VALUE_PATTERNS.every((pattern) => !pattern.test(value)),
      "Linux Secret Service probe contains runtime identity or local path data"
    );
  }
}

function fact(capability, state, reasonCode) {
  return state === "supported" ? { capability, state } : { capability, state, reasonCode };
}

function mandatoryProtocolFacts(catalog) {
  return catalog.order
    .map((id) => catalog.byId.get(id))
    .filter((definition) => definition.mandatory && !definition.derived)
    .map((definition) => ({ capability: definition.id, state: "supported" }));
}

function classifySecretServiceAvailability(probe) {
  if (probe.api === "absent") {
    return { state: "unsupported", reasonCode: "linux_secret_service_api_absent" };
  }
  if (probe.session === "failed") {
    return { state: "temporarily_unavailable", reasonCode: "linux_secret_service_session_failed" };
  }
  if (probe.defaultCollection === "absent") {
    return {
      state: "unsupported",
      reasonCode: "linux_secret_service_default_collection_absent"
    };
  }
  if (probe.collection === "locked") {
    return {
      state: "temporarily_unavailable",
      reasonCode: "linux_secret_service_collection_locked"
    };
  }
  if (probe.prompt === "required") {
    return {
      state: "temporarily_unavailable",
      reasonCode: "linux_secret_service_prompt_required"
    };
  }
  if (probe.service === "disappeared") {
    return {
      state: "temporarily_unavailable",
      reasonCode: "linux_secret_service_disappeared"
    };
  }
  if (probe.service === "temporarily_unavailable") {
    return {
      state: "temporarily_unavailable",
      reasonCode: "linux_secret_service_temporarily_unavailable"
    };
  }
  if (probe.api === "available" && probe.session === "established" &&
      probe.defaultCollection === "available" && probe.collection === "unlocked" &&
      probe.prompt === "not_required" && probe.service === "stable") {
    return { state: "supported", reasonCode: null };
  }
  return { state: "unverified", reasonCode: "linux_secret_service_probe_incomplete" };
}

export function validateLinuxSecretServiceProbe(probe) {
  requireValue(isPlainObject(probe), "Linux Secret Service probe must be an object");
  const keys = Object.keys(probe);
  requireValue(
    keys.length === EXACT_PROBE_KEYS.length && EXACT_PROBE_KEYS.every((key) => keys.includes(key)),
    "Linux Secret Service probe fields are not exact"
  );
  requireValue(probe.schemaVersion === linuxSecretServiceProbeSchemaVersion,
    "Linux Secret Service probe schema version is unsupported");
  for (const [key, allowed] of Object.entries(ENUMS)) {
    requireValue(allowed.has(probe[key]), `Linux Secret Service probe ${key} fact is invalid`);
  }
  requireValue(probe.ordinaryFilePersistence !== "detected",
    "Linux Secret Service probe detected forbidden ordinary-file secret persistence");
  scanNoRuntimeIdentity(probe);
  return Object.freeze({ ...probe });
}

export function reduceLinuxSecretServiceProbe(probe, catalog = loadCapabilityCatalog()) {
  const normalized = validateLinuxSecretServiceProbe(probe);
  const osStoreOperational = normalized.api === "available" &&
    normalized.session === "established" &&
    normalized.defaultCollection === "available" &&
    normalized.collection === "unlocked" &&
    normalized.prompt === "not_required" &&
    normalized.read === "supported" &&
    normalized.write === "supported" &&
    normalized.delete === "supported" &&
    normalized.service === "stable" &&
    normalized.ordinaryFilePersistence === "absent";
  const serviceAvailability = classifySecretServiceAvailability(normalized);
  const osAvailability = osStoreOperational
    ? { state: "supported", reasonCode: null }
    : serviceAvailability.state === "supported"
      ? { state: "unverified", reasonCode: "linux_secret_service_probe_incomplete" }
      : serviceAvailability;
  const facts = [
    ...mandatoryProtocolFacts(catalog),
    fact("custody.os_secure_store", osAvailability.state, osAvailability.reasonCode),
    fact(
      "custody.linux_secret_service",
      serviceAvailability.state,
      serviceAvailability.reasonCode
    )
  ];
  const capabilityReport = reduceCapabilityFacts(facts, catalog);
  validateCapabilityReport(capabilityReport, catalog);
  requireValue(capabilityReport.custody !== null, "Linux custody projection has no safe strategy");
  requireValue(
    capabilityReport.custody.strategy === (osStoreOperational ? "os_secure_store" : "memory_only_ephemeral"),
    "Linux custody projection selected an inconsistent strategy"
  );
  if (!osStoreOperational) {
    requireValue(capabilityReport.custody.restartSemantics === "re_pair_rekey_after_restart",
      "Linux memory-only custody omitted restart re-pair/rekey semantics");
  }
  return Object.freeze({
    probe: normalized,
    capabilityReport,
    osStoreOperational
  });
}

export function validateLinuxSecretServiceProjection(probe, capabilityReport, catalog = loadCapabilityCatalog()) {
  validateCapabilityReport(capabilityReport, catalog);
  const expected = reduceLinuxSecretServiceProbe(probe, catalog).capabilityReport;
  requireValue(
    JSON.stringify(canonicalJsonValue(capabilityReport)) ===
      JSON.stringify(canonicalJsonValue(expected)),
    "Linux Secret Service capability report is not the exact shared reducer projection");
  return Object.freeze({
    ok: true,
    custodyStrategy: capabilityReport.custody.strategy,
    restartSemantics: capabilityReport.custody.restartSemantics,
    catalogDigest: capabilityReport.catalogDigest
  });
}

export function linuxSecretServiceProbeFixture(scenario) {
  const base = {
    schemaVersion: linuxSecretServiceProbeSchemaVersion,
    interaction: "noninteractive",
    api: "available",
    session: "established",
    defaultCollection: "available",
    collection: "unlocked",
    prompt: "not_required",
    read: "supported",
    write: "supported",
    delete: "supported",
    service: "stable",
    ordinaryFilePersistence: "absent"
  };
  const overrides = {
    unlocked: {},
    absent: {
      api: "absent",
      session: "failed",
      defaultCollection: "absent",
      collection: "unverified",
      prompt: "not_attempted",
      read: "unverified",
      write: "unverified",
      delete: "unverified",
      service: "temporarily_unavailable"
    },
    session_failure: {
      session: "failed",
      defaultCollection: "unverified",
      collection: "unverified",
      prompt: "not_attempted",
      read: "unverified",
      write: "unverified",
      delete: "unverified",
      service: "temporarily_unavailable"
    },
    no_default_collection: {
      defaultCollection: "absent",
      collection: "unverified",
      prompt: "not_attempted",
      read: "unverified",
      write: "unverified",
      delete: "unverified"
    },
    locked: {
      collection: "locked",
      prompt: "not_attempted",
      read: "unverified",
      write: "unverified",
      delete: "unverified"
    },
    prompt_required: {
      collection: "unlocked",
      prompt: "required",
      read: "unverified",
      write: "unverified",
      delete: "unverified"
    },
    service_disappeared: {
      read: "unverified",
      write: "unverified",
      delete: "unverified",
      service: "disappeared"
    }
  };
  requireValue(Object.hasOwn(overrides, scenario), "unknown Linux Secret Service fixture scenario");
  return { ...base, ...overrides[scenario] };
}
