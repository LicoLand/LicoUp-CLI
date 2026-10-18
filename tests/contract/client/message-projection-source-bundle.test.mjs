import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const projectionRoot =
  "crates/licoup-native/src/domain/conversation/history/message_projection";
const productionLeaves = Object.freeze([
  "antigravity.rs",
  "generated_context.rs",
  "json_extract.rs",
  "projection.rs",
  "semantic.rs",
  "structured_privacy.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${projectionRoot}/${leaf}`),
  ])));
}

test("message projection facade is thin and owns exactly six production leaves", async () => {
  const facade = await read(`${projectionRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    [...productionLeaves].sort(),
  );
  for (const implementation of [
    "fn plain_history_message",
    "fn sanitize_structured_event_text",
    "fn strip_generated_context_blocks",
    "fn extract_text_at_depth",
    "OnceLock<Regex>",
  ]) {
    assert.equal(facade.includes(implementation), false);
  }
});

test("projection leaves have explicit acyclic dependency direction", async () => {
  const source = await sources();
  for (const leaf of productionLeaves) {
    assert.equal(source[leaf].includes("use super::*"), false, `${leaf} has wildcard coupling`);
  }
  assert.equal(source["semantic.rs"].includes("super::"), false);
  assert.equal(source["antigravity.rs"].includes("super::"), false);
  assert.ok(source["generated_context.rs"].includes("super::antigravity"));
  assert.equal(source["generated_context.rs"].includes("super::semantic"), false);
  assert.ok(source["structured_privacy.rs"].includes("super::semantic"));
  for (const forbidden of ["super::projection", "super::json_extract"] ) {
    assert.equal(source["generated_context.rs"].includes(forbidden), false);
    assert.equal(source["structured_privacy.rs"].includes(forbidden), false);
  }
  assert.ok(source["json_extract.rs"].includes("super::generated_context"));
  assert.ok(source["projection.rs"].includes("super::structured_privacy"));
});

test("structured privacy has one cached regex and complete redaction authority", async () => {
  const source = (await sources())["structured_privacy.rs"];
  for (const token of [
    "OnceLock<Regex>",
    "Bearer [redacted]",
    "[local path hidden]",
    "[opaque value hidden]",
    "looks_like_raw_structured_payload",
  ]) {
    assert.ok(source.includes(token), `missing privacy boundary: ${token}`);
  }
  assert.equal(source.includes("MAX_STRUCTURED_EVENT_TEXT_CHARS"), false);
  assert.equal(source.includes("MAX_REASONING_SUMMARY_DEPTH"), false);
  assert.equal(source.includes("println!"), false);
  assert.equal(source.includes("eprintln!"), false);
  assert.equal(source.includes("dbg!"), false);
});

test("Antigravity and generated-context policies remain separate and fail closed", async () => {
  const source = await sources();
  const antigravity = source["antigravity.rs"];
  const generated = source["generated_context.rs"];
  for (const token of [
    "extract_user_request",
    "strip_system_messages",
    "strip_artifact_noise",
    "looks_like_artifact_dump",
    "ordered_list_line_regex",
  ]) {
    assert.ok(antigravity.includes(token), `missing Antigravity policy: ${token}`);
  }
  for (const token of [
    "strip_generated_context_blocks",
    "generated_context_block_close_marker",
    "background_context_prompt_text",
    "generated_control_text",
    '"<permissions instructions"',
    '"<local-command-output"',
  ]) {
    assert.ok(generated.includes(token), `missing generated-context policy: ${token}`);
  }
  const sessionMetadata = await read(
    "crates/licoup-native/src/domain/conversation/history/session_metadata.rs",
  );
  assert.equal(sessionMetadata.includes("fn generated_control_text"), false);
});

test("iterative JSON extraction is complete and keeps role, time, and session projection", async () => {
  const source = (await sources())["json_extract.rs"];
  for (const token of [
    "extract_text_iterative",
    "structured_content_object_is_tool_or_metadata",
    "parse_embedded_json_text",
    "fn extract_role",
    "fn extract_timestamp",
    "fn extract_native_session_id",
    "fn find_string",
  ]) {
    assert.ok(source.includes(token), `missing recursive extraction boundary: ${token}`);
  }
  assert.equal(source.includes("MAX_TEXT_EXTRACTION_DEPTH"), false);
});

test("semantic classification, title policy, and layer projection retain dedicated tests", async () => {
  const source = await sources();
  for (const token of [
    "enum HistoryMessageKind",
    "history_message_kind_from_semantic",
    "normalize_history_message_semantic",
  ]) {
    assert.ok(source["semantic.rs"].includes(token), `missing semantic authority: ${token}`);
  }
  assert.equal(source["semantic.rs"].includes("delegated_subagent_prompt_title"), false);
  const queryFilter = await read(
    "crates/licoup-native/src/domain/conversation/history/query_filter.rs",
  );
  assert.ok(queryFilter.includes("fn title_from_text"));
  assert.ok(source["projection.rs"].includes("SemanticLayer::Thread"));
  assert.ok(source["projection.rs"].includes("SemanticLayer::Execution"));
  assert.ok(source["projection.rs"].includes("clean_antigravity_message_text"));
  const testFacade = await read(`${projectionRoot}/tests/mod.rs`);
  assert.deepEqual(
    [...testFacade.matchAll(/^mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    [...productionLeaves].map((leaf) => leaf.replace(".rs", "")).sort(),
  );
});
