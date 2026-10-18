import { pathsOverlap, requireFact } from "./canonical.mjs";
import { CLIENT_MODULE_CATALOG } from "../../regression/client-module-catalog.mjs";
import {
  changedPathsSince,
  normalizeRepoPath,
  selectModulesForChangedPaths,
} from "../../regression/client-module-selection.mjs";

/**
 * Bridge to the repository's existing regression selection.
 *
 * The catalog and its selection functions already own "which client regression
 * modules cover these changed paths". This tool does not add a second gate, a
 * second catalog, or a second execution platform: it consumes the existing
 * selection and attaches the resulting module ids and exact argv to graph work
 * items and impact reports. Running those commands stays with the existing
 * `client:regression` entry points.
 */

export const REGRESSION_SELECTION_KIND = "licoup-regression-selection.v1";

const snapshots = new WeakMap();

/** Memoized because the catalog is large and immutable per identity. */
export function catalogSnapshot(catalog = CLIENT_MODULE_CATALOG) {
  if (snapshots.has(catalog)) return snapshots.get(catalog);
  const snapshot = catalog.map((module) => ({
    id: module.id,
    kind: module.kind,
    summary: module.summary,
    lane: module.regression.lane,
    stage: module.regression.stage,
    environment: module.regression.environment,
    weight: module.regression.weight,
    batch_key: module.regression.batchKey,
    inputs: [...module.inputs],
    command: { program: module.command.program, args: [...module.command.args], cwd: module.command.cwd, timeout_ms: module.command.timeoutMs },
  }));
  snapshots.set(catalog, snapshot);
  return snapshot;
}

function commandKey(command) {
  return JSON.stringify([command.program, command.args, command.cwd]);
}

export function modulesCoveringPath(candidate, catalog = CLIENT_MODULE_CATALOG) {
  const normalized = normalizeRepoPath(candidate);
  return selectModulesForChangedPaths([normalized], catalog).map((module) => module.id);
}

export function selectForChangedPaths(paths, catalog = CLIENT_MODULE_CATALOG) {
  if (paths.length === 0) return { modules: [], unmappedPaths: [] };
  const normalized = [...new Set(paths.map((entry) => normalizeRepoPath(entry)))];
  const selected = selectModulesForChangedPaths(normalized, catalog);
  const coverageByPath = new Map(normalized.map((entry) => [entry, modulesCoveringPath(entry, catalog)]));
  return {
    modules: selected,
    coverageByPath,
    unmappedPaths: normalized.filter((entry) => coverageByPath.get(entry).length === 0).sort(),
  };
}

/**
 * Observed coverage: which catalog modules actually watch the repository path a
 * declared architecture module points at. A module with no observing entry is an
 * honest gap, reported rather than hidden.
 */
export function declaredModuleCoverage(graph, catalog = CLIENT_MODULE_CATALOG) {
  const snapshot = catalogSnapshot(catalog);
  const coverage = [];
  for (const module of graph.modules.values()) {
    const declaredPath = module.path;
    const observed = snapshot.filter((entry) => entry.inputs.some((input) => pathsOverlap(input, declaredPath)));
    coverage.push({
      module: module.id,
      declared_path: declaredPath,
      catalog_module_ids: observed.map((entry) => entry.id),
      covered: observed.length > 0,
    });
  }
  return coverage;
}

export function buildRegressionSelection({
  graph,
  paths,
  changedFrom = null,
  repoRoot,
  catalog = CLIENT_MODULE_CATALOG,
  coverage = null,
}) {
  const { modules, unmappedPaths } = selectForChangedPaths(paths, catalog);
  const grouped = new Map();
  for (const module of modules) {
    const key = commandKey(module.command);
    if (!grouped.has(key)) {
      grouped.set(key, {
        program: module.command.program,
        args: [...module.command.args],
        cwd: module.command.cwd,
        timeout_ms: module.command.timeoutMs,
        module_ids: [],
      });
    }
    grouped.get(key).module_ids.push(module.id);
  }
  const commands = [...grouped.values()];
  // Coverage depends on the graph and the catalog, not on the changed paths, so
  // a caller producing many selections may compute it once and pass it in.
  const declaredCoverage = coverage ?? declaredModuleCoverage(graph, catalog);
  const uncovered = declaredCoverage.filter((entry) => !entry.covered).map((entry) => ({ module: entry.module, declared_path: entry.declared_path }));
  return {
    kind: REGRESSION_SELECTION_KIND,
    selection_owner: "tools/regression/client-module-selection.mjs",
    catalog_owner: "tools/regression/client-module-catalog.mjs",
    changed_from: changedFrom,
    changed_paths: [...paths].sort(),
    catalog_modules: modules.map((module) => ({
      id: module.id,
      kind: module.kind,
      lane: module.regression.lane,
      stage: module.regression.stage,
      weight: module.regression.weight,
    })),
    commands,
    unmapped_paths: unmappedPaths,
    declared_module_coverage: declaredCoverage,
    architecture_modules_without_observing_entry: uncovered,
    aggregate_gate_added: false,
    note: "Selection only. This tool does not execute the listed commands and does not add an aggregate gate; run them through the repository's existing regression entry points.",
  };
}

export async function changedPaths({ revision, repoRoot, spawnImpl }) {
  requireFact(typeof revision === "string" && revision.length > 0, "a changed-from revision is required");
  return changedPathsSince({ revision, repoRoot, ...(spawnImpl ? { spawnImpl } : {}) });
}
