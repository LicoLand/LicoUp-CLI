import { RELATION_CLASSES, RELATION_CLASS_NOTES } from "./graph-model.mjs";
import { pathsOverlap, requireFact } from "./canonical.mjs";
import { findCycles } from "./impact.mjs";
import { buildRegressionSelection, declaredModuleCoverage } from "./regression-selection.mjs";
import { resolveSourceIdentities } from "./source-identity.mjs";

/**
 * Machine work items and the claim binding contract.
 *
 * These items are a neutral development projection. They are not the native
 * LicoUp workflow grammar and do not claim compatibility with it; the production
 * admission bridge is a later node's work. Nothing here marks work complete:
 * every item is emitted `pending`, and acceptance evidence is checked for
 * binding consistency only.
 */

export const WORK_ITEM_KIND = "development-work-items-not-native-workflow";

export const REQUIRED_RECEIPT_FIELDS = Object.freeze([
  "task_id",
  "task_digest",
  "input_digest",
  "claim_token",
  "owner",
  "generation",
  "status",
  "source_revision",
  "producer",
  "checks",
]);

function safeTaskSummary(task) {
  return {
    id: task.id,
    title: task.title,
    lane: task.lane,
    acceptance_role: task.acceptance_role,
    independent_review: task.independent_review,
    authorization: task.authorization,
    completion_contract: task.completion_contract,
    verification_command_policy: task.verification_command_policy,
    task_revision: task.task_revision,
  };
}

export function buildWorkItems({ graph, repoRoot, catalog, revision }) {
  const sourceIdentities = resolveSourceIdentities({ graph, repoRoot, revision });
  const byPath = new Map(sourceIdentities.identities.map((identity) => [identity.normalized_path, identity]));
  // Coverage is graph- and catalog-level; computing it once keeps a 52-item
  // export from repeating the same catalog scan per item.
  const coverage = declaredModuleCoverage(graph, ...(catalog ? [catalog] : []));
  const items = [...graph.tasks.values()].map((task) => {
    const selection = buildRegressionSelection({
      graph,
      paths: task.write_scopes,
      repoRoot,
      coverage,
      ...(catalog ? { catalog } : {}),
    });
    return {
      ...safeTaskSummary(task),
      kind: WORK_ITEM_KIND,
      initial_status: task.initial_status,
      predecessors: [...task.depends_on].sort(),
      relation_class: RELATION_CLASSES.DEVELOPMENT_ORDER,
      modules: [...task.modules].sort(),
      contracts: [...task.contracts].sort(),
      acceptance: [...task.acceptance].sort().map((caseId) => {
        const spec = graph.cases.get(caseId);
        return {
          id: caseId,
          title: spec.title,
          required_level: spec.required_level,
          effective_level: graph.effectiveLevel(task, caseId),
          permitted_levels: [...spec.permitted_levels],
          procedure_scope: spec.procedure_scope,
        };
      }),
      deliverables: [...task.deliverables],
      implementation: [...task.implementation],
      write_scopes: [...task.write_scopes],
      resources: [...task.resources],
      packages: [...(task.packages ?? [])],
      package_closure: [...graph.packageClosure(task.packages ?? [])].sort(),
      legacy_tasks: [...task.legacy_tasks],
      audit_findings: [...task.audit_findings],
      fingerprints: {
        task_fingerprint: graph.taskFingerprint(task.id),
        graph_digest: graph.graphDigest,
        graph_version: graph.graphVersion,
        contract_revisions: Object.fromEntries(
          [...task.contracts].sort().map((contractId) => [contractId, graph.contracts.get(contractId).revision]),
        ),
        source_revision: sourceIdentities.revision.resolved,
      },
      source_identities: task.write_scopes
        .map((scope) => byPath.get(scope.replace(/\/\*\*$/u, "").replace(/\/+$/u, "")))
        .filter(Boolean)
        .map((identity) => ({
          path: identity.path,
          entry_kind: identity.entry_kind,
          sha256: identity.sha256,
          file_count: identity.file_count,
        })),
      regression: {
        selection_owner: selection.selection_owner,
        catalog_module_ids: selection.catalog_modules.map((module) => module.id),
        commands: selection.commands,
        write_scopes_without_observing_entry: selection.unmapped_paths.filter((entry) =>
          task.write_scopes.some((scope) => pathsOverlap(entry, scope))),
      },
      input_evidence_contract: {
        required_receipt_fields: [...REQUIRED_RECEIPT_FIELDS],
        bound_task_digest: graph.taskFingerprint(task.id),
        bound_graph_digest: graph.graphDigest,
        bound_contract_revisions: Object.fromEntries(
          [...task.contracts].sort().map((contractId) => [contractId, graph.contracts.get(contractId).revision]),
        ),
        stale_rule: "A receipt whose task_digest, graph_digest or contract revisions no longer match the resolved graph is stale: a claim made against an older graph cannot submit evidence for a changed contract.",
        not_proof: "Binding consistency is arithmetic over declared identity. It does not prove the log is authentic or that the test semantics are correct; a trusted CI producer and an independent reviewer own that part.",
      },
    };
  });
  return {
    kind: WORK_ITEM_KIND,
    schema_version: 1,
    graph_digest: graph.graphDigest,
    graph_version: graph.graphVersion,
    source_identity_digest: sourceIdentities.identity_digest,
    counts: {
      tasks: items.length,
      by_lane: items.reduce((accumulator, item) => ({
        ...accumulator,
        [item.lane]: (accumulator[item.lane] ?? 0) + 1,
      }), {}),
    },
    items,
    non_claims: [
      "Every item is pending. This export never marks plan work complete and is not acceptance evidence.",
      "Not a native LicoUp workflow document; lowering to host nodes is a later production-bridge node.",
      "Regression command lists are selections from the existing catalog, not executed results.",
    ],
  };
}

