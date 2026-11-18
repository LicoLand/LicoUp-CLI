import fs from "node:fs";
import path from "node:path";

import { assertRepoRelativePath, digestOf, requireFact } from "./canonical.mjs";
import { readJsonDocument, requireStructure } from "./schema-check.mjs";

export { GraphError } from "./canonical.mjs";

/**
 * The typed graph model behind contract C08.
 *
 * One resolved graph composes two inventories that have different audiences:
 * the architecture inventory (modules, contracts, goals, ports) and the private
 * execution inventory (tasks, acceptance, milestones). This module is the single
 * owner of relation meaning for both. Views (Mermaid, SVG, HTML, work items,
 * the public projection) are projections of this model and never a second
 * source of truth.
 */

/** Relation classes. Only `development-order` is allowed to schedule work. */
export const RELATION_CLASSES = Object.freeze({
  GOAL_REFERENCE: "goal-reference",
  PORT_CALL: "port-call",
  DEVELOPMENT_ORDER: "development-order",
  IMPACT_TRACEABILITY: "impact-traceability",
  DEPLOYMENT_DELIVERY: "deployment-delivery",
});

export const DEVELOPMENT_DAG_CLASS = RELATION_CLASSES.DEVELOPMENT_ORDER;

export const RELATION_CLASS_NOTES = Object.freeze({
  [RELATION_CLASSES.GOAL_REFERENCE]:
    "Architecture goal reference: consumer module -> provider module. Verified for direction and impact scope; never lowered into a runtime or development wait.",
  [RELATION_CLASSES.PORT_CALL]:
    "Assembly port call: caller module -> callee module through a named contract. Feedback is expected; this class may contain cycles and is not a DAG.",
  [RELATION_CLASSES.DEVELOPMENT_ORDER]:
    "Development order: prerequisite task -> dependent task, lowered from task.depends_on. The only class that enters the development DAG.",
  [RELATION_CLASSES.IMPACT_TRACEABILITY]:
    "Impact and traceability: task -> module/contract/acceptance/write scope, milestone -> task. Drives selective invalidation and traceability reports, not scheduling.",
  [RELATION_CLASSES.DEPLOYMENT_DELIVERY]:
    "Deployment and delivery family (fourth class): package -> required package decides the install closure only; task.packages, package -> module, profile selection and source -> package record delivery responsibility. Never enters the development task DAG.",
});

export const DEPLOYMENT_RELATIONS = Object.freeze([
  "requires_package",
  "ships_module",
  "package_implementation_task",
  "task_package",
  "profile_selects_package",
  "profile_forbids_package",
  "source_delivers_package",
]);

function indexById(values, kind) {
  const index = new Map();
  for (const value of values) {
    requireFact(value && typeof value === "object", `${kind} entry must be an object`);
    requireFact(typeof value.id === "string" && value.id.length > 0, `${kind} entry needs a string id`);
    requireFact(/^[A-Za-z0-9][A-Za-z0-9_.-]*$/u.test(value.id), `${kind} id is not a safe identifier: ${value.id}`);
    requireFact(!index.has(value.id), `duplicate ${kind} id: ${value.id}`);
    index.set(value.id, value);
  }
  return index;
}

export function topologicalLayers(ids, edges) {
  const unique = [...new Set(ids)];
  const followers = new Map(unique.map((id) => [id, new Set()]));
  const indegree = new Map(unique.map((id) => [id, 0]));
  for (const [from, to] of edges) {
    requireFact(followers.has(from) && followers.has(to), `relation endpoint is not declared: ${from} -> ${to}`);
    requireFact(from !== to, `self relation is not allowed: ${from}`);
    if (!followers.get(from).has(to)) {
      followers.get(from).add(to);
      indegree.set(to, indegree.get(to) + 1);
    }
  }
  const remaining = new Map(indegree);
  const result = [];
  while (remaining.size > 0) {
    const ready = [...remaining.keys()].filter((id) => remaining.get(id) === 0).sort();
    requireFact(ready.length > 0, `relation cycle among: ${[...remaining.keys()].sort().join(", ")}`);
    result.push(ready);
    for (const id of ready) {
      remaining.delete(id);
      for (const follower of followers.get(id)) {
        remaining.set(follower, remaining.get(follower) - 1);
      }
    }
  }
  return result;
}

