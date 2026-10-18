import os from "node:os";

const MAX_ERROR_CHARS = 1600;

/**
 * Sanitize an error for inclusion in evidence reports. Redacts credentials,
 * paths, tokens, keys, and other sensitive material using allowlisted patterns.
 *
 * Every evidence-producing script must use this shared utility so that
 * redaction coverage stays consistent and new patterns propagate to all
 * callers.
 */
export function sanitizeError(error) {
  const text = error instanceof Error ? error.message : String(error);
  return text
    // OAuth / bearer tokens.
    .replace(/Bearer\s+[A-Za-z0-9._~-]+/g, "Bearer [redacted]")
    // Structured secret key-value pairs (pcToken, mobileToken, etc.).
    .replace(
      /"?(pcToken|mobileToken|pairingCode|privateKeyBase64url|pairingSecretBase64url|e2eePairingSecret)"?\s*[:=]\s*"[^"]+"/gi,
      "$1:[redacted]",
    )
    // Home directory.
    .replace(new RegExp(escapeRegExp(os.homedir()), "g"), "~")
    // Absolute user paths.
    .replace(/\/Users\/[^\s"']+/g, "[user-path-redacted]")
    .replace(/\/home\/[^\s"']+/g, "[home-path-redacted]")
    // Windows absolute paths.
    .replace(/[A-Za-z]:\\(?:[^\\\s"'`]+\\)*[^\\\s"'`]*/g, "[local-path-redacted]")
    // UUIDs.
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi, "[uuid-redacted]")
    // GitHub tokens.
    .replace(/gh[pousr]_[A-Za-z0-9_]+/g, "[github-token-redacted]")
    // OpenAI / generic API keys.
    .replace(/sk-[A-Za-z0-9_-]+/g, "[api-key-redacted]")
    // PEM-encoded private keys (RSA, EC, Ed25519, etc.).
    .replace(
      /-----BEGIN [A-Z ]+ PRIVATE KEY-----[A-Za-z0-9+/=\s]+-----END [A-Z ]+ PRIVATE KEY-----/g,
      "[pem-redacted]",
    )
    // file:// URLs.
    .replace(/file:\/\/\/[^\s"']+/g, "[file-url-redacted]")
    // JWT tokens (three base64url segments).
    .replace(
      /eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g,
      "[jwt-redacted]",
    )
    // macOS temporary / private directories.
    .replace(/\/var\/folders\/[^\s"']+/g, "[temp-path-redacted]")
    .replace(/\/private\/var\/folders\/[^\s"']+/g, "[temp-path-redacted]")
    // Standard temp directories.
    .replace(/\/tmp\/[^\s"']+/g, "[temp-path-redacted]")
    // ADB device identifiers.
    .replace(/adb\s+-s\s+\S+/g, "adb -s [redacted]")
    // Trace / span identifiers.
    .replace(/trace_[A-Za-z0-9-]+/g, "trace_[redacted]")
    // E2EE plaintext canaries (hex, base64, unicode-escaped).
    .replace(
      /native-e2e-plaintext-canary-[0-9a-fA-F-]+/gu,
      "native-e2e-plaintext-canary:[redacted]",
    )
    .replace(
      /bmF0aXZlLWUyZS1wbGFpbnRleHQtY2FuYXJ5[A-Za-z0-9+/_=-]*/gu,
      "native-e2e-plaintext-canary:[redacted]",
    )
    .replace(
      /6e61746976652d6532652d706c61696e746578742d63616e617279[0-9a-fA-F]*/gu,
      "native-e2e-plaintext-canary:[redacted]",
    )
    .replace(
      /(?:\\u[0-9a-fA-F]{4}){24,}/gu,
      "native-e2e-plaintext-canary:[redacted]",
    )
    // Length bound.
    .slice(0, MAX_ERROR_CHARS);
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