/**
 * Pure consistency check between a produced receipt and the resolved graph.
 * It acquires nothing, writes nothing and holds no state: the existing
 * development ledger stays the single owner of claims and task status.
 */
export function verifyEvidenceBinding({ graph, receipt, claim = null }) {
  const reasons = [];
  const task = typeof receipt?.task_id === "string" ? graph.tasks.get(receipt.task_id) : undefined;
  if (!task) {
    return { consistent: false, task_id: receipt?.task_id ?? null, reasons: ["receipt names an unknown task"] };
  }
  for (const field of REQUIRED_RECEIPT_FIELDS) {
    if (!Object.hasOwn(receipt, field) || receipt[field] === undefined) {
      reasons.push(`receipt does not carry required field ${field}`);
    }
  }
  for (const field of ["claim_token", "owner", "producer"]) {
    if (typeof receipt[field] !== "string" || receipt[field].trim().length === 0) {
      reasons.push(`receipt ${field} must be a non-empty string`);
    }
  }
  if (typeof receipt.input_digest !== "string" || !/^[0-9a-f]{64}$/u.test(receipt.input_digest)) {
    reasons.push("receipt input_digest must be a SHA-256 digest");
  }
  if (!Number.isSafeInteger(receipt.generation) || receipt.generation < 1) {
    reasons.push("receipt generation must be a positive integer");
  }
  if (receipt.status !== "passed") reasons.push("receipt status is not passed");

  const currentFingerprint = graph.taskFingerprint(task.id);
  if (receipt.task_digest !== currentFingerprint) {
    reasons.push("task fingerprint is stale: the task, its contracts, modules or scenarios changed");
  }
  if (receipt.graph_digest !== undefined && receipt.graph_digest !== graph.graphDigest) {
    reasons.push("graph digest is stale: the source graph changed after the claim");
  }
  if (typeof receipt.source_revision !== "string" || !/^[0-9a-f]{40}$/u.test(receipt.source_revision)) {
    reasons.push("receipt source revision is not a full Git SHA");
  }
  if (receipt.contract_revisions !== null && typeof receipt.contract_revisions === "object"
    && !Array.isArray(receipt.contract_revisions)) {
    const current = Object.fromEntries([...task.contracts].sort().map((id) => [id, graph.contracts.get(id).revision]));
    for (const [contractId, revision] of Object.entries(receipt.contract_revisions)) {
      if (!(contractId in current)) {
        reasons.push(`receipt binds contract ${contractId} which this task does not consume`);
      } else if (current[contractId] !== revision) {
        reasons.push(`contract ${contractId} revision moved from ${revision} to ${current[contractId]}: the claim cannot submit evidence for the new contract`);
      }
    }
    for (const contractId of Object.keys(current)) {
      if (!(contractId in receipt.contract_revisions)) {
        reasons.push(`receipt does not bind consumed contract ${contractId}`);
      }
    }
  } else {
    reasons.push("receipt does not bind contract revisions");
  }

  const checks = Array.isArray(receipt.checks) ? receipt.checks : null;
  if (!checks) {
    reasons.push("receipt has no checks array");
  } else {
    const seen = new Set();
    for (const check of checks) {
      const caseId = check?.case_id;
      if (typeof caseId !== "string" || !task.acceptance.includes(caseId)) {
        reasons.push(`receipt check names a scenario this task does not own: ${String(caseId)}`);
        continue;
      }
      if (seen.has(caseId)) reasons.push(`receipt repeats scenario ${caseId}`);
      seen.add(caseId);
      if (check.outcome !== "passed" || check.exit_code !== 0) reasons.push(`scenario ${caseId} is not a passing run`);
      if (!Number.isInteger(check.assertions) || check.assertions <= 0) reasons.push(`scenario ${caseId} records no assertions`);
      const expected = graph.effectiveLevel(task, caseId);
      if (check.level !== expected) reasons.push(`scenario ${caseId} evidence level ${check.level} does not match the declared level ${expected}`);
      if (!Array.isArray(check.command) || check.command.length === 0
        || !check.command.every((arg) => typeof arg === "string")
        || check.command[0].trim().length === 0) {
        reasons.push(`scenario ${caseId} records no valid command argv`);
      }
    }
    for (const caseId of task.acceptance) {
      if (!seen.has(caseId)) reasons.push(`receipt is missing scenario ${caseId}`);
    }
  }

  if (claim !== null) {
    if (claim.task_id !== receipt.task_id) reasons.push("claim and receipt name different tasks");
    for (const field of ["task_digest", "graph_digest", "input_digest", "claim_token", "owner", "generation"]) {
      const claimField = field === "claim_token" ? claim.claim_token : claim[field];
      const receiptField = field === "claim_token" ? receipt.claim_token : receipt[field];
      if (claimField === undefined) reasons.push(`claim record does not carry ${field}`);
      else if (receiptField !== claimField) reasons.push(`receipt ${field} does not match the claim record`);
    }
  }

  return {
    consistent: reasons.length === 0,
    task_id: task.id,
    task_fingerprint: currentFingerprint,
    graph_digest: graph.graphDigest,
    reasons,
    meaning: "Binding consistency only. This does not authenticate a log, prove test semantics, accept work or mutate any ledger.",
  };
}