export class Graph {
  constructor({ projectPath, project, architecture, execution, distribution, documents }) {
    this.projectPath = projectPath;
    this.project = project;
    this.architecture = architecture;
    this.execution = execution;
    this.distribution = distribution ?? null;
    this.documents = documents;

    this.modules = indexById(architecture.modules, "module");
    this.contracts = indexById(architecture.contracts, "contract");
    this.tasks = indexById(execution.tasks, "task");
    this.cases = indexById(execution.cases, "acceptance case");
    this.milestones = indexById(execution.milestones, "milestone");
    this.packages = distribution ? indexById(distribution.packages, "package") : new Map();
    this.profiles = distribution ? indexById(distribution.profiles, "profile") : new Map();

    this.#requireSemantics();
  }

  #requireSemantics() {
    const { architecture, execution, distribution } = this;

    requireFact(execution.architecture_revision === architecture.revision,
      `execution graph targets architecture revision ${execution.architecture_revision} but the architecture is ${architecture.revision}`);
    if (distribution) {
      requireFact(distribution.revision === architecture.revision,
        `distribution graph targets revision ${distribution.revision} but the architecture is ${architecture.revision}`);
    }

    const everyId = [
      ...this.modules.keys(), ...this.contracts.keys(), ...this.tasks.keys(),
      ...this.cases.keys(), ...this.milestones.keys(), ...this.packages.keys(), ...this.profiles.keys(),
    ];
    requireFact(everyId.length === new Set(everyId).size, "IDs must be unique across every graph node type");

    const directions = architecture.directions;
    requireFact(directions && typeof directions === "object", "architecture directions are required");
    for (const key of ["depends_on", "runtime_calls", "precedes"]) {
      requireFact(typeof directions[key] === "string" && directions[key].length > 0,
        `architecture direction is not declared: ${key}`);
    }
    requireFact(
      !distribution || (typeof directions.requires_package === "string" && directions.requires_package.length > 0),
      "a distribution graph requires the requires_package direction to be declared",
    );

    for (const edge of architecture.edges) {
      requireFact(edge.type === "depends_on" || edge.type === "runtime_calls",
        `architecture edge type must be depends_on or runtime_calls: ${edge.type}`);
      requireFact(this.modules.has(edge.source), `architecture edge source is unknown: ${edge.source}`);
      requireFact(this.modules.has(edge.target), `architecture edge target is unknown: ${edge.target}`);
      requireFact(edge.source !== edge.target, `architecture edge is a self reference: ${edge.source}`);
      if (edge.type === "runtime_calls") {
        requireFact(this.contracts.has(edge.contract), `runtime edge needs a known contract: ${edge.source} -> ${edge.target}`);
      }
    }
    // Goal references must be acyclic; port calls are explicitly allowed to
    // contain feedback loops and are therefore not layered.
    this.architectureLayerRows = topologicalLayers(
      [...this.modules.keys()],
      architecture.edges.filter((edge) => edge.type === "depends_on").map((edge) => [edge.source, edge.target]),
    );

    for (const contract of this.contracts.values()) {
      requireFact(this.modules.has(contract.owner_module), `contract owner module is unknown: ${contract.id}`);
      requireFact(Number.isInteger(contract.revision) && contract.revision >= 1,
        `contract revision must be a positive integer: ${contract.id}`);
      for (const consumer of contract.consumers) {
        requireFact(this.modules.has(consumer), `contract consumer module is unknown: ${contract.id} / ${consumer}`);
      }
    }

    for (const task of this.tasks.values()) {
      requireFact(Number.isInteger(task.task_revision) && task.task_revision > 0, `task_revision required: ${task.id}`);
      requireFact(task.acceptance.length > 0, `task has no acceptance scenario: ${task.id}`);
      requireFact(task.write_scopes.length > 0, `task has no write scope: ${task.id}`);
      requireFact(task.initial_status === "pending", `task must be declared pending: ${task.id}`);
      for (const scope of task.write_scopes) assertRepoRelativePath(scope, `task ${task.id} write scope`);
      for (const resource of task.resources) {
        requireFact(typeof resource === "string" && resource.length > 0, `task resource is invalid: ${task.id}`);
      }
      const references = [
        ["modules", task.modules, this.modules],
        ["contracts", task.contracts, this.contracts],
        ["acceptance", task.acceptance, this.cases],
        ["depends_on", task.depends_on, this.tasks],
      ];
      for (const [label, values, index] of references) {
        requireFact(new Set(values).size === values.length, `task ${task.id} repeats a ${label} reference`);
        for (const value of values) {
          requireFact(index.has(value), `task ${task.id} references unknown ${label} entry: ${value}`);
        }
      }
      for (const [caseId, level] of Object.entries(task.acceptance_levels)) {
        requireFact(task.acceptance.includes(caseId), `task ${task.id} overrides a level for an unowned scenario: ${caseId}`);
        const spec = this.cases.get(caseId);
        requireFact(spec.permitted_levels.includes(level),
          `task ${task.id} claims an unpermitted evidence level for ${caseId}: ${level}`);
        if (task.acceptance_role === "final") {
          requireFact(level === spec.required_level,
            `final acceptance for ${task.id} cannot downgrade ${caseId} below ${spec.required_level}`);
        }
      }
    }

