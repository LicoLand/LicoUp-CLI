import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const capabilityReportSchemaVersion = 1;
export const capabilityCatalogPath = fileURLToPath(new URL(
  "../../../crates/licoup-native/resources/secure-mesh-capability-catalog.json",
  import.meta.url
));

const FACT_STATES = new Set([
  "supported",
  "unsupported",
  "temporarily_unavailable",
  "unverified"
]);
const CAPABILITY_SCOPES = new Set(["protocol_session", "local_custody"]);
const REASON_CODE = /^[a-z0-9._-]{1,96}$/;
const SHA256_DIGEST = /^[a-f0-9]{64}$/;
const FORBIDDEN_POSTURE_KEYS = new Set([
  "tier",
  "level",
  "custodyProfile",
  "productionReady",
  "releaseReady",
  "platformReady"
]);

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) &&
    (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
}

function requireExactKeys(value, allowedKeys, label) {
  requireValue(isPlainObject(value), `${label} must be an object`);
  const keys = Object.keys(value);
  requireValue(keys.every((key) => allowedKeys.includes(key)), `${label} contains an unknown field`);
  requireValue(allowedKeys.every((key) => keys.includes(key)), `${label} is missing a required field`);
}

function sha256Hex(value) {
  return createHash("sha256").update(value).digest("hex");
}

function canonicalIds(catalog, ids) {
  const selected = ids instanceof Set ? ids : new Set(ids);
  return catalog.order.filter((id) => selected.has(id));
}

function sameOrderedArray(left, right) {
  return Array.isArray(left) && Array.isArray(right) &&
    left.length === right.length && left.every((value, index) => value === right[index]);
}

function requireCanonicalCapabilitySet(value, catalog, label) {
  requireValue(Array.isArray(value), `${label} must be an array`);
  requireValue(value.every((id) => typeof id === "string" && catalog.byId.has(id)),
    `${label} contains an unknown capability`);
  requireValue(new Set(value).size === value.length, `${label} contains a duplicate capability`);
  requireValue(sameOrderedArray(value, canonicalIds(catalog, value)),
    `${label} is not in canonical catalog order`);
  return new Set(value);
}

function scanForbiddenPostureKeys(value) {
  if (Array.isArray(value)) {
    for (const item of value) scanForbiddenPostureKeys(item);
    return;
  }
  if (!isPlainObject(value)) return;
  for (const [key, nested] of Object.entries(value)) {
    requireValue(!FORBIDDEN_POSTURE_KEYS.has(key), `capability report contains retired posture field: ${key}`);
    scanForbiddenPostureKeys(nested);
  }
}

function topologicalCatalog(capabilities) {
  const orderIndex = new Map(capabilities.map((definition, index) => [definition.id, index]));
  const indegree = new Map(capabilities.map((definition) => [definition.id, definition.prerequisites.length]));
  const dependents = new Map(capabilities.map((definition) => [definition.id, []]));
  let edgeCount = 0;
  for (const definition of capabilities) {
    for (const prerequisite of definition.prerequisites) {
      dependents.get(prerequisite).push(definition.id);
      edgeCount += 1;
    }
  }
  for (const entries of dependents.values()) {
    entries.sort((left, right) => orderIndex.get(left) - orderIndex.get(right));
  }
  const roots = capabilities
    .filter((definition) => indegree.get(definition.id) === 0)
    .map((definition) => definition.id)
    .sort((left, right) => orderIndex.get(left) - orderIndex.get(right));
  const order = [];
  while (roots.length > 0) {
    const id = roots.shift();
    order.push(id);
    for (const dependent of dependents.get(id)) {
      const next = indegree.get(dependent) - 1;
      indegree.set(dependent, next);
      if (next === 0) {
        roots.push(dependent);
        roots.sort((left, right) => orderIndex.get(left) - orderIndex.get(right));
      }
    }
  }
  requireValue(order.length === capabilities.length, "capability catalog contains a dependency cycle");
  return { order, edgeCount };
}

