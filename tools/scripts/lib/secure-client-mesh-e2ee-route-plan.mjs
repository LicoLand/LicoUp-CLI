import fs from "node:fs/promises";

const routeConfigUrl = new URL("../config/secure-mesh-e2ee-evidence-routes.json", import.meta.url);
const routeConfigRef = "tools/scripts/config/secure-mesh-e2ee-evidence-routes.json";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_field", /(?:privateKey|sessionKey|rootKey|chainKey|messageKey)/u]
]);
let cachedRouteConfig;

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function normalizeSafeRef(value, label) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref || ref.startsWith("/") || ref.startsWith("file:") || /^https?:\/\//iu.test(ref) || ref.split("/").includes("..")) {
    throw new Error(`Invalid Secure Client Mesh ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeIdentifier(value, label) {
  const identifier = String(value || "").trim();
  if (!/^[A-Z0-9_]+$/u.test(identifier)) {
    throw new Error(`Invalid Secure Client Mesh ${label}: ${identifier || "<empty>"}`);
  }
  return identifier;
}

function normalizeCommand(value, label) {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return normalizeCommandDescriptor(value, label);
  }
  const command = String(value || "").trim();
  if (!command || /[\r\n]/u.test(command) || command.length > 240) {
    throw new Error(`Invalid Secure Client Mesh ${label}`);
  }
  assertNoLeak(command, label);
  return command;
}

function normalizeNpmScriptPart(value, label) {
  const script = String(value || "").trim();
  if (!/^[a-z0-9:_-]+$/u.test(script) || script.includes("..")) {
    throw new Error(`Invalid Secure Client Mesh ${label}: ${script || "<empty>"}`);
  }
  return script;
}

function normalizeCommandDescriptor(value, label) {
  const command = asRecord(value);
  const runner = String(command.runner || "").trim();
  const action = String(command.action || "").trim();
  if (runner !== "npm" || action !== "run") {
    throw new Error(`Invalid Secure Client Mesh ${label} command descriptor`);
  }
  const scriptNamespace = normalizeNpmScriptPart(command.scriptNamespace, `${label} script namespace`);
  const scriptName = normalizeNpmScriptPart(command.scriptName, `${label} script name`);
  const rendered = `${runner} ${action} ${scriptNamespace}:${scriptName}`;
  assertNoLeak(rendered, label);
  return rendered;
}

function normalizeDiagnosticRefs(value) {
  const diagnosticConfig = asRecord(value);
  return {
    stationAcceptance: normalizeSafeRef(
      diagnosticConfig.stationAcceptance,
      "Lico Arc BadTower diagnostic acceptance report ref"
    ),
    physicalEvidenceManifest: normalizeSafeRef(
      diagnosticConfig.physicalEvidenceManifest,
      "physical evidence manifest diagnostic report ref"
    )
  };
}

async function loadRawRouteConfig() {
  if (cachedRouteConfig) {
    return cachedRouteConfig;
  }
  const payload = JSON.parse(await fs.readFile(routeConfigUrl, "utf8"));
  if (payload?.schemaVersion !== "licomesh.secure-mesh.e2ee-evidence-routes.v2") {
    throw new Error("Secure Client Mesh evidence route config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh evidence route config");
  cachedRouteConfig = payload;
  return cachedRouteConfig;
}

export async function loadSecureClientMeshE2eeEvidenceRoutePlan({
  canonicalBlockers = []
} = {}) {
  const config = await loadRawRouteConfig();
  const canonical = new Set([].concat(canonicalBlockers || []).map((blocker) => String(blocker || "").trim()).filter(Boolean));
  if (canonical.size === 0) {
    throw new Error("Secure Client Mesh route plan requires canonical blockers");
  }
  const routes = asRecord(config.routes);
  const unknown = Object.keys(routes).filter((blocker) => !canonical.has(blocker));
  if (unknown.length > 0) {
    throw new Error(`Secure Client Mesh route config contains non-contract blockers: ${unknown.join(", ")}`);
  }
  const missing = [...canonical].filter((blocker) => !Object.prototype.hasOwnProperty.call(routes, blocker));
  if (missing.length > 0) {
    throw new Error(`Secure Client Mesh route config does not cover contract blockers: ${missing.join(", ")}`);
  }

  const authorityTemplate = asRecord(config.authorityProofTemplate);
  const authorityProofTemplate = {
    envKeys: [].concat(authorityTemplate.envKeys || [])
      .map((key) => normalizeIdentifier(key, "authority proof template env key")),
    ref: normalizeSafeRef(authorityTemplate.ref, "authority proof template ref")
  };
  if (authorityProofTemplate.envKeys.length === 0) {
    throw new Error("Secure Client Mesh route config must define authority proof template env keys");
  }

  const diagnosticRefs = normalizeDiagnosticRefs(config.diagnosticRefs);

  const evidenceRoutePlan = Object.fromEntries(Object.entries(routes).map(([blocker, route]) => {
    const refs = [].concat(route?.refs || []).map((ref) => normalizeSafeRef(ref, `${blocker} route ref`));
    const commands = [].concat(route?.commands || []).map((command) => normalizeCommand(command, `${blocker} route command`));
    if (refs.length === 0) {
      throw new Error(`Secure Client Mesh route config for ${blocker} has no evidence refs`);
    }
    return [blocker, { refs, commands }];
  }));

  return {
    configRef: routeConfigRef,
    schemaVersion: config.schemaVersion,
    evidenceRoutePlan,
    authorityProofTemplate,
    diagnosticRefs,
    coverage: {
      canonicalBlockerCount: canonical.size,
      missingRouteBlockers: missing
    }
  };
}

export async function loadSecureClientMeshE2eeDiagnosticRefs() {
  const config = await loadRawRouteConfig();
  return {
    configRef: routeConfigRef,
    schemaVersion: config.schemaVersion,
    diagnosticRefs: normalizeDiagnosticRefs(config.diagnosticRefs)
  };
}
