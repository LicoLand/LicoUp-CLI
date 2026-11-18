import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { loadGraph } from "../lib/graph-model.mjs";
import { collectDeclaredIdentities, resolveSourceIdentities } from "../lib/source-identity.mjs";
import { fixtureDocuments, fixtureGraph, graphFromDocuments, REPOSITORY_ROOT, skipWithoutRealPlan } from "./fixtures.mjs";

const PROJECT = path.join(REPOSITORY_ROOT, "docs/plans/v7/graph/project.json");

function temporaryRoot() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "architecture-graph-source-"));
}

/**
 * A source-list implementation with the same contract as the Git one:
 * `(repoRoot, directoryRelativeToRepoRoot) -> sorted repository-relative files`.
 */
function sourceLister(repositoryRoot) {
  return (_repoRoot, directory) => {
    const absolute = path.join(repositoryRoot, directory);
    if (!fs.existsSync(absolute)) return [];
    return fs.readdirSync(absolute)
      .filter((name) => !name.startsWith("."))
      .sort()
      .map((name) => `${directory}/${name}`);
  };
}

test("declared identities separate module projections from task write scopes", () => {
  const graph = fixtureGraph();
  const declared = collectDeclaredIdentities(graph);
  assert.ok(declared.declared.has("src/provider/"));
  assert.ok(declared.declared.has("src/consumer/"));
  assert.ok(declared.declared.has("scopes/T01/"));
  const owners = declared.declared.get("src/provider/");
  assert.deepEqual(owners, [{ kind: "module", id: "M01" }]);
  assert.equal(declared.writeLockOwners.get("scopes/T01/"), "T01");
  assert.equal(declared.writeLockOwners.get("src/provider/"), undefined);
});

test("file, directory and absent identities are resolved distinctly and deterministically", () => {
  const root = temporaryRoot();
  fs.mkdirSync(path.join(root, "src/provider"), { recursive: true });
  fs.mkdirSync(path.join(root, "src/consumer"), { recursive: true });
  fs.writeFileSync(path.join(root, "src/provider/lib.rs"), "pub fn one() {}\n");
  fs.writeFileSync(path.join(root, "src/provider/extra.rs"), "pub fn two() {}\n");
  fs.writeFileSync(path.join(root, "src/consumer/lib.rs"), "pub fn three() {}\n");

  const graph = fixtureGraph();
  const listSources = sourceLister(root);
  const first = resolveSourceIdentities({ graph, repoRoot: root, listSources });
  const second = resolveSourceIdentities({ graph, repoRoot: root, listSources });
  assert.deepEqual(first, second, "identity resolution must be reproducible");

  const provider = first.identities.find((identity) => identity.path === "src/provider/");
  assert.equal(provider.entry_kind, "directory");
  assert.equal(provider.file_count, 2);
  assert.equal(provider.source_list_method, "git-ls-files-cached-and-untracked-excluding-ignored");
  assert.equal(provider.write_lock.kind, "module-projection");
  assert.equal(provider.write_lock.lock, false);

  const taskScope = first.identities.find((identity) => identity.path === "scopes/T01/");
  assert.equal(taskScope.entry_kind, "absent", "a declared but unrealized scope is reported as absent, not as empty content");
  assert.equal(taskScope.sha256, null);
  assert.equal(taskScope.write_lock.kind, "task-write-scope");
  assert.equal(taskScope.write_lock.lock, true);

  fs.writeFileSync(path.join(root, "src/consumer/lib.rs"), "pub fn three() { /* changed */ }\n");
  const afterEdit = resolveSourceIdentities({ graph, repoRoot: root, listSources });
  const consumer = afterEdit.identities.find((identity) => identity.path === "src/consumer/");
  const consumerBefore = first.identities.find((identity) => identity.path === "src/consumer/");
  assert.notEqual(consumer.sha256, consumerBefore.sha256, "editing a source file must move its identity digest");
  assert.notEqual(afterEdit.identity_digest, first.identity_digest);

  fs.rmSync(root, { recursive: true, force: true });
});

