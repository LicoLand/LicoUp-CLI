import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";

import { loadGraph } from "../lib/graph-model.mjs";
import { buildWorkItems, legacyMapping, REQUIRED_RECEIPT_FIELDS, verifyEvidenceBinding } from "../lib/work-items.mjs";
import { CLIENT_MODULE_CATALOG } from "../../regression/client-module-catalog.mjs";
import { selectModulesForChangedPaths } from "../../regression/client-module-selection.mjs";
import { fixtureDistribution, fixtureGraph, REPOSITORY_ROOT, revalidateGraph, skipWithoutRealPlan } from "./fixtures.mjs";

const PROJECT = path.join(REPOSITORY_ROOT, "docs/plans/v7/graph/project.json");

function receiptFor(graph, taskId, overrides = {}) {
  const task = graph.tasks.get(taskId);
  const caseId = task.acceptance[0];
  return {
    task_id: taskId,
    input_digest: "b".repeat(64),
    claim_token: "fixture-claim",
    owner: "fixture-worker",
    generation: 1,
    status: "passed",
    source_revision: graph.architecture.baseline_commit,
    task_digest: graph.taskFingerprint(taskId),
    graph_digest: graph.graphDigest,
    contract_revisions: Object.fromEntries([...task.contracts].sort().map((id) => [id, graph.contracts.get(id).revision])),
    producer: "tools/architecture-graph/test",
    checks: [{
      case_id: caseId,
      outcome: "passed",
      exit_code: 0,
      assertions: 5,
      level: graph.effectiveLevel(task, caseId),
      command: ["node", "--test", "tools/architecture-graph/test/work-items.test.mjs"],
    }],
    ...overrides,
  };
}

test("work items never claim completion and stay a neutral development projection", () => {
  const draft = fixtureGraph({ distribution: fixtureDistribution() });
  draft.execution.tasks[0].packages = ["org.test.core"];
  const graph = revalidateGraph(draft);
  const workItems = buildWorkItems({ graph, repoRoot: REPOSITORY_ROOT });
  assert.equal(workItems.items.length, graph.tasks.size);
  assert.ok(workItems.items.every((item) => item.initial_status === "pending"));
  assert.ok(workItems.items.every((item) => item.kind === "development-work-items-not-native-workflow"));
  assert.ok(!JSON.stringify(workItems).includes("accepted"));
  assert.ok(workItems.non_claims.some((claim) => claim.includes("pending")));
  const item = workItems.items.find((entry) => entry.id === "T01");
  assert.equal(item.relation_class, "development-order");
  assert.deepEqual(item.acceptance.map((entry) => `${entry.id}@${entry.effective_level}`), ["A01@tool-test"]);
  assert.deepEqual(item.fingerprints.graph_version.baseline_commit, graph.architecture.baseline_commit);
  assert.deepEqual(REQUIRED_RECEIPT_FIELDS, item.input_evidence_contract.required_receipt_fields);
  assert.deepEqual(item.package_closure.sort(), ["org.test.core"]);
});

test("the available local plan work items project the bound graph identity", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const workItems = buildWorkItems({ graph, repoRoot: REPOSITORY_ROOT });
  assert.equal(workItems.items.length, graph.tasks.size);
  const item = workItems.items.find((entry) => entry.id === "V7-G1");
  assert.equal(item.relation_class, "development-order");
  assert.deepEqual(item.acceptance.map((entry) => `${entry.id}@${entry.effective_level}`), ["A27@tool-test"]);
  assert.deepEqual(item.fingerprints.graph_version.baseline_commit, graph.architecture.baseline_commit);
  assert.equal(item.fingerprints.task_fingerprint, graph.taskFingerprint("V7-G1"));
});

test("work items carry the existing regression selection for their own write scopes", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  const workItems = buildWorkItems({ graph, repoRoot: REPOSITORY_ROOT });
  const item = workItems.items.find((entry) => entry.id === "T01");
  // The carried selection is the live answer of the existing selection owner
  // for this task's declared write scopes, so a catalog registration elsewhere
  // changes the item rather than breaking a frozen expectation here.
  const expected = selectModulesForChangedPaths([...item.write_scopes], CLIENT_MODULE_CATALOG).map((module) => module.id).sort();
  assert.deepEqual([...item.regression.catalog_module_ids].sort(), expected);
  assert.equal(item.regression.selection_owner, "tools/regression/client-module-selection.mjs");
  assert.deepEqual(
    item.regression.write_scopes_without_observing_entry,
    item.write_scopes.filter((scope) => selectModulesForChangedPaths([scope], CLIENT_MODULE_CATALOG).length === 0),
  );
  assert.ok(item.source_identities.every((identity) => identity.entry_kind === "absent"),
    "a declared but unrealized write scope is reported absent rather than silently hashed");
});

