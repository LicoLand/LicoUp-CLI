import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { DatabaseSync } from "node:sqlite";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  FRONTIER_REF,
  loadEmbeddedFrontier,
  planSteps,
} from "../../../tools/scripts/client-state-migration/frontier.mjs";
import {
  ADAPTIVE_FLYWHEEL_SCHEMA_VERSIONS,
  DURABLE_SHAPES,
  GATEWAY_CUSTODY_DOMAIN,
  LEGACY_MISSING_VERSION_DOMAINS,
  LEGACY_MISSING_VERSION_POLICY,
  gatewayCredentialMigrationDisposition,
} from "../../../tools/scripts/client-state-migration/probe.mjs";
import {
  evaluateMigrationState,
  exitCodeForVerdict,
} from "../../../tools/scripts/client-state-migration/report.mjs";
import { repairDomain } from "../../../tools/scripts/client-state-migration/repair.mjs";
import { writePrivateJsonAtomic } from "../../../tools/scripts/client-state-migration/util.mjs";
import { inspectConversationContract, inspectStrategyContract } from "../../../tools/scripts/client-state-migration/sqlite-contract.mjs";
import { selectModulesForChangedPaths } from "../../../tools/regression/client-module-selection.mjs";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadeRef = "tools/scripts/client-state-migration.mjs";
const moduleRoot = "tools/scripts/client-state-migration";
const MIGRATION_MODULE = "crates/licoup-native/src/domain/client_state_migration.rs";
const STORES_MODULE = "crates/licoup-native/src/domain/client_state_migration/stores.rs";
const STRATEGY_STORE_MODULE =
  "crates/licoup-native/src/domain/client_state_migration/strategy_store.rs";
const CLIENT_STATE_POLICY = "crates/licoup-client-state/src/policy.rs";
const CLIENT_STATE_MIGRATION =
  "crates/licoup-client-state/src/migration.rs";
const APPEARANCE_OWNER =
  "crates/licoup-native/src/domain/client_state_migration/appearance.rs";
const CONVERSATION_STORE = "crates/licoup-conversation/src/store/mod.rs";
const CONVERSATION_SCHEMA = "crates/licoup-conversation/src/store/schema.rs";
const WORKFLOW_STORE_MODULE = "crates/licoup-workflow-store/src/store.rs";
const FRONTIER_CONSUMER = "tools/scripts/client-state-migration/frontier.mjs";
const REPORT_CONSUMER = "tools/scripts/client-state-migration/report.mjs";
const RELEASED_FIXTURE = "tests/fixtures/client_state_migration/released_source.rs";
const BACKSLASH = String.fromCharCode(92);

/** A Rust source file from the repository. */
function ownerSource(relative) {
  return fs.readFileSync(path.join(repoRoot, relative), "utf8");
}

/**
 * The quoted Rust string literal containing `anchor`, unescaped. Fixtures are
 * built from the actual owner DDL and the frozen fixture, never from the
 * validator contract the tests are checking.
 */
function rustLiteralContaining(source, anchor) {
  const index = source.indexOf(anchor);
  assert.ok(index !== -1, `the Rust source no longer contains ${anchor}`);
  const start = source.lastIndexOf('"', index);
  const end = source.indexOf('"', index);
  assert.ok(start !== -1 && end !== -1 && end > start, `${anchor} is not a quoted literal`);
  return source
    .slice(start + 1, end)
    .replace(/\\n/gu, "\n")
    .replace(/\\"/gu, '"');
}

/** A `const NAME: &str = "..."` value. */
function rustConstString(source, name) {
  const marker = `const ${name}: &str = "`;
  const start = source.indexOf(marker);
  assert.ok(start !== -1, `${name} moved`);
  const from = start + marker.length;
  const end = source.indexOf('";', from);
  assert.ok(end !== -1, `${name} is not closed`);
  return source.slice(from, end);
}

/** A `const NAME: &str = r#"..."#` value. */
function rustRawString(source, name) {
  const marker = `const ${name}: &str = r#"`;
  const start = source.indexOf(marker);
  assert.ok(start !== -1, `${name} moved`);
  const from = start + marker.length;
  const end = source.indexOf('"#;', from);
  assert.ok(end !== -1, `${name} is not closed`);
  return source.slice(from, end);
}

function currentConversationSchemaVersion() {
  const match = ownerSource(CONVERSATION_STORE).match(
    /pub const CURRENT_SCHEMA_VERSION: &str = "(\d+)";/u,
  );
  assert.ok(match, "the conversation store schema version moved");
  return match[1];
}

/** The owner's current strategy core DDL plus the active-conversation index. */
function ownerStrategyCurrentDdl() {
  const source = ownerSource(WORKFLOW_STORE_MODULE);
  return [
    rustLiteralContaining(source, "CREATE TABLE IF NOT EXISTS strategy_meta"),
    rustLiteralContaining(source, "CREATE INDEX IF NOT EXISTS strategy_runs_active_conversation_idx"),
  ].join("\n");
}

function releasedStrategyDdl({ producerUpgraded = false } = {}) {
  let ddl = rustRawString(ownerSource(RELEASED_FIXTURE), "RELEASED_STRATEGY_SCHEMA");
  if (producerUpgraded) {
    // The published producer added this column through `ensure_column`, which
    // leaves it nullable.
    ddl = ddl.replace("terminal INTEGER NOT NULL,", "terminal INTEGER,");
    assert.ok(!ddl.includes("terminal INTEGER NOT NULL,"));
  }
  return ddl;
}

test("custody applicability follows the platform owner's shared disposition", () => {
  const root = tempRoot("custody-platform-policy");
  try {
    const frontier = loadEmbeddedFrontier();
    seedAdmittedRoot(root, frontier, { withCustody: false });
    const before = snapshot(root);
    const policy = JSON.parse(ownerSource("crates/licoup-native/resources/gateway-credential-migration.json"));
    for (const [platform, ownerPlatform] of [["darwin", "macos"], ["linux", "linux"], ["win32", "windows"]]) {
      const report = evaluateMigrationState({ root, frontier, binaryProductVersion: "0.3.0", platform });
      const custody = report.domains.find((domain) => domain.domainId === GATEWAY_CUSTODY_DOMAIN);
      assert.equal(custody.pendingAuthorization, policy[ownerPlatform] === "requires-authorization");
      assert.equal(snapshot(root), before);
    }
    assert.throws(() => evaluateMigrationState({ root, frontier, binaryProductVersion: "0.3.0", platform: "unrecognized" }), /probe_capability_unavailable/u);
  } finally { removeRoot(root); }
});

test("strategy admission recognizes both published ordinal producers without relaxing other defaults", () => {
  for (const version of ["2", "3"]) {
    for (const ordinal of ["ordinal INTEGER NOT NULL DEFAULT 0,", "ordinal INTEGER NOT NULL,"]) {
      const database = new DatabaseSync(":memory:");
      try {
        database.exec(releasedStrategyDdl().replace("ordinal INTEGER NOT NULL DEFAULT 0,", ordinal));
        database.prepare("UPDATE strategy_meta SET value=? WHERE key='version'").run(version);
        assert.doesNotThrow(() => inspectStrategyContract(database));
      } finally { database.close(); }
    }
  }
  for (const ordinal of ["ordinal INTEGER NOT NULL DEFAULT 1,", "ordinal INTEGER DEFAULT 0,"]) {
    const database = new DatabaseSync(":memory:");
    try {
      database.exec(releasedStrategyDdl().replace("ordinal INTEGER NOT NULL DEFAULT 0,", ordinal));
      assert.throws(() => inspectStrategyContract(database), /unsupported_state_shape/u);
    } finally { database.close(); }
  }
});

/** The shared fixture's business rows with its own constants substituted. */
function releasedStrategyRows() {
  const fixture = ownerSource(RELEASED_FIXTURE);
  const template = rustLiteralContaining(fixture, "INSERT INTO strategy_definitions");
  return template
    .replace(/\{\{/gu, "{")
    .replace(/\}\}/gu, "}")
    .replace(/\{([A-Z_]+)\}/gu, (_, name) =>
      fixture.includes(`const ${name}: &str = "`)
        ? rustConstString(fixture, name)
        : rustRawString(fixture, name),
    );
}

function tempRoot(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), `licoup-migration-${label}-`));
}

function removeRoot(root) {
  fs.rmSync(root, { recursive: true, force: true });
}

function runCli(args) {
  return spawnSync(process.execPath, [path.join(repoRoot, facadeRef), ...args], {
    cwd: repoRoot,
    encoding: "utf8",
  });
}

