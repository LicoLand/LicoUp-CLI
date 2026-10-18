import { leakPatterns } from "./constants.mjs";

export function assertNoLeak(value, label) {
  const text = JSON.stringify(value);
  for (const [kind, pattern] of leakPatterns) {
    if (pattern.test(text)) {
      throw new Error(
        `${label} contains sensitive data: ${kind} at ${findLeakPath(value, pattern)}`
      );
    }
  }
}

export function findLeakPath(value, pattern, pathPrefix = "$") {
  if (typeof value === "string") {
    pattern.lastIndex = 0;
    return pattern.test(value) ? pathPrefix : "<unknown>";
  }
  if (Array.isArray(value)) {
    for (let index = 0; index < value.length; index += 1) {
      const found = findLeakPath(value[index], pattern, `${pathPrefix}[${index}]`);
      if (found !== "<unknown>") return found;
    }
    return "<unknown>";
  }
  if (value && typeof value === "object") {
    for (const [key, nested] of Object.entries(value)) {
      pattern.lastIndex = 0;
      if (!pattern.test(JSON.stringify({ [key]: nested }))) continue;
      const found = findLeakPath(nested, pattern, `${pathPrefix}.${key}`);
      return found !== "<unknown>" ? found : `${pathPrefix}.${key}`;
    }
  }
  return "<unknown>";
}

export function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^\s"')\]]+/gu, "<local-path>")
    .replace(/\/home\/[^\s"')\]]+/gu, "<local-path>")
    .replace(/\/private\/[^\s"')\]]+/gu, "<local-path>")
    .replace(/\/var\/folders\/[^\s"')\]]+/gu, "<local-path>")
    .replace(/[A-Za-z]:\\[^\s"')\]]+/gu, "<local-path>")
    .replace(/\/Users\//gu, "<local-path>/")
    .replace(/\/private\//gu, "<local-path>/")
    .replace(/\/var\/folders\//gu, "<local-path>/")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}
