import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { loadGraph, RELATION_CLASSES } from "../lib/graph-model.mjs";
import { analyzeImpact, findCycles } from "../lib/impact.mjs";
import { CLIENT_MODULE_CATALOG } from "../../regression/client-module-catalog.mjs";
import { command, defineModule } from "../../regression/client-module-catalog/helpers.mjs";
import { selectModulesForChangedPaths } from "../../regression/client-module-selection.mjs";
import { buildRegressionSelection, selectForChangedPaths } from "../lib/regression-selection.mjs";
import { expectGraphError, fixtureGraph, REPOSITORY_ROOT, skipWithoutRealPlan, UNSAFE_PATH_SAMPLES } from "./fixtures.mjs";

const PROJECT = path.join(REPOSITORY_ROOT, "docs/plans/v7/graph/project.json");

test("impact follows declared relations and reports the unaffected remainder", () => {
  const graph = fixtureGraph();
  const report = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, contracts: ["C01"] });
  assert.deepEqual(report.direct.tasks, ["T01", "T02"]);
  assert.deepEqual(report.development.affected_tasks, ["T01", "T02"]);
  assert.deepEqual(report.development.unaffected_tasks, []);
  assert.equal(report.development.relation_class, RELATION_CLASSES.DEVELOPMENT_ORDER);
  assert.equal(report.architecture.enters_development_dag, false);
  assert.ok(report.non_claims.length >= 3);

  const unrelated = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, tasks: ["T01"] });
  assert.deepEqual(unrelated.development.affected_tasks, ["T01", "T02"], "a dependent task is affected through precedes");
  const leaf = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, tasks: ["T02"] });
  assert.deepEqual(leaf.development.affected_tasks, ["T02"], "an independent task does not pull its predecessor in");
});

test("impact distinguishes a write scope from a module projection", () => {
  const graph = fixtureGraph();
  const byScope = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, paths: ["scopes/T02/"] });
  assert.deepEqual(byScope.direct.tasks_via_write_scope, ["T02"]);
  assert.deepEqual(byScope.direct.tasks_via_module_path, []);

  const byModulePath = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, paths: ["src/other/"] });
  assert.deepEqual(byModulePath.direct.tasks_via_write_scope, []);
  assert.deepEqual(byModulePath.direct.tasks_via_module_path, ["T02"]);
});

test("impact rejects unknown references instead of silently returning nothing", () => {
  const graph = fixtureGraph();
  assert.match(expectGraphError(() => analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, contracts: ["C99"] })), /unknown contract/);
  assert.match(expectGraphError(() => analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, modules: ["M99"] })), /unknown module/);
  assert.match(expectGraphError(() => analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, paths: [UNSAFE_PATH_SAMPLES[0]] })), /changed path must be repository-relative/);
  assert.match(expectGraphError(() => analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, paths: ["../outside"] })), /changed path must not contain relative segments/);
});

test("the catalog selection is the existing one, not a second implementation", () => {
  const changed = ["docs/architecture/architecture-map.json", "apps/desktop/lib/src/frontend/layout/dock/x.dart"];
  const expected = selectModulesForChangedPaths(changed, CLIENT_MODULE_CATALOG).map((module) => module.id).sort();
  const selection = buildRegressionSelection({ graph: fixtureGraph(), paths: changed, repoRoot: REPOSITORY_ROOT });
  assert.ok(expected.length > 0, "the public catalog must observe a public architecture path");
  assert.deepEqual(selection.catalog_modules.map((module) => module.id).sort(), expected);
  assert.equal(selection.selection_owner, "tools/regression/client-module-selection.mjs");
  assert.equal(selection.aggregate_gate_added, false);
  assert.ok(selection.commands.every((command) => command.program === "node" || command.program === "cargo"));
  assert.ok(selection.commands.every((command) => Array.isArray(command.args) && command.args.length > 0));
  assert.ok(!selection.commands.some((command) => command.args.some((arg) => arg.includes("client:gate:"))),
    "a work item must not reach for an aggregate gate");
});

