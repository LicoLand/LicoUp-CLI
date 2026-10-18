import { createHash } from "node:crypto";
import { assert } from "./assert.mjs";
import { canonicalJson } from "./util.mjs";

export function publicIdentityDigest(secureMesh) {
  assert(secureMesh && typeof secureMesh === "object", "Linux public endpoint identity is missing");
  const identity = secureMesh.endpointIdentity || secureMesh.deviceIdentity || {
    endpointId: secureMesh.endpointId,
    identityPublicKey: secureMesh.identityPublicKeyBase64url,
    signingPublicKey: secureMesh.signingPublicKeyBase64url,
    rotationEpoch: secureMesh.rotationEpoch
  };
  const serialized = canonicalJson(identity);
  assert(serialized.length > 32, "Linux public endpoint identity is incomplete");
  return createHash("sha256").update(serialized).digest("hex");
}
