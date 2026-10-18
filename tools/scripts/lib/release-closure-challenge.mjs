import { createHash, randomBytes } from "node:crypto";

export const RELEASE_CLOSURE_CHALLENGE_ENV = "LICO_CLIENT_RELEASE_CLOSURE_CHALLENGE";
export const RELEASE_CLOSURE_STARTED_AT_ENV = "LICO_CLIENT_RELEASE_CLOSURE_STARTED_AT";
export const RELEASE_INVOCATION_NONCE_ENV = "LICO_CLIENT_RELEASE_INVOCATION_NONCE";

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

export function validateReleaseClosureChallenge(value) {
  const challenge = String(value || "").trim();
  requireValue(/^[A-Za-z0-9_-]{43}$/u.test(challenge),
    "release closure challenge is invalid");
  const decoded = Buffer.from(challenge, "base64url");
  requireValue(decoded.length === 32 && decoded.toString("base64url") === challenge,
    "release closure challenge encoding is invalid");
  return challenge;
}

export function createReleaseClosureChallenge() {
  return validateReleaseClosureChallenge(randomBytes(32).toString("base64url"));
}

export function releaseClosureChallengeDigest(value) {
  const challenge = validateReleaseClosureChallenge(value);
  return `sha256:${createHash("sha256").update(challenge, "utf8").digest("hex")}`;
}

export function requiredReleaseClosureChallenge(env = process.env) {
  return validateReleaseClosureChallenge(env[RELEASE_CLOSURE_CHALLENGE_ENV]);
}

export function requiredReleaseClosureStartedAt(env = process.env) {
  const value = String(env[RELEASE_CLOSURE_STARTED_AT_ENV] || "").trim();
  const milliseconds = Date.parse(value);
  requireValue(Number.isFinite(milliseconds), "release closure start time is invalid");
  return { value: new Date(milliseconds).toISOString(), milliseconds };
}

export function releaseClosureEnvironment(challenge, startedAt = new Date()) {
  const validated = validateReleaseClosureChallenge(challenge);
  const date = startedAt instanceof Date ? startedAt : new Date(startedAt);
  requireValue(Number.isFinite(date.getTime()), "release closure start time is invalid");
  return {
    [RELEASE_CLOSURE_CHALLENGE_ENV]: validated,
    [RELEASE_CLOSURE_STARTED_AT_ENV]: date.toISOString(),
  };
}

export function createReleaseInvocationNonce() {
  return validateReleaseClosureChallenge(randomBytes(32).toString("base64url"));
}

export function requiredReleaseInvocationNonce(env = process.env) {
  return validateReleaseClosureChallenge(env[RELEASE_INVOCATION_NONCE_ENV]);
}

export function releaseInvocationNonceDigest(value) {
  return releaseClosureChallengeDigest(value);
}

export function releaseInvocationEnvironment(nonce) {
  return {
    [RELEASE_INVOCATION_NONCE_ENV]: validateReleaseClosureChallenge(nonce),
  };
}

export function optionalReleaseInvocationBinding(env = process.env) {
  const challenge = String(env[RELEASE_CLOSURE_CHALLENGE_ENV] || "").trim();
  const nonce = String(env[RELEASE_INVOCATION_NONCE_ENV] || "").trim();
  if (!challenge && !nonce) return Object.freeze({});
  if (!challenge || !nonce) {
    throw new Error("release invocation binding is incomplete");
  }
  return Object.freeze({
    closureChallengeDigest: releaseClosureChallengeDigest(
      requiredReleaseClosureChallenge(env),
    ),
    invocationNonceDigest: releaseInvocationNonceDigest(
      requiredReleaseInvocationNonce(env),
    ),
  });
}