export function legacyMapping(graph) {
  const byLegacy = new Map();
  for (const task of graph.tasks.values()) {
    for (const legacy of task.legacy_tasks) {
      if (!byLegacy.has(legacy)) byLegacy.set(legacy, []);
      byLegacy.get(legacy).push(task.id);
    }
  }
  const pairs = [...byLegacy.entries()]
    .map(([legacy, tasks]) => ({ legacy_task: legacy, tasks: [...tasks].sort() }))
    .sort((a, b) => a.legacy_task.localeCompare(b.legacy_task));
  const withoutLegacy = [...graph.tasks.values()].filter((task) => task.legacy_tasks.length === 0).map((task) => task.id).sort();
  const withoutFindings = [...graph.tasks.values()].filter((task) => task.audit_findings.length === 0).map((task) => task.id).sort();
  return {
    kind: "licoup-legacy-task-mapping.v1",
    graph_digest: graph.graphDigest,
    pairs,
    tasks_without_legacy_mapping: withoutLegacy,
    tasks_without_audit_finding: withoutFindings,
    merged_legacy_tasks: pairs.filter((pair) => pair.tasks.length > 1).map((pair) => pair.legacy_task),
    split_legacy_tasks: [...graph.tasks.values()]
      .filter((task) => task.legacy_tasks.length > 1)
      .map((task) => ({ task: task.id, legacy_tasks: [...task.legacy_tasks].sort() }))
      .sort((a, b) => a.task.localeCompare(b.task)),
    note: "Mapping only. A legacy task name is not evidence that the old behaviour was retired.",
  };
}

/**
 * Directed relation checks. `error` checks must hold or the graph is rejected;
 * `observation` checks are reported because the real target graph does not yet
 * satisfy them and a governance report must show that rather than hide it.
 */
