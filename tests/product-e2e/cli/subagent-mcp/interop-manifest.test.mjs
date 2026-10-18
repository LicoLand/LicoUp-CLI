import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { INTEROP_MANIFEST_FIELDS, INTEROP_MANIFEST_RELATIVE_PATH, TARGET_AGENTS, InteropManifestError,
  createInteropRecord, parseInteropManifestYaml, persistTargetRecord, readRepoAppVersion,
  isInteropVersion, renderInteropManifestYaml, shouldSkipTarget, upsertTargetRecord } from "./interop-manifest.mjs";

const passed = (targetAgent = "cursor", targetAgentVersion = "2.5.0") => ({
  appVersion: "0.1.1", callerAgent: targetAgent === "codex" ? "cursor" : "codex",
  callerAgentVersion: "1.2.3", targetAgent, targetAgentVersion, results: "passed", notes: "",
});

function assertTrackedManifestState(raw, currentAppVersion) {
  const records = parseInteropManifestYaml(raw);
  if (records.length === 0) return;
  assert.equal(records.length, TARGET_AGENTS.length);
  assert.deepEqual(
    records.map((record) => record.targetAgent).sort(),
    [...TARGET_AGENTS].sort(),
  );
  assert.ok(records.every((record) => record.appVersion === currentAppVersion
    && record.results === "passed" && record.notes === ""));
}

test("Manifest rows have the exact seven ordered fields and one target key", () => {
  assert.deepEqual(INTEROP_MANIFEST_FIELDS, ["App Version", "Caller Agent", "Caller Agent Version", "Target Agent", "Target Agent Version", "Results", "Notes"]);
  const yaml = renderInteropManifestYaml([passed()]);
  const fieldLines = yaml.split("\n").filter((line) => /^- |^  /u.test(line));
  assert.deepEqual(fieldLines.map((line) => line.replace(/^- |^  /u, "").split(":")[0]), INTEROP_MANIFEST_FIELDS);
  assert.deepEqual(parseInteropManifestYaml(yaml), [createInteropRecord(passed())]);
});