export function validateCapabilityCatalogText(source) {
  let raw;
  try {
    raw = JSON.parse(source);
  } catch {
    throw new Error("capability catalog is not valid JSON");
  }
  requireExactKeys(raw, ["schemaVersion", "capabilities"], "capability catalog");
  requireValue(raw.schemaVersion === 1, "capability catalog schema version is unsupported");
  requireValue(Array.isArray(raw.capabilities) && raw.capabilities.length > 0,
    "capability catalog is empty");

  const byId = new Map();
  for (const definition of raw.capabilities) {
    requireExactKeys(
      definition,
      ["id", "scope", "mandatory", "derived", "prerequisites"],
      "capability definition"
    );
    requireValue(typeof definition.id === "string" && /^(protocol|custody)\.[a-z0-9_]+$/.test(definition.id),
      "capability identifier is invalid");
    requireValue(!byId.has(definition.id), "capability catalog contains a duplicate identifier");
    requireValue(CAPABILITY_SCOPES.has(definition.scope), "capability scope is invalid");
    requireValue(typeof definition.mandatory === "boolean", "capability mandatory flag is invalid");
    requireValue(typeof definition.derived === "boolean", "capability derived flag is invalid");
    requireValue(Array.isArray(definition.prerequisites), "capability prerequisites must be an array");
    requireValue(new Set(definition.prerequisites).size === definition.prerequisites.length,
      "capability definition contains a duplicate prerequisite");
    requireValue(!(definition.mandatory && definition.scope !== "protocol_session"),
      "only protocol capabilities may be mandatory");
    byId.set(definition.id, Object.freeze({
      id: definition.id,
      scope: definition.scope,
      mandatory: definition.mandatory,
      derived: definition.derived,
      prerequisites: Object.freeze([...definition.prerequisites])
    }));
  }
  for (const definition of byId.values()) {
    requireValue(definition.prerequisites.every((id) => byId.has(id)),
      "capability prerequisite is missing from the catalog");
    requireValue(!definition.prerequisites.includes(definition.id),
      "capability cannot depend on itself");
  }

  const { order, edgeCount } = topologicalCatalog([...byId.values()]);
  return Object.freeze({
    schemaVersion: raw.schemaVersion,
    digest: sha256Hex(source),
    byId,
    order: Object.freeze(order),
    edgeCount,
    source
  });
}

let cachedCatalog;

export function loadCapabilityCatalog() {
  if (!cachedCatalog) {
    cachedCatalog = validateCapabilityCatalogText(readFileSync(capabilityCatalogPath, "utf8"));
  }
  return cachedCatalog;
}

function evaluateAvailableSet(catalog, availableInput) {
  const available = new Set(availableInput);
  const enabled = new Set();
  let visitedNodeCount = 0;
  let traversedEdgeCount = 0;
  for (const id of catalog.order) {
    visitedNodeCount += 1;
    const definition = catalog.byId.get(id);
    let dependenciesEnabled = true;
    for (const prerequisite of definition.prerequisites) {
      traversedEdgeCount += 1;
      dependenciesEnabled = enabled.has(prerequisite) && dependenciesEnabled;
    }
    const supported = definition.derived ? dependenciesEnabled : available.has(id);
    if (definition.derived && supported) available.add(id);
    if (supported && dependenciesEnabled) enabled.add(id);
  }
  return { available, enabled, visitedNodeCount, traversedEdgeCount };
}

