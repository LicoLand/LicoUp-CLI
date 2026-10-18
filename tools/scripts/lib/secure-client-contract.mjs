import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import * as releaseContract from "./secure-client-mesh-release-contract.mjs";

export * from "./secure-client-mesh-release-contract.mjs";

export async function loadSecureClientContract() {
  return Object.freeze({ ...releaseContract });
}

const DIGEST_PATTERN = /^sha256:[a-f0-9]{64}$/u;
const MAX_EXTERNAL_JSON_BYTES = 4 * 1024 * 1024;

export async function loadDigestBoundJsonInput({
  filePath,
  expectedDigest,
  label = "external report",
} = {}) {
  const explicitPath = String(filePath || "").trim();
  const digest = String(expectedDigest || "").trim();
  if (!explicitPath) throw new Error(`${label} path must be provided explicitly`);
  if (!DIGEST_PATTERN.test(digest)) {
    throw new Error(`${label} digest must be an explicit sha256 value`);
  }
  let handle;
  let bytes;
  try {
    handle = await fs.open(path.resolve(explicitPath), "r");
    const stat = await handle.stat();
    if (!stat.isFile() || stat.size > MAX_EXTERNAL_JSON_BYTES) {
      throw new Error("unsafe external JSON input");
    }
    bytes = await handle.readFile();
  } catch {
    throw new Error(`${label} could not be read`);
  } finally {
    await handle?.close().catch(() => {});
  }
  const actualDigest = `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
  if (actualDigest !== digest) throw new Error(`${label} digest mismatch`);
  try {
    return Object.freeze({
      digest: actualDigest,
      value: JSON.parse(bytes.toString("utf8")),
    });
  } catch {
    throw new Error(`${label} must contain valid JSON`);
  }
}
