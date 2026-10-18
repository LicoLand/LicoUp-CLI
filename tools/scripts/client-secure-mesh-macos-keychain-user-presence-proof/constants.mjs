import path from "node:path";
import { fileURLToPath } from "node:url";
import { loadSecureMeshPhysicalEvidenceConfig } from "../lib/secure-mesh-physical-evidence-config.mjs";

export const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
export const VERIFIER_REF = "tools/scripts/client-secure-mesh-macos-keychain-user-presence-proof.mjs";
export const reportSchemaVersion = "licomesh.secure-mesh.macos-adaptive-custody-proof.v2";

export const leakPatterns = Object.freeze([
  ["local_path", /(?:^|["\s])(?:\/Users\/|\/private\/|\/var\/folders\/|\/tmp\/|[A-Za-z]:\\)/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u],
]);

export async function loadPhysicalReportDefaults() {
  const physicalEvidenceConfig = await loadSecureMeshPhysicalEvidenceConfig();
  const physicalReportRefs = physicalEvidenceConfig.linkedReports;
  return {
    physicalEvidenceConfig,
    physicalReportRefs,
    defaultReportPath: physicalReportRefs.macosUserPresenceProof,
  };
}