export function reduceCapabilityFacts(facts, catalog = loadCapabilityCatalog()) {
  requireValue(Array.isArray(facts), "capability facts must be an array");
  const states = new Map();
  const reasonById = new Map();
  for (const fact of facts) {
    requireValue(isPlainObject(fact), "capability fact must be an object");
    requireValue(typeof fact.capability === "string" && catalog.byId.has(fact.capability),
      "capability fact contains an unknown identifier");
    requireValue(catalog.byId.get(fact.capability).derived === false,
      "derived capability cannot be supplied as a platform fact");
    requireValue(!states.has(fact.capability), "capability facts contain a duplicate identifier");
    requireValue(FACT_STATES.has(fact.state), "capability fact state is invalid");
    if (fact.reasonCode !== undefined && fact.reasonCode !== null) {
      requireValue(typeof fact.reasonCode === "string" && REASON_CODE.test(fact.reasonCode),
        "capability reason code is invalid");
      reasonById.set(fact.capability, fact.reasonCode);
    }
    states.set(fact.capability, fact.state);
  }

  const supported = new Set([...states]
    .filter(([, state]) => state === "supported")
    .map(([id]) => id));
  const evaluated = evaluateAvailableSet(catalog, supported);
  const unavailable = new Set([...states]
    .filter(([, state]) => state === "unsupported" || state === "temporarily_unavailable")
    .map(([id]) => id));
  const unverified = new Set();
  const reasons = {};
  for (const id of catalog.order) {
    const definition = catalog.byId.get(id);
    if (evaluated.enabled.has(id)) continue;
    const state = states.get(id);
    if (!evaluated.available.has(id) && !unavailable.has(id)) unverified.add(id);
    const dependenciesEnabled = definition.prerequisites.every((prerequisite) =>
      evaluated.enabled.has(prerequisite));
    reasons[id] = reasonById.get(id) || (
      state === "unsupported" ? "capability_not_supported" :
      state === "temporarily_unavailable" ? "capability_temporarily_unavailable" :
      state === "supported" && !dependenciesEnabled ? "capability_dependency_unmet" :
      definition.derived && !dependenciesEnabled ? "capability_dependency_unmet" :
      "capability_unverified"
    );
  }
  const missingMandatory = new Set(catalog.order.filter((id) =>
    catalog.byId.get(id).mandatory && !evaluated.enabled.has(id)));
  const custodyIds = new Set([...evaluated.enabled].filter((id) => id.startsWith("custody.")));
  const osStoreEnabled = evaluated.enabled.has("custody.os_secure_store");
  const custody = evaluated.enabled.has("custody.memory_only_ephemeral") ? {
    strategy: osStoreEnabled ? "os_secure_store" : "memory_only_ephemeral",
    restartSemantics: osStoreEnabled ? "persistent_state_available" : "re_pair_rekey_after_restart",
    enabledHardening: canonicalIds(catalog, osStoreEnabled ? custodyIds : new Set(["custody.memory_only_ephemeral"]))
  } : null;
  const orderedReasons = Object.fromEntries(
    Object.entries(reasons).sort(([left], [right]) => left < right ? -1 : left > right ? 1 : 0)
  );

  return {
    schemaVersion: capabilityReportSchemaVersion,
    catalogDigest: catalog.digest,
    mandatoryFoundationComplete: missingMandatory.size === 0,
    enabled: canonicalIds(catalog, evaluated.enabled),
    available: canonicalIds(catalog, evaluated.available),
    unavailable: canonicalIds(catalog, unavailable),
    unverified: canonicalIds(catalog, unverified),
    missingMandatory: canonicalIds(catalog, missingMandatory),
    reasons: orderedReasons,
    custody
  };
}

