// Read-only counterpart of Foundation's sqlite_contract. SQL remains owned by
// the stores; fixtures are never imported into the diagnostic implementation.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { MigrationStateError } from "./errors.mjs";
import { requireValue } from "./util.mjs";

const refused = () => { throw new MigrationStateError("unsupported_state_shape"); };
const same = (left, right) => JSON.stringify(left) === JSON.stringify(right);

function memoryDatabase() {
  try {
    const { DatabaseSync } = createRequire(import.meta.url)("node:sqlite");
    return new DatabaseSync(":memory:");
  } catch { throw new MigrationStateError("probe_capability_unavailable"); }
}

export function sqlTokens(sql) {
  const result = [];
  const pattern = /\s+|--[^\n]*(?:\n|$)|\/\*[\s\S]*?\*\/|'(?:[^']|'')*'|"(?:[^"]|"")*"|`(?:[^`]|``)*`|\[[^\]]*\]|[\p{L}\p{N}_]+|[^\s]/guy;
  let position = 0;
  while (position < sql.length) {
    pattern.lastIndex = position;
    const match = pattern.exec(sql);
    if (!match) refused();
    position = pattern.lastIndex;
    const token = match[0];
    if (/^(?:\s|--|\/\*)/u.test(token)) continue;
    if (token.startsWith("'")) result.push(token);
    else if (/^["`[]/u.test(token)) {
      const quote = token[0];
      result.push(token.slice(1, -1).replaceAll(quote + quote, quote).toLowerCase());
    } else result.push(token.toLowerCase());
  }
  while (result.at(-1) === ";") result.pop();
  return result;
}

function declarations(sql) {
  const tokens = sqlTokens(sql);
  const using = tokens.indexOf("using");
  if (using !== -1) return [tokens.slice(using)];
  let start = tokens.indexOf("(") + 1;
  if (start === 0) refused();
  let depth = 1;
  const entries = [];
  for (let index = start; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (token === "(") depth += 1;
    else if (token === ")") depth -= 1;
    else if (token === "," && depth === 1) {
      entries.push(tokens.slice(start, index)); start = index + 1;
    }
    if (depth === 0) {
      entries.push(tokens.slice(start, index));
      entries.sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b), "en"));
      entries.push(tokens.slice(index + 1));
      return entries;
    }
  }
  refused();
}

function tableSql(database, table) {
  return database.prepare("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?").get(table)?.sql ?? null;
}

export function tableNames(database) {
  return database.prepare("SELECT name FROM pragma_table_list WHERE schema='main' AND type IN ('table','virtual') AND name NOT LIKE 'sqlite_%' ORDER BY name").all().map((row) => row.name);
}

function definitions(database, table, kind) {
  return database.prepare("SELECT name,sql FROM sqlite_schema WHERE type=? AND tbl_name=? AND sql IS NOT NULL ORDER BY name").all(kind, table).map((row) => {
    const tokens = sqlTokens(row.sql);
    const index = tokens.findIndex((token, i) => token === "if" && tokens[i + 1] === "not" && tokens[i + 2] === "exists");
    if (index !== -1) tokens.splice(index, 3);
    return [row.name, tokens];
  });
}

export function requireTable(database, reference, table, variant = {}) {
  const sql = tableSql(database, table);
  if (sql === null) refused();
  const expected = declarations(tableSql(reference, table));
  const actual = declarations(sql);
  const columns = (db) => db.prepare("SELECT name,type,\"notnull\",dflt_value,pk,hidden FROM pragma_table_xinfo(?) ORDER BY name").all(table).map((row) => [row.name, row.type.toUpperCase(), Number(row.notnull), row.dflt_value === null ? null : sqlTokens(row.dflt_value), Number(row.pk), Number(row.hidden)]);
  const actualColumns = columns(database);
  const expectedColumns = columns(reference);
  const nullableVariant = variant.nullable ?? null;
  if (nullableVariant !== null && actualColumns.find((row) => row[0] === nullableVariant)?.[2] === 0) {
    expectedColumns.find((row) => row[0] === nullableVariant)[2] = 0;
    const entry = expected.find((entry) => entry[0] === nullableVariant);
    const index = entry.findIndex((token, i) => token === "not" && entry[i + 1] === "null");
    if (index !== -1) entry.splice(index, 2);
  }
  const missingZeroDefault = variant.missingZeroDefault ?? null;
  if (missingZeroDefault !== null && actualColumns.find((row) => row[0] === missingZeroDefault)?.[3] === null) {
    const column = expectedColumns.find((row) => row[0] === missingZeroDefault);
    requireValue(same(column?.[3], ["0"]), "unsupported_state_shape");
    column[3] = null;
    const entry = expected.find((entry) => entry[0] === missingZeroDefault);
    const index = entry.findIndex((token, i) => token === "default" && entry[i + 1] === "0");
    requireValue(index !== -1, "unsupported_state_shape");
    entry.splice(index, 2);
  }
  requireValue(same(actual, expected) && same(actualColumns, expectedColumns), "unsupported_state_shape");
  requireValue(same(definitions(database, table, "index"), definitions(reference, table, "index")), "unsupported_state_shape");
  requireValue(same(definitions(database, table, "trigger"), definitions(reference, table, "trigger")), "unsupported_state_shape");
  // Unowned relations constrain their own operations, not admission of this
  // owner's required layout. Do not inspect or modify their retained rows.
}

