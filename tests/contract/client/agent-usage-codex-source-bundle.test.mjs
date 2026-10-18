import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { CLIENT_MODULE_CATALOG } from "../../../tools/regression/client-module-catalog.mjs";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const sourceRoot =
  "crates/licoup-native/src/domain/agent_usage/agent_usage_codex";
const integrationRoot = "crates/licoup-native/tests/agent_usage_cache_cases";
const integrationFacade =
  "crates/licoup-native/tests/agent_usage_incremental_cache.rs";
const architectureRegistry =
  "apps/desktop/scripts/client-architecture/checks/native/domain-and-crypto-boundaries.mjs";
const compositionModuleId = "rust.domain.agent-usage-cache";

const productionLeaves = Object.freeze([
  "aggregation.rs",
  "append_guard.rs",
  "cache.rs",
  "cache_batch.rs",
  "cache_cleanup.rs",
  "constants.rs",
  "event_hash.rs",
  "file_collection.rs",
  "identity_refresh.rs",
  "lineage.rs",
  "models.rs",
  "parser.rs",
  "rollup.rs",
  "scan.rs",
  "scan_params.rs",
  "utils.rs",
]);

const integrationLeaves = Object.freeze([
  "adapter_coverage.rs",
  "append_refresh.rs",
  "cache_runtime.rs",
  "cumulative_resume.rs",
  "dedup_lineage.rs",
  "fallback_coverage.rs",
  "generic_usage.rs",
  "native_rollup.rs",
  "reconciliation.rs",
  "retained_reports.rs",
  "support.rs",
  "two_phase.rs",
  "windows.rs",
]);

const preciseScenarioModules = Object.freeze({
  "adapter_coverage.rs": "rust.domain.agent-usage-cache.adapter-coverage",
  "append_refresh.rs": "rust.domain.agent-usage-cache.append-refresh",
  "cache_runtime.rs": "rust.domain.agent-usage-cache.runtime",
  "cumulative_resume.rs": "rust.domain.agent-usage-cache.cumulative-resume",
  "dedup_lineage.rs": "rust.domain.agent-usage-cache.dedup-lineage",
  "fallback_coverage.rs": "rust.domain.agent-usage-cache.fallback-coverage",
  "generic_usage.rs": "rust.domain.agent-usage-cache.generic-usage",
  "native_rollup.rs": "rust.domain.agent-usage-cache.native-rollup",
  "reconciliation.rs": "rust.domain.agent-usage-cache.reconciliation",
  "retained_reports.rs": "rust.domain.agent-usage-cache.retained-reports",
  "two_phase.rs": "rust.domain.agent-usage-cache.two-phase",
  "windows.rs": "rust.domain.agent-usage-cache.windows",
});

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function discoverIntegrationLeaves() {
  const rustLeaves = [];
  async function visit(relativeDirectory) {
    const entries = await fs.readdir(
      path.join(repoRoot, integrationRoot, relativeDirectory),
      { withFileTypes: true },
    );
    for (const entry of entries) {
      const relativePath = path.posix.join(relativeDirectory, entry.name);
      if (entry.isDirectory()) {
        await visit(relativePath);
        continue;
      }
      assert.equal(
        entry.isFile(),
        true,
        `${relativePath} must be an ordinary Codex usage integration entry`,
      );
      if (!entry.name.endsWith(".rs") || relativePath === "mod.rs") continue;
      rustLeaves.push(relativePath);
    }
  }
  await visit("");
  return rustLeaves.sort();
}

function declaredIntegrationLeaves(composition) {
  return [...composition.matchAll(/^mod ([a-z_]+);$/gmu)].map(
    (match) => `${match[1]}.rs`,
  );
}

function assertIntegrationTestCommand(module, expectedFilter) {
  assert.equal(module.command.program, "cargo");
  assert.equal(module.command.args[0], "test");
  const testFlagIndexes = module.command.args
    .map((argument, index) => argument === "--test" ? index : -1)
    .filter((index) => index !== -1);
  assert.equal(testFlagIndexes.length, 1);
  assert.equal(
    module.command.args[testFlagIndexes[0] + 1],
    "agent_usage_incremental_cache",
  );
  assert.equal(module.command.args.at(-1), expectedFilter);
}

