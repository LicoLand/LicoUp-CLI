import { createHash } from "node:crypto";

const SHA256_DIGEST = /^sha256:[a-f0-9]{64}$/u;
const SAFE_AGENT_ID = /^[a-z0-9][a-z0-9-]{0,63}$/u;

function requireValue(condition, code) {
  if (!condition) throw new Error(code);
}

export function nativeContinuityDigest(nativeSessionId) {
  const value = String(nativeSessionId || "");
  requireValue(value.length > 0 && value.length <= 512, "native_continuity_id_invalid");
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}

export function productContinuityBindingDigest({
  artifactDigest,
  invocationChallengeDigest,
  agentId,
  model,
  nativeDigest,
}) {
  requireValue(SHA256_DIGEST.test(artifactDigest), "artifact_digest_invalid");
  requireValue(
    SHA256_DIGEST.test(invocationChallengeDigest),
    "invocation_challenge_digest_invalid",
  );
  requireValue(SAFE_AGENT_ID.test(agentId), "agent_id_invalid");
  requireValue(typeof model === "string" && model.length > 0 && model.length <= 80,
    "model_binding_invalid");
  requireValue(SHA256_DIGEST.test(nativeDigest), "native_continuity_digest_invalid");
  return `sha256:${createHash("sha256")
    .update(artifactDigest)
    .update("\0")
    .update(invocationChallengeDigest)
    .update("\0")
    .update(agentId)
    .update("\0")
    .update(model)
    .update("\0")
    .update(nativeDigest)
    .digest("hex")}`;
}
