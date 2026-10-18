import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const parserRoot =
  "crates/licoup-native/src/domain/conversation/history/cursor_openagent";
const productionLeaves = Object.freeze([
  "codec.rs",
  "composition.rs",
  "cursor.rs",
  "cursor_cli.rs",
  "cursor_projection.rs",
  "fallback.rs",
  "openagent.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${parserRoot}/${leaf}`),
  ])));
}

test("Cursor and OpenAgent facade is thin and owns exactly seven production leaves", async () => {
  const facade = await read(`${parserRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^(?:pub\(super\)\s+)?mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    [...productionLeaves].sort(),
  );
  assert.equal(facade.includes("use super::*"), false);
  for (const implementation of [
    "Connection::open_with_flags",
    "fn parse_cursor_sqlite_sessions(",
    "fn parse_openagent_sqlite_sessions(",
    "fn parse_generic_sqlite_sessions(",
    "fn sqlite_value_text(",
  ]) {
    assert.equal(facade.includes(implementation), false);
  }
});

test("parser leaves are bounded and use an explicit acyclic dependency direction", async () => {
  const source = await sources();
  for (const leaf of productionLeaves) {
    const production = source[leaf].split("#[cfg(test)]", 1)[0];
    assert.equal(production.includes("use super::*"), false, `${leaf} has wildcard coupling`);
  }
  assert.equal(source["codec.rs"].includes("use super::"), false);
  assert.ok(source["cursor.rs"].includes("super::codec"));
  assert.ok(source["cursor.rs"].includes("super::cursor_projection"));
  assert.ok(source["cursor_cli.rs"].includes("super::codec"));
  assert.ok(source["cursor_cli.rs"].includes("super::cursor_projection"));
  assert.equal(source["cursor_projection.rs"].includes("super::cursor"), false);
  assert.ok(source["openagent.rs"].includes("super::codec"));
  assert.ok(source["fallback.rs"].includes("super::codec"));
  for (const dependency of ["codec", "cursor", "cursor_cli", "fallback", "openagent"]) {
    assert.ok(source["composition.rs"].includes(`super::${dependency}`));
  }
});

test("SQLite codec owns read-only access and complete field, value, and row projection", async () => {
  const source = (await sources())["codec.rs"];
  for (const token of [
    "SQLITE_OPEN_READ_ONLY",
    "SQLITE_OPEN_URI",
    "SQLITE_OPEN_NO_MUTEX",
    "sqlite_row_fields",
    "sqlite_value_text",
  ]) {
    assert.ok(source.includes(token), `missing SQLite codec boundary: ${token}`);
  }
  for (const retired of [
    "MAX_SQLITE_FIELDS_PER_ROW",
    "MAX_SQLITE_VALUE_BYTES",
    "MAX_SQLITE_ROW_BYTES",
  ]) {
    assert.equal(source.includes(retired), false);
  }
  assert.equal(source.includes("SQLITE_OPEN_READ_WRITE"), false);
  assert.equal(source.includes("SQLITE_OPEN_CREATE"), false);
});

test("Cursor SQL parsing and model/usage projection remain separate", async () => {
  const source = await sources();
  for (const token of [
    "parse_cursor_sqlite_sessions",
    "cursor_composer_rows",
    "cursor_bubble_ids_for_composer",
    "cursor_disk_kv_json",
    "TransactionBehavior::Deferred",
  ]) {
    assert.ok(source["cursor.rs"].includes(token), `missing Cursor parser boundary: ${token}`);
  }
  for (const token of [
    "cursor_message_from_bubble",
    "cursor_composer_model_from_config",
    "selectedModels",
    "cursor_bubble_usage",
    "normalize_cursor_model_name",
  ]) {
    assert.ok(source["cursor_projection.rs"].includes(token), `missing Cursor projection boundary: ${token}`);
  }
});

test("OpenAgent precise parsing and generic fallback retain independent row policies", async () => {
  const source = await sources();
  for (const token of [
    "parse_openagent_sqlite_sessions",
    "openagent_session_rows",
    "openagent_messages_for_session",
    "openagent_parts_by_message",
    "openagent_usage_from_columns",
    "TransactionBehavior::Deferred",
  ]) {
    assert.ok(source["openagent.rs"].includes(token), `missing OpenAgent boundary: ${token}`);
  }
  for (const token of [
    "parse_generic_sqlite_sessions",
    'SELECT * FROM',
    "rows.next()",
    "sqlite_row_fields",
    "sqlite_row_may_hold_history",
  ]) {
    assert.ok(source["fallback.rs"].includes(token), `missing fallback boundary: ${token}`);
  }
  for (const retired of [
    "MAX_SQLITE_ROWS_PER_TABLE",
    "ARCHIVE_SQLITE_PAGE_ROWS",
    "LIMIT {} OFFSET {}",
  ]) {
    assert.equal(source["fallback.rs"].includes(retired), false);
  }
});

test("every parser and codec leaf retains dedicated regression coverage", async () => {
  const testFacade = await read(`${parserRoot}/tests/mod.rs`);
  const externalTestLeaves = productionLeaves.filter((leaf) => leaf !== "cursor_cli.rs");
  assert.deepEqual(
    [...testFacade.matchAll(/^mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    externalTestLeaves.map((leaf) => leaf.replace(".rs", "")).sort(),
  );
  for (const leaf of externalTestLeaves) {
    await fs.access(path.join(repoRoot, `${parserRoot}/tests/${leaf}`));
  }
  assert.match((await sources())["cursor_cli.rs"], /#\[cfg\(test\)\]\s+mod tests \{/u);
});
