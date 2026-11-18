import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../../../../",
);

export const TARGET = "continuity_scenarios";
export const CASE_MARKER = "CONTINUITY_SCENARIO_CASE:";
export const ORACLE_MARKER = "CONTINUITY_SCENARIO_ORACLE:";

export function fixturesRoot() {
  return path.join(repoRoot, "tests/fixtures/continuous-assistant/scenarios");
}

export function loadCatalog() {
  const catalog = JSON.parse(
    readFileSync(path.join(fixturesRoot(), "catalog.json"), "utf8"),
  );
  if (!Array.isArray(catalog.cases) || catalog.cases.length === 0) {
    throw new Error("scenario catalog has zero cases");
  }
  return catalog;
}

export function requiredIds(catalog = loadCatalog()) {
  return catalog.cases.map((item) => item.id);
}
