import { assert } from "./util.mjs";

export function parseArgs(args) {
  const parsed = {};
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith("--")) throw new Error("Unknown Linux VM package receipt argument");
    const [rawKey, inline] = arg.slice(2).split("=", 2);
    const key = rawKey.replace(/-([a-z])/gu, (_, letter) => letter.toUpperCase());
    parsed[key] = inline ?? args[index + 1] ?? "";
    if (inline === undefined) index += 1;
  }
  requiredOptionFrom(parsed, "archive");
  requiredOptionFrom(parsed, "verificationManifest");
  requiredOptionFrom(parsed, "expectedSourceDigest");
  requiredOptionFrom(parsed, "report");
  assert(/^sha256:[a-f0-9]{64}$/u.test(parsed.expectedSourceDigest),
    "Linux VM package receipt source digest is invalid");
  return parsed;
}

export function requiredOptionFrom(parsed, name) {
  if (!String(parsed[name] || "").trim()) {
    throw new Error("Linux VM package receipt option is missing");
  }
}

export function requiredOption(options, name) {
  const value = String(options[name] || "").trim();
  assert(value, `Linux VM package receipt requires --${name.replace(/[A-Z]/g, (letter) =>
    `-${letter.toLowerCase()}`)}`);
  return value;
}
