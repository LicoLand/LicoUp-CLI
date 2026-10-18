import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(fileURLToPath(new URL("../../..", import.meta.url)));
const facadePath = "crates/licoup-native/src/core/secure_mesh_capability.rs";
const root = "crates/licoup-native/src/core/secure_mesh_capability";
const productionLeaves = Object.freeze([
  "catalog.rs",
  "custody.rs",
  "evaluation.rs",
  "facts.rs",
  "report.rs",
  "taxonomy.rs",
]);

async function read(relativePath) {
  return fs.readFile(path.join(repoRoot, relativePath), "utf8");
}

async function sourceFiles(relativeRoot) {
  const found = [];
  async function visit(relativeDirectory) {
    for (const entry of await fs.readdir(path.join(repoRoot, relativeDirectory), {
      withFileTypes: true,
    })) {
      const relativePath = path.posix.join(relativeDirectory, entry.name);
      if (entry.isDirectory()) await visit(relativePath);
      else if (entry.isFile() && relativePath.endsWith(".rs")) found.push(relativePath);
    }
  }
  await visit(relativeRoot);
  return found.sort();
}

test("secure mesh capability root is an exact restricted stable facade", async () => {
  const facade = await read(facadePath);
  for (const leaf of productionLeaves) {
    assert.match(facade, new RegExp(`mod ${leaf.replace(".rs", "")};`, "u"));
    await fs.access(path.join(repoRoot, root, leaf));
  }
  const entries = await fs.readdir(path.join(repoRoot, root), { withFileTypes: true });
  assert.deepEqual(
    entries.filter((entry) => entry.isFile()).map((entry) => entry.name).sort(),
    [...productionLeaves].sort(),
  );
  for (const forbidden of [
    "struct ", "enum ", "impl ", "fn ", "include_str!", "OnceLock", "BTreeSet",
  ]) assert.equal(facade.includes(forbidden), false, forbidden);
});

test("taxonomy owns exhaustive constant-time identifiers and serde", async () => {
  const taxonomy = await read(`${root}/taxonomy.rs`);
  assert.match(taxonomy, /pub const COUNT: usize = 31;/u);
  assert.match(taxonomy, /pub const fn id\(self\)/u);
  assert.match(taxonomy, /pub fn from_id\(id: &str\)[\s\S]*match id \{/u);
  assert.match(taxonomy, /impl Serialize for SecurityCapability/u);
  assert.match(taxonomy, /impl<'de> Deserialize<'de> for SecurityCapability/u);
  assert.equal(taxonomy.includes(".find("), false);
  assert.equal(taxonomy.includes("CapabilityCatalog"), false);
});

test("catalog exclusively owns bounded embedded DAG construction and cache", async () => {
  const catalog = await read(`${root}/catalog.rs`);
  for (const token of [
    "MAX_CAPABILITY_CATALOG_BYTES", "include_str!", "OnceLock", "indegree",
    "dependents", "pop_first()", "validated_topological_order", "require_complete",
  ]) assert.equal(catalog.includes(token), true, token);
  assert.match(catalog, /\[0usize; SecurityCapability::COUNT\]/u);
  assert.match(catalog, /definitions\[capability\.index\(\)\]/u);
  for (const foreign of [
    "CapabilityFact", "CapabilityEvaluation", "CustodySelection",
    "CapabilityEvaluationReport",
  ]) assert.equal(catalog.includes(foreign), false, foreign);
});

test("facts custody evaluation and report form one-way leaves", async () => {
  const sources = Object.fromEntries(await Promise.all([
    "facts.rs", "custody.rs", "evaluation.rs", "report.rs",
  ].map(async (leaf) => [leaf, await read(`${root}/${leaf}`)])));

  assert.match(sources["facts.rs"], /reason_code\.len\(\) <= 96/u);
  assert.match(sources["facts.rs"], /serde\(deny_unknown_fields/u);
  assert.equal(sources["facts.rs"].includes("CapabilityEvaluation"), false);
  assert.equal(sources["facts.rs"].includes("CustodySelection"), false);

  assert.match(sources["custody.rs"], /custody_selection_from_enabled/u);
  assert.equal(sources["custody.rs"].includes("CapabilityCatalog"), false);
  assert.equal(sources["custody.rs"].includes("CapabilityEvaluation"), false);

  assert.match(
    sources["evaluation.rs"],
    /\[Option<&CapabilityFact>; SecurityCapability::COUNT\]/u,
  );
  assert.match(
    sources["evaluation.rs"],
    /\[false; SecurityCapability::COUNT\]/u,
  );
  assert.equal(
    (sources["evaluation.rs"].match(/for capability in self\.topological_order\(\)/gu) ?? []).length,
    1,
  );
  assert.match(sources["evaluation.rs"], /facts\.len\(\) <= SecurityCapability::COUNT/u);
  assert.equal(sources["evaluation.rs"].includes("CapabilityEvaluationReport"), false);
  assert.equal(sources["evaluation.rs"].includes("Serialize"), false);

  assert.match(sources["report.rs"], /impl CapabilityEvaluation/u);
  assert.match(sources["report.rs"], /CAPABILITY_REPORT_SCHEMA_VERSION/u);
  assert.equal(sources["report.rs"].includes("impl CapabilityCatalog"), false);
  assert.equal(sources["report.rs"].includes("topological_order"), false);
});

test("external consumers cannot depend on capability implementation leaves", async () => {
  const internalModules = "catalog|custody|evaluation|facts|report|taxonomy";
  const internalPath = new RegExp(`secure_mesh_capability::(?:${internalModules})::`, "u");
  const consumers = (await sourceFiles("crates/licoup-native/src"))
    .filter((relativePath) => relativePath !== facadePath && !relativePath.startsWith(`${root}/`));
  for (const relativePath of consumers) {
    const source = await read(relativePath);
    assert.equal(internalPath.test(source), false, relativePath);
  }
});

test("capability production leaves contain no egress or unsafe runtime authority", async () => {
  const production = (await Promise.all(
    productionLeaves.map((leaf) => read(`${root}/${leaf}`)),
  )).join("\n");
  for (const forbidden of [
    "ureq::", "reqwest::", "TcpStream", "UdpSocket", "unsafe {", "Command::new",
  ]) assert.equal(production.includes(forbidden), false, forbidden);
});

test("every capability responsibility owns a dedicated narrow regression", async () => {
  const entries = (await fs.readdir(path.join(repoRoot, root, "tests"))).sort();
  assert.deepEqual(entries, [
    "catalog.rs", "composition.rs", "custody.rs", "evaluation.rs", "facts.rs", "mod.rs",
    "report.rs", "support.rs", "taxonomy.rs",
  ]);
});
