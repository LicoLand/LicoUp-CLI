export function dedupeRemainingGates(gates) {
  const seen = new Set();
  return (Array.isArray(gates) ? gates : [])
    .map((gate) => String(gate || "").trim())
    .filter((gate) => {
      if (!gate || seen.has(gate)) {
        return false;
      }
      seen.add(gate);
      return true;
    });
}

export function stableStringList(value) {
  return Array.from(
    new Set(
      (Array.isArray(value) ? value : [])
        .map((item) => String(item || "").trim())
        .filter(Boolean)
    )
  ).sort();
}

export function summarizeOutput(value = "") {
  return String(value || "")
    .trim()
    .split(/\r?\n/)
    .filter(Boolean)
    .slice(0, 4)
    .join("\n");
}

export function reportRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

export function sanitizeError(error) {
  return String(error instanceof Error ? error.message : error)
    .replace(/\/Users\/[^/\s"]+/gu, "<user-home>")
    .replace(/\/private\/var\/folders\/[^\s"]+/gu, "<local-temp>")
    .replace(/[A-Za-z]:\\[^\s"]+/gu, "<local-path>")
    .replace(/file:\/\/\/[^\s"]+/gu, "file:///<redacted>")
    .replace(/Bearer\s+\S+/gu, "Bearer [redacted]")
    .replace(/\b(?:gh[pousr]_|github_pat_|sk-)[A-Za-z0-9._-]+\b/gu, "[redacted]")
    .slice(0, 1200);
}
