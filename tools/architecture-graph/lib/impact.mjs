import { assertRepoRelativePath, pathsOverlap, requireFact } from "./canonical.mjs";
import { RELATION_CLASSES, RELATION_CLASS_NOTES } from "./graph-model.mjs";
import { buildRegressionSelection } from "./regression-selection.mjs";

/**
 * Impact analysis over the resolved graph.
 *
 * Declared relations only. An undeclared real code dependency cannot be inferred
 * from JSON and is still the repository's existing architecture checks and the
 * regression catalog that observe it; the catalog part of that observation is
 * attached here via the existing selection bridge.
 */

export function findCycles(nodes, edges) {
  const adjacency = new Map([...nodes].map((id) => [id, []]));
  for (const [from, to] of edges) {
    if (adjacency.has(from)) adjacency.get(from).push(to);
  }
  const state = new Map();
  const stack = [];
  const cycles = [];
  const seen = new Set();
  const visit = (node) => {
    state.set(node, "visiting");
    stack.push(node);
    for (const next of adjacency.get(node) ?? []) {
      if (!adjacency.has(next)) continue;
      if (state.get(next) === "visiting") {
        const cycle = stack.slice(stack.indexOf(next)).sort();
        const key = cycle.join("\0");
        if (!seen.has(key)) {
          seen.add(key);
          cycles.push(cycle);
        }
      } else if (!state.has(next)) {
        visit(next);
      }
    }
    stack.pop();
    state.set(node, "visited");
  };
  for (const node of [...adjacency.keys()].sort()) {
    if (!state.has(node)) visit(node);
  }
  return cycles.sort((a, b) => a.join().localeCompare(b.join()));
}