    // Reject invalid task order at the model boundary, not only when a caller
    // asks for a DAG view. Export, bind and relations use the same model.
    topologicalLayers(
      [...this.tasks.keys()],
      [...this.tasks.values()].flatMap((task) => task.depends_on.map((id) => [id, task.id])),
    );

    for (const milestone of this.milestones.values()) {
      for (const taskId of milestone.tasks) {
        requireFact(this.tasks.has(taskId), `milestone ${milestone.id} references unknown task: ${taskId}`);
      }
    }

    this.#requireDeploymentSemantics();
  }

  #requireDeploymentSemantics() {
    if (!this.distribution) {
      for (const task of this.tasks.values()) {
        requireFact((task.packages ?? []).length === 0,
          `task ${task.id} declares packages but no distribution graph is configured`);
      }
      return;
    }
    const distribution = this.distribution;
    requireFact(this.packages.has(distribution.core_package), "distribution core package is not declared");
    const core = this.packages.get(distribution.core_package);
    requireFact(core.optional === false, "the core package cannot be optional");
    requireFact(core.requires_package.length === 0, "the core package cannot require other packages");

    const packageEdges = [];
    for (const pkg of this.packages.values()) {
      requireFact(typeof pkg.optional === "boolean", `package optional flag must be boolean: ${pkg.id}`);
      requireFact(new Set(pkg.requires_package).size === pkg.requires_package.length,
        `package repeats a dependency: ${pkg.id}`);
      for (const dependency of pkg.requires_package) {
        requireFact(this.packages.has(dependency), `package dependency is unknown: ${pkg.id} -> ${dependency}`);
      }
      for (const moduleId of pkg.modules) {
        requireFact(this.modules.has(moduleId), `package declares an unknown module: ${pkg.id} / ${moduleId}`);
      }
      for (const taskId of pkg.implementation_tasks) {
        requireFact(this.tasks.has(taskId), `package declares an unknown implementation task: ${pkg.id} / ${taskId}`);
      }
      requireFact(pkg.measured_bytes === null
        || (Number.isInteger(pkg.measured_bytes) && pkg.measured_bytes >= 0),
      `package measured_bytes must be a non-negative integer or null: ${pkg.id}`);
      packageEdges.push(...pkg.requires_package.map((dependency) => [pkg.id, dependency]));
    }
    // The install closure is a real DAG; it is still not a development schedule.
    topologicalLayers([...this.packages.keys()], packageEdges);

    // A package may only link statically to modules inside its own install closure.
    for (const pkg of this.packages.values()) {
      const visible = new Set(
        [...this.packageClosure([pkg.id])].flatMap((id) => this.packages.get(id).modules),
      );
      for (const edge of this.architecture.edges) {
        if (edge.type !== "depends_on" || !pkg.modules.includes(edge.source)) continue;
        requireFact(visible.has(edge.target),
          `static module dependency escapes the install closure of ${pkg.id}: ${edge.source} -> ${edge.target}`);
      }
    }

    for (const task of this.tasks.values()) {
      requireFact(Array.isArray(task.packages ?? []), `task packages must be an array: ${task.id}`);
      for (const packageId of task.packages ?? []) {
        requireFact(this.packages.has(packageId), `task ${task.id} declares an unknown package: ${packageId}`);
      }
    }

    for (const profile of this.profiles.values()) {
      const declared = [...profile.selected_packages, ...profile.forbidden_packages];
      for (const packageId of declared) {
        requireFact(this.packages.has(packageId), `profile ${profile.id} references an unknown package: ${packageId}`);
      }
      const closure = this.packageClosure(profile.selected_packages);
      requireFact(closure.has(distribution.core_package), `profile ${profile.id} does not select the core package`);
      const forbidden = profile.forbidden_packages.filter((id) => closure.has(id));
      requireFact(forbidden.length === 0,
        `profile ${profile.id} pulls a forbidden package through its install closure: ${forbidden.join(", ")}`);
    }
  }

  packageClosure(selected) {
    const closure = new Set(selected);
    const pending = [...closure];
    for (const packageId of pending) {
      requireFact(this.packages.has(packageId), `unknown selected package: ${packageId}`);
    }
    while (pending.length > 0) {
      for (const dependency of this.packages.get(pending.pop()).requires_package) {
        if (!closure.has(dependency)) {
          closure.add(dependency);
          pending.push(dependency);
        }
      }
    }
    return closure;
  }

  get graphVersion() {
    return {
      architecture_revision: this.architecture.revision,
      execution_revision: this.execution.revision,
      distribution_revision: this.distribution ? this.distribution.revision : null,
      plan: this.project.plan,
      baseline_commit: this.architecture.baseline_commit,
    };
  }

  /** Same byte form as the Python reference tool's graph digest. */
  get graphDigest() {
    return digestOf({
      architecture: this.architecture,
      execution: this.execution,
      distribution: this.distribution,
    });
  }

  /** Same recipe as the Python reference tool's task digest; parity is tested. */
  taskFingerprint(taskId) {
    const task = this.tasks.get(taskId);
    requireFact(task !== undefined, `unknown task: ${taskId}`);
    const sorted = (values) => [...values].sort();
    return digestOf({
      task,
      contracts: sorted(task.contracts).map((id) => this.contracts.get(id)),
      modules: sorted(task.modules).map((id) => this.modules.get(id)),
      cases: sorted(task.acceptance).map((id) => this.cases.get(id)),
      architecture_edges: this.architecture.edges.filter((edge) => task.modules.includes(edge.source)),
      baseline: this.execution.baseline,
      policy: this.execution.policy,
      package_inputs: sorted(this.packageClosure(task.packages ?? [])).map((id) => this.packages.get(id)),
    });
  }

  contractRevisions() {
    return Object.fromEntries(
      [...this.contracts.values()]
        .sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
        .map((contract) => [contract.id, contract.revision]),
    );
  }

  effectiveLevel(task, caseId) {
    return task.acceptance_levels?.[caseId] ?? this.cases.get(caseId).required_level;
  }

  /**
   * Every relation in the resolved graph, each carrying its class and whether it
   * may schedule work. `precedes` is the only derived scheduling relation; it is
   * lowered from `task.depends_on` and never from an architecture or deployment
   * relation.
   */
  relations() {
    const edges = [];
    const push = (relationClass, relation, source, target, detail = {}) => {
      edges.push({
        class: relationClass,
        relation,
        source,
        target,
        enters_development_dag: relationClass === DEVELOPMENT_DAG_CLASS,
        ...(Object.keys(detail).length > 0 ? { detail } : {}),
      });
    };

    for (const edge of this.architecture.edges) {
      if (edge.type === "depends_on") {
        push(RELATION_CLASSES.GOAL_REFERENCE, "depends_on", edge.source, edge.target);
      } else {
        push(RELATION_CLASSES.PORT_CALL, "runtime_calls", edge.source, edge.target, { contract: edge.contract });
      }
    }

    for (const task of this.tasks.values()) {
      for (const predecessor of task.depends_on) {
        push(RELATION_CLASSES.DEVELOPMENT_ORDER, "precedes", predecessor, task.id);
      }
      for (const moduleId of task.modules) push(RELATION_CLASSES.IMPACT_TRACEABILITY, "impacts_module", task.id, moduleId);
      for (const contractId of task.contracts) push(RELATION_CLASSES.IMPACT_TRACEABILITY, "consumes_contract", task.id, contractId);
      for (const caseId of task.acceptance) {
        push(RELATION_CLASSES.IMPACT_TRACEABILITY, "verifies_acceptance", task.id, caseId,
          { effective_level: this.effectiveLevel(task, caseId) });
      }
      for (const scope of task.write_scopes) {
        push(RELATION_CLASSES.IMPACT_TRACEABILITY, "owns_write_scope", task.id, scope, { source_kind: "source-identity" });
      }
      for (const packageId of task.packages ?? []) {
        push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "task_package", task.id, packageId);
        for (const scope of task.write_scopes) {
          push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "source_delivers_package", scope, packageId,
            { source_kind: "source-identity", via_task: task.id });
        }
      }
    }

    for (const milestone of this.milestones.values()) {
      for (const taskId of milestone.tasks) {
        push(RELATION_CLASSES.IMPACT_TRACEABILITY, "reports_milestone", milestone.id, taskId);
      }
    }

    for (const pkg of this.packages.values()) {
      for (const dependency of pkg.requires_package) {
        push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "requires_package", pkg.id, dependency);
      }
      for (const moduleId of pkg.modules) {
        push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "ships_module", pkg.id, moduleId);
      }
      for (const taskId of pkg.implementation_tasks) {
        push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "package_implementation_task", pkg.id, taskId);
      }
    }

    for (const profile of this.profiles.values()) {
      for (const packageId of profile.selected_packages) {
        push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "profile_selects_package", profile.id, packageId);
      }
      for (const packageId of profile.forbidden_packages) {
        push(RELATION_CLASSES.DEPLOYMENT_DELIVERY, "profile_forbids_package", profile.id, packageId);
      }
    }

    return edges;
  }

  /** Layered goal-reference view of the modules (port calls are excluded). */
  moduleLayers() {
    return this.architectureLayerRows.map((layer) => [...layer]);
  }

  developmentDag() {
    const edges = this.relations()
      .filter((edge) => edge.enters_development_dag)
      .map((edge) => [edge.source, edge.target]);
    return { edges, layers: topologicalLayers([...this.tasks.keys()], edges) };
  }

  /** Consumers of a contract, then their development descendants. */
  downstreamTasks(initial) {
    const relations = this.developmentDag().edges;
    const found = new Set(initial);
    for (;;) {
      const extra = relations.filter(([from]) => found.has(from)).map(([, to]) => to).filter((id) => !found.has(id));
      if (extra.length === 0) return found;
      for (const id of extra) found.add(id);
    }
  }

  moduleReachability(initial, relationNames) {
    const edges = this.architecture.edges
      .filter((edge) => relationNames.includes(edge.type))
      .map((edge) => [edge.source, edge.target]);
    const found = new Set(initial);
    for (;;) {
      const extra = edges.filter(([from]) => found.has(from)).map(([, to]) => to).filter((id) => !found.has(id));
      if (extra.length === 0) return found;
      for (const id of extra) found.add(id);
    }
  }

  /** Modules that reach `initial` through the named relations (reverse direction). */
  moduleDependents(initial, relationNames) {
    const edges = this.architecture.edges
      .filter((edge) => relationNames.includes(edge.type))
      .map((edge) => [edge.source, edge.target]);
    const found = new Set(initial);
    for (;;) {
      const extra = edges.filter(([, to]) => found.has(to)).map(([from]) => from).filter((id) => !found.has(id));
      if (extra.length === 0) return found;
      for (const id of extra) found.add(id);
    }
  }
}