function runJson(args) {
  const result = runCli([...args, "--json"]);
  const envelope = JSON.parse(result.stdout);
  return { ...result, envelope };
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value)}\n`);
}

/**
 * Migration metadata is private state: the admission reads it through the
 * 0700/0600-enforcing path, so a fixture that is going to be certified healthy
 * has to carry those modes.
 */
function writePrivateJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true, mode: 0o700 });
  fs.writeFileSync(filePath, `${JSON.stringify(value)}\n`, { mode: 0o600 });
}

/** Every path and byte under `root`, so a read-only command can be proven inert. */
function snapshot(root) {
  const entries = [];
  const visit = (directory, prefix) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true }).sort()) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        entries.push(`${relative}/`);
        visit(absolute, relative);
      } else {
        entries.push(`${relative}:${fs.readFileSync(absolute).toString("base64")}`);
      }
    }
  };
  visit(root, "");
  return entries.join("\n");
}

function seedLedger(root, frontier, domains) {
  writePrivateJson(path.join(root, "client-state/migrations/ledger.json"), {
    schemaVersion: "v0.0.1:client-state-migration-ledger-1",
    highestAdmittedProductVersion: "0.3.0",
    frontierId: frontier.frontierId,
    domains,
  });
}

// Independent frozen producer output, never the evaluator's reference builder.
function createCurrentConversationStore(database) {
  database.exec(rustRawString(ownerSource(RELEASED_FIXTURE), "RELEASED_CONVERSATION_SCHEMA"));
  database.exec(rustRawString(ownerSource("tests/fixtures/client_state_migration/owner_layouts.rs"), "CURRENT_CONVERSATION_ADDITIONS"));
  database.exec(
    `INSERT INTO schema_meta(key,value) VALUES ('version','${currentConversationSchemaVersion()}');`,
  );
}

test("empty stores initialize and complete current stores can reconstruct only missing metadata", () => {
  for (const seed of ["", "CREATE TABLE retained_fixture(value TEXT); INSERT INTO retained_fixture VALUES ('retained');"]) {
    const database = new DatabaseSync(":memory:");
    try {
      database.exec(seed);
      assert.equal(inspectConversationContract(database, currentConversationSchemaVersion()).version, null);
    } finally { database.close(); }
  }
  for (const mutation of ["DROP TABLE schema_meta;", "DELETE FROM schema_meta WHERE key='version';"]) {
    const database = new DatabaseSync(":memory:");
    try {
      createCurrentConversationStore(database);
      database.exec(mutation);
      assert.equal(inspectConversationContract(database, currentConversationSchemaVersion()).version, currentConversationSchemaVersion());
      database.exec("DROP TABLE events;");
      assert.throws(() => inspectConversationContract(database, currentConversationSchemaVersion()), /unsupported_state_shape/u);
    } finally { database.close(); }
  }
});

test("unused related tables and their rows coexist with the current Conversation owner", () => {
  const database = new DatabaseSync(":memory:");
  try {
    createCurrentConversationStore(database);
    database.exec(ownerSource("tests/fixtures/client_state_migration/retained_related_tables.sql"));
    database.exec("INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('kept','Synthetic retained title',1,1); INSERT INTO peer_bindings VALUES (x'01',x'02','kept','member','provider',1);");
    assert.equal(inspectConversationContract(database, currentConversationSchemaVersion()).version, currentConversationSchemaVersion());
    assert.equal(database.prepare("SELECT count(*) AS count FROM peer_bindings").get().count, 1);
    database.exec("CREATE TRIGGER deny_current_insert BEFORE INSERT ON conversations BEGIN SELECT RAISE(ABORT,'blocked'); END;");
    assert.throws(() => inspectConversationContract(database, currentConversationSchemaVersion()), /unsupported_state_shape/u);
  } finally { database.close(); }
});

test("current continuity owner composes with Conversation and malformed effects remain refused", () => {
  const selected = selectModulesForChangedPaths(["crates/licoup-conversation/src/continuity/migrate.rs"])
    .filter((module) => module.id === "regression.client-state-contracts");
  assert.equal(selected.length, 1, "the actual continuity producer must select its diagnostic consumer");
  const fixture = rustRawString(
    ownerSource("tests/fixtures/client_state_migration/continuity_layout.rs"),
    "CURRENT_CONTINUITY_SCHEMA",
  );
  for (const mutation of [
    null,
    "DROP TRIGGER continuity_bump_designation_epoch;",
    "DROP TRIGGER continuity_bump_designation_epoch; CREATE TRIGGER continuity_bump_designation_epoch AFTER UPDATE OF assistant_membership_id ON conversations BEGIN DELETE FROM conversations WHERE id=NEW.id; END;",
    "CREATE TRIGGER extra_effect BEFORE INSERT ON conversations BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
    "DROP TABLE continuity_goals;",
    "DELETE FROM continuity_schema WHERE key='version';",
  ]) {
    const database = new DatabaseSync(":memory:");
    try {
      createCurrentConversationStore(database);
      database.exec(fixture);
      if (mutation) database.exec(mutation);
      if (mutation) assert.throws(() => inspectConversationContract(database, currentConversationSchemaVersion()), /unsupported_state_shape/u);
      else assert.equal(inspectConversationContract(database, currentConversationSchemaVersion()).version, currentConversationSchemaVersion());
    } finally { database.close(); }
  }
  const database = new DatabaseSync(":memory:");
  try {
    createCurrentConversationStore(database);
    database.exec("CREATE TABLE continuity_schema(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO continuity_schema VALUES ('version','7');");
    assert.throws(() => inspectConversationContract(database, currentConversationSchemaVersion()), /unsupported_state_shape/u,
      "an extension marker alone cannot manufacture its owned structures");
  } finally { database.close(); }
});

/// The current strategy layout as its owner actually creates it.
function createCurrentStrategyStore(database) {
  database.exec(ownerStrategyCurrentDdl());
}

/// The frozen released Conversation layout from the shared fixture.
function createReleasedConversationStore(database) {
  database.exec(rustRawString(ownerSource(RELEASED_FIXTURE), "RELEASED_CONVERSATION_SCHEMA"));
  database.exec(
    "INSERT INTO schema_meta(key,value) VALUES ('version','12');",
  );
}

/// The frozen released strategy layout, optionally the published producer's
/// in-place upgrade output (nullable `terminal`).
function createReleasedStrategyStore(database, options = {}) {
  database.exec(releasedStrategyDdl(options));
  database.exec(releasedStrategyRows());
}

function supportedConversationSchema(version) {
  let sql = rustRawString(ownerSource(RELEASED_FIXTURE), "RELEASED_CONVERSATION_SCHEMA");
  if (version <= 4) sql = sql.replace("runtime_cursor INTEGER, ", "");
  if (version <= 5) sql = sql.replace(", strategy_revision TEXT", "");
  if (version <= 6) sql = sql.replace("assistant_membership_id TEXT REFERENCES memberships(id),", "");
  if (version <= 8) sql = sql.replace(", preferred_reasoning_effort TEXT", "");
  if (version <= 9) sql = sql.replace(/CREATE TABLE IF NOT EXISTS subagent_dispatch_claims[\s\S]*?(?=CREATE TABLE IF NOT EXISTS subagent_mcp_inbound)/u, "");
  if (version <= 10) sql = sql.replace(/CREATE TABLE IF NOT EXISTS subagent_mcp_inbound[\s\S]*?(?=CREATE TABLE IF NOT EXISTS migration_provenance)/u, "");
  if (version < 12) sql = sql.replace("CREATE UNIQUE INDEX IF NOT EXISTS memberships_principal_unique\n            ON memberships(conversation_id, principal_id);", "CREATE UNIQUE INDEX memberships_active_unique ON memberships(conversation_id,principal_id) WHERE status='active';");
  if (version >= 18) sql += rustRawString(ownerSource("tests/fixtures/client_state_migration/owner_layouts.rs"), "CURRENT_CONVERSATION_ADDITIONS");
  return sql;
}

const structuralCases = JSON.parse(ownerSource("tests/fixtures/client_state_migration/structural_cases.json"));
for (const version of [11, 12, 18]) {
  for (const mutation of structuralCases) {
    test(`actual evaluator refuses schema${version}: ${mutation.name}`, () => {
      const root = tempRoot("structural-refusal");
      try {
        seedAdmittedRoot(root, loadEmbeddedFrontier());
        const filename = path.join(root, "client-state/conversations/conversations.sqlite3");
        fs.unlinkSync(filename);
        let ddl = supportedConversationSchema(version);
        if (mutation.from !== undefined) {
          assert.ok(ddl.includes(mutation.from));
          ddl = ddl.replaceAll(mutation.from, mutation.to);
        }
        if (mutation.before) ddl = ddl.replace("CREATE TABLE subagent_dispatch_deliveries", "CREATE TABLE IF NOT EXISTS subagent_dispatch_deliveries");
        const database = new DatabaseSync(filename);
        if (mutation.before) database.exec(mutation.sql);
        database.exec(ddl);
        database.prepare("INSERT INTO schema_meta VALUES ('version',?)").run(String(version));
        if (mutation.sql && !mutation.before) database.exec(mutation.sql);
        if (mutation.rows) database.exec(mutation.rows);
        database.close();
        const before = snapshot(root);
        const result = evaluateMigrationState({ root, frontier: loadEmbeddedFrontier(), binaryProductVersion: "0.3.0", platform: "darwin" });
        const domain = result.domains.find((domain) => domain.domainId === "canonical-conversation");
        assert.ok(domain.codes.includes("unsupported_state_shape"));
        assert.equal(result.verdict, "invalid");
        assert.equal(exitCodeForVerdict(result.verdict), 4);
        assert.equal(result.domains.find((domain) => domain.domainId === "adaptive-flywheel").observedSchemaVersion, 2);
        assert.equal(snapshot(root), before);
      } finally { removeRoot(root); }
    });
  }
}

for (const version of [...Array.from({ length: 12 }, (_, index) => index + 1), 18]) {
  test(`actual evaluator accepts independent supported Conversation schema${version}`, () => {
    const root = tempRoot("supported-layout");
    try {
      seedAdmittedRoot(root, loadEmbeddedFrontier());
      const filename = path.join(root, "client-state/conversations/conversations.sqlite3");
      fs.unlinkSync(filename);
      const database = new DatabaseSync(filename);
      database.exec(supportedConversationSchema(version));
      database.prepare("INSERT INTO schema_meta VALUES ('version',?)").run(String(version));
      database.exec("INSERT INTO principals VALUES ('principal','human','Synthetic principal',NULL,1); INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('conversation','Synthetic retained title',1,1);");
      database.close();
      const before = snapshot(root);
      const result = evaluateMigrationState({ root, frontier: loadEmbeddedFrontier(), binaryProductVersion: "0.3.0", platform: "darwin" });
      assert.equal(result.verdict, "healthy");
      assert.equal(exitCodeForVerdict(result.verdict), 0);
      assert.equal(snapshot(root), before);
    } finally { removeRoot(root); }
  });
}

for (const version of [2, 3]) {
  for (const [label, from, to, extra] of [
    ["missing required default", "model TEXT NOT NULL DEFAULT ''", "model TEXT NOT NULL", ""],
    ["FK update action", "REFERENCES strategy_definitions(revision_digest) ON DELETE CASCADE", "REFERENCES strategy_definitions(revision_digest) ON DELETE CASCADE ON UPDATE CASCADE", ""],
    ["changed default", "model TEXT NOT NULL DEFAULT ''", "model TEXT NOT NULL DEFAULT 'unexpected'", ""],
    ["malformed auxiliary table", null, null, "CREATE TABLE workflow_queue(request_id TEXT PRIMARY KEY);"],
    ["write trigger", null, null, "CREATE TRIGGER deny_run BEFORE INSERT ON strategy_runs BEGIN SELECT RAISE(ABORT,'synthetic refusal'); END;"],
  ]) {
    test(`actual evaluator refuses strategy${version}: ${label}`, () => {
      const root = tempRoot("strategy-structure");
      try {
        seedAdmittedRoot(root, loadEmbeddedFrontier());
        const filename = path.join(root, "client-state/adaptive-flywheel/strategies.sqlite3");
        fs.unlinkSync(filename);
        const database = new DatabaseSync(filename);
        const ddl = releasedStrategyDdl();
        database.exec(from === null ? ddl : ddl.replaceAll(from, to));
        database.exec(extra);
        database.prepare("UPDATE strategy_meta SET value=?").run(String(version));
        database.close();
        const before = snapshot(root);
        const result = evaluateMigrationState({ root, frontier: loadEmbeddedFrontier(), binaryProductVersion: "0.3.0", platform: "darwin" });
        assert.equal(result.verdict, "invalid");
        assert.ok(result.domains.find((domain) => domain.domainId === "adaptive-flywheel").codes.includes("unsupported_state_shape"));
        assert.equal(snapshot(root), before);
      } finally { removeRoot(root); }
    });
  }
}

/** The state a completed admission leaves behind, for the healthy verdict. */
function seedAdmittedRoot(root, frontier, { withCustody = true } = {}) {
  const expectedSteps = (domain) =>
    domain.steps.filter((step) => step.toSchemaVersion <= domain.targetSchemaVersion)
      .map((step) => step.stepId);
  const domains = {};
  for (const domain of frontier.domains) {
    // A macOS admission leaves the protected custody domain pending until the
    // operation completes; the marker exists only after that operation.
    if (!withCustody && domain.domainId === "gateway-credential-custody") continue;
    domains[domain.domainId] = {
      schemaVersion: domain.targetSchemaVersion,
      completedStepIds: expectedSteps(domain),
    };
  }
  seedLedger(root, frontier, domains);
  for (const domain of frontier.domains) {
    if (!withCustody && domain.domainId === "gateway-credential-custody") continue;
    writePrivateJson(path.join(root, `client-state/migrations/domain-state/${domain.domainId}.json`), {
      schemaVersion: "v0.0.1:client-state-domain-marker-1",
      domainId: domain.domainId,
      authoritativeSchemaVersion: domain.targetSchemaVersion,
    });
  }
  writeJson(path.join(root, ".licoup-workspace.json"), { schemaVersion: 1 });
  writeJson(path.join(root, "client-state/appearance-preferences.json"), {
    schemaVersion: 1,
    appearancePresetId: "synthetic",
  });
  writeJson(path.join(root, "client-state/agent-tool-allowlists.json"), { schemaVersion: 1 });
  writeJson(path.join(root, "client-state/current-client-view.json"), { schemaVersion: 1 });
  writeJson(path.join(root, "client-state/skill-hub-preferences.json"), { schemaVersion: 1 });
  writeJson(path.join(root, "client-state/mobile-home-layout.json"), { schemaVersion: 2 });
  writeJson(path.join(root, "client-state/agent-tab-order.json"), {
    schemaVersion: 1,
    order: [],
  });
  writeJson(path.join(root, "client-state/mobile-relay/config.json"), { schemaVersion: 2 });
  writeJson(path.join(root, "client-state/settings.json"), {
    schemaVersion: "v0.0.1:schema:definition-1",
    collection: "settings",
    items: [],
  });
  const conversations = path.join(root, "client-state/conversations");
  fs.mkdirSync(conversations, { recursive: true });
  const database = new DatabaseSync(path.join(conversations, "conversations.sqlite3"));
  createCurrentConversationStore(database);
  database.close();
  fs.writeFileSync(
    path.join(conversations, "migration-v5.complete"),
    ["schema=v5", "status=complete", ""].join("\n"),
  );
  const flywheel = path.join(root, "client-state/adaptive-flywheel");
  fs.mkdirSync(flywheel, { recursive: true });
  const strategies = new DatabaseSync(path.join(flywheel, "strategies.sqlite3"));
  createCurrentStrategyStore(strategies);
  strategies.close();
}

test("status reports every domain from the ledger and the durable stores, and changes nothing", () => {
  const root = tempRoot("status");
  try {
    const before = snapshot(root);
    const frontier = loadEmbeddedFrontier();
    const { envelope, status } = runJson(["status", "--root", root]);
    assert.equal(status, 2);
    assert.equal(envelope.command, "status");
    assert.equal(envelope.verdict, "behind");
    assert.equal(envelope.domains.length, frontier.domains.length);
    assert.deepEqual(
      envelope.domains.map((domain) => domain.domainId),
      frontier.domains.map((domain) => domain.domainId),
    );
    const appearance = envelope.domains.find(
      (domain) => domain.domainId === "appearance-presentation",
    );
    assert.equal(appearance.targetSchemaVersion, 1);
    assert.equal(appearance.observedSchemaVersion, 0);
    assert.equal(appearance.ledgerSchemaVersion, null);
    assert.deepEqual(appearance.completedStepIds, []);
    assert.deepEqual(appearance.missingStepIds, ["appearance-presentation.absent-to-1"]);
    assert.equal(snapshot(root), before, "status must not touch the data root");
  } finally {
    removeRoot(root);
  }

  const admitted = tempRoot("status-admitted");
  try {
    const frontier = loadEmbeddedFrontier();
    seedAdmittedRoot(admitted, frontier);
    const before = snapshot(admitted);
    const { envelope } = runJson(["status", "--root", admitted]);
    assert.equal(envelope.verdict, "healthy");
    assert.deepEqual(envelope.codes, []);
    assert.equal(
      envelope.domains.every(
        (domain) =>
          domain.observedSchemaVersion === domain.targetSchemaVersion &&
          domain.missingStepIds.length === 0,
      ),
      true,
    );
    assert.equal(snapshot(admitted), before, "status must not rewrite admitted state");
  } finally {
    removeRoot(admitted);
  }
});

test("doctor certifies an admitted root and fails closed on every deviation", () => {
  const frontier = loadEmbeddedFrontier();
  const healthy = tempRoot("doctor-healthy");
  try {
    seedAdmittedRoot(healthy, frontier);
    const { envelope, status } = runJson(["doctor", "--root", healthy]);
    assert.equal(status, 0);
    assert.equal(envelope.verdict, "healthy");
    assert.deepEqual(envelope.codes, []);
    assert.equal(envelope.ledger.readable, true);
  } finally {
    removeRoot(healthy);
  }

  const cases = [
    {
      label: "ledger is not the admission's contract",
      seed: (root) => writePrivateJson(path.join(root, "client-state/migrations/ledger.json"), {
        schemaVersion: "v0.0.1:client-state-migration-ledger-1",
        highestAdmittedProductVersion: "0.3.0",
        frontierId: frontier.frontierId,
        domains: { "appearance-presentation": { schemaVersion: 1, completedStepIds: [] } },
      }),
      code: "migration_ledger_invalid",
      exitCode: 4,
      verdict: "invalid",
    },
    {
      label: "ledger claims a domain the frontier does not define",
      seed: (root) => seedLedger(root, frontier, {
        "not-a-frontier-domain": { schemaVersion: 1, completedStepIds: [] },
      }),
      code: "migration_ledger_invalid",
      exitCode: 4,
      verdict: "invalid",
    },
    {
      label: "state was admitted by a newer binary",
      seed: (root) => seedLedger(root, frontier, {
        "appearance-presentation": {
          schemaVersion: 1,
          completedStepIds: ["appearance-presentation.absent-to-1"],
        },
      }) ||
        writePrivateJson(path.join(root, "client-state/migrations/ledger.json"), {
          schemaVersion: "v0.0.1:client-state-migration-ledger-1",
          highestAdmittedProductVersion: "99.0.0",
          frontierId: frontier.frontierId,
          domains: {},
        }),
      code: "state_newer_than_binary",
      exitCode: 3,
      verdict: "ahead",
    },
    {
      label: "a durable store is newer than the binary",
      seed: (root) => writeJson(path.join(root, "client-state/appearance-preferences.json"), {
        schemaVersion: 9,
      }),
      code: "state_newer_than_binary",
      exitCode: 3,
      verdict: "ahead",
    },
    {
      label: "a durable store is an unknown shape",
      seed: (root) => writeJson(path.join(root, "client-state/current-client-view.json"), {
        schemaVersion: 1.5,
      }),
      code: "unsupported_state_shape",
      exitCode: 4,
      verdict: "invalid",
    },
    {
      label: "migration metadata is not private state",
      seed: (root) => {
        writePrivateJson(path.join(root, "client-state/migrations/ledger.json"), {
          schemaVersion: "v0.0.1:client-state-migration-ledger-1",
          highestAdmittedProductVersion: "0.3.0",
          frontierId: frontier.frontierId,
          domains: {},
        });
        fs.chmodSync(path.join(root, "client-state/migrations/ledger.json"), 0o644);
      },
      code: "migration_ledger_invalid",
      exitCode: 4,
      verdict: "invalid",
    },
    {
      label: "an update handoff is waiting for the next startup",
      seed: (root) => writePrivateJson(
        path.join(root, "client-state/migrations/update-handoff.json"),
        {
          schemaVersion: "v0.0.1:client-update-handoff-1",
          state: "pending",
          version: "0.0.1-alpha",
          targetReleaseTrack: "nightly",
          migrationFrontier: { frontierId: "licoup-state-0.2.1", domains: [] },
          receiptId: `sha256:${"a".repeat(64)}`,
          targetPath: "/synthetic/target",
          backupPath: "/synthetic/backup",
        },
      ),
      code: "update_handoff_pending",
      exitCode: 4,
      verdict: "invalid",
    },
    {
      label: "a durable store is not a regular file",
      seed: (root) => {
        fs.mkdirSync(path.join(root, "client-state"), { recursive: true });
        fs.symlinkSync(
          path.join(root, "missing-target"),
          path.join(root, "client-state/appearance-preferences.json"),
        );
      },
      code: "unsupported_state_shape",
      exitCode: 4,
      verdict: "invalid",
    },
  ];
  for (const scenario of cases) {
    const root = tempRoot("doctor");
    try {
      scenario.seed(root);
      const { envelope, status } = runJson(["doctor", "--root", root]);
      assert.equal(status, scenario.exitCode, scenario.label);
      assert.equal(envelope.verdict, scenario.verdict, scenario.label);
      assert.equal(
        envelope.codes.some((entry) => entry.code === scenario.code),
        true,
        `${scenario.label}: ${JSON.stringify(envelope.codes)}`,
      );
    } finally {
      removeRoot(root);
    }
  }
});

test("a gap and an ambiguous plan each report their own stable code", () => {
  const root = tempRoot("plan");
  try {
    const frontier = loadEmbeddedFrontier();
    const base = frontier.domains[0];
    const gap = {
      ...frontier,
      domains: [{ ...base, targetSchemaVersion: 3, steps: [base.steps[0], {
        stepId: `${base.domainId}.jump-to-3`,
        fromSchemaVersion: 2,
        toSchemaVersion: 3,
      }] }],
    };
    assert.deepEqual(planSteps(gap.domains[0], 1), { steps: [], code: "migration_plan_gap" });

    // The store is behind its target and the frontier defines no step from
    // where it stands.
    const leadingGap = {
      ...frontier,
      domains: [{ ...base, targetSchemaVersion: 2, steps: [{
        stepId: `${base.domainId}.synthetic-to-2`,
        fromSchemaVersion: 1,
        toSchemaVersion: 2,
      }] }],
    };
    const leadingReport = evaluateMigrationState({
      root,
      frontier: leadingGap,
      binaryProductVersion: "0.3.0",
    });
    assert.equal(leadingReport.verdict, "invalid");
    assert.equal(leadingReport.codes[0].code, "migration_plan_gap");
    const gapReport = evaluateMigrationState({
      root,
      frontier: gap,
      binaryProductVersion: "0.3.0",
    });
    assert.equal(gapReport.verdict, "invalid");
    assert.equal(gapReport.codes[0].code, "migration_plan_gap");

    const ambiguous = {
      ...frontier,
      domains: [{ ...base, steps: [base.steps[0], {
        stepId: `${base.domainId}.competing-to-1`,
        fromSchemaVersion: 0,
        toSchemaVersion: 1,
      }] }],
    };
    assert.deepEqual(planSteps(ambiguous.domains[0], 0), {
      steps: [],
      code: "migration_plan_ambiguous",
    });
    const ambiguousReport = evaluateMigrationState({
      root,
      frontier: ambiguous,
      binaryProductVersion: "0.3.0",
    });
    assert.equal(ambiguousReport.verdict, "invalid");
    assert.equal(ambiguousReport.codes[0].code, "migration_plan_ambiguous");
  } finally {
    removeRoot(root);
  }
});

test("repair applies exactly one step, is idempotent, and keeps the ledger consistent", () => {
  const root = tempRoot("repair");
  try {
    const frontier = loadEmbeddedFrontier();
    const first = repairDomain({
      root,
      frontier,
      domainId: "appearance-presentation",
      binaryProductVersion: "0.3.0",
    });
    assert.deepEqual(first.mutations, [{
      domainId: "appearance-presentation",
      stepId: "appearance-presentation.absent-to-1",
      toSchemaVersion: 1,
      storeWritten: false,
      ledgerUpdated: true,
      applied: true,
    }]);
    const ledger = JSON.parse(
      fs.readFileSync(path.join(root, "client-state/migrations/ledger.json"), "utf8"),
    );
    assert.deepEqual(ledger.domains, {
      "appearance-presentation": {
        schemaVersion: 1,
        completedStepIds: ["appearance-presentation.absent-to-1"],
      },
    });
    assert.equal(ledger.highestAdmittedProductVersion, "0.0.0");
    assert.deepEqual(
      JSON.parse(
        fs.readFileSync(
          path.join(root, "client-state/migrations/domain-state/appearance-presentation.json"),
          "utf8",
        ),
      ),
      {
        schemaVersion: "v0.0.1:client-state-domain-marker-1",
        domainId: "appearance-presentation",
        authoritativeSchemaVersion: 1,
      },
    );

    const second = repairDomain({
      root,
      frontier,
      domainId: "appearance-presentation",
      binaryProductVersion: "0.3.0",
    });
    assert.deepEqual(second.mutations, [{
      domainId: "appearance-presentation",
      stepId: null,
      toSchemaVersion: 1,
      storeWritten: false,
      ledgerUpdated: false,
      applied: false,
    }]);
    assert.equal(second.report.verdict, "behind");
    // The admission's own reconciliation rules accept what the repair wrote.
    assert.deepEqual(
      second.report.codes.filter((entry) => entry.code === "migration_ledger_invalid"),
      [],
    );
    assert.equal(
      second.report.domains.find((domain) => domain.domainId === "appearance-presentation")
        .verdict,
      "healthy",
    );
  } finally {
    removeRoot(root);
  }
});

test("repair rewrites only the store the step owns, and never invents a missing definition", () => {
  const legacy = tempRoot("repair-legacy");
  try {
    const frontier = loadEmbeddedFrontier();
    writeJson(path.join(legacy, "client-state/appearance-preferences.json"), {
      appearancePresetId: "canary-preset",
      localePreference: "canary-locale",
    });
    writeJson(path.join(legacy, "client-state/agent-tab-order.json"), [{ agent: "canary" }]);
    const appearance = repairDomain({
      root: legacy,
      frontier,
      domainId: "appearance-presentation",
      binaryProductVersion: "0.3.0",
    });
    assert.equal(appearance.mutations[0].storeWritten, true);
    assert.deepEqual(
      JSON.parse(fs.readFileSync(path.join(legacy, "client-state/appearance-preferences.json"), "utf8")),
      { appearancePresetId: "canary-preset", localePreference: "canary-locale", schemaVersion: 1 },
    );
    const tabOrder = repairDomain({
      root: legacy,
      frontier,
      domainId: "agent-tab-order",
      binaryProductVersion: "0.3.0",
    });
    assert.equal(tabOrder.mutations[0].storeWritten, true);
    assert.deepEqual(
      JSON.parse(fs.readFileSync(path.join(legacy, "client-state/agent-tab-order.json"), "utf8")),
      { schemaVersion: 1, order: [{ agent: "canary" }] },
    );
    // A collection marker that is a string is the admission's marker; a numeric
    // one is a legacy document, not an unknown shape.
    writeJson(path.join(legacy, "client-state/settings.json"), {
      collection: "settings",
      items: [],
    });
    writeJson(path.join(legacy, "client-state/pins.json"), { schemaVersion: 3, items: [] });
    const collections = evaluateMigrationState({
      root: legacy,
      frontier,
      binaryProductVersion: "0.3.0",
    });
    assert.equal(
      collections.domains.find((domain) => domain.domainId === "client-state").verdict,
      "behind",
    );
  } finally {
    removeRoot(legacy);
  }

  const untypedMarker = tempRoot("repair-untyped-marker");
  try {
    const frontier = loadEmbeddedFrontier();
    // A marker that is not a version leaves the document legacy, so the tool
    // must both report it as behind *and* be able to stamp it: reporting a
    // repair it would then refuse would be worse than refusing up front.
    writeJson(path.join(untypedMarker, "client-state/appearance-preferences.json"), {
      schemaVersion: "not-a-version",
      appearancePresetId: "canary-preset",
    });
    const reported = evaluateMigrationState({
      root: untypedMarker,
      frontier,
      binaryProductVersion: "0.3.0",
    }).domains.find((domain) => domain.domainId === "appearance-presentation");
    assert.equal(reported.verdict, "behind");
    assert.equal(reported.repairable, true);
    assert.deepEqual(reported.codes, []);
    const repaired = repairDomain({
      root: untypedMarker,
      frontier,
      domainId: "appearance-presentation",
      binaryProductVersion: "0.3.0",
    });
    assert.equal(repaired.mutations[0].storeWritten, true);
    assert.deepEqual(
      JSON.parse(
        fs.readFileSync(
          path.join(untypedMarker, "client-state/appearance-preferences.json"),
          "utf8",
        ),
      ),
      { schemaVersion: 1, appearancePresetId: "canary-preset" },
    );
  } finally {
    removeRoot(untypedMarker);
  }

  const refused = tempRoot("repair-refused");
  try {
    const frontier = loadEmbeddedFrontier();
    // Each of these steps is defined by the admission, not by the frontier, so
    // the frontier alone cannot authorize the write.
    for (const domainId of [
      "adaptive-flywheel",
      "canonical-conversation",
      "client-state",
      "mobile-home-layout",
      "mobile-relay",
    ]) {
      assert.throws(
        () => repairDomain({ root: refused, frontier, domainId, binaryProductVersion: "0.3.0" }),
        { code: "repair_requires_native_admission" },
        domainId,
      );
    }
    assert.throws(
      () => repairDomain({
        root: refused,
        frontier,
        domainId: "gateway-credential-custody",
        binaryProductVersion: "0.3.0",
      }),
      { code: "migration_authorization_required" },
    );
    assert.throws(
      () => repairDomain({
        root: refused,
        frontier,
        domainId: "unknown-domain",
        binaryProductVersion: "0.3.0",
      }),
      { code: "repair_domain_unknown" },
    );
    assert.equal(snapshot(refused), "", "a refused repair must change nothing");
  } finally {
    removeRoot(refused);
  }

  const newerBinary = tempRoot("repair-newer-binary");
  try {
    const frontier = loadEmbeddedFrontier();
    seedLedger(newerBinary, frontier, {});
    writePrivateJson(path.join(newerBinary, "client-state/migrations/ledger.json"), {
      schemaVersion: "v0.0.1:client-state-migration-ledger-1",
      highestAdmittedProductVersion: "99.0.0",
      frontierId: frontier.frontierId,
      domains: {},
    });
    const before = snapshot(newerBinary);
    // `reject_older_binary` is a precondition of the whole admission: in this
    // state the binary may not write anything, including one domain step.
    assert.throws(
      () => repairDomain({
        root: newerBinary,
        frontier,
        domainId: "appearance-presentation",
        binaryProductVersion: "0.3.0",
      }),
      { code: "state_newer_than_binary" },
    );
    assert.equal(snapshot(newerBinary), before);
  } finally {
    removeRoot(newerBinary);
  }

  const resumable = tempRoot("repair-resumable");
  try {
    const frontier = loadEmbeddedFrontier();
    const extended = {
      ...frontier,
      domains: frontier.domains.map((domain) =>
        domain.domainId === "appearance-presentation"
          ? {
              ...domain,
              targetSchemaVersion: 2,
              steps: [...domain.steps, {
                stepId: "appearance-presentation.synthetic-to-2",
                fromSchemaVersion: 1,
                toSchemaVersion: 2,
              }],
            }
          : domain),
    };
    const first = repairDomain({
      root: resumable,
      frontier: extended,
      domainId: "appearance-presentation",
      binaryProductVersion: "0.3.0",
    });
    assert.equal(first.mutations[0].stepId, "appearance-presentation.absent-to-1");
    assert.deepEqual(
      first.report.domains.find((domain) => domain.domainId === "appearance-presentation")
        .missingStepIds,
      ["appearance-presentation.synthetic-to-2"],
    );
    // The frontier states nothing about this document's version 2 marker, so
    // the second step is refused rather than guessed.
    assert.throws(
      () => repairDomain({
        root: resumable,
        frontier: extended,
        domainId: "appearance-presentation",
        binaryProductVersion: "0.3.0",
      }),
      { code: "repair_requires_native_admission" },
    );
  } finally {
    removeRoot(resumable);
  }
});

test("the private write refuses to clobber a document that changed after the read", () => {
  const root = tempRoot("cas");
  try {
    const target = path.join(root, "client-state/appearance-preferences.json");
    writeJson(target, { appearancePresetId: "first" });
    const stale = fs.readFileSync(target, "utf8");
    writeJson(target, { appearancePresetId: "second" });
    assert.throws(
      () => writePrivateJsonAtomic(target, { schemaVersion: 1 }, { expectedBytes: stale }),
      { code: "repair_conflict" },
    );
    assert.equal(
      fs.readFileSync(target, "utf8"),
      `${JSON.stringify({ appearancePresetId: "second" })}\n`,
      "a refused compare-and-swap must leave the document alone",
    );
    writePrivateJsonAtomic(target, { schemaVersion: 1 }, {
      expectedBytes: fs.readFileSync(target, "utf8"),
    });
    assert.deepEqual(JSON.parse(fs.readFileSync(target, "utf8")), { schemaVersion: 1 });
    assert.equal(fs.readdirSync(path.dirname(target)).length, 1, "no temporary file is left behind");
  } finally {
    removeRoot(root);
  }
});

test("root symlinks follow the private-state owner trust boundary", () => {
  const real = tempRoot("symlink-real");
  const link = `${real}-link`;
  try {
    seedAdmittedRoot(real, loadEmbeddedFrontier());
    fs.symlinkSync(real, link);
    const systemOwned = fs.lstatSync(link).uid === 0;
    const { envelope, status } = runJson(["doctor", "--root", link]);
    assert.equal(status, systemOwned ? 0 : 4);
    assert.equal(
      envelope.codes.some((entry) => entry.code === "migration_ledger_invalid"),
      !systemOwned,
      JSON.stringify(envelope.codes),
    );
  } finally {
    fs.rmSync(link, { force: true });
    removeRoot(real);
  }
});

test("no output carries a local path, a stored value or credential material", () => {
  const root = tempRoot("privacy-canary");
  const canaries = [
    "canary-stored-value-7f3a",
    "pcToken",
    "pairingCode",
    os.homedir(),
    root,
    os.tmpdir(),
  ];
  try {
    writeJson(path.join(root, "client-state/appearance-preferences.json"), {
      appearancePresetId: canaries[0],
      localePreference: canaries[0],
    });
    writeJson(path.join(root, "client-state/mobile-relay/config.json"), {
      schemaVersion: 1,
      pcToken: canaries[0],
      pairingCode: canaries[0],
      mobileRelayE2ee: { protocolVersion: "incompatible-protocol" },
    });
    const outputs = [
      runCli(["status", "--root", root]),
      runCli(["doctor", "--root", root]),
      runCli(["repair", "--domain", "appearance-presentation", "--root", root]),
      runCli(["repair", "--domain", "mobile-relay", "--root", root]),
      runCli(["status", "--root", root, "--json"]),
      runCli(["repair", "--domain", "gateway-credential-custody", "--root", root, "--json"]),
    ];
    for (const result of outputs) {
      for (const stream of [result.stdout, result.stderr]) {
        for (const canary of canaries) {
          assert.equal(
            stream.includes(canary),
            false,
            `output leaked ${canary}: ${stream.slice(0, 200)}`,
          );
        }
      }
    }
    assert.equal(outputs[0].stdout.includes("appearance-presentation"), true);
  } finally {
    removeRoot(root);
  }
});

test("the client never invokes the migration CLI and the tool never guesses a data root", async () => {
  const offenders = [];
  const visit = async (relative) => {
    const absolute = path.join(repoRoot, relative);
    if (!fs.existsSync(absolute)) return;
    for (const entry of await fs.promises.readdir(absolute, { withFileTypes: true })) {
      const child = path.join(relative, entry.name);
      if (entry.isDirectory()) {
        await visit(child);
      } else if (/\.(rs|dart)$/u.test(entry.name)) {
        const source = await fs.promises.readFile(path.join(repoRoot, child), "utf8");
        if (source.includes(`client-state-migration${".mjs"}`)) offenders.push(child);
      }
    }
  };
  await visit("crates");
  await visit("apps/desktop/lib");
  assert.deepEqual(offenders, [], "startup migration stays exclusively the Rust admission");

  const admission = await fs.promises.readFile(path.join(repoRoot, MIGRATION_MODULE), "utf8");
  assert.doesNotMatch(admission, /Command::new|std::process::Command/u);
  const bridge = JSON.parse(
    await fs.promises.readFile(path.join(repoRoot, "schemas/client_bridge/state.json"), "utf8"),
  );
  assert.deepEqual(bridge.operations, ["get", "set", "admit"]);

  // The CLI has no default data root: an operator always names the root, so a
  // self-test cannot read a real installation.
  const missingRoot = runCli(["status"]);
  assert.equal(missingRoot.status, 64);
  assert.match(missingRoot.stderr, /usage: client-state-migration/u);
  for (const source of await Promise.all(
    fs.readdirSync(path.join(repoRoot, moduleRoot))
      .map((leaf) => fs.promises.readFile(path.join(repoRoot, moduleRoot, leaf), "utf8")),
  )) {
    assert.doesNotMatch(source, /homedir\(\)|Application Support|APPDATA/u);
  }
});

test("exit codes stay distinct for healthy, behind, ahead, invalid, and usage", () => {
  const frontier = loadEmbeddedFrontier();
  const healthy = tempRoot("exit-healthy");
  const ahead = tempRoot("exit-ahead");
  const pending = tempRoot("exit-pending");
  try {
    seedAdmittedRoot(healthy, frontier);
    assert.equal(runCli(["doctor", "--root", healthy]).status, 0);
    assert.equal(runCli(["doctor", "--root", tempRoot("exit-behind")]).status, 2);
    writeJson(path.join(ahead, "client-state/appearance-preferences.json"), { schemaVersion: 9 });
    assert.equal(runCli(["doctor", "--root", ahead]).status, 3);
    fs.writeFileSync(path.join(ahead, "client-state/appearance-preferences.json"), "not json");
    assert.equal(runCli(["doctor", "--root", ahead]).status, 4);
    seedAdmittedRoot(pending, frontier, { withCustody: false });
    assert.equal(
      runCli(["doctor", "--root", pending]).status,
      gatewayCredentialMigrationDisposition(process.platform) === "requires-authorization" ? 5 : 0,
    );
    assert.equal(runCli(["status", "--root", "relative/path"]).status, 64);
    assert.equal(runCli(["unknown-command", "--root", healthy]).status, 64);
    assert.equal(runCli(["repair", "--root", healthy]).status, 64);
  } finally {
    removeRoot(healthy);
    removeRoot(ahead);
    removeRoot(pending);
  }
});

test("the evaluator refuses undeclared frontiers and mirrors the owner's store layouts", () => {
  const frontier = loadEmbeddedFrontier();
  const seedNamedLedger = (root, frontierId) =>
    writePrivateJson(path.join(root, "client-state/migrations/ledger.json"), {
      schemaVersion: "v0.0.1:client-state-migration-ledger-1",
      highestAdmittedProductVersion: "0.3.0",
      frontierId,
      domains: {},
    });

  for (const named of ["licoup-state-0.2.1", "licoup-state-0.2.2"]) {
    const root = tempRoot("frontier-refusal");
    try {
      seedNamedLedger(root, named);
      const report = evaluateMigrationState({
        root,
        frontier,
        binaryProductVersion: "0.3.0",
      });
      assert.equal(report.verdict, "invalid", named);
      assert.ok(
        report.codes.some((entry) => entry.code === "unsupported_state_shape"),
        `${named} must be refused: ${JSON.stringify(report.codes)}`,
      );
    } finally {
      removeRoot(root);
    }
  }
  for (const named of [frontier.sourceFrontierId, frontier.frontierId]) {
    const root = tempRoot("frontier-accepted");
    try {
      seedNamedLedger(root, named);
      const report = evaluateMigrationState({
        root,
        frontier,
        binaryProductVersion: "0.3.0",
      });
      assert.equal(
        report.codes.some((entry) => entry.code === "unsupported_state_shape"),
        false,
        `${named} is a declared endpoint`,
      );
    } finally {
      removeRoot(root);
    }
  }
});

test("the evaluator refuses fake conversation and malformed strategy databases", () => {
  const frontier = loadEmbeddedFrontier();
  const cases = [
    {
      label: "a current-stamped two-table conversation store",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        database.exec(
          "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);" +
            "INSERT INTO schema_meta(key,value) VALUES ('version','18');" +
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, title TEXT NOT NULL);",
        );
        database.close();
        fs.writeFileSync(
          path.join(directory, "migration-v5.complete"),
          ["schema=v5", "status=complete", ""].join("\n"),
        );
      },
    },
    {
      label: "a development conversation schema",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        database.exec(
          "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);" +
            "INSERT INTO schema_meta(key,value) VALUES ('version','15');",
        );
        database.close();
      },
    },
    {
      label: "a strategy version row on a truncated database",
      seed: (root) => {
        const directory = path.join(root, "client-state/adaptive-flywheel");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "strategies.sqlite3"));
        database.exec(
          "CREATE TABLE strategy_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);" +
            "INSERT INTO strategy_meta(key,value) VALUES ('version','3');",
        );
        database.close();
      },
    },
    {
      label: "a legacy strategy stamp",
      seed: (root) => {
        const directory = path.join(root, "client-state/adaptive-flywheel");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "strategies.sqlite3"));
        database.exec(
          "CREATE TABLE strategy_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);" +
            "INSERT INTO strategy_meta(key,value) VALUES ('version','1');",
        );
        database.close();
      },
    },
  ];
  for (const scenario of cases) {
    const root = tempRoot("refused-shape");
    try {
      scenario.seed(root);
      const report = evaluateMigrationState({
        root,
        frontier,
        binaryProductVersion: "0.3.0",
      });
      assert.equal(report.verdict, "invalid", scenario.label);
      assert.ok(
        report.codes.some((entry) => entry.code === "unsupported_state_shape"),
        `${scenario.label}: ${JSON.stringify(report.codes)}`,
      );
    } finally {
      removeRoot(root);
    }
  }
});

test("released and producer-variant stores are accepted; mutations and malformed inspections are bounded", () => {
  const frontier = loadEmbeddedFrontier();
  const seedReleasedConversation = (root) => {
    const directory = path.join(root, "client-state/conversations");
    fs.mkdirSync(directory, { recursive: true });
    const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
    createReleasedConversationStore(database);
    database.close();
    fs.writeFileSync(
      path.join(directory, "migration-v5.complete"),
      ["schema=v5", "status=complete", ""].join("\n"),
    );
  };
  const seedReleasedStrategy = (root, options = {}) => {
    const directory = path.join(root, "client-state/adaptive-flywheel");
    fs.mkdirSync(directory, { recursive: true });
    const database = new DatabaseSync(path.join(directory, "strategies.sqlite3"));
    createReleasedStrategyStore(database, options);
    database.close();
  };
  const evaluate = (root) =>
    evaluateMigrationState({ root, frontier, binaryProductVersion: "0.3.0", platform: "linux" });

  // The frozen released layouts, including the published producer's in-place
  // upgrade output with a nullable terminal, are owner-supported sources.
  const accepted = tempRoot("released-accepted");
  try {
    seedReleasedConversation(accepted);
    seedReleasedStrategy(accepted, { producerUpgraded: true });
    const report = evaluate(accepted);
    const conversation = report.domains.find(
      (domain) => domain.domainId === "canonical-conversation",
    );
    assert.equal(conversation.observedSchemaVersion, 1, JSON.stringify(report.codes));
    const adaptive = report.domains.find((domain) => domain.domainId === "adaptive-flywheel");
    assert.equal(adaptive.observedSchemaVersion, 1, JSON.stringify(report.codes));
    assert.equal(
      report.codes.some((entry) => entry.code === "unsupported_state_shape"),
      false,
      JSON.stringify(report.codes),
    );
  } finally {
    removeRoot(accepted);
  }

  // The same released layout stamped as the adjacent legacy version stays a
  // legitimate source: the JS evaluator must not over-refuse it.
  const legacy = tempRoot("released-legacy");
  try {
    seedReleasedConversation(legacy);
    const database = new DatabaseSync(
      path.join(legacy, "client-state/conversations/conversations.sqlite3"),
    );
    database.exec("UPDATE schema_meta SET value='11' WHERE key='version';");
    database.close();
    const report = evaluate(legacy);
    const conversation = report.domains.find(
      (domain) => domain.domainId === "canonical-conversation",
    );
    assert.equal(conversation.observedSchemaVersion, 1, JSON.stringify(report.codes));
    assert.equal(
      report.codes.some((entry) => entry.code === "unsupported_state_shape"),
      false,
      JSON.stringify(report.codes),
    );
  } finally {
    removeRoot(legacy);
  }

  const mutations = [
    {
      label: "the released membership identity index is not unique",
      domainId: "canonical-conversation",
      seed: (root) => {
        seedReleasedConversation(root);
        const database = new DatabaseSync(
          path.join(root, "client-state/conversations/conversations.sqlite3"),
        );
        database.exec(
          "DROP INDEX memberships_principal_unique;" +
            "CREATE INDEX memberships_principal_unique ON memberships(conversation_id, principal_id);",
        );
        database.close();
      },
    },
    {
      label: "the released partial predicate was weakened",
      domainId: "adaptive-flywheel",
      seed: (root) => {
        seedReleasedStrategy(root, { producerUpgraded: true });
        const database = new DatabaseSync(
          path.join(root, "client-state/adaptive-flywheel/strategies.sqlite3"),
        );
        database.exec(
          "DROP INDEX strategy_authorization_active_idx;" +
            "CREATE UNIQUE INDEX strategy_authorization_active_idx" +
            " ON strategy_authorizations(revision_digest) WHERE active=1 AND 0;",
        );
        database.close();
      },
    },
    {
      label: "an older published stamp on two tables",
      domainId: "canonical-conversation",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        database.exec(
          "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);" +
            "INSERT INTO schema_meta(key,value) VALUES ('version','11');" +
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, title TEXT NOT NULL);",
        );
        database.close();
        fs.writeFileSync(
          path.join(directory, "migration-v5.complete"),
          ["schema=v5", "status=complete", ""].join("\n"),
        );
      },
    },
    {
      label: "a current stamped store without a conversation title",
      domainId: "canonical-conversation",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        createCurrentConversationStore(database);
        database.exec("ALTER TABLE conversations DROP COLUMN title;");
        database.close();
        fs.writeFileSync(
          path.join(directory, "migration-v5.complete"),
          ["schema=v5", "status=complete", ""].join("\n"),
        );
      },
    },
    {
      label: "an identity-only older store",
      domainId: "canonical-conversation",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        database.exec(
          "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);" +
            "INSERT INTO schema_meta(key,value) VALUES ('version','11');" +
            "CREATE TABLE principals(id TEXT PRIMARY KEY);" +
            "CREATE TABLE conversations(id TEXT PRIMARY KEY, title TEXT NOT NULL);" +
            "CREATE TABLE memberships(id TEXT PRIMARY KEY);" +
            "CREATE TABLE events(id TEXT PRIMARY KEY);",
        );
        database.close();
        fs.writeFileSync(
          path.join(directory, "migration-v5.complete"),
          ["schema=v5", "status=complete", ""].join("\n"),
        );
      },
    },
    {
      label: "a current store missing a principal business field",
      domainId: "canonical-conversation",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        createCurrentConversationStore(database);
        database.exec("ALTER TABLE principals DROP COLUMN display_name;");
        database.close();
        fs.writeFileSync(
          path.join(directory, "migration-v5.complete"),
          ["schema=v5", "status=complete", ""].join("\n"),
        );
      },
    },
    {
      label: "a released store without event sequence uniqueness",
      domainId: "canonical-conversation",
      seed: (root) => {
        seedReleasedConversation(root);
        const database = new DatabaseSync(
          path.join(root, "client-state/conversations/conversations.sqlite3"),
        );
        database.exec(
          "PRAGMA foreign_keys=OFF;" +
            "CREATE TABLE events_rebuilt (" +
            "id TEXT PRIMARY KEY," +
            "conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE," +
            "sequence INTEGER NOT NULL," +
            "author_membership_id TEXT REFERENCES memberships(id)," +
            "kind TEXT NOT NULL, causation_id TEXT, correlation_id TEXT," +
            "created_at INTEGER NOT NULL," +
            "finalized INTEGER NOT NULL DEFAULT 0 CHECK(finalized IN (0,1))," +
            "CHECK(1));" +
            "INSERT INTO events_rebuilt SELECT id, conversation_id, sequence," +
            " author_membership_id, kind, causation_id, correlation_id, created_at, finalized" +
            " FROM events;" +
            "DROP TABLE events;" +
            "ALTER TABLE events_rebuilt RENAME TO events;",
        );
        database.close();
      },
    },
    {
      label: "a malformed schema_meta inspection",
      domainId: "canonical-conversation",
      seed: (root) => {
        const directory = path.join(root, "client-state/conversations");
        fs.mkdirSync(directory, { recursive: true });
        const database = new DatabaseSync(path.join(directory, "conversations.sqlite3"));
        database.exec("CREATE TABLE schema_meta(key TEXT);");
        database.close();
        fs.writeFileSync(
          path.join(directory, "migration-v5.complete"),
          ["schema=v5", "status=complete", ""].join("\n"),
        );
      },
    },
  ];
  for (const mutation of mutations) {
    const root = tempRoot("released-mutation");
    try {
      mutation.seed(root);
      const report = evaluate(root);
      assert.equal(report.verdict, "invalid", mutation.label);
      const domain = report.domains.find((entry) => entry.domainId === mutation.domainId);
      assert.equal(domain.verdict, "invalid", mutation.label);
      assert.ok(
        domain.codes.includes("unsupported_state_shape"),
        `${mutation.label}: ${JSON.stringify(domain.codes)}`,
      );
      assert.equal(
        report.domains.length,
        frontier.domains.length,
        `${mutation.label}: every other domain stays reported`,
      );
    } finally {
      removeRoot(root);
    }
  }
});

test("pending custody is incomplete, never healthy", () => {
  const frontier = loadEmbeddedFrontier();
  const root = tempRoot("pending-custody");
  try {
    seedAdmittedRoot(root, frontier, { withCustody: false });
    const report = evaluateMigrationState({
      root,
      frontier,
      binaryProductVersion: "0.3.0",
      platform: "darwin",
    });
    assert.equal(report.verdict, "pending_authorization");
    assert.equal(exitCodeForVerdict(report.verdict), 5);
    const gateway = report.domains.find(
      (domain) => domain.domainId === "gateway-credential-custody",
    );
    assert.equal(gateway.pendingAuthorization, true);
    assert.equal(gateway.observedSchemaVersion, 0);
    assert.equal(
      fs.existsSync(
        path.join(
          root,
          "client-state/migrations/domain-state/gateway-credential-custody.json",
        ),
      ),
      false,
      "the tool must not fabricate a custody marker",
    );
  } finally {
    removeRoot(root);
  }
});

test("every mirrored durable shape and constant still matches the Rust admission", async () => {
  const [
    migration,
    stores,
    strategyStore,
    workflowStore,
    policy,
    clientStateMigration,
    conversationStore,
  ] = await Promise.all([
    fs.promises.readFile(path.join(repoRoot, MIGRATION_MODULE), "utf8"),
    fs.promises.readFile(path.join(repoRoot, STORES_MODULE), "utf8"),
    fs.promises.readFile(path.join(repoRoot, STRATEGY_STORE_MODULE), "utf8"),
    fs.promises.readFile(path.join(repoRoot, WORKFLOW_STORE_MODULE), "utf8"),
    fs.promises.readFile(path.join(repoRoot, CLIENT_STATE_POLICY), "utf8"),
    fs.promises.readFile(path.join(repoRoot, CLIENT_STATE_MIGRATION), "utf8"),
    fs.promises.readFile(path.join(repoRoot, CONVERSATION_STORE), "utf8"),
  ]);
  // The domain routing lives in the stores leaf, the strategy layouts in the
  // strategy-store leaf, and the owner's exact core-table descriptor lives in
  // the workflow-store owner, so the mirror reads the owners together.
  // Whitespace is stripped so the bindings survive any rustfmt layout.
  const compact = `${migration}${stores}${strategyStore}${workflowStore}`.replace(/\s+/gu, "");

  const jsonDocuments = new Map();
  // The generic JSON-schema route is deliberately two-argument: it carries one
  // published shape and one version, and a document that records no version is
  // not a shape it walks. The domain that publishes a legacy source shape
  // answers for itself through its own owner, so a third argument here would
  // mean a second implementation of that domain's policy.
  for (const [domainId, shape] of Object.entries(DURABLE_SHAPES)) {
    if (shape.probe !== "probe_json_schema") continue;
    // The call itself is pinned: the arm's own braces may wrap it.
    const route = new RegExp(
      `"${domainId}"=>\\{?probe_json_schema\\(&root\\.join\\("${shape.document.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}"\\),${shape.schemaVersion}\\)`,
      "u",
    );
    assert.ok(
      route.test(compact),
      `${domainId} must route through the generic JSON-schema probe as ${route.source}`,
    );
    jsonDocuments.set(domainId, {
      document: shape.document,
      schemaVersion: shape.schemaVersion,
    });
  }
  const mirrored = Object.fromEntries(
    Object.entries(DURABLE_SHAPES)
      .filter(([, shape]) => shape.probe === "probe_json_schema")
      .map(([domainId, shape]) => [domainId, {
        document: shape.document,
        schemaVersion: shape.schemaVersion,
      }]),
  );
  assert.deepEqual(Object.fromEntries(jsonDocuments), mirrored);
  // A domain routed through its own owner must not also be routed through the
  // generic route, and the legacy policy belongs to that owner.
  const appearance = await fs.promises.readFile(path.join(repoRoot, APPEARANCE_OWNER), "utf8");
  const appearanceDomain = appearance.match(
    /pub\(super\) const DOMAIN_ID: &str = "([a-z0-9-]+)";/u,
  );
  assert.ok(appearanceDomain, "the appearance owner no longer declares its domain id");
  const ownedDomains = Object.entries(DURABLE_SHAPES)
    .filter(([, shape]) => shape.probe === "owner")
    .map(([domainId]) => domainId);
  assert.deepEqual(
    ownedDomains,
    [appearanceDomain[1]],
    "exactly the domains whose owner answers for them are routed outside the generic route",
  );
  assert.deepEqual(
    [...LEGACY_MISSING_VERSION_DOMAINS],
    [appearanceDomain[1]],
    "the domains whose owner answers a versionless document with version 0",
  );
  // The owner answers the policy rather than naming it: a store that records
  // no version reports version 0. The mirror's token still names that answer,
  // so the binding asserted here is the owner's behavior, not a constant.
  const appearanceCompact = appearance.replace(/\s+/gu, "");
  assert.ok(
    appearanceCompact.includes("None=>Ok(AuthoritativeProbe{version:0,present:true,})"),
    "the appearance owner no longer answers a versionless document with version 0",
  );
  assert.equal(
    LEGACY_MISSING_VERSION_POLICY,
    "missing-is-legacy",
    "the mirror's legacy policy token moved",
  );

  const standaloneRoute = (name, document) =>
    `${name}(&root.join("${document}"))`;
  assert.ok(
    compact.includes(standaloneRoute("probe_agent_tab_order", DURABLE_SHAPES["agent-tab-order"].document)),
  );
  assert.ok(
    compact.includes(standaloneRoute("probe_mobile_relay", DURABLE_SHAPES["mobile-relay"].document)),
  );
  assert.ok(
    compact.includes(
      'constSTRATEGY_STORE_DATABASE:&str="client-state/adaptive-flywheel/strategies.sqlite3";',
    ),
    "the strategy database path must stay the admission's own constant",
  );
  // The Node tool maps a store's own `strategy_meta` version to a domain
  // version. Read that mapping out of the Rust layout registry so the mirror
  // cannot drift from the formats the admission actually knows.
  const formatVersions = {};
  for (const match of compact.matchAll(
    /meta_versions:&\["(\d+)"\],domain_schema_version:(\d+),/gu,
  )) {
    formatVersions[match[1]] = Number(match[2]);
  }
  assert.deepEqual(
    formatVersions,
    { ...ADAPTIVE_FLYWHEEL_SCHEMA_VERSIONS },
    "the Node strategy-version mapping must mirror the Rust layout registry",
  );
  // The tool derives the Conversation contract from the owner DDL at runtime,
  // so the anchors it reads must stay present in the owner source.
  const conversationSchema = await fs.promises.readFile(
    path.join(repoRoot, CONVERSATION_SCHEMA),
    "utf8",
  );
  for (const anchor of [
    "fn ensure_current_layout",
    "DROP INDEX IF EXISTS memberships_active_unique",
    "CREATE TABLE IF NOT EXISTS principals",
  ]) {
    assert.ok(
      conversationSchema.includes(anchor),
      `the Conversation owner no longer carries ${anchor}`,
    );
  }
  const [frontierConsumer, reportConsumer] = await Promise.all([
    fs.promises.readFile(path.join(repoRoot, FRONTIER_CONSUMER), "utf8"),
    fs.promises.readFile(path.join(repoRoot, REPORT_CONSUMER), "utf8"),
  ]);
  assert.ok(frontierConsumer.includes("sourceFrontierId"));
  assert.ok(reportConsumer.includes("sourceFrontierId"));
  assert.ok(reportConsumer.includes("pending_authorization"));
  assert.ok(compact.includes('root.join("client-state/conversations/conversations.sqlite3")'));
  assert.ok(compact.includes('root.join("client-state/conversations/migration-v5.complete")'));
  const completionSource =
    `value=="schema=v5${BACKSLASH}nstatus=complete${BACKSLASH}n"`;
  assert.ok(
    compact.includes(completionSource),
    "the completion marker contract moved in the admission",
  );
  const currentSchema = conversationStore.match(
    /pub const CURRENT_SCHEMA_VERSION: &str = "(\d+)";/u,
  );
  assert.ok(currentSchema, "the conversation store schema version moved");
  const probe = await fs.promises.readFile(
    path.join(repoRoot, "tools/scripts/client-state-migration/probe.mjs"),
    "utf8",
  );
  assert.ok(probe.includes(`const CONVERSATION_SCHEMA_VERSION = "${currentSchema[1]}";`));

  const collections = policy
    .slice(
      policy.indexOf("pub const COLLECTIONS"),
      policy.indexOf("];", policy.indexOf("pub const COLLECTIONS")),
    )
    .match(/"([a-z0-9-]+)"/gu)
    .map((quoted) => quoted.slice(1, -1));
  assert.deepEqual(collections, [
    "settings",
    "targets",
    "target-discovery-cache",
    "pairings",
    "skills",
    "pins",
    "identities",
    "conversation-archive-profiles",
    "agent-usage-reports",
    "provider-quota-snapshots",
    "skill-usage",
    "collaboration-plugins",
    "local-server-assemblies",
    "local-server-assembly-cleanup",
    "local-server-assembly-transaction",
    "mcp-install-transactions",
  ]);
  const stateSchema = policy.match(
    /pub const STATE_SCHEMA_VERSION: &str = "([^"]+)";/u,
  );
  assert.ok(stateSchema, "the client-state schema marker moved");
  assert.ok(probe.includes(`const CLIENT_STATE_SCHEMA_VERSION = "${stateSchema[1]}";`));
  assert.match(clientStateMigration, /"client state collection owner mismatch"/u);
});

test("the diagnostic module keeps its facade, its leaf set, and one authority per leaf", async () => {
  const leaves = fs.readdirSync(path.join(repoRoot, moduleRoot)).sort();
  assert.deepEqual(leaves, [
    "cli.mjs",
    "errors.mjs",
    "frontier.mjs",
    "ledger.mjs",
    "probe.mjs",
    "repair.mjs",
    "report.mjs",
    "sqlite-contract.mjs",
    "util.mjs",
  ]);
  const facade = await fs.promises.readFile(path.join(repoRoot, facadeRef), "utf8");
  assert.match(facade, /runClientStateMigrationCli\(\);/u);
  assert.equal(facade.includes("readFileSync"), false);
  const sources = Object.fromEntries(await Promise.all(leaves.map(async (leaf) => [
    leaf,
    await fs.promises.readFile(path.join(repoRoot, moduleRoot, leaf), "utf8"),
  ])));
  for (const [leaf, source] of Object.entries(sources)) {
    assert.equal(
      source.includes(`../client-state-migration${".mjs"}`),
      false,
      `${leaf} must not import the facade`,
    );
  }
  const owners = (declaration) =>
    leaves.filter((leaf) => new RegExp(`export (?:async )?function ${declaration}\\(`, "u")
      .test(sources[leaf]));
  assert.deepEqual(owners("parseArgs"), ["cli.mjs"]);
  assert.deepEqual(owners("evaluateMigrationState"), ["report.mjs"]);
  assert.deepEqual(owners("repairDomain"), ["repair.mjs"]);
  assert.deepEqual(owners("loadEmbeddedFrontier"), ["frontier.mjs"]);
  assert.deepEqual(owners("loadLedger"), ["ledger.mjs"]);
  assert.deepEqual(owners("probeDomain"), ["probe.mjs"]);
  assert.deepEqual(owners("writePrivateJsonAtomic"), ["util.mjs"]);
  assert.equal(fs.existsSync(path.join(repoRoot, FRONTIER_REF)), true);
});
