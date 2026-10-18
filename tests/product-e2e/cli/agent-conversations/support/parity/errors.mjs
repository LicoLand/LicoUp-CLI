import { createHash } from "node:crypto";

export class AcceptanceError extends Error {
  constructor(code, details = {}) {
    super(code);
    this.code = safeErrorCode(code);
    this.details = details;
  }
}

export function safeErrorCode(value) {
  const normalized = String(value || "unexpected_failure").toLowerCase();
  return /^[a-z0-9][a-z0-9_-]{0,95}$/u.test(normalized)
    ? normalized
    : "unexpected_failure";
}

export function requireFact(condition, code, details = {}) {
  if (!condition) {
    throw new AcceptanceError(code, details);
  }
}

export function digest(value) {
  return createHash("sha256").update(stableJson(value)).digest("hex");
}

export function stableJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(stableJson).join(",")}]`;
  }
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}
