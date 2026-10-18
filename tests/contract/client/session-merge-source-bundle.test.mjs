import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const mergeRoot =
  "crates/licoup-native/src/domain/conversation/history/session_merge";
const productionLeaves = Object.freeze([
  "codex_lineage.rs",
  "composition.rs",
  "dedupe_paging.rs",
  "delegated_merge.rs",
  "model_names.rs",
  "session_index.rs",
  "stable_order.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${mergeRoot}/${leaf}`),
  ])));
}

test("session merge facade is thin and owns exactly seven production leaves", async () => {
  const facade = await read(`${mergeRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    [...productionLeaves].sort(),
  );
  assert.equal(facade.includes("use super::*"), false);
  for (const implementation of [
    "fn merge_codex_rollout_lineage_sessions(",
    "fn merge_delegated_subagent_sessions(",
    "fn collect_history_model_names(",
    "fn read_codex_session_index_titles_file(",
    "fn sort_sessions_by_updated_at(",
  ]) {
    assert.equal(facade.includes(implementation), false);
  }
});

test("session merge leaves are bounded and use an explicit acyclic dependency direction", async () => {
  const source = await sources();
  for (const leaf of productionLeaves) {
    assert.equal(source[leaf].includes("use super::*"), false, `${leaf} has wildcard coupling`);
  }
  assert.equal(source["dedupe_paging.rs"].includes("super::stable_order"), false);
  assert.equal(source["model_names.rs"].includes("super::"), false);
  assert.ok(source["stable_order.rs"].includes("super::dedupe_paging"));
  assert.ok(source["codex_lineage.rs"].includes("super::dedupe_paging"));
  assert.ok(source["codex_lineage.rs"].includes("super::stable_order"));
  assert.ok(source["delegated_merge.rs"].includes("super::stable_order"));
  assert.ok(source["composition.rs"].includes("super::codex_lineage"));
  assert.ok(source["composition.rs"].includes("super::delegated_merge"));
});

test("delegated merge uses explicit leaf-to-root closure and bounded previews", async () => {
  const source = (await sources())["delegated_merge.rs"];
  for (const token of [
    "merge_explicit_parent_child_lineages",
    "parent_by_child",
    "remaining_children",
    "VecDeque",
    "saturating_sub",
    "MAX_SUBAGENT_PREVIEW_CHARS",
  ]) {
    assert.ok(source.includes(token), `missing delegated merge boundary: ${token}`);
  }
  assert.equal(source.includes("indexed_sessions.remove"), false);
  assert.equal(source.includes("loop {"), false);
});

test("Codex lineage keeps cycle-safe roots, replay dedupe, and deterministic collapse", async () => {
  const source = (await sources())["codex_lineage.rs"];
  for (const token of [
    "codex_rollout_lineage_parents",
    "codex_rollout_lineage_root",
    "visited",
    "collapse_codex_rollout_lineage_group",
    "merge_codex_lineage_messages",
    "codex_lineage_message_fingerprint",
    "Sha256",
    "lineageSessionIds",
  ]) {
    assert.ok(source.includes(token), `missing Codex lineage boundary: ${token}`);
  }
});

test("paging, model discovery, index IO, and stable ordering retain separate bounds", async () => {
  const source = await sources();
  for (const token of [
    "dedupe_history_sessions",
    "paged_history_sessions",
    "history_session_dedupe_key",
  ]) {
    assert.ok(source["dedupe_paging.rs"].includes(token));
  }
  for (const token of [
    "MAX_MODEL_DISCOVERY_DEPTH",
    "MAX_MODEL_NAME_CHARS",
    "MAX_MODEL_NAME_BYTES",
    "collect_history_model_names",
    "sanitize_history_model_name",
  ]) {
    assert.ok(source["model_names.rs"].includes(token));
  }
  for (const token of [
    "codex_session_index_candidates",
    "read_codex_session_index_titles_file",
    "parse_codex_session_index_titles",
  ]) {
    assert.ok(source["session_index.rs"].includes(token));
  }
  for (const token of [
    "history_time_order_key",
    "message_order_key",
    "session_updated_order_key",
    "sort_sessions_by_updated_at",
  ]) {
    assert.ok(source["stable_order.rs"].includes(token));
  }
});

test("every session merge leaf retains its own dedicated regression module", async () => {
  const testFacade = await read(`${mergeRoot}/tests/mod.rs`);
  assert.deepEqual(
    [...testFacade.matchAll(/^mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    [...productionLeaves].map((leaf) => leaf.replace(".rs", "")).sort(),
  );
  for (const leaf of productionLeaves) {
    await fs.access(path.join(repoRoot, `${mergeRoot}/tests/${leaf}`));
  }
});