test("the available local plan task scopes are observed by the existing catalog", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const workItems = buildWorkItems({ graph, repoRoot: REPOSITORY_ROOT });
  const item = workItems.items.find((entry) => entry.id === "V7-G1");
  const expected = selectModulesForChangedPaths([...item.write_scopes], CLIENT_MODULE_CATALOG).map((module) => module.id).sort();
  assert.deepEqual([...item.regression.catalog_module_ids].sort(), expected);
  assert.ok(expected.includes("regression.documentation-governance"),
    "the regression module this task declares must be observed by the existing catalog");
  assert.ok(item.source_identities.some((identity) => identity.path === "tools/architecture-graph/" && identity.entry_kind === "directory"));
});

test("fingerprints are stable across runs and move when the graph moves", () => {
  const graph = fixtureGraph();
  assert.equal(graph.taskFingerprint("T01"), graph.taskFingerprint("T01"));
  const before = graph.taskFingerprint("T01");
  const mutated = fixtureGraph();
  const original = mutated.taskFingerprint("T01");
  mutated.contracts.get("C01").revision = 2;
  assert.notEqual(revalidateGraph(mutated).taskFingerprint("T01"), original);
  assert.notEqual(before, graph.taskFingerprint("T02"));
});

test("a consistent receipt binds and an old claim cannot submit evidence for a new contract", () => {
  const graph = fixtureGraph();
  const receipt = receiptFor(graph, "T01");
  const accepted = verifyEvidenceBinding({ graph, receipt });
  assert.equal(accepted.consistent, true, accepted.reasons.join("; "));

  // The contract moves after the claim was made.
  graph.contracts.get("C01").revision = 2;
  const afterDrift = verifyEvidenceBinding({ graph: revalidateGraph(graph), receipt });
  assert.equal(afterDrift.consistent, false);
  assert.ok(afterDrift.reasons.some((reason) => /cannot submit evidence for the new contract/u.test(reason)));
  assert.ok(afterDrift.reasons.some((reason) => /fingerprint is stale/u.test(reason)));

  const pinned = verifyEvidenceBinding({
    graph: fixtureGraph(),
    receipt: { ...receipt, contract_revisions: { C01: 1, C12: 1 } },
  });
  assert.equal(pinned.consistent, false);
  assert.ok(pinned.reasons.some((reason) => reason.includes("does not consume")));

  const unbound = verifyEvidenceBinding({ graph: fixtureGraph(), receipt: { ...receipt, contract_revisions: undefined } });
  assert.ok(unbound.reasons.some((reason) => reason.includes("does not bind contract revisions")));
});

test("tampered evidence is rejected field by field", () => {
  const graph = fixtureGraph();
  const base = receiptFor(graph, "T01");
  const tampering = {
    "not passed": { status: "unverified" },
    "no assertions": { checks: [{ ...base.checks[0], assertions: 0 }] },
    "skipped scenario": { checks: [{ ...base.checks[0], outcome: "skipped" }] },
    "non zero exit": { checks: [{ ...base.checks[0], exit_code: 1 }] },
    "wrong level": { checks: [{ ...base.checks[0], level: "production-profile" }] },
    "missing argv": { checks: [{ ...base.checks[0], command: [] }] },
    "unowned scenario": { checks: [{ ...base.checks[0], case_id: "A99" }] },
    "missing scenario": { checks: [] },
    "prose instead of a revision": { source_revision: "HEAD" },
    "unknown task": { task_id: "T99" },
  };
  for (const [label, override] of Object.entries(tampering)) {
    const result = verifyEvidenceBinding({ graph, receipt: { ...base, ...override } });
    assert.equal(result.consistent, false, `${label} must be rejected`);
    assert.ok(result.reasons.length > 0, `${label} must explain why`);
  }
  assert.equal(verifyEvidenceBinding({ graph, receipt: { ...base, task_digest: "0".repeat(64) } }).consistent, false);
  assert.equal(verifyEvidenceBinding({ graph, receipt: { ...base, graph_digest: "0".repeat(64) } }).consistent, false);
  const repeated = verifyEvidenceBinding({ graph, receipt: { ...base, checks: [base.checks[0], base.checks[0]] } });
  assert.ok(repeated.reasons.some((reason) => reason.includes("repeats scenario")));
});

