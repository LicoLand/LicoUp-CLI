import fs from "node:fs/promises";

const configUrl = new URL("../config/secure-mesh-acp-relay-governed-baseline.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-acp-relay-governed-baseline.json";
const schemaVersion = "licomesh.secure-mesh.acp-relay-governed-baseline-config.v2";
const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u]
]);
let cachedConfig;

function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(`${label} contains sensitive data: ${kind}`);
    }
  }
}

function normalizeId(value, label) {
  const id = String(value || "").trim();
  if (!/^[a-z0-9][a-z0-9_-]{2,120}$/u.test(id)) {
    throw new Error(`Invalid Secure Mesh ACP governed baseline ${label}: ${id || "<empty>"}`);
  }
  return id;
}

function normalizeSafeRef(value, label, pattern) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref || ref.startsWith("/") || ref.split("/").includes("..") || !pattern.test(ref)) {
    throw new Error(`Invalid Secure Mesh ACP governed baseline ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokens(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()).filter(Boolean) : [];
  if (tokens.length === 0) {
    throw new Error(`Secure Mesh ACP governed baseline config must define ${label}`);
  }
  for (const token of tokens) {
    assertNoLeak(token, `secure mesh ACP governed baseline ${label}`);
  }
  return Object.freeze(tokens);
}

function normalizeEnvKeys(value, label) {
  const keys = Array.isArray(value)
    ? value.map((item) => String(item || "").trim()).filter(Boolean)
    : [];
  if (keys.length === 0 || new Set(keys).size !== keys.length ||
    keys.some((key) => !/^[A-Z][A-Z0-9_]+$/u.test(key))) {
    throw new Error(`Secure Mesh ACP governed baseline config must define unique ${label}`);
  }
  return Object.freeze(keys);
}

function normalizeExternalGatewayEvidence(value) {
  const input = value && typeof value === "object" && !Array.isArray(value) ? value : {};
  return Object.freeze({
    pathEnvKeys: normalizeEnvKeys(input.pathEnvKeys, "external gateway evidence path env keys"),
    digestEnvKeys: normalizeEnvKeys(input.digestEnvKeys, "external gateway evidence digest env keys")
  });
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  return Object.freeze(checks.map((check, index) => {
    const refs = Array.isArray(check.files) ? check.files : [check.file];
    if (refs.length === 0) {
      throw new Error(`Secure Mesh ACP governed baseline config must define source check ${index + 1} files`);
    }
    const files = Object.freeze(refs.map((ref, fileIndex) => normalizeSafeRef(
      ref,
      `source check ${index + 1} file ${fileIndex + 1}`,
      /^(?:apps|crates|docs|tools)\/.+\.(?:rs|mjs|dart|json|md)$/u
    )));
    return Object.freeze({
      id: normalizeId(check.id, `source check ${index + 1} id`),
      file: files[0],
      files,
      tokens: normalizeTokens(check.tokens, `source check ${index + 1} tokens`)
    });
  }));
}

function normalizeNativeTestFilters(value) {
  const filters = normalizeTokens(value, "native test filters");
  for (const filter of filters) {
    if (!/^[a-z0-9_]+$/u.test(filter)) {
      throw new Error(`Invalid Secure Mesh ACP governed baseline native test filter: ${filter}`);
    }
  }
  return filters;
}

async function loadRawConfig() {
  if (cachedConfig) {
    return cachedConfig;
  }
  const payload = JSON.parse(await fs.readFile(configUrl, "utf8"));
  if (payload?.schemaVersion !== schemaVersion) {
    throw new Error("Secure Mesh ACP governed baseline config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh ACP governed baseline config");
  cachedConfig = Object.freeze({
    configRef,
    schemaVersion,
    reportOutput: normalizeSafeRef(
      payload.reportOutput,
      "reportOutput",
      /^build\/reports\/[\w./-]+\.json$/u
    ),
    ownership: Object.freeze({
      acpRelayGovernance: String(payload.ownership?.acpRelayGovernance || "core"),
      secureMeshAcpEnvelope: String(payload.ownership?.secureMeshAcpEnvelope || "client"),
      note: String(payload.ownership?.note || "")
    }),
    externalGatewayEvidence: normalizeExternalGatewayEvidence(payload.externalGatewayEvidence),
    sourceChecks: normalizeSourceChecks(payload.sourceChecks),
    nativeTestFilters: normalizeNativeTestFilters(payload.nativeTestFilters)
  });
  return cachedConfig;
}

export async function loadSecureMeshAcpRelayGovernedBaselineConfig() {
  return loadRawConfig();
}
