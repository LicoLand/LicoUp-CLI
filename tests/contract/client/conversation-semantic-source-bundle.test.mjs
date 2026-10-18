import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const semanticRoot =
  "crates/licoup-native/src/domain/conversation_semantic";
const productionLeaves = Object.freeze([
  "artifact_projection.rs",
  "builder.rs",
  "execution_projection.rs",
  "io.rs",
  "markdown.rs",
  "model.rs",
  "privacy.rs",
  "thread_projection.rs",
  "validation.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${semanticRoot}/${leaf}`),
  ])));
}

test("conversation semantic facade is thin and owns exactly nine production leaves", async () => {
  const facade = await read(`${semanticRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    [...productionLeaves].sort(),
  );
  for (const implementation of [
    "fn build_semantic_conversation(",
    "fn validate_semantic_conversation(",
    "fn sanitize_default_view_text(",
    "fn render_semantic_markdown(",
    "fs::write(",
  ]) {
    assert.equal(facade.includes(implementation), false);
  }
});

test("semantic leaves are bounded and use an explicit acyclic dependency direction", async () => {
  const source = await sources();
  for (const leaf of productionLeaves) {
    assert.equal(source[leaf].includes("use super::*"), false, `${leaf} has wildcard coupling`);
  }
  for (const independent of [
    "artifact_projection.rs",
    "markdown.rs",
    "privacy.rs",
    "thread_projection.rs",
  ]) {
    assert.equal(source[independent].includes("use super::"), false, independent);
  }
  assert.ok(source["execution_projection.rs"].includes("event_semantics::execution_event_kind"));
  assert.ok(source["validation.rs"].includes("super::model"));
  assert.ok(source["validation.rs"].includes("super::privacy"));
  assert.ok(source["builder.rs"].includes("super::validation"));
  assert.ok(source["io.rs"].includes("super::markdown"));
  assert.equal(source["privacy.rs"].includes("super::validation"), false);
  assert.equal(source["model.rs"].includes("super::builder"), false);
});

test("builder owns layer assembly while projections remain independently testable", async () => {
  const source = await sources();
  for (const token of [
    "build_semantic_conversation",
    "timeline_messages_from_semantic",
    "infer_layer_from_message",
    "append_thread_timeline",
    "append_execution_timeline",
    "privacy_defaults()",
    "validate_semantic_conversation(&semantic)",
  ]) {
    assert.ok(source["builder.rs"].includes(token), `missing builder boundary: ${token}`);
  }
  assert.ok(source["thread_projection.rs"].includes("thread_wire_message_from_tagged"));
  assert.ok(source["execution_projection.rs"].includes("execution_wire_message_from_tagged"));
  assert.ok(source["artifact_projection.rs"].includes("artifact_from_message"));
});

test("privacy and schema validation remain fail-closed and separately owned", async () => {
  const source = await sources();
  for (const token of [
    "sanitize_default_view_text",
    "assert_no_default_view_leakage",
    "redact_path_ref",
    "[redacted-token]",
    "[redacted-tool-args]",
  ]) {
    assert.ok(source["privacy.rs"].includes(token), `missing privacy boundary: ${token}`);
  }
  for (const token of [
    "validate_privacy_defaults",
    "validate_thread",
    "validate_execution",
    "validate_artifacts",
    "validate_audit",
    "validate_raw",
    "assert_no_default_view_leakage(value)",
  ]) {
    assert.ok(source["validation.rs"].includes(token), `missing schema boundary: ${token}`);
  }
  for (const leaf of productionLeaves) {
    assert.equal(source[leaf].includes("println!"), false);
    assert.equal(source[leaf].includes("eprintln!"), false);
    assert.equal(source[leaf].includes("dbg!"), false);
  }
});

test("Markdown rendering and fixture materialization are separate IO boundaries", async () => {
  const source = await sources();
  assert.ok(source["markdown.rs"].includes("render_semantic_markdown"));
  assert.ok(source["markdown.rs"].includes("## Audit (diagnostics)"));
  assert.equal(source["markdown.rs"].includes("fs::"), false);
  assert.ok(source["io.rs"].includes("materialize_semantic_documents"));
  assert.ok(source["io.rs"].includes("load_and_validate_fixture"));
  assert.ok(source["io.rs"].includes("fs::write"));
  assert.ok(source["io.rs"].includes("validate_semantic_conversation"));
});

test("every semantic leaf retains its own dedicated regression module", async () => {
  const testFacade = await read(`${semanticRoot}/tests/mod.rs`);
  assert.deepEqual(
    [...testFacade.matchAll(/^mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    [
      ...productionLeaves.map((leaf) => leaf.replace(".rs", "")),
      "composition",
    ].sort(),
  );
  for (const leaf of productionLeaves) {
    await fs.access(path.join(repoRoot, `${semanticRoot}/tests/${leaf}`));
  }
});
