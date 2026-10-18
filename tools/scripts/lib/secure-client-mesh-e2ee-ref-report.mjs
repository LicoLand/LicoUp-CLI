import fs from "node:fs/promises";

const scopeConfigUrl = new URL("../config/secure-mesh-e2ee-report-scope.json", import.meta.url);
const scopeClaimPattern = /^[A-Za-z][A-Za-z0-9]*Claims$/u;
const authorityPattern = /^[a-z][a-z0-9-]*$/u;
let cachedScopeConfig;

async function loadScopeConfig() {
  if (cachedScopeConfig) {
    return cachedScopeConfig;
  }
  const payload = JSON.parse(await fs.readFile(scopeConfigUrl, "utf8"));
  cachedScopeConfig = normalizeScopeConfig(payload);
  return cachedScopeConfig;
}

function normalizeScopeConfig(payload = {}) {
  if (payload?.schemaVersion !== "licomesh.secure-mesh.e2ee-report-scope-config.v2") {
    throw new Error("Secure Client Mesh scope config schema version mismatch");
  }
  if (!payload.reports || typeof payload.reports !== "object" || Array.isArray(payload.reports)) {
    throw new Error("Secure Client Mesh scope config must contain reports");
  }
  normalizeFreshnessSeconds(payload.scopeEvidenceFreshnessSeconds);
  return payload;
}

function normalizeReportRef(value) {
  const ref = String(value || "").trim().replaceAll("\\", "/");
  if (!ref || ref.startsWith("/") || ref.startsWith("file:") || /^https?:\/\//iu.test(ref) || ref.split("/").includes("..")) {
    throw new Error(`Invalid Secure Client Mesh scope report ref: ${ref || "<empty>"}`);
  }
  return ref;
}

function normalizeClaim(value) {
  const claim = String(value || "").trim();
  if (!scopeClaimPattern.test(claim)) {
    throw new Error(`Invalid Secure Client Mesh scope claim: ${claim || "<empty>"}`);
  }
  return claim;
}

function normalizeCheckedAt(value) {
  const checkedAt = String(value || "").trim();
  if (!Number.isFinite(Date.parse(checkedAt))) {
    throw new Error("Secure Client Mesh scope evidence requires a parseable checkedAt timestamp");
  }
  return checkedAt;
}

function normalizeFreshUntil(value) {
  const freshUntil = String(value || "").trim();
  if (!Number.isFinite(Date.parse(freshUntil))) {
    throw new Error("Secure Client Mesh scope evidence requires a parseable freshUntil timestamp");
  }
  return freshUntil;
}

function normalizeFreshnessSeconds(value) {
  const seconds = Number(value);
  if (!Number.isInteger(seconds) || seconds <= 0 || seconds > 31_536_000) {
    throw new Error("Secure Client Mesh scope config requires scopeEvidenceFreshnessSeconds between 1 and 31536000");
  }
  return seconds;
}

function addSecondsToIsoTimestamp(value, seconds) {
  const timestampMs = Date.parse(value);
  if (!Number.isFinite(timestampMs)) {
    throw new Error("Secure Client Mesh scope evidence timestamp is invalid");
  }
  return new Date(timestampMs + seconds * 1000).toISOString();
}

function normalizeEvidenceType(value) {
  const evidenceType = String(value || "").trim();
  if (!evidenceType) {
    throw new Error("Secure Client Mesh scope evidence requires a non-empty evidenceType");
  }
  return evidenceType;
}

function normalizeAuthority(value, label = "Secure Client Mesh scope authority") {
  const authority = String(value || "").trim();
  if (!authorityPattern.test(authority)) {
    throw new Error(`${label} is invalid: ${authority || "<empty>"}`);
  }
  return authority;
}

function contractFunction(contract, name) {
  const value = contract?.[name];
  if (typeof value !== "function") {
    throw new Error(`Client-pinned Secure Client Mesh contract is missing ${name}`);
  }
  return value;
}

function contractBlockers(contract) {
  const blockers = Array.isArray(contract?.SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS)
    ? contract.SECURE_CLIENT_MESH_PRODUCTION_BLOCKERS
    : [];
  if (blockers.length === 0) {
    throw new Error("Client-pinned Secure Client Mesh contract does not expose production blockers");
  }
  return blockers;
}

function defaultExternalClientAuthority(contract) {
  const authority = String(contract?.SECURE_CLIENT_MESH_EVIDENCE_AUTHORITY?.externalClient || "").trim();
  if (!authority) {
    throw new Error("Client-pinned Secure Client Mesh contract does not expose external-client authority");
  }
  return authority;
}

function scopeClaimAuthorities(contract, claim) {
  return contractFunction(contract, "requiredSecureClientMeshEvidenceScopeClaimAuthorities")(claim)
    .map((authority) => String(authority || "").trim())
    .filter(Boolean);
}