export function loadGraph({ projectPath, repoRoot }) {
  requireFact(fs.existsSync(projectPath), `project document is missing: ${projectPath}`);
  const project = readJsonDocument(projectPath, "project");
  requireFact(project.schema_version === 1, "unsupported project schema version");
  // Graph sources are located relative to the project document, never through a
  // machine-local absolute path: the tool must run in any collaborator checkout.
  const graphRoot = path.dirname(projectPath);
  const resolveSource = (relative) => {
    assertRepoRelativePath(relative, "graph source");
    return path.resolve(graphRoot, relative);
  };

  const architecturePath = resolveSource(project.architecture);
  const executionPath = resolveSource(project.execution);
  const distributionPath = project.distribution ? resolveSource(project.distribution) : null;

  const architecture = readJsonDocument(architecturePath, "architecture graph");
  const execution = readJsonDocument(executionPath, "execution graph");
  const distribution = distributionPath ? readJsonDocument(distributionPath, "distribution graph") : null;

  const structure = [
    requireStructure({ document: project, documentPath: projectPath }),
    requireStructure({ document: architecture, documentPath: architecturePath }),
    requireStructure({ document: execution, documentPath: executionPath }),
  ];
  if (distributionPath) {
    structure.push(requireStructure({ document: distribution, documentPath: distributionPath }));
  }
  requireFact(architecture.schema_version === 1 && execution.schema_version === 1,
    "unsupported graph schema version");
  if (distribution) requireFact(distribution.schema_version === 1, "unsupported distribution schema version");

  return new Graph({
    projectPath,
    project,
    architecture,
    execution,
    distribution,
    documents: { architecturePath, executionPath, distributionPath, structure },
  });
}