function source(relative) {
  try { return readFileSync(new URL(`../../../${relative}`, import.meta.url), "utf8"); }
  catch { throw new MigrationStateError("probe_capability_unavailable"); }
}

function literal(text, anchor) {
  const at = text.indexOf(anchor);
  const begin = text.lastIndexOf('"', at);
  const end = text.indexOf('"', at);
  if (at === -1 || begin === -1 || end <= begin) throw new MigrationStateError("probe_capability_unavailable");
  return text.slice(begin + 1, end).replaceAll("\\n", "\n").replaceAll('\\"', '"');
}

function functionSource(text, name) {
  const begin = text.indexOf(`fn ${name}(`);
  const next = text.indexOf("\nfn ", begin + 1);
  if (begin === -1) throw new MigrationStateError("probe_capability_unavailable");
  return text.slice(begin, next === -1 ? undefined : next);
}

function ensureColumn(database, table, column, declaration) {
  if (!database.prepare("SELECT name FROM pragma_table_info(?)").all(table).some((row) => row.name === column)) {
    database.exec(`ALTER TABLE ${table} ADD COLUMN ${column} ${declaration};`);
  }
}

function ensureColumns(database, text) {
  for (const match of text.matchAll(/ensure_column\(\s*connection,\s*"(\w+)",\s*"(\w+)",\s*"([^"]+)"\s*,?\s*\)/gu)) {
    ensureColumn(database, match[1], match[2], match[3]);
  }
}

function ensureMigrationColumn(database, text, migration, table, column) {
  if (!database.prepare("SELECT name FROM pragma_table_info(?)").all(table).some((row) => row.name === column)) {
    database.exec(literal(functionSource(text, migration), `ALTER TABLE ${table} ADD COLUMN ${column}`));
  }
}

const conversationSource = () => source("crates/licoup-conversation/src/store/schema.rs");
function currentConversationLayout(database, text) {
  database.exec(literal(text, "CREATE TABLE IF NOT EXISTS principals"));
  database.exec(literal(source("crates/licoup-conversation/src/store/native_sessions.rs"), "CREATE TABLE IF NOT EXISTS conversation_native_sessions"));
  database.exec(literal(source("crates/licoup-conversation/src/store/admission_keys.rs"), "CREATE TABLE IF NOT EXISTS event_admission_keys"));
  const current = functionSource(text, "ensure_current_layout");
  ensureColumns(database, current);
  database.exec(literal(current, "CREATE INDEX IF NOT EXISTS conversation_dispatches_native_provenance_idx"));
  database.exec(literal(text, "DROP INDEX IF EXISTS memberships_active_unique"));
}

function conversationReference() {
  const database = memoryDatabase();
  try {
    const text = conversationSource();
    database.exec(literal(text, "CREATE TABLE IF NOT EXISTS schema_meta"));
    currentConversationLayout(database, text);
    return database;
  } catch (error) { database.close(); throw error; }
}

function continuityContract(reference, actual) {
  const owner = source("crates/licoup-conversation/src/continuity/migrate.rs");
  const start = owner.indexOf("const STATEMENTS:");
  const end = owner.indexOf("\n];", start);
  if (start === -1 || end === -1) throw new MigrationStateError("probe_capability_unavailable");
  const base = tableNames(reference);
  for (const match of owner.slice(start, end).matchAll(/"((?:[^"\\]|\\.)*)"/gu)) {
    reference.exec(match[1].replaceAll("\\n", "\n").replaceAll('\\"', '"'));
  }
  const extension = tableNames(reference).filter((table) => !base.includes(table));
  const existing = tableNames(actual);
  const epoch = actual.prepare("SELECT name FROM pragma_table_info('conversations')").all().some((column) => column.name === "designation_epoch");
  if (epoch) {
    reference.exec(literal(owner, "ALTER TABLE conversations ADD COLUMN designation_epoch"));
    reference.exec(literal(owner, "CREATE TRIGGER continuity_bump_designation_epoch"));
  }
  if (!epoch && !extension.some((table) => existing.includes(table))) return base;
  const version = actual.prepare("SELECT value FROM continuity_schema WHERE key='version'").get()?.value;
  const current = Number(owner.match(/CURRENT_CONTINUITY_SCHEMA_VERSION:\s*u32\s*=\s*(\d+)/u)?.[1]);
  if (!Number.isSafeInteger(current)) throw new MigrationStateError("probe_capability_unavailable");
  requireValue(/^\d+$/u.test(version ?? "") && Number.isSafeInteger(Number(version)) && Number(version) <= current, "unsupported_state_shape");
  if (Number(version) === current) requireValue(epoch, "unsupported_state_shape");
  return [...base, ...extension.filter((table) => Number(version) === current || existing.includes(table))];
}

