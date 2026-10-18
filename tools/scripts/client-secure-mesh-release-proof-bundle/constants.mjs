export const VERIFIER_REF = "tools/scripts/client-secure-mesh-release-proof-bundle.mjs";

export const maxReportFutureSkewSeconds = 5 * 60;

export const leakPatterns = Object.freeze([
  ["local_path", /\/Users\/|\/private\/|\/var\/folders\/|[A-Za-z]:\\/u],
  ["bearer", /Bearer\s+(?!\[redacted\])\S+/u],
  ["token", /\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]{8,}\b/u],
  ["pem_material", /-----BEGIN|-----END/u],
  ["adb_public_key", /AAAA[0-9A-Za-z+/]{40,}={0,2}/u],
  ["labeled_device_identifier", /\b(?:UDID|ECID|Serial(?:Number)?|DeviceIdentifier)\s*[:=]\s*[A-Za-z0-9-]{8,}\b/u],
  ["raw_secret_value", /"(?:privateKeyBase64url|signingKeyBase64url|signedPrekeyPrivateKeyBase64url|oneTimePrekeyPrivateKeyBase64url|pairingSecretBase64url|sessionKey|rootKey|chainKey|messageKey)"\s*:\s*"[^"]{8,}"/u],
  ["file_url", /file:\/\/\//u]
]);

export const rustCryptoSchemaVersion =
  "licomesh.secure-mesh.pairwise-content-audit-report.v1";
export const platformCryptoSchemaVersion =
  "licomesh.secure-mesh.platform-secret-store-matrix-report.v2";
export const androidPlatformCryptoSchemaVersion =
  "licomesh.secure-mesh.android-platform-crypto-acceptance.v1";