function normalizeClaimAuthorities(value = {}, configuredClaims = null, label = "Secure Client Mesh scope claimAuthorities") {
  const record = value && typeof value === "object" && !Array.isArray(value) ? value : {};
  const configuredClaimSet = Array.isArray(configuredClaims) ? new Set(configuredClaims) : null;
  const entries = Object.entries(record);
  for (const [claim] of entries) {
    const normalizedClaim = normalizeClaim(claim);
    if (configuredClaimSet && !configuredClaimSet.has(normalizedClaim)) {
      throw new Error(`${label} contains authority for unconfigured claim ${normalizedClaim}`);
    }
  }
  return Object.fromEntries(entries.map(([claim, authority]) => [
    normalizeClaim(claim),
    normalizeAuthority(authority, `${label}.${claim}`)
  ]));
}

function claimAuthorityFor({
  contract,
  claim,
  entryClaimAuthorities = {},
  configClaimAuthorities = {},
  entryAuthority = "",
  configAuthority = ""
} = {}) {
  const authority = entryClaimAuthorities[claim] ||
    configClaimAuthorities[claim] ||
    entryAuthority ||
    configAuthority ||
    defaultExternalClientAuthority(contract);
  return normalizeAuthority(authority, `Secure Client Mesh scope authority for ${claim}`);
}

export async function createSecureClientMeshE2eeRefReportScope({
  contract,
  reportRef,
  blocker,
  checkedAt,
  scopeConfig
} = {}) {
  const config = scopeConfig ? normalizeScopeConfig(scopeConfig) : await loadScopeConfig();
  const normalizedReportRef = normalizeReportRef(reportRef);
  const entry = config.reports[normalizedReportRef];
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) {
    throw new Error(`Secure Client Mesh scope config missing report ${normalizedReportRef}`);
  }

  const canonicalBlocker = String(blocker || "").trim();
  const configuredBlocker = String(entry.blocker || "").trim();
  if (!contractBlockers(contract).includes(canonicalBlocker)) {
    throw new Error(`Secure Client Mesh blocker is not contract-defined: ${canonicalBlocker || "<empty>"}`);
  }
  if (configuredBlocker !== canonicalBlocker) {
    throw new Error(`Secure Client Mesh scope config blocker mismatch for ${normalizedReportRef}`);
  }

  const requiredClaims = contractFunction(contract, "requiredSecureClientMeshEvidenceRefScopeClaims")(canonicalBlocker)
    .map(normalizeClaim);
  const configuredClaims = [...new Set([].concat(entry.claims || []).map(normalizeClaim))];
  if (configuredClaims.length === 0) {
    throw new Error(`Secure Client Mesh scope config has no claims for ${normalizedReportRef}`);
  }
  const missingConfiguredClaims = requiredClaims.filter((claim) => !configuredClaims.includes(claim));
  if (missingConfiguredClaims.length > 0) {
    throw new Error(`Secure Client Mesh scope config missing required claims for ${canonicalBlocker}: ${missingConfiguredClaims.join(", ")}`);
  }
  for (const claim of configuredClaims) {
    if (!requiredClaims.includes(claim)) {
      throw new Error(`Secure Client Mesh scope claim ${claim} is not required for ${canonicalBlocker}`);
    }
  }

  const entryAuthority = entry.authority
    ? normalizeAuthority(entry.authority, `Secure Client Mesh scope authority for ${normalizedReportRef}`)
    : "";
  const configAuthority = config.authority
    ? normalizeAuthority(config.authority, "Secure Client Mesh default scope authority")
    : "";
  const entryClaimAuthorities = normalizeClaimAuthorities(
    entry.claimAuthorities,
    configuredClaims,
    `Secure Client Mesh scope claimAuthorities for ${normalizedReportRef}`
  );
  const configClaimAuthorities = normalizeClaimAuthorities(
    config.claimAuthorities,
    null,
    "Secure Client Mesh default scope claimAuthorities"
  );
  const claimAuthorities = Object.fromEntries(configuredClaims.map((claim) => [
    claim,
    claimAuthorityFor({
      contract,
      claim,
      entryClaimAuthorities,
      configClaimAuthorities,
      entryAuthority,
      configAuthority
    })
  ]));
  for (const claim of configuredClaims) {
    const acceptedAuthorities = scopeClaimAuthorities(contract, claim);
    const claimAuthority = claimAuthorities[claim];
    if (!acceptedAuthorities.includes(claimAuthority)) {
      throw new Error(`Secure Client Mesh scope authority ${claimAuthority} is not accepted for ${claim}`);
    }
  }

  const normalizedCheckedAt = normalizeCheckedAt(checkedAt);
  const freshnessSeconds = normalizeFreshnessSeconds(
    entry.scopeEvidenceFreshnessSeconds || config.scopeEvidenceFreshnessSeconds
  );
  const normalizedFreshUntil = normalizeFreshUntil(
    entry.freshUntil || config.freshUntil || addSecondsToIsoTimestamp(normalizedCheckedAt, freshnessSeconds)
  );
  const evidenceType = normalizeEvidenceType(entry.evidenceType);
  const requiredScopeClaimAuthorities = Object.fromEntries(requiredClaims.map((claim) => [
    claim,
    scopeClaimAuthorities(contract, claim)
  ]));

  return {
    scopeConfig: {
      ref: "tools/scripts/config/secure-mesh-e2ee-report-scope.json",
      reportRef: normalizedReportRef,
      schemaVersion: config.schemaVersion,
      authority: entryAuthority || configAuthority || defaultExternalClientAuthority(contract),
      claimAuthorities,
      blocker: canonicalBlocker
    },
	    requiredScopeClaims: requiredClaims,
	    requiredScopeClaimAuthorities,
	    freshUntil: normalizedFreshUntil,
	    scope: Object.fromEntries(configuredClaims.map((claim) => [claim, true])),
    scopeEvidence: Object.fromEntries(configuredClaims.map((claim) => [claim, {
      ok: true,
      redacted: true,
      authority: claimAuthorities[claim],
      evidenceType,
      checkedAt: normalizedCheckedAt,
      freshUntil: normalizedFreshUntil
    }]))
  };
}

