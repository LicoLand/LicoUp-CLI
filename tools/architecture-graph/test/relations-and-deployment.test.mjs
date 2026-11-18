import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { loadGraph, RELATION_CLASSES } from "../lib/graph-model.mjs";
import { analyzeImpact, findCycles } from "../lib/impact.mjs";
import { relationChecks } from "../lib/work-items.mjs";
import { expectGraphError, fixtureDistribution, fixtureDocuments, fixtureGraph, graphFromDocuments, REPOSITORY_ROOT, revalidateGraph, skipWithoutRealPlan } from "./fixtures.mjs";

const PROJECT = path.join(REPOSITORY_ROOT, "docs/plans/v7/graph/project.json");

test("the fourth relation class is distinguished from the scheduling class", () => {
  const documents = fixtureDocuments({ distribution: fixtureDistribution() });
  documents.execution.tasks[0].packages = ["org.test.core"];
  documents.execution.tasks[1].packages = ["org.test.optional"];
  const graph = graphFromDocuments(documents);
  const relations = graph.relations();
  const deployment = relations.filter((edge) => edge.class === RELATION_CLASSES.DEPLOYMENT_DELIVERY);
  const relations4 = deployment.map((edge) => edge.relation);
  for (const expected of ["requires_package", "ships_module", "package_implementation_task", "task_package", "profile_selects_package", "profile_forbids_package", "source_delivers_package"]) {
    assert.ok(relations4.includes(expected), `missing deployment relation: ${expected}`);
  }
  assert.ok(deployment.every((edge) => edge.enters_development_dag === false));
  assert.ok(relations.filter((edge) => edge.enters_development_dag).every((edge) => edge.class === RELATION_CLASSES.DEVELOPMENT_ORDER));
});

test("requires_package decides the install closure only", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  assert.deepEqual([...graph.packageClosure(["org.test.optional"])].sort(), ["org.test.core", "org.test.optional"]);
  assert.deepEqual([...graph.packageClosure(["org.test.core"])], ["org.test.core"]);
  assert.match(expectGraphError(() => graph.packageClosure(["org.test.absent"])), /unknown selected package/);
});

function optionalPackage(id, requires) {
  return {
    id,
    title: id,
    requires_package: requires,
    modules: [],
    optional: true,
    provides: [`${id}.v1`],
    activation: "user-import",
    artifact_status: "not-built",
    measured_bytes: null,
    implementation_tasks: [],
  };
}

test("a package install cycle is rejected while a task DAG that would contradict it is accepted", () => {
  const cyclicDistribution = fixtureDistribution({
    packages: [
      fixtureDistribution().packages[0],
      optionalPackage("org.test.a", ["org.test.b"]),
      optionalPackage("org.test.b", ["org.test.a"]),
    ],
    profiles: [{ id: "minimal", title: "Minimal", selected_packages: ["org.test.core"], forbidden_packages: [] }],
  });
  assert.match(
    expectGraphError(() => fixtureGraph({ distribution: cyclicDistribution })),
    /relation cycle among/,
  );

  // The optional package requires core. Reversing the task order does not create
  // a contradiction, because a package requirement is never lowered to a wait.
  const reversed = fixtureGraph({
    execution: {
      tasks: [
        { ...fixtureGraph().execution.tasks[0], depends_on: ["T02"] },
        { ...fixtureGraph().execution.tasks[1], depends_on: [] },
      ],
    },
    distribution: fixtureDistribution(),
  });
  assert.deepEqual(reversed.developmentDag().edges, [["T02", "T01"]]);
  const deploymentEdges = reversed.relations().filter((edge) => edge.class === RELATION_CLASSES.DEPLOYMENT_DELIVERY);
  assert.ok(deploymentEdges.length > 0);
});

test("a static module dependency may not escape its package install closure", () => {
  // M02 depends_on M01, but M01 is not shipped by this package or its closure.
  const escaping = {
    ...fixtureDistribution(),
    packages: [{
      id: "org.test.core",
      title: "Core",
      requires_package: [],
      modules: ["M02"],
      optional: false,
      provides: ["core.v1"],
      activation: "core-start",
      artifact_status: "not-built",
      measured_bytes: null,
      implementation_tasks: ["T01"],
    }],
    profiles: [{ id: "minimal", title: "Minimal", selected_packages: ["org.test.core"], forbidden_packages: [] }],
  };
  assert.match(
    expectGraphError(() => fixtureGraph({ distribution: escaping })),
    /escapes the install closure/,
  );
});