function architectureIntegrationLeaves(source) {
  const registryMarker =
    "const agentUsageCacheScenarioLeaves = new Set([";
  const registryStart = source.indexOf(registryMarker);
  assert.notEqual(registryStart, -1, "missing Codex usage architecture registry");
  const entriesStart = registryStart + registryMarker.length;
  const registryEnd = source.indexOf("]);", entriesStart);
  assert.notEqual(registryEnd, -1, "unterminated Codex usage architecture registry");
  const nextDeclaration = source.indexOf(
    "const agentUsageCacheIntegrationFacade",
    registryEnd + 3,
  );
  assert.notEqual(
    nextDeclaration,
    -1,
    "missing declaration after Codex usage architecture registry",
  );
  assert.match(
    source.slice(registryEnd + 3, nextDeclaration),
    /^\s*$/u,
    "Codex usage architecture registry must end after its static Set",
  );

  const entriesSource = source.slice(entriesStart, registryEnd);
  const entryPattern =
    /`\$\{agentUsageCacheIntegrationRoot\}\/([a-z_]+\.rs)`/yu;
  const leaves = [];
  let cursor = 0;
  while (cursor < entriesSource.length) {
    const whitespace = /^\s*/u.exec(entriesSource.slice(cursor));
    cursor += whitespace[0].length;
    if (cursor === entriesSource.length) break;
    entryPattern.lastIndex = cursor;
    const entry = entryPattern.exec(entriesSource);
    assert.ok(
      entry,
      `non-static Codex usage architecture registry entry at offset ${cursor}`,
    );
    leaves.push(entry[1]);
    cursor = entryPattern.lastIndex;
    const trailingWhitespace = /^\s*/u.exec(entriesSource.slice(cursor));
    cursor += trailingWhitespace[0].length;
    if (cursor === entriesSource.length) break;
    assert.equal(
      entriesSource[cursor],
      ",",
      `missing Codex usage architecture registry delimiter at offset ${cursor}`,
    );
    cursor += 1;
  }
  return leaves;
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${sourceRoot}/${leaf}`),
  ])));
}

test("Codex usage facade is thin and owns every production leaf", async () => {
  const facade = await read(`${sourceRoot}.rs`);
  assert.deepEqual(
    [...facade.matchAll(/^mod ([a-z_]+);$/gmu)]
      .map((match) => match[1])
      .filter((moduleName) => moduleName !== "tests")
      .map((moduleName) => `${moduleName}.rs`)
      .sort(),
    [...productionLeaves].sort(),
  );
  for (const implementationToken of [
    "struct TokenTotals",
    "Connection::open",
    "fs::read_dir",
    "BufReader::new",
    "include!(",
    "#[path",
  ]) {
    assert.equal(facade.includes(implementationToken), false);
  }
});

test("Codex usage leaves retain bounded single responsibilities", async () => {
  const source = await sources();
  for (const [leaf, body] of Object.entries(source)) {
    assert.equal(body.includes("include!("), false);
    assert.equal(body.includes("#[path"), false);
    assert.equal(/^mod [a-z_]+;$/mu.test(body), false);
  }
});

test("Codex usage discovery and parsing stay local, read-only, and bounded", async () => {
  const source = await sources();
  const collection = source["file_collection.rs"];
  const parser = source["parser.rs"];
  for (const token of [
    "HashSet::<PathBuf>::new()",
    "BTreeSet::<PathBuf>::new()",
    "file_type().is_symlink()",
  ])
    assert.ok(collection.includes(token), `missing bounded discovery token: ${token}`);
  assert.ok(collection.includes("fs::read_dir"));
  assert.ok(parser.includes("fs::File::open(path)"));
  assert.ok(parser.includes("BufReader::new(file)"));
  assert.ok(parser.includes("return Ok(line_start)"));
  for (const mutationToken of ["OpenOptions", ".write_all(", ".set_len(", "fs::remove_file"])
    assert.equal(parser.includes(mutationToken), false);
});

test("Codex usage cache keeps incremental guards, prepared batches, and indexed queries", async () => {
  const source = await sources();
  const joined = Object.values(source).join("\n");
  for (const token of [
    'pragma_update(None, "journal_mode", "WAL")',
    "TransactionBehavior::Immediate",
    "CREATE INDEX usage_rows_window",
    "CREATE INDEX usage_rows_identity",
    "struct CacheBatch",
    "struct ParserBatch",
    "Statement<'connection>",
    "append_guard_matches",
    "extend_content_guard",
  ]) {
    assert.ok(joined.includes(token), `missing incremental cache boundary: ${token}`);
  }
});

test("Codex usage projection exposes aggregates without local paths or content", async () => {
  const source = await sources();
  const aggregation = source["aggregation.rs"];
  const schema = source["cache.rs"];
  const scan = source["scan.rs"];
  assert.ok(aggregation.includes('source: Some("codex-local-token-events")'));
  assert.ok(aggregation.includes('"codex-local-usage-store".to_string()'));
  assert.equal(aggregation.includes("to_string_lossy"), false);
  assert.equal(aggregation.includes("PathBuf"), false);
  for (const forbiddenColumn of ["source_path TEXT", "raw_content", "message_content"])
    assert.equal(schema.includes(forbiddenColumn), false);
  assert.ok(scan.includes('"code": "codex_local_token_event_scan_failed"'));
  assert.equal(scan.includes('"error":'), false);
  assert.equal(scan.includes("error.to_string()"), false);
});

test("Codex usage canonical identity and lineage remain deterministic", async () => {
  const source = await sources();
  assert.ok(source["event_hash.rs"].includes("hash_canonical_json"));
  assert.ok(source["event_hash.rs"].includes("keys.sort()"));
  assert.ok(source["lineage.rs"].includes("HashMap<String, String>"));
  assert.ok(source["lineage.rs"].includes("BTreeSet::<String>::new()"));
  assert.ok(source["rollup.rs"].includes("prior.event_identity=r.event_identity"));
  assert.ok(source["rollup.rs"].includes("prior_file.lineage_scope=f.lineage_scope"));
});

test("Codex usage integration composition matches every ordinary leaf on disk exactly once", async () => {
  const crateRoot = await read(integrationFacade);
  const composition = await read(`${integrationRoot}/mod.rs`);
  const discoveredLeaves = await discoverIntegrationLeaves();
  const declaredLeaves = declaredIntegrationLeaves(composition);
  const expectedLeaves = [...integrationLeaves].sort();
  assert.equal(crateRoot.trim(), "mod agent_usage_cache_cases;");
  assert.deepEqual(discoveredLeaves, expectedLeaves);
  assert.equal(declaredLeaves.length, new Set(declaredLeaves).size);
  assert.deepEqual([...declaredLeaves].sort(), discoveredLeaves);
  for (const leaf of integrationLeaves) {
    const body = await read(`${integrationRoot}/${leaf}`);
    assert.equal(body.includes("include!("), false);
    assert.equal(body.includes("#[path"), false);
  }
});

test("Codex usage architecture registry owns the complete integration leaf set exactly once", async () => {
  const registeredLeaves = architectureIntegrationLeaves(
    await read(architectureRegistry),
  );
  assert.equal(registeredLeaves.length, new Set(registeredLeaves).size);
  assert.deepEqual(
    [...registeredLeaves].sort(),
    [...integrationLeaves].sort(),
  );
});

test("Codex usage regression catalog gives every scenario one precise owner and support one composition owner", () => {
  const expectedScenarioLeaves = Object.keys(preciseScenarioModules).sort();
  assert.equal(expectedScenarioLeaves.length, 12);
  assert.deepEqual(
    integrationLeaves
      .filter((leaf) => leaf !== "support.rs")
      .sort(),
    expectedScenarioLeaves,
  );

  const moduleById = new Map(CLIENT_MODULE_CATALOG.map((module) => [
    module.id,
    module,
  ]));
  assert.equal(moduleById.size, CLIENT_MODULE_CATALOG.length);
  const scenarioPaths = new Set(Object.keys(preciseScenarioModules).map(
    (leaf) => `${integrationRoot}/${leaf}`,
  ));
  const scenarioInputOwners = new Map();
  const supportPath = `${integrationRoot}/support.rs`;
  const supportInputOwners = [];
  for (const module of CLIENT_MODULE_CATALOG) {
    for (const input of module.inputs) {
      if (scenarioPaths.has(input)) {
        const owners = scenarioInputOwners.get(input) ?? [];
        owners.push(module.id);
        scenarioInputOwners.set(input, owners);
      }
      if (input === supportPath) supportInputOwners.push(module.id);
    }
  }

  for (const [leaf, moduleId] of Object.entries(preciseScenarioModules)) {
    const module = moduleById.get(moduleId);
    const scenarioPath = `${integrationRoot}/${leaf}`;
    assert.ok(module, `missing precise regression module for ${leaf}`);
    assert.equal(module.inputs.includes(scenarioPath), true);
    assertIntegrationTestCommand(
      module,
      `agent_usage_cache_cases::${leaf.slice(0, -3)}::`,
    );
    assert.deepEqual(scenarioInputOwners.get(scenarioPath), [moduleId]);
  }
  assert.deepEqual(
    [...scenarioInputOwners.keys()].sort(),
    [...scenarioPaths].sort(),
  );

  const compositionModule = moduleById.get(compositionModuleId);
  assert.ok(compositionModule, "missing Codex usage composition/support module");
  assert.ok(compositionModule.inputs.includes(integrationFacade));
  assert.ok(compositionModule.inputs.includes(`${integrationRoot}/mod.rs`));
  assert.ok(compositionModule.inputs.includes(supportPath));
  assert.deepEqual(supportInputOwners, [compositionModuleId]);
  assertIntegrationTestCommand(compositionModule, "agent_usage_cache_cases::");
});