export async function verifySecureClientMeshE2eeRefReportScopeSelfTest({ contract } = {}) {
  const checkedAt = "2026-01-01T00:00:00.000Z";
  const reportRef = "build/reports/secure-mesh-pairwise-content-crypto-audit.json";
  const blocker = "pairwise/content crypto audit";
  const baseConfig = {
    schemaVersion: "licomesh.secure-mesh.e2ee-report-scope-config.v2",
    authority: "external-client",
    scopeEvidenceFreshnessSeconds: 86400,
    reports: {
      [reportRef]: {
        blocker,
        claims: [
          "clientRuntimeClaims",
          "independentCryptoReviewClaims"
        ],
        evidenceType: "scope-helper-self-test",
        freshUntil: "2999-01-01T00:00:00.000Z",
        claimAuthorities: {
          clientRuntimeClaims: "external-client",
          independentCryptoReviewClaims: "independent-audit"
        }
      }
    }
  };
  const accepted = await createSecureClientMeshE2eeRefReportScope({
    contract,
    reportRef,
    blocker,
    checkedAt,
    scopeConfig: baseConfig
  });
  if (accepted.scopeEvidence?.clientRuntimeClaims?.authority !== "external-client" ||
    accepted.scopeEvidence?.independentCryptoReviewClaims?.authority !== "independent-audit" ||
    accepted.scopeEvidence?.clientRuntimeClaims?.freshUntil !== "2999-01-01T00:00:00.000Z" ||
    accepted.scopeEvidence?.independentCryptoReviewClaims?.freshUntil !== "2999-01-01T00:00:00.000Z") {
    throw new Error("Secure Client Mesh scope self-test did not preserve per-claim authorities");
  }
  let missingClaimRejected = false;
  try {
    await createSecureClientMeshE2eeRefReportScope({
      contract,
      reportRef,
      blocker,
      checkedAt,
      scopeConfig: {
        ...baseConfig,
        reports: {
          [reportRef]: {
            ...baseConfig.reports[reportRef],
            claims: ["clientRuntimeClaims"],
            claimAuthorities: {
              clientRuntimeClaims: "external-client"
            }
          }
        }
      }
    });
  } catch (error) {
    missingClaimRejected = /missing required claims/u.test(String(error?.message || error));
  }
  if (!missingClaimRejected) {
    throw new Error("Secure Client Mesh scope self-test accepted a config missing required claims");
  }
  let rejected = false;
  try {
    await createSecureClientMeshE2eeRefReportScope({
      contract,
      reportRef,
      blocker,
      checkedAt,
      scopeConfig: {
        ...baseConfig,
        reports: {
          [reportRef]: {
            ...baseConfig.reports[reportRef],
            claimAuthorities: {
              clientRuntimeClaims: "external-client",
              independentCryptoReviewClaims: "external-client"
            }
          }
        }
      }
    });
  } catch (error) {
    rejected = /not accepted for independentCryptoReviewClaims/u.test(String(error?.message || error));
  }
  if (!rejected) {
    throw new Error("Secure Client Mesh scope self-test accepted external-client authority for independentCryptoReviewClaims");
  }
  let schemaRejected = false;
  try {
    await createSecureClientMeshE2eeRefReportScope({
      contract,
      reportRef,
      blocker,
      checkedAt,
      scopeConfig: {
        ...baseConfig,
        schemaVersion: "invalid"
      }
    });
  } catch (error) {
    schemaRejected = /schema version mismatch/u.test(String(error?.message || error));
  }
  if (!schemaRejected) {
    throw new Error("Secure Client Mesh scope self-test accepted an injected config with an invalid schema");
  }
  return {
    ok: true,
    perClaimAuthoritiesAccepted: true,
    completeRequiredClaimSetEnforced: true,
    scopeEvidenceFreshUntilEmitted: true,
    independentAuditClaimRejectsExternalClient: true,
    injectedScopeConfigSchemaGuarded: true
  };
}