test("an uncovered changed path is reported instead of being silently skipped", () => {
  const changed = [
    "tools/architecture-graph/lib/graph-model.mjs",
    "docs/architecture/architecture-map.json",
    "unobserved/not-in-any-catalog-module.txt",
  ];
  const selection = buildRegressionSelection({ graph: fixtureGraph(), paths: changed, repoRoot: REPOSITORY_ROOT });
  // The expectation is derived from the existing selection owner rather than
  // frozen here: a later catalog registration must not need this test edited,
  // and an unobserved path must still be reported instead of dropped.
  const expectedGaps = changed.filter((entry) => selectModulesForChangedPaths([entry], CLIENT_MODULE_CATALOG).length === 0).sort();
  assert.deepEqual(selection.unmapped_paths, expectedGaps);
  assert.ok(selection.unmapped_paths.includes("unobserved/not-in-any-catalog-module.txt"),
    "a changed path no catalog module observes must be reported");
  assert.ok(selection.catalog_modules.some((module) => module.id === "regression.documentation-governance"));
});

test("declared module coverage exposes architecture modules with no observing entry", () => {
  const catalog = [defineModule({
    id: "test.module",
    kind: "regression-infrastructure",
    summary: "injected catalog entry",
    inputs: ["src/provider/**"],
    command: command("node", ["--test", "x.test.mjs"], 1000),
  })];
  const selection = buildRegressionSelection({
    graph: fixtureGraph(),
    paths: ["unobserved/file.mjs"],
    repoRoot: REPOSITORY_ROOT,
    catalog,
  });
  const covered = selection.declared_module_coverage.find((entry) => entry.module === "M01");
  assert.equal(covered.declared_path, "src/provider/");
  assert.equal(covered.covered, true);
  assert.deepEqual(covered.catalog_module_ids, ["test.module"]);
  const uncovered = selection.declared_module_coverage.find((entry) => entry.module === "M02");
  assert.equal(uncovered.declared_path, "src/consumer/");
  assert.equal(uncovered.covered, false);
  assert.deepEqual(uncovered.catalog_module_ids, []);
  assert.deepEqual(selection.architecture_modules_without_observing_entry.map((entry) => entry.module), ["M02", "M03"]);
});

test("the local plan graph module is observed by the existing selection owner", (t) => {
  if (skipWithoutRealPlan(t)) return;
  // Live graph: the flag for the graph tool's own module must agree with the
  // existing selection owner, whichever catalog entries exist at the time.
  const live = buildRegressionSelection({
    graph: loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT }),
    paths: ["tools/architecture-graph/"],
    repoRoot: REPOSITORY_ROOT,
  });
  const graphModule = live.declared_module_coverage.find((entry) => entry.module === "M24");
  assert.equal(graphModule.declared_path, "tools/architecture-graph/");
  assert.equal(graphModule.covered, selectModulesForChangedPaths([graphModule.declared_path], CLIENT_MODULE_CATALOG).length > 0);
});

test("selection can be driven by an injected catalog and still uses the repository matcher", () => {
  const catalog = [defineModule({
    id: "test.module",
    kind: "regression-infrastructure",
    summary: "injected catalog entry",
    inputs: ["scopes/**"],
    command: command("node", ["--test", "x.test.mjs"], 1000),
  })];
  const graph = fixtureGraph();
  const selection = buildRegressionSelection({ graph, paths: ["scopes/T01/"], repoRoot: REPOSITORY_ROOT, catalog });
  assert.deepEqual(selection.catalog_modules.map((module) => module.id), ["test.module"]);
  assert.deepEqual(selection.unmapped_paths, []);
  const { modules, unmappedPaths } = selectForChangedPaths(["scopes/T01/", "unobserved/file.mjs"], catalog);
  assert.deepEqual(modules.map((module) => module.id), ["test.module"]);
  assert.deepEqual(unmappedPaths, ["unobserved/file.mjs"]);
});

test("a changed path outside the repository is rejected before selection", () => {
  assert.throws(() => selectForChangedPaths([UNSAFE_PATH_SAMPLES[0]]), /repository path must be relative/u);
  assert.throws(() => selectForChangedPaths(["../outside"]), /repository path must stay inside/u);
});

test("cycle detection is deterministic and reports the participating nodes", () => {
  assert.deepEqual(findCycles(["a"], []), []);
  assert.deepEqual(findCycles(["a", "b", "c"], [["a", "b"], ["b", "c"]]), []);
  assert.deepEqual(findCycles(["a", "b", "c"], [["a", "b"], ["b", "a"], ["b", "c"], ["c", "b"]]), [["a", "b"], ["b", "c"]]);
});
