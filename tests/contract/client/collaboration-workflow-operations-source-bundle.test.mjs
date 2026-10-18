import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
const operationsRoot =
  "crates/licoup-native/src/domain/collaboration_plugin/workflow/operations";
const productionLeaves = Object.freeze([
  "apply_local.rs",
  "apply_mcp.rs",
  "cancel.rs",
  "destination_policy.rs",
  "package_revalidation.rs",
  "plan_local.rs",
  "plan_mcp.rs",
  "projection.rs",
  "staging.rs",
  "validation.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sources() {
  return Object.fromEntries(await Promise.all(productionLeaves.map(async (leaf) => [
    leaf,
    await read(`${operationsRoot}/${leaf}`),
  ])));
}

test("collaboration workflow operations facade is thin and owns exactly ten leaves", async () => {
  const facade = await read(`${operationsRoot}.rs`);
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
    "fn validate_apply_binding(",
    "fn validate_new_destination(",
    "fn revalidate_payload(",
    "fn stage_mcp_units(",
    "fn plan_projection(",
  ]) {
    assert.equal(facade.includes(implementation), false);
  }
});

test("operation leaves are bounded and retain an explicit acyclic dependency direction", async () => {
  const source = await sources();
  for (const leaf of productionLeaves) {
    assert.equal(source[leaf].includes("use super::*"), false, `${leaf} has wildcard coupling`);
  }
  for (const base of ["projection.rs", "validation.rs"]) {
    for (const dependency of [
      "apply_local", "apply_mcp", "cancel", "destination_policy",
      "package_revalidation", "plan_local", "plan_mcp", "staging",
    ]) {
      assert.equal(source[base].includes(`super::${dependency}`), false, `${base} depends upward`);
    }
  }
  assert.ok(source["destination_policy.rs"].includes("super::validation"));
  assert.ok(source["package_revalidation.rs"].includes("super::destination_policy"));
  assert.equal(source["destination_policy.rs"].includes("super::package_revalidation"), false);
  for (const operation of ["plan_local.rs", "apply_local.rs"]) {
    assert.ok(source[operation].includes("super::destination_policy"));
    assert.ok(source[operation].includes("super::package_revalidation"));
    assert.ok(source[operation].includes("super::projection"));
    assert.ok(source[operation].includes("super::validation"));
  }
  for (const operation of ["plan_mcp.rs", "apply_mcp.rs"]) {
    assert.ok(source[operation].includes("super::destination_policy"));
    assert.ok(source[operation].includes("super::validation"));
  }
  assert.equal(source["plan_mcp.rs"].includes("super::staging"), false);
  assert.ok(source["apply_mcp.rs"].includes("super::staging"));
});

test("validation and cancellation retain explicit approval and digest binding", async () => {
  const source = await sources();
  for (const token of [
    "requestOrigin",
    "agentTriggered",
    "scheduled",
    "startupTriggered",
    "validate_expected_digests",
    "validate_sha256",
    "WORKFLOW_PLAN_TTL_SECONDS",
    "collaboration_workflow_selection_duplicate",
  ]) {
    assert.ok(source["validation.rs"].includes(token), `missing validation boundary: ${token}`);
  }
  assert.ok(source["cancel.rs"].includes("require_direct_confirmation"));
  assert.ok(source["cancel.rs"].includes("validate_expected_digests"));
  assert.ok(source["cancel.rs"].includes("abandon_claim"));
});

test("destination and package revalidation fail closed before commit", async () => {
  const source = await sources();
  for (const token of [
    "MAX_AGENT_DESTINATIONS",
    "MAX_DESTINATION_BYTES",
    "open_directory_path_no_follow",
    "validate_export_destination",
    "symlink_metadata",
    "collaboration_workflow_destination_overlap",
  ]) {
    assert.ok(source["destination_policy.rs"].includes(token), `missing destination boundary: ${token}`);
  }
  for (const token of [
    "inspect_current_plugin",
    "collaboration_plugin_installed_digest_mismatch",
    "collaboration_workflow_installed_package_changed",
    "planned_payload(&payload)? == record.payload_files",
  ]) {
    assert.ok(source["package_revalidation.rs"].includes(token), `missing package binding: ${token}`);
  }
});

test("staging owns private registrations, rollback cleanup, and claim settlement", async () => {
  const source = (await sources())["staging.rs"];
  for (const token of [
    "stage_private_registration",
    "collaboration_mcp_registration_digest_mismatch",
    "cleanup_staged",
    "settle_apply_claim",
    "abandon_claim",
  ]) {
    assert.ok(source.includes(token), `missing staging boundary: ${token}`);
  }
});

test("every operation leaf plus facade retains a dedicated regression module", async () => {
  const testFacade = await read(`${operationsRoot}/tests/mod.rs`);
  assert.deepEqual(
    [...testFacade.matchAll(/^mod ([a-z_]+);$/gmu)].map((match) => match[1]).sort(),
    [
      ...productionLeaves.map((leaf) => leaf.replace(".rs", "")),
      "composition",
    ].sort(),
  );
  for (const leaf of productionLeaves) {
    await fs.access(path.join(repoRoot, `${operationsRoot}/tests/${leaf}`));
  }
  await fs.access(path.join(repoRoot, `${operationsRoot}/tests/composition.rs`));
});
