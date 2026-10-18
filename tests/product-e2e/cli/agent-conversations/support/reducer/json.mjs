import { createHash } from "node:crypto";
import { fail } from "./errors.mjs";

export function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function normalizedKey(key) {
  return key.toLowerCase().replaceAll(/[^a-z0-9]/g, "");
}
export function assertOnlyFields(value, allowedFields, errorCode) {
  if (!isPlainObject(value)) {
    fail(errorCode);
  }
  for (const key of Object.keys(value)) {
    if (!allowedFields.has(key)) {
      fail(errorCode);
    }
  }
}

export function canonicalJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (isPlainObject(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function digest(value) {
  return `sha256:${createHash("sha256").update(canonicalJson(value)).digest("hex")}`;
}

export function parseJson(text, errorCode) {
  try {
    return JSON.parse(text);
  } catch {
    fail(errorCode);
  }
}