export function validateCapabilityReport(report, catalog = loadCapabilityCatalog()) {
  scanForbiddenPostureKeys(report);
  requireExactKeys(report, [
    "schemaVersion",
    "catalogDigest",
    "mandatoryFoundationComplete",
    "enabled",
    "available",
    "unavailable",
    "unverified",
    "missingMandatory",
    "reasons",
    "custody"
  ], "capability report");
  requireValue(report.schemaVersion === capabilityReportSchemaVersion,
    "capability report schema version is unsupported");
  requireValue(typeof report.catalogDigest === "string" && SHA256_DIGEST.test(report.catalogDigest),
    "capability report catalog digest is invalid");
  requireValue(report.catalogDigest === catalog.digest, "capability report catalog digest is stale");
  requireValue(typeof report.mandatoryFoundationComplete === "boolean",
    "capability report mandatory foundation result is invalid");

  const enabled = requireCanonicalCapabilitySet(report.enabled, catalog, "enabled capabilities");
  const available = requireCanonicalCapabilitySet(report.available, catalog, "available capabilities");
  const unavailable = requireCanonicalCapabilitySet(report.unavailable, catalog, "unavailable capabilities");
  const unverified = requireCanonicalCapabilitySet(report.unverified, catalog, "unverified capabilities");
  const missingMandatory = requireCanonicalCapabilitySet(
    report.missingMandatory,
    catalog,
    "missing mandatory capabilities"
  );
  requireValue([...enabled].every((id) => available.has(id)),
    "enabled capabilities must be available");
  requireValue([...available].every((id) => !unavailable.has(id) && !unverified.has(id)),
    "available capability states overlap");
  requireValue([...unavailable].every((id) => !unverified.has(id)),
    "unavailable and unverified capability states overlap");
  requireValue(catalog.order.every((id) => available.has(id) || unavailable.has(id) || unverified.has(id)),
    "capability report does not classify every catalog node");

  const evaluated = evaluateAvailableSet(catalog, available);
  requireValue(sameOrderedArray(report.enabled, canonicalIds(catalog, evaluated.enabled)),
    "enabled capabilities are not the exact dependency closure");
  requireValue(sameOrderedArray(report.available, canonicalIds(catalog, evaluated.available)),
    "derived available capabilities are incomplete");
  requireValue(evaluated.visitedNodeCount === catalog.order.length &&
    evaluated.traversedEdgeCount === catalog.edgeCount,
  "capability reducer did not traverse the graph in one bounded pass");

  const expectedMissing = canonicalIds(catalog, catalog.order.filter((id) =>
    catalog.byId.get(id).mandatory && !enabled.has(id)));
  requireValue(sameOrderedArray(report.missingMandatory, expectedMissing),
    "missing mandatory capability set is incorrect");
  requireValue(report.mandatoryFoundationComplete === (expectedMissing.length === 0),
    "mandatory foundation result is inconsistent");

  requireValue(isPlainObject(report.reasons), "capability reasons must be an object");
  const expectedReasonIds = catalog.order
    .filter((id) => !enabled.has(id))
    .sort();
  requireValue(sameOrderedArray(Object.keys(report.reasons), expectedReasonIds),
    "capability reasons are incomplete or not in canonical order");
  requireValue(Object.values(report.reasons).every((reason) =>
    typeof reason === "string" && REASON_CODE.test(reason)),
  "capability report contains an invalid reason code");

  requireValue(report.custody !== null, "capability report has no safe custody strategy");
  requireExactKeys(report.custody, ["strategy", "restartSemantics", "enabledHardening"],
    "capability custody selection");
  const custodyEnabled = new Set([...enabled].filter((id) => id.startsWith("custody.")));
  const osStoreEnabled = enabled.has("custody.os_secure_store");
  const expectedStrategy = osStoreEnabled ? "os_secure_store" : "memory_only_ephemeral";
  const expectedRestart = osStoreEnabled ? "persistent_state_available" : "re_pair_rekey_after_restart";
  requireValue(report.custody.strategy === expectedStrategy, "capability custody strategy is inconsistent");
  requireValue(report.custody.restartSemantics === expectedRestart,
    "capability custody restart semantics are inconsistent");
  requireCanonicalCapabilitySet(report.custody.enabledHardening, catalog, "enabled custody hardening");
  const expectedHardening = canonicalIds(
    catalog,
    osStoreEnabled ? custodyEnabled : new Set(["custody.memory_only_ephemeral"])
  );
  requireValue(sameOrderedArray(report.custody.enabledHardening, expectedHardening),
    "enabled custody hardening set is inconsistent");

  return Object.freeze({
    ok: true,
    catalogDigest: catalog.digest,
    mandatoryFoundationComplete: report.mandatoryFoundationComplete,
    custodyStrategy: report.custody.strategy,
    enabledCount: report.enabled.length,
    unavailableCount: report.unavailable.length,
    unverifiedCount: report.unverified.length
  });
}
