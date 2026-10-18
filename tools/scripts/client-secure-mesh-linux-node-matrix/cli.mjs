export function parseArgs(args) {
  const parsed = { selfTest: false };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--self-test") {
      parsed.selfTest = true;
      continue;
    }
    if (!arg.startsWith("--")) throw new Error("Unknown Linux node matrix argument");
    const [rawKey, inline] = arg.slice(2).split("=", 2);
    const key = rawKey.replace(/-([a-z])/gu, (_, letter) => letter.toUpperCase());
    parsed[key] = inline ?? args[index + 1] ?? "";
    if (inline === undefined) index += 1;
  }
  return parsed;
}
