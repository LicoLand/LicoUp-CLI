import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const frontierPath = "crates/licoup-native/resources/client-state-migration-frontier.json";
const migrationPath = "crates/licoup-native/src/domain/client_state_migration.rs";
const storesPath = "crates/licoup-native/src/domain/client_state_migration/stores.rs";
const lifecyclePath = "apps/desktop/lib/src/application/controller/client_lifecycle_facade.dart";
const portableRootPath = "apps/desktop/lib/src/platform/storage/portable_data_root.dart";
const conversationServicePath =
  "crates/licoup-native/src/domain/client_conversation/service.rs";

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

test("embedded migration frontier is closed, unique, and contiguous", async () => {
  const frontier = JSON.parse(await read(frontierPath));
  assert.equal(
    frontier.schemaVersion,
    "v0.0.1:client-state-migration-frontier-1",
  );
  assert.match(frontier.frontierId, /^[a-z0-9][a-z0-9.-]+$/u);
  assert.ok(frontier.domains.length > 0);

  const domainIds = new Set();
  const stepIds = new Set();
  for (const domain of frontier.domains) {
    assert.equal(domain.durability, "durable");
    assert.ok(!domainIds.has(domain.domainId), `duplicate domain ${domain.domainId}`);
    domainIds.add(domain.domainId);
    let cursor = 0;
    for (const step of domain.steps) {
      assert.equal(step.fromSchemaVersion, cursor, `${step.stepId} is not contiguous`);
      assert.ok(step.toSchemaVersion > cursor, `${step.stepId} does not advance`);
      assert.ok(!stepIds.has(step.stepId), `duplicate step ${step.stepId}`);
      stepIds.add(step.stepId);
      cursor = step.toSchemaVersion;
    }
    assert.equal(cursor, domain.targetSchemaVersion, `${domain.domainId} has an incomplete path`);
  }
});

test("Rust admission owns every frontier adapter and the legacy conversation import", async () => {
  const [frontier, migration, stores, conversationService] = await Promise.all([
    read(frontierPath).then(JSON.parse),
    read(migrationPath),
    read(storesPath),
    read(conversationServicePath),
  ]);
  const appearance = await read("crates/licoup-native/src/domain/client_state_migration/appearance.rs");
  const appearanceDomain = /const DOMAIN_ID: &str = ("[^"]+")/.exec(appearance);
  assert.ok(appearanceDomain, "appearance admission must declare its domain identity");
  const routedStores = stores.replaceAll("super::appearance::DOMAIN_ID", appearanceDomain[1]);
  const admissionSource = `${migration}\n${routedStores}`;
  for (const domain of frontier.domains) {
    const occurrences = admissionSource.split(`"${domain.domainId}"`).length - 1;
    assert.ok(
      occurrences >= 2,
      `${domain.domainId} must have explicit probe and apply routing`,
    );
  }
  assert.match(stores, /client_conversation::migrate_legacy_state/u);
  assert.doesNotMatch(conversationService, /migrate_legacy_state/u);
  assert.match(migration, /write_json_atomic\(&ledger_path/u);
  assert.match(migration, /probe_domain\(&marker_root/u);
  assert.match(admissionSource, /state_newer_than_binary/u);
});

test("desktop startup admits the raw root before loading product state", async () => {
  const [lifecycle, portableRoot, bridge] = await Promise.all([
    read(lifecyclePath),
    read(portableRootPath),
    read("schemas/client_bridge/state.json").then(JSON.parse),
  ]);
  const rootStep = lifecycle.indexOf("id: 'client_storage_root'");
  const migrationStep = lifecycle.indexOf("id: 'client_state_migration'");
  const storageStep = lifecycle.indexOf("id: 'client_storage'");
  assert.ok(rootStep >= 0 && rootStep < migrationStep && migrationStep < storageStep);

  const rawRootBody = lifecycle.slice(
    lifecycle.indexOf("Future<void> _resolveClientStorageRoot"),
    lifecycle.indexOf("Future<void> _admitClientStateMigration"),
  );
  assert.match(rawRootBody, /portableData\.dataDirectory\(\)/u);
  assert.doesNotMatch(rawRootBody, /loadWorkspaceManifest/u);
  assert.match(lifecycle, /await portableData\.loadWorkspaceManifest\(\)/u);

  const dataDirectoryBody = portableRoot.slice(
    portableRoot.indexOf("Future<Directory> dataDirectory()"),
    portableRoot.indexOf("Future<Directory> clientDirectory()"),
  );
  assert.doesNotMatch(dataDirectoryBody, /loadOrCreate/u);
  assert.deepEqual(bridge.operations, ["get", "set", "admit"]);
  for (const code of [
    "migration_lock_unavailable",
    "migration_ledger_invalid",
    "state_newer_than_binary",
    "migration_frontier_incomplete",
    "migration_step_failed",
    "migration_postcondition_failed",
    "update_handoff_mismatch",
    "unsupported_state_shape",
  ]) {
    assert.ok(bridge.failureCodes.includes(code), `missing bridge failure ${code}`);
  }
});