test("receipt binding requires every declared receipt field even without a claim record", () => {
  const graph = fixtureGraph();
  const base = receiptFor(graph, "T01");
  for (const field of REQUIRED_RECEIPT_FIELDS) {
    const receipt = { ...base };
    delete receipt[field];
    assert.equal(verifyEvidenceBinding({ graph, receipt }).consistent, false,
      `a receipt missing ${field} must be rejected`);
  }
});

test("malformed receipt fields produce a rejection rather than success or a TypeError", () => {
  const graph = fixtureGraph();
  const base = receiptFor(graph, "T01");
  const overrides = [
    { input_digest: "not-a-digest" },
    { claim_token: "" },
    { owner: " " },
    { generation: 0 },
    { generation: 1.5 },
    { generation: "1" },
    { producer: "" },
    { producer: {} },
    { contract_revisions: null },
    { contract_revisions: [] },
    { checks: [null] },
    { checks: [{ ...base.checks[0], command: [null] }] },
    { checks: [{ ...base.checks[0], command: [""] }] },
  ];
  for (const override of overrides) {
    assert.equal(verifyEvidenceBinding({ graph, receipt: { ...base, ...override } }).consistent,
      false, `malformed ${Object.keys(override)[0]} must be rejected`);
  }
});

test("a stale claim record cannot be paired with a fresh receipt", () => {
  const graph = fixtureGraph();
  const receipt = receiptFor(graph, "T01");
  const claim = {
    task_id: "T01",
    claim_token: "token-a",
    owner: "worker-a",
    generation: 3,
    task_digest: receipt.task_digest,
    graph_digest: receipt.graph_digest,
    input_digest: "b".repeat(64),
  };
  const paired = { ...receipt, claim_token: "token-a", owner: "worker-a", generation: 3, input_digest: "b".repeat(64) };
  assert.equal(verifyEvidenceBinding({ graph, receipt: paired, claim }).consistent, true);

  const oldGeneration = verifyEvidenceBinding({
    graph,
    receipt: { ...paired, generation: 2 },
    claim,
  });
  assert.equal(oldGeneration.consistent, false);
  assert.ok(oldGeneration.reasons.some((reason) => reason.includes("generation")));

  const missingField = verifyEvidenceBinding({
    graph,
    receipt: paired,
    claim: { task_id: "T01", claim_token: "token-a" },
  });
  assert.ok(missingField.reasons.some((reason) => reason.includes("claim record does not carry")));
});

test("legacy mapping records merges, splits and tasks with no old name", () => {
  const graph = fixtureGraph();
  graph.execution.tasks[0].legacy_tasks = ["legacy-shared"];
  graph.execution.tasks[1].legacy_tasks = ["legacy-shared", "legacy-T02"];
  graph.execution.tasks.push({ ...graph.execution.tasks[0], id: "T03", legacy_tasks: [], write_scopes: ["scopes/T03/"] });
  const mapping = legacyMapping(revalidateGraph(graph));
  assert.deepEqual(mapping.tasks_without_legacy_mapping, ["T03"]);
  assert.deepEqual(mapping.merged_legacy_tasks, ["legacy-shared"]);
  assert.deepEqual(mapping.split_legacy_tasks, [{ task: "T02", legacy_tasks: ["legacy-T02", "legacy-shared"] }]);
  assert.ok(mapping.note.includes("not evidence"));
});

test("the available local plan records merged legacy tasks", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const repository = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const repositoryMapping = legacyMapping(repository);
  assert.ok(repositoryMapping.pairs.length > 0);
  assert.ok(repositoryMapping.merged_legacy_tasks.length > 0, "the repository graph merges old tasks into multiple work items");
});
