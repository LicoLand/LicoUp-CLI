import fs from "node:fs/promises";

const configUrl = new URL("../config/secure-mesh-acp-archive-release-proof.json", import.meta.url);
const configRef = "tools/scripts/config/secure-mesh-acp-archive-release-proof.json";
const schemaVersion = "licomesh.secure-mesh.acp-archive-release-proof-config.v1";
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
    throw new Error(`Invalid Secure Mesh ACP archive release proof ${label}: ${id || "<empty>"}`);
  }
  return id;
}

function normalizeSafeRef(value, label, pattern) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref || ref.startsWith("/") || ref.split("/").includes("..") || !pattern.test(ref)) {
    throw new Error(`Invalid Secure Mesh ACP archive release proof ${label}: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeTokens(value, label) {
  const tokens = Array.isArray(value) ? value.map((item) => String(item || "").trim()).filter(Boolean) : [];
  if (tokens.length === 0) {
    throw new Error(`Secure Mesh ACP archive release proof config must define ${label}`);
  }
  for (const token of tokens) {
    assertNoLeak(token, `secure mesh ACP archive release proof ${label}`);
  }
  return Object.freeze(tokens);
}

function normalizeSourceChecks(value) {
  const checks = Array.isArray(value) ? value : [];
  return Object.freeze(checks.map((check, index) => {
    const refs = Array.isArray(check.files) ? check.files : [check.file];
    if (refs.length === 0) {
      throw new Error(`Secure Mesh ACP archive release proof config must define source check ${index + 1} files`);
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
      throw new Error(`Invalid Secure Mesh ACP archive release proof native test filter: ${filter}`);
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
    throw new Error("Secure Mesh ACP archive release proof config schema version mismatch");
  }
  assertNoLeak(payload, "secure mesh ACP archive release proof config");
  cachedConfig = Object.freeze({
    configRef,
    schemaVersion,
    reportOutput: normalizeSafeRef(
      payload.reportOutput,
      "reportOutput",
      /^build\/reports\/[\w./-]+\.json$/u
    ),
    sourceChecks: normalizeSourceChecks(payload.sourceChecks),
    nativeTestFilters: normalizeNativeTestFilters(payload.nativeTestFilters)
  });
  return cachedConfig;
}

export async function loadSecureMeshAcpArchiveReleaseProofConfig() {
  return loadRawConfig();
}
