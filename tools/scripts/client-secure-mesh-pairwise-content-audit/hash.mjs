import crypto from "node:crypto";

export function sha256Text(value) {
  return `sha256:${crypto.createHash("sha256").update(String(value), "utf8").digest("hex")}`;
}

export function sha256Json(value) {
  return sha256Text(JSON.stringify(value));
}
