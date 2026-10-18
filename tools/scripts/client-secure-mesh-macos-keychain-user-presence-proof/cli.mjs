export function parseArgs(args) {
  const booleanOptions = new Set(["interactive", "selfTest"]);
  const parsed = {};
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (!arg.startsWith("--")) continue;
    const [rawKey, inlineValue] = arg.slice(2).split("=", 2);
    const key = rawKey.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
    if (inlineValue !== undefined) {
      parsed[key] = inlineValue;
    } else if (booleanOptions.has(key)) {
      parsed[key] = true;
    } else {
      parsed[key] = args[index + 1] ?? "";
      index += 1;
    }
  }
  return parsed;
}
