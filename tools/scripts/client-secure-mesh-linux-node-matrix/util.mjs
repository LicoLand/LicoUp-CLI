import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, lstatSync } from "node:fs";
import path from "node:path";
import { sha256File as stableSha256File } from "../lib/client-release-artifact-digest.mjs";
import { LINUX_TAR_RESOURCE_LIMITS } from "../lib/linux-tar-resource-bounds.mjs";
import { assert } from "./assert.mjs";

export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().map((key) =>
      `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export function randomMarker() {
  return `protected-${randomBytes(18).toString("base64url")}`;
}

export function sha256File(file) {
  return stableSha256File(file);
}


export function requiredFile(value, label) {
  const file = path.resolve(String(value || ""));
  const info = value && existsSync(file)
    ? lstatSync(file, { throwIfNoEntry: false })
    : undefined;
  assert(info?.isFile() === true && info.isSymbolicLink() === false,
    `${label} is missing or unsafe`);
  return file;
}

export function extractArchive(archive, destination) {
  const result = spawnSync("/usr/bin/tar", ["-xzf", archive, "-C", destination], {
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
    timeout: LINUX_TAR_RESOURCE_LIMITS.extractTimeoutMs,
  });
  assert(result.status === 0 && result.error?.code !== "ETIMEDOUT",
    "Linux node archive extraction failed or timed out");
}
