import { spawnSync } from "node:child_process";
import { stableReadFile } from "../lib/client-release-artifact-digest.mjs";
import { repoRoot } from "./constants.mjs";

export function fail(code) {
  throw new Error(code);
}

export function requireValue(condition, code) {
  if (!condition) fail(code);
}

export function text(value) {
  return String(value || "").trim();
}

export function readJsonStable(filePath) {
  return JSON.parse(stableReadFile(filePath, {
    maxBytes: 2 * 1024 * 1024,
  }).toString("utf8"));
}

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

export function run(command, args, options = {}) {
  return spawnSync(command, args, {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: "pipe",
    timeout: 30_000,
    maxBuffer: 16 * 1024 * 1024,
    ...options,
  });
}

export function requireSuccess(result, code) {
  requireValue(result.status === 0, code);
  return result;
}

export function wait(milliseconds) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}
