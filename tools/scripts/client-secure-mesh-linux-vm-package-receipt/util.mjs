import { spawnSync } from "node:child_process";
import { existsSync, lstatSync } from "node:fs";
import path from "node:path";
import { sha256File as stableSha256File } from "../lib/client-release-artifact-digest.mjs";

export function assert(condition, message) {
  if (!condition) throw new Error(message);
}

export function assertArm64Executable(file) {
  const result = spawnSync("file", ["-b", file], { encoding: "utf8" });
  assert(result.status === 0 && /(?:ARM aarch64|ARM64)/iu.test(String(result.stdout || "")),
    "Installed Linux executable is not ARM64");
}

export function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    ...options
  });
  assert(result.status === 0, "Linux VM package command failed");
  return result;
}

export function runJson(command, args, env) {
  const result = run(command, args, { env });
  try {
    return JSON.parse(String(result.stdout || ""));
  } catch {
    throw new Error("Linux VM package command returned invalid JSON");
  }
}

export function assertTargetScan(scan) {
  assert(scan?.ok === true && Array.isArray(scan.candidates), "Linux CLI target scan failed");
  const targets = new Set(scan.candidates.map((candidate) => candidate?.target).filter(Boolean));
  for (const target of ["openclaw", "codex", "opencode"]) {
    assert(targets.has(target), "Linux CLI target scan omitted a required adapter");
  }
}

export async function waitFor(probe, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = probe();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, 125));
  }
  throw new Error(`${label} timed out`);
}

export function sha256File(file) {
  return stableSha256File(file);
}

export function requiredFile(value, label) {
  const resolved = path.resolve(String(value || ""));
  const info = value && existsSync(resolved)
    ? lstatSync(resolved, { throwIfNoEntry: false })
    : undefined;
  assert(info?.isFile() === true && info.isSymbolicLink() === false,
    `${label} is missing or unsafe`);
  return resolved;
}

export function decodeCanonicalBase64(value, label) {
  const encoded = String(value || "").trim();
  assert(encoded.length > 0 && encoded.length <= 16 * 1024 &&
    /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(encoded),
  `${label} is not canonical base64`);
  const bytes = Buffer.from(encoded, "base64");
  assert(bytes.length > 0 && bytes.toString("base64") === encoded,
    `${label} is not canonical base64`);
  return bytes;
}
