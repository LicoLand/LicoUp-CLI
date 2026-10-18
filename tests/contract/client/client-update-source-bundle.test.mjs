import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/domain/client_update.rs";
const moduleRoot = "crates/licoup-native/src/domain/client_update";
const productionFiles = [
  facadePath,
  ...[
    "apply.rs",
    "archive.rs",
    "canonical.rs",
    "check.rs",
    "constants.rs",
    "dispatch.rs",
    "download.rs",
    "github_source.rs",
    "keys.rs",
    "metadata.rs",
    "model.rs",
    "native_runner/mod.rs",
    "native_runner/macos_integrity.rs",
    "native_runner/plan.rs",
    "native_runner/script.rs",
    "native_runner/spawn.rs",
    "params.rs",
    "release.rs",
    "release/artifact.rs",
    "receipt.rs",
    "revocation.rs",
    "selection.rs",
    "signature.rs",
    "staging.rs",
    "staging/copy.rs",
    "staging/path.rs",
    "status.rs",
    "tree.rs",
    "verify.rs",
  ].map((file) => `${moduleRoot}/${file}`),
];
const expectedTestLeaves = [
  "admission.rs",
  "archive.rs",
  "artifact_binding.rs",
  "github_source.rs",
  "native_runner.rs",
  "release_selection.rs",
  "revocation.rs",
  "signature_roles.rs",
  "staging_paths.rs",
  "support.rs",
  "workflow.rs",
];

test("client update source bundle preserves fail-closed split release authority", async () => {
  const sources = await Promise.all(productionFiles.map((sourceRef) =>
    fs.readFile(path.join(repoRoot, sourceRef), "utf8")));
  const facade = sources[0];
  const source = sources.join("\n");
  for (const moduleName of [
    "apply",
    "archive",
    "check",
    "download",
    "github_source",
    "keys",
    "native_runner",
    "receipt",
    "release",
    "revocation",
    "selection",
    "signature",
    "staging",
    "tree",
    "verify",
  ]) {
    assert.ok(facade.includes(`mod ${moduleName};`), `client update facade is missing ${moduleName}`);
  }
  for (const token of [
    "verify_manifest_role_signatures",
    "requires a valid offline root signature",
    "requires a valid online signing signature",
    "manifest-2",
    "runningReleaseTrack",
    "targetReleaseTrack",
    'get("keys")',
    "cmp_precedence",
    "CLIENT_UPDATE_ARTIFACT_RECEIPT_SCHEMA",
    "CLIENT_UPDATE_REVOCATION_SCHEMA",
    "verify_required_signature",
    "reject_artifact_overrides",
    '"expectedSize"',
    "sha256_file_exact",
    "symlink_metadata",
    "fs::canonicalize",
    "MAX_ARCHIVE_ENTRIES",
    "entry.unpack_in",
    'bool_param(params, "execute")?',
    '"pathRedacted": true',
    '"publicMetadataOnly": true',
  ]) {
    assert.ok(source.includes(token), `client update source bundle is missing ${token}`);
  }
  for (const token of ["#[path", "include!(", "mod tests {"]) {
    assert.ok(!source.includes(token), `client update source bundle contains retired ${token}`);
  }
  for (const retired of ["allowDowngrade", '"rollback"', '"rolledBack"']) {
    assert.ok(!source.includes(retired), `client update source bundle contains retired ${retired}`);
  }
  const outputSources = await Promise.all([
    "apply.rs",
    "check.rs",
    "download.rs",
    "native_runner/mod.rs",
    "status.rs",
    "verify.rs",
  ].map((file) => fs.readFile(path.join(repoRoot, moduleRoot, file), "utf8")));
  for (const outputSource of outputSources) {
    for (const field of ["installedAppPath", "stagedAppPath", "restoredFrom", "sourcePath"]) {
      assert.ok(!outputSource.includes(`"${field}":`), `client update output exposes ${field}`);
    }
  }
});

test("client update regressions remain independently selectable leaves", async () => {
  const entries = await fs.readdir(path.join(repoRoot, moduleRoot, "tests"), {
    withFileTypes: true,
  });
  const leaves = entries
    .filter((entry) => entry.isFile() && entry.name.endsWith(".rs"))
    .map((entry) => entry.name)
    .sort();
  assert.deepEqual(leaves, [...expectedTestLeaves].sort());
  const testFacade = await fs.readFile(path.join(repoRoot, `${moduleRoot}/tests.rs`), "utf8");
  assert.ok(!testFacade.includes("mod tests {"));
  assert.ok(!testFacade.includes("#[path"));
});