test("an explicit revision is recorded and flagged when it differs from the declared baseline", () => {
  const graph = fixtureGraph();
  const declared = resolveSourceIdentities({ graph, repoRoot: REPOSITORY_ROOT, listSources: () => [] });
  assert.equal(declared.revision.drift, false);
  assert.equal(declared.revision.resolved, graph.architecture.baseline_commit);

  const pinned = resolveSourceIdentities({
    graph,
    repoRoot: REPOSITORY_ROOT,
    revision: "a".repeat(40),
    listSources: () => [],
  });
  assert.equal(pinned.revision.resolved, "a".repeat(40));
  assert.equal(pinned.revision.declared, graph.architecture.baseline_commit);
  assert.equal(pinned.revision.drift, true);
  assert.ok(pinned.identities.every((identity) => identity.revision === "a".repeat(40)));

  assert.throws(
    () => resolveSourceIdentities({ graph, repoRoot: REPOSITORY_ROOT, revision: "HEAD", listSources: () => [] }),
    /full 40-character Git SHA/u,
  );
});

test("source identities refuse file and ancestor symlinks without exposing their targets", (t) => {
  const root = temporaryRoot();
  const outside = temporaryRoot();
  t.after(() => {
    fs.rmSync(root, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  });
  fs.mkdirSync(path.join(root, "src/provider"), { recursive: true });
  fs.writeFileSync(path.join(outside, "synthetic.rs"), "synthetic outside content");
  const graph = fixtureGraph();
  const resolve = () => resolveSourceIdentities({ graph, repoRoot: root, listSources: sourceLister(root) });
  const rejectsPrivately = () => assert.throws(resolve, (error) => {
    assert.equal(error.message, "source identity refuses symbolic links");
    assert.equal(error.message.includes(outside), false);
    return true;
  });
  const link = path.join(root, "src/provider/link.rs");
  fs.symlinkSync(path.join(outside, "synthetic.rs"), link);
  rejectsPrivately();
  fs.unlinkSync(link);
  fs.symlinkSync(outside, path.join(root, "src/consumer"));
  rejectsPrivately();
  fs.unlinkSync(path.join(root, "src/consumer"));
  fs.rmSync(path.join(root, "src"), { recursive: true });
  fs.symlinkSync(outside, path.join(root, "src"));
  rejectsPrivately();
});

test("source listings cannot supply files outside the declared directory", (t) => {
  const root = temporaryRoot();
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "src/provider"), { recursive: true });
  assert.throws(() => resolveSourceIdentities({
    graph: fixtureGraph(), repoRoot: root, listSources: () => ["other/file.rs"],
  }), /source listing leaves its declared directory/u);
});

test("a prose module path stays logical instead of being treated as a repository path", () => {
  const documents = fixtureDocuments();
  documents.architecture.modules.push({
    ...documents.architecture.modules[0],
    id: "M04",
    title: "Logical projection",
    path: "Cross-cutting contract: prose, not a repository path",
  });
  const graph = graphFromDocuments(documents);
  const identities = resolveSourceIdentities({ graph, repoRoot: REPOSITORY_ROOT, listSources: () => [] });
  const logical = identities.logical_declarations.map((entry) => entry.module);
  assert.deepEqual(logical, ["M04"]);
  assert.ok(identities.gaps.some((gap) => gap.kind === "logical-module-path"));
  assert.ok(identities.identities.every((identity) => identity.path !== "Cross-cutting contract: prose, not a repository path"),
    "a prose path must never become a hashed source identity");
});

test("the available local plan graph stays logical where a module path is prose", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const identities = resolveSourceIdentities({ graph, repoRoot: REPOSITORY_ROOT });
  const logical = identities.logical_declarations.map((entry) => entry.module);
  assert.ok(logical.length > 0, "the repository graph contains at least one logical module path");
  assert.ok(identities.gaps.some((gap) => gap.kind === "logical-module-path"));
  assert.equal(identities.revision.resolved, graph.architecture.baseline_commit);
});

test("the available local plan graph resolves every declared source identity without a machine path", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const resolved = resolveSourceIdentities({ graph, repoRoot: REPOSITORY_ROOT });
  const graphDirectory = resolved.identities.find((identity) => identity.path === "tools/architecture-graph/");
  assert.ok(graphDirectory, "the tool's own module path must resolve");
  assert.equal(graphDirectory.entry_kind, "directory");
  assert.ok(graphDirectory.file_count > 0);
  assert.equal(graphDirectory.owner_tasks.includes("V7-G1"), true);
  assert.ok(resolved.identities.every((identity) => !path.isAbsolute(identity.path)));
});