test("a profile may not pull a forbidden package through its closure", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  graph.distribution.profiles[1].forbidden_packages = ["org.test.optional"];
  assert.match(expectGraphError(() => revalidateGraph(graph)), /pulls a forbidden package/);

  const missingCore = fixtureGraph({ distribution: fixtureDistribution({
    profiles: [{ id: "broken", title: "Broken", selected_packages: ["org.test.optional"], forbidden_packages: [] }],
  }) });
  assert.doesNotThrow(() => revalidateGraph(missingCore), "the closure pulls the core package, so this profile is valid");
});

test("unknown package references are rejected in tasks, packages and profiles", () => {
  const task = fixtureGraph({ distribution: fixtureDistribution() });
  task.execution.tasks[0].packages = ["org.test.absent"];
  assert.match(expectGraphError(() => revalidateGraph(task)), /unknown package/);

  const packageDependency = fixtureGraph({ distribution: fixtureDistribution() });
  packageDependency.distribution.packages[1].requires_package = ["org.test.absent"];
  assert.match(expectGraphError(() => revalidateGraph(packageDependency)), /package dependency is unknown/);

  const withoutDistribution = fixtureGraph();
  withoutDistribution.execution.tasks[0].packages = ["org.test.core"];
  assert.match(expectGraphError(() => revalidateGraph(withoutDistribution)), /no distribution graph is configured/);
});

test("a runtime feedback loop does not pollute the development DAG", () => {
  const graph = fixtureGraph();
  graph.architecture.edges.push({ type: "runtime_calls", source: "M01", target: "M02", contract: "C01" });
  const revalidated = revalidateGraph(graph);
  assert.deepEqual(findCycles([...revalidated.modules.keys()], [
    ["M01", "M02"],
    ["M02", "M01"],
  ]), [["M01", "M02"]]);
  assert.deepEqual(revalidated.developmentDag().edges, [["T01", "T02"]]);
  const checks = relationChecks(revalidated);
  assert.equal(checks.errors.length, 0, JSON.stringify(checks.errors));
  assert.deepEqual(checks.checks.find((check) => check.id === "port-call-cycles-stay-out-of-the-development-dag").failures, []);
});

test("the deployment class never appears as a development wait in the available local plan", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const checks = relationChecks(graph);
  const deploymentCheck = checks.checks.find((check) => check.id === "deployment-is-not-a-task-wait");
  assert.equal(deploymentCheck.passed, true, JSON.stringify(deploymentCheck.failures));
  const schedulingCheck = checks.checks.find((check) => check.id === "only-precedes-schedules");
  assert.equal(schedulingCheck.passed, true);
  assert.equal(checks.errors.length, 0);
});

test("a package requirement is visible as deployment impact and not as a task wait", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  const report = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, packages: ["org.test.core"] });
  // Every package installs on top of core, so the closure reaches the whole
  // catalog; that is a deployment fact and must not become a task wait.
  assert.equal(report.deployment.affected_packages.length, graph.packages.size);
  assert.ok(report.deployment.affected_packages.includes("org.test.optional"));
  assert.equal(report.deployment.enters_development_dag, false);
  assert.equal(report.regression.aggregate_gate_added, false);
  assert.deepEqual(report.development.affected_tasks, []);
});

test("the available local plan package impact stays a deployment fact", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const report = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, packages: ["org.licoland.core"] });
  assert.equal(report.deployment.affected_packages.length, graph.packages.size);
  assert.ok(report.deployment.affected_packages.includes("org.licoland.feature.gateway"));
  assert.equal(report.deployment.enters_development_dag, false);
  assert.equal(report.regression.aggregate_gate_added, false);
});

test("module and path impact includes packages depending on the module's package", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  for (const seed of [{ modules: ["M01"] }, { paths: ["src/provider/lib.rs"] }]) {
    const report = analyzeImpact({ graph, repoRoot: REPOSITORY_ROOT, ...seed });
    assert.deepEqual(report.deployment.affected_packages, ["org.test.core", "org.test.optional"]);
    assert.equal(report.deployment.enters_development_dag, false);
  }
});
