import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
export const VERIFIER_REF = "tools/scripts/client-secure-mesh-pairwise-content-audit.mjs";
export const PRODUCER_AUTHORITY_REF =
  "tools/scripts/client-secure-mesh-pairwise-content-audit/run.mjs";

export const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["private_material", /-----BEGIN|privateKey|sessionKey|rootKey|chainKey|messageKey|"(?:shared_secret|root_key|chain_key|message_key|identity_secret|prekey_secret)"\s*:/u]
]);