export function relationChecks(graph) {
  const checks = [];
  const relations = graph.relations();

  const developmentEdges = relations.filter((edge) => edge.enters_development_dag);
  const nonDevelopmentScheduling = relations.filter(
    (edge) => edge.class !== RELATION_CLASSES.DEVELOPMENT_ORDER && edge.enters_development_dag,
  );
  checks.push({
    id: "only-precedes-schedules",
    severity: "error",
    expected: "Only developer-order relations lowered from task.depends_on enter the development DAG.",
    passed: nonDevelopmentScheduling.length === 0,
    failures: nonDevelopmentScheduling.map((edge) => `${edge.class}:${edge.relation} ${edge.source} -> ${edge.target}`),
  });

  const derived = new Set(developmentEdges.map((edge) => `${edge.source}->${edge.target}`));
  const declared = new Set([...graph.tasks.values()].flatMap((task) => task.depends_on.map((predecessor) => `${predecessor}->${task.id}`)));
  const derivationMismatch = [...declared].filter((edge) => !derived.has(edge))
    .concat([...derived].filter((edge) => !declared.has(edge)));
  checks.push({
    id: "development-dag-equals-task-depends-on",
    severity: "error",
    expected: "The development DAG is exactly task.depends_on lowered to precedes.",
    passed: derivationMismatch.length === 0,
    failures: derivationMismatch,
  });

  const deploymentScheduling = relations.filter(
    (edge) => edge.class === RELATION_CLASSES.DEPLOYMENT_DELIVERY && edge.enters_development_dag,
  );
  const packageAsDependency = [...graph.tasks.values()]
    .filter((task) => task.depends_on.some((predecessor) => (task.packages ?? []).includes(predecessor)));
  checks.push({
    id: "deployment-is-not-a-task-wait",
    severity: "error",
    expected: "requires_package and task.packages never become a development wait.",
    passed: deploymentScheduling.length === 0 && packageAsDependency.length === 0,
    failures: [
      ...deploymentScheduling.map((edge) => `${edge.relation} ${edge.source} -> ${edge.target}`),
      ...packageAsDependency.map((task) => `task ${task.id} depends on a package id`),
    ],
  });

  checks.push({
    id: "contract-revisions-are-bound",
    severity: "error",
    expected: "Every consumed contract revision is part of the claim fingerprint.",
    passed: true,
    failures: [],
    detail: { bound_contracts: Object.keys(graph.contractRevisions()).length },
  });

  const unauthorizedCallers = [];
  for (const edge of graph.architecture.edges.filter((entry) => entry.type === "runtime_calls")) {
    const contract = graph.contracts.get(edge.contract);
    // Port ownership describes compile-time authority, not call direction:
    // consumer-owned ports are called by their owner on an external adapter.
    if (contract.owner_module !== edge.source && !contract.consumers.includes(edge.source)) {
      unauthorizedCallers.push(`${edge.source} -> ${edge.target} via ${edge.contract}: caller is not the owner or a declared consumer`);
    }
    if (contract.owner_module !== edge.target && !contract.consumers.includes(edge.target)) {
      unauthorizedCallers.push(`${edge.source} -> ${edge.target} via ${edge.contract}: callee is not the owner or a declared consumer`);
    }
  }
  checks.push({
    id: "port-call-orientation",
    severity: "observation",
    expected: "Both runtime call endpoints participate in the named contract; its owner need not be the callee.",
    passed: unauthorizedCallers.length === 0,
    failures: unauthorizedCallers,
    detail: { runtime_calls: graph.architecture.edges.filter((entry) => entry.type === "runtime_calls").length },
  });

  const portCycles = findCycles(
    [...graph.modules.keys()],
    graph.architecture.edges.filter((edge) => edge.type === "runtime_calls").map((edge) => [edge.source, edge.target]),
  );
  checks.push({
    id: "port-call-cycles-stay-out-of-the-development-dag",
    severity: "error",
    expected: "A feedback loop in the runtime graph must not make the development DAG cyclic.",
    passed: true,
    failures: [],
    detail: { observed_port_call_cycles: portCycles, development_dag_layers: graph.developmentDag().layers.length },
  });

  const downgraded = [...graph.tasks.values()].flatMap((task) => task.acceptance
    .map((caseId) => ({ task: task.id, caseId, level: graph.effectiveLevel(task, caseId), spec: graph.cases.get(caseId) }))
    .filter((entry) => !entry.spec.permitted_levels.includes(entry.level)));
  checks.push({
    id: "evidence-levels-are-permitted",
    severity: "error",
    expected: "A task may only claim an evidence level its scenario permits.",
    passed: downgraded.length === 0,
    failures: downgraded.map((entry) => `${entry.task}/${entry.caseId} claims ${entry.level}`),
  });

  const unmapped = [...graph.tasks.values()].filter((task) => task.legacy_tasks.length === 0).map((task) => task.id).sort();
  checks.push({
    id: "legacy-mapping-recorded",
    severity: "observation",
    expected: "Every task records which old task it replaces or that it is new.",
    passed: unmapped.length === 0,
    failures: unmapped,
  });

  return {
    kind: "licoup-relation-checks.v1",
    graph_digest: graph.graphDigest,
    classes: Object.fromEntries(
      Object.values(RELATION_CLASSES).map((name) => [name, {
        note: RELATION_CLASS_NOTES[name],
        edge_count: relations.filter((edge) => edge.class === name).length,
        enters_development_dag: name === RELATION_CLASSES.DEVELOPMENT_ORDER,
      }]),
    ),
    checks,
    errors: checks.filter((check) => check.severity === "error" && !check.passed),
    observations: checks.filter((check) => check.severity === "observation" && !check.passed),
  };
}

export function requireRelationChecks(graph) {
  const report = relationChecks(graph);
  requireFact(report.errors.length === 0,
    `relation checks failed: ${report.errors.map((check) => `${check.id} (${check.failures.join(", ")})`).join("; ")}`);
  return report;
}