test("closed parser rejects duplicate, extra, missing, reordered, and unsafe fields", () => {
  const valid = renderInteropManifestYaml([passed()]);
  const duplicate = `${valid.trimEnd()}\n\n${valid.split("\n").slice(3).join("\n")}`;
  const invalid = [valid.replace("  Notes:", "  Extra: \"x\"\n  Notes:"), valid.replace(/  Notes:.*\n/u, ""),
    valid.replace("  Caller Agent:", "  Target Agent:"), valid.replace("2.5.0", "not-a-version"),
    duplicate,
    valid.replace(/^#/u, "# altered header")];
  for (const yaml of invalid) assert.throws(() => parseInteropManifestYaml(yaml), InteropManifestError);
  assert.throws(() => createInteropRecord({ ...passed(), notes: "arbitrary_text" }), InteropManifestError);
});

test("same-version target upsert replaces one target and a new App Version drops old rows", () => {
  let records = upsertTargetRecord([], passed("codex", "5.3.0"));
  records = upsertTargetRecord(records, passed("cursor", "2.5.0"));
  records = upsertTargetRecord(records, { ...passed("codex", "5.4.0"), results: "failed", notes: "direct_mcp_failed" });
  assert.equal(records.length, 2); assert.equal(records[0].targetAgentVersion, "5.4.0");
  records = upsertTargetRecord(records, { ...passed("antigravity", "3.7.0"), appVersion: "0.2.0" });
  assert.equal(records.length, 1); assert.equal(records[0].appVersion, "0.2.0");
});

test("skip eligibility is exact App Version, target, target version, and passed result", () => {
  const records = [createInteropRecord(passed())];
  assert.equal(shouldSkipTarget(records, { appVersion: "0.1.1", targetAgent: "cursor", targetAgentVersion: "2.5.0" }), true);
  assert.equal(shouldSkipTarget(records, { appVersion: "0.1.1", targetAgent: "cursor", targetAgentVersion: "2.5.1" }), false);
  assert.equal(shouldSkipTarget(records, { appVersion: "0.2.0", targetAgent: "cursor", targetAgentVersion: "2.5.0" }), false);
  assert.equal(shouldSkipTarget([{ ...records[0], results: "failed", notes: "direct_mcp_failed" }], { appVersion: "0.1.1", targetAgent: "cursor", targetAgentVersion: "2.5.0" }), false);
});

test("persistence produces exactly one atomic row for each target after a complete run", () => {
  const directory = mkdtempSync(join(tmpdir(), "lico-direct-manifest-")); const path = join(directory, "interop.yaml");
  try {
    writeFileSync(path, renderInteropManifestYaml([passed("cursor", "2.4.0")]));
    for (const [target, version] of [["codex", "5.3.0"], ["cursor", "2.5.0"], ["antigravity", "3.7.0"]]) persistTargetRecord({ path, record: passed(target, version) });
    const raw = readFileSync(path, "utf8"); const records = parseInteropManifestYaml(raw);
    assert.deepEqual(records.map((row) => row.targetAgent), ["codex", "cursor", "antigravity"]);
    assert.doesNotMatch(raw, /\/Users\/|\/private\/|Bearer|token|prompt|conversation/iu);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("tracked Manifest is either clean or exact complete current-App passing evidence", () => {
  assert.equal(INTEROP_MANIFEST_RELATIVE_PATH, "tests/product-e2e/cli/subagent-mcp/interop-manifest.yaml");
  const currentAppVersion = readRepoAppVersion();
  assertTrackedManifestState(
    readFileSync(INTEROP_MANIFEST_RELATIVE_PATH, "utf8"),
    currentAppVersion,
  );
});

test("tracked Manifest state rejects partial, failed, stale, unsafe, or duplicate evidence", () => {
  const currentAppVersion = readRepoAppVersion();
  const complete = TARGET_AGENTS.map((targetAgent, index) => ({
    ...passed(targetAgent, `2.${index + 1}.0`),
    appVersion: currentAppVersion,
  }));
  assert.doesNotThrow(() => assertTrackedManifestState(
    renderInteropManifestYaml([]),
    currentAppVersion,
  ));
  assert.doesNotThrow(() => assertTrackedManifestState(
    renderInteropManifestYaml(complete),
    currentAppVersion,
  ));
  assert.throws(() => assertTrackedManifestState(
    renderInteropManifestYaml(complete.slice(0, 2)),
    currentAppVersion,
  ));
  assert.throws(() => assertTrackedManifestState(renderInteropManifestYaml([
    { ...complete[0], results: "failed", notes: "direct_mcp_failed" },
    ...complete.slice(1),
  ]), currentAppVersion));
  assert.throws(() => assertTrackedManifestState(renderInteropManifestYaml(
    complete.map((record) => ({ ...record, appVersion: "0.1.0" })),
  ), currentAppVersion));

  const valid = renderInteropManifestYaml(complete);
  assert.throws(() => assertTrackedManifestState(
    valid.replace("2.1.0", "not-a-version"),
    currentAppVersion,
  ));
  const duplicateRow = valid.split("\n").slice(3, 10).join("\n");
  assert.throws(() => assertTrackedManifestState(
    `${valid.trimEnd()}\n\n${duplicateRow}\n`,
    currentAppVersion,
  ));
});

test("invalid App and Agent versions fail closed before evidence can be admitted", () => {
  const root = mkdtempSync(join(tmpdir(), "lico-version-admission-"));
  try {
    mkdirSync(join(root, "tools"));
    writeFileSync(join(root, "tools", "client-version.json"), JSON.stringify({ productVersion: "not-a-version" }));
    assert.throws(() => readRepoAppVersion(root), InteropManifestError);
    assert.equal(isInteropVersion("2.5.0"), true);
    assert.equal(isInteropVersion("not-a-version"), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