function copySchema(databaseSource) {
  const database = memoryDatabase();
  try {
    const names = new Set(tableNames(databaseSource));
    for (const row of databaseSource.prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'index' THEN 1 WHEN 'trigger' THEN 2 ELSE 3 END").all()) {
      if (row.type === "table" && !names.has(row.name)) continue;
      if (row.type === "trigger") refused();
      requireValue(!sqlTokens(row.sql).includes(";"), "unsupported_state_shape");
      database.exec(row.sql);
    }
    return database;
  } catch (error) { database.close(); throw error; }
}

// The structural part of the real owner upgrade. Reads the owner's actual SQL
// statements; data-dependent conversions intentionally remain apply/retry work.
function upgradeConversationLayout(database, version, text) {
  database.exec(literal(text, "CREATE TABLE IF NOT EXISTS principals"));
  if (version <= 4) ensureMigrationColumn(database, text, "migrate_runtime_replay_v5", "event_parts", "runtime_cursor");
  if (version <= 5) ensureMigrationColumn(database, text, "migrate_strategy_selection_v6", "conversations", "strategy_revision");
  if (version <= 6) ensureMigrationColumn(database, text, "migrate_assistant_profile_v7", "conversations", "assistant_membership_id");
  if (version <= 7) {
    const columns = database.prepare("SELECT name FROM pragma_table_info('membership_profiles')").all().map((row) => row.name);
    if (!columns.includes("required_capabilities") || columns.includes("authority")) {
      const migration = functionSource(text, "migrate_profile_intent_v8");
      database.exec(literal(migration, "ALTER TABLE membership_profiles RENAME"));
      database.exec(literal(migration, "DROP TABLE membership_profiles_v7_draft"));
    }
  }
  if (version <= 8) ensureMigrationColumn(database, text, "migrate_profile_reasoning_effort_v9", "membership_profiles", "preferred_reasoning_effort");
  if (version < 12) ensureColumns(database, functionSource(text, "initialize_schema_unchecked"));
  currentConversationLayout(database, text);
}

export function inspectConversationContract(database, currentVersion) {
  const names = tableNames(database);
  const reference = conversationReference();
  let version = names.includes("schema_meta")
    ? database.prepare("SELECT value FROM schema_meta WHERE key='version'").get()?.value
    : null;
  const missingMarker = version == null;
  let required;
  let upgraded;
  try {
    if (missingMarker) {
      required = continuityContract(reference, database);
      if (!required.some((table) => table !== "schema_meta" && names.includes(table))) {
        if (names.includes("schema_meta")) requireTable(database, reference, "schema_meta");
        return { version: null };
      }
      // Reconstructible metadata is not a reason to reject a complete current layout.
      version = currentVersion;
    }
    if (version !== currentVersion && !/^(?:[1-9]|10|11|12|18)$/u.test(version ?? "")) refused();
    if (version !== currentVersion) {
      for (const [table, columns] of [["schema_meta", ["key", "value"]], ["principals", ["id"]], ["conversations", ["id", "title"]], ["memberships", ["id"]], ["events", ["id"]]]) {
        const found = database.prepare("SELECT name FROM pragma_table_info(?)").all(table).map((row) => row.name);
        requireValue(columns.every((column) => found.includes(column)), "unsupported_state_shape");
      }
      upgraded = copySchema(database);
      upgradeConversationLayout(upgraded, Number(version), conversationSource());
    }
    const actual = upgraded ?? database;
    for (const table of required ?? continuityContract(reference, actual)) {
      if (missingMarker && table === "schema_meta" && !names.includes(table)) continue;
      requireTable(actual, reference, table);
    }
    return { version };
  } finally { upgraded?.close(); reference.close(); }
}

export function inspectStrategyContract(database) {
  const reference = memoryDatabase();
  try {
    const root = "crates/licoup-workflow-store/src/";
    const text = source(`${root}store.rs`);
    reference.exec(literal(text, "CREATE TABLE IF NOT EXISTS strategy_meta"));
    reference.exec(literal(text, "CREATE INDEX IF NOT EXISTS strategy_runs_active_conversation_idx"));
    const required = tableNames(reference);
    for (const [file, anchor] of [["queue", "workflow_store_meta"], ["subscriptions", "workflow_subscriptions"], ["commit", "workflow_transition_intents"], ["controlled", "workflow_graph_state"]]) {
      reference.exec(literal(source(`${root}${file}.rs`), `CREATE TABLE IF NOT EXISTS ${anchor}`));
    }
    const existing = tableNames(database);
    for (const table of tableNames(reference)) {
      if (!required.includes(table) && !existing.includes(table)) continue;
      requireTable(database, reference, table, table === "strategy_runs"
        ? { nullable: "terminal" }
        : table === "strategy_bindings" ? { missingZeroDefault: "ordinal" } : {});
    }
  } finally { reference.close(); }
}