export function analyzeImpact({
  graph,
  repoRoot,
  paths = [],
  modules = [],
  contracts = [],
  packages = [],
  tasks = [],
  changedFrom = null,
  catalog,
}) {
  for (const candidate of paths) assertRepoRelativePath(candidate, "changed path");
  for (const id of modules) requireFact(graph.modules.has(id), `unknown module: ${id}`);
  for (const id of contracts) requireFact(graph.contracts.has(id), `unknown contract: ${id}`);
  for (const id of packages) requireFact(graph.packages.has(id), `unknown package: ${id}`);
  for (const id of tasks) requireFact(graph.tasks.has(id), `unknown task: ${id}`);

  const requestedPaths = [...paths].sort();
  const directTasks = new Set(tasks);
  const tasksByWriteScope = new Set();
  const tasksByModulePath = new Set();
  const modulesFromPaths = new Set(modules);

  for (const task of graph.tasks.values()) {
    if (contracts.some((contractId) => task.contracts.includes(contractId))) directTasks.add(task.id);
    if (modules.some((moduleId) => task.modules.includes(moduleId))) directTasks.add(task.id);
    if (packages.some((packageId) => graph.packageClosure(task.packages ?? []).has(packageId))) directTasks.add(task.id);
    if (requestedPaths.some((candidate) => task.write_scopes.some((scope) => pathsOverlap(candidate, scope)))) {
      directTasks.add(task.id);
      tasksByWriteScope.add(task.id);
    }
  }

  for (const candidate of requestedPaths) {
    for (const module of graph.modules.values()) {
      if (pathsOverlap(candidate, module.path)) {
        modulesFromPaths.add(module.id);
        for (const task of graph.tasks.values()) {
          if (task.modules.includes(module.id)) {
            directTasks.add(task.id);
            tasksByModulePath.add(task.id);
          }
        }
      }
    }
  }

  const affected = graph.downstreamTasks(directTasks);
  const goalEdges = graph.architecture.edges.filter((edge) => edge.type === "depends_on").map((edge) => [edge.source, edge.target]);
  const portEdges = graph.architecture.edges.filter((edge) => edge.type === "runtime_calls").map((edge) => [edge.source, edge.target]);
  const moduleIds = [...graph.modules.keys()];

  const withoutSeeds = (found) => [...found].filter((id) => !modulesFromPaths.has(id)).sort();
  const dependents = graph.moduleDependents(modulesFromPaths, ["depends_on"]);
  const portDependents = graph.moduleDependents(modulesFromPaths, ["runtime_calls"]);
  const providers = graph.moduleReachability(modulesFromPaths, ["depends_on"]);

  // Install closures are computed once and reused: a nested closure call per
  // package/profile pair would be quadratic in the number of packages.
  const closures = new Map([...graph.packages.keys()].map((id) => [id, graph.packageClosure([id])]));
  const affectedPackages = new Set(packages);
  for (const moduleId of modulesFromPaths) {
    for (const pkg of graph.packages.values()) if (pkg.modules.includes(moduleId)) affectedPackages.add(pkg.id);
  }
  const packageSeeds = [...affectedPackages];
  for (const pkg of graph.packages.values()) {
    if (packageSeeds.some((packageId) => closures.get(pkg.id).has(packageId))) affectedPackages.add(pkg.id);
  }
  const affectedProfiles = [...graph.profiles.values()]
    .filter((profile) => profile.selected_packages.some((packageId) =>
      [...affectedPackages].some((affectedPackage) => closures.get(packageId).has(affectedPackage))))
    .map((profile) => profile.id)
    .sort();

  const selection = buildRegressionSelection({
    graph,
    paths: requestedPaths,
    repoRoot,
    changedFrom,
    ...(catalog ? { catalog } : {}),
  });

  return {
    kind: "licoup-impact.v1",
    graph_digest: graph.graphDigest,
    graph_version: graph.graphVersion,
    requested: {
      changed_from: changedFrom,
      paths: requestedPaths,
      modules: [...modules].sort(),
      contracts: [...contracts].sort(),
      packages: [...packages].sort(),
      tasks: [...tasks].sort(),
    },
    direct: {
      tasks: [...directTasks].sort(),
      tasks_via_write_scope: [...tasksByWriteScope].sort(),
      tasks_via_module_path: [...tasksByModulePath].sort(),
      modules: [...modulesFromPaths].sort(),
      contracts: [...contracts].sort(),
      packages: [...packages].sort(),
    },
    development: {
      relation_class: RELATION_CLASSES.DEVELOPMENT_ORDER,
      affected_tasks: [...affected].sort(),
      unaffected_tasks: [...graph.tasks.keys()].filter((id) => !affected.has(id)).sort(),
      note: "Affected tasks are computed by lowering task.depends_on to precedes. Architecture and deployment relations never add a wait here.",
    },
    architecture: {
      declared_goal_reference_edges: goalEdges.length,
      declared_port_call_edges: portEdges.length,
      dependents_of_affected_modules: withoutSeeds(dependents),
      port_callers_of_affected_modules: withoutSeeds(portDependents),
      providers_of_affected_modules: withoutSeeds(providers),
      port_call_cycles: findCycles(moduleIds, portEdges),
      enters_development_dag: false,
      note: RELATION_CLASS_NOTES[RELATION_CLASSES.GOAL_REFERENCE],
    },
    deployment: {
      affected_packages: [...affectedPackages].sort(),
      install_closures: Object.fromEntries([...affectedPackages].sort().map((id) => [id, [...graph.packageClosure([id])].sort()])),
      affected_profiles: affectedProfiles,
      enters_development_dag: false,
      note: RELATION_CLASS_NOTES[RELATION_CLASSES.DEPLOYMENT_DELIVERY],
    },
    regression: selection,
    non_claims: [
      "Declared relations only; undeclared real code dependencies are not inferred from JSON.",
      "A listed regression command is a selection result, not a result that was run or passed.",
      "Impact does not release, transfer or invalidate any held claim; the existing development ledger owns that.",
    ],
  };
}
