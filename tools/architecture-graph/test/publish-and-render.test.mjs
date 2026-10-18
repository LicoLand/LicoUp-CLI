import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  buildPublicMap,
  checkPublicMap,
  PUBLIC_MAP_SHAPE,
  scanPublicText,
  serializePublicMap,
} from "../lib/publish.mjs";
import { loadGraph } from "../lib/graph-model.mjs";
import { renderViews } from "../lib/render.mjs";
import { DEFAULT_PUBLIC_MAP, DEFAULT_RENDER_DIRECTORY } from "../config.mjs";
import { expectGraphError, fixtureDistribution, fixtureGraph, REPOSITORY_ROOT, skipWithoutRealPlan } from "./fixtures.mjs";

const PROJECT = path.join(REPOSITORY_ROOT, "docs/plans/v7/graph/project.json");
const PUBLIC_MAP = path.join(REPOSITORY_ROOT, DEFAULT_PUBLIC_MAP);

function temporaryDirectory() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "architecture-graph-render-"));
}

/**
 * Assembled from parts: the repository boundary check forbids a literal host
 * home directory appearing in tracked sources, so this test builds the sample
 * it needs instead of embedding one.
 */
const HOST_HOME_SAMPLE = ["", "Users", "sample"].join("/");

test("the available local public projection is current with the graph it describes", (t) => {
  if (skipWithoutRealPlan(t)) return;
  const graph = loadGraph({ projectPath: PROJECT, repoRoot: REPOSITORY_ROOT });
  const checked = checkPublicMap({ graph, outputPath: PUBLIC_MAP });
  assert.equal(checked.matches, true, checked.reason);
  assert.ok(checked.bytes > 0);
});

test("a hand edit to a generated projection is detected", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  const directory = temporaryDirectory();
  const target = path.join(directory, "architecture-map.json");
  fs.writeFileSync(target, `${JSON.stringify(buildPublicMap(graph), null, 2)}\n`);
  assert.equal(checkPublicMap({ graph, outputPath: target }).matches, true);

  const edited = JSON.parse(fs.readFileSync(target, "utf8"));
  edited.modules[0].title = "hand edited";
  fs.writeFileSync(target, `${JSON.stringify(edited, null, 2)}\n`);
  const drift = checkPublicMap({ graph, outputPath: target });
  assert.equal(drift.matches, false);
  assert.match(drift.reason, /hand edit or stale source graph/u);
  assert.equal(typeof drift.first_differing_line, "number");

  fs.writeFileSync(path.join(directory, "absent.json"), "{}");
  fs.rmSync(path.join(directory, "absent.json"));
  assert.equal(checkPublicMap({ graph, outputPath: path.join(directory, "absent.json") }).matches, false);
  fs.rmSync(directory, { recursive: true, force: true });
});

test("the projection is allow-listed, so a new private field cannot leak by default", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  const map = buildPublicMap(graph);
  for (const key of Object.keys(map)) assert.ok(PUBLIC_MAP_SHAPE.$.includes(key), `unlisted root field: ${key}`);
  for (const key of Object.keys(map.packages[0])) {
    assert.ok(PUBLIC_MAP_SHAPE["packages[]"].includes(key), `unlisted package field: ${key}`);
    assert.notEqual(key, "implementation_tasks");
  }
  assert.equal("tasks" in map, false);
  assert.equal("execution" in map, false);
  assert.equal(JSON.stringify(map).includes("V7-"), false, "private task identifiers must not be published");
  assert.equal(JSON.stringify(map).includes("write_scopes"), false);

  const withExtra = structuredClone(map);
  withExtra.packages[0].implementation_tasks = ["V7-G1"];
  assert.match(expectGraphError(() => serializePublicMap(withExtra)), /unlisted field/);

  const withExtraPackageField = structuredClone(map);
  withExtraPackageField.packages[0].delivery_owner = "V7-U10";
  assert.match(expectGraphError(() => serializePublicMap(withExtraPackageField)), /unlisted field/);
});

test("the local-identity scan is live in both directions", () => {
  assert.deepEqual(scanPublicText('{"kind":"clean"}'), []);
  assert.equal(scanPublicText(`{"x":"${HOST_HOME_SAMPLE}/repo"}`)[0].rule, "ABSOLUTE_POSIX_PATH");
  assert.equal(scanPublicText('{"x":"C:\\\\repo"}')[0].rule, "ABSOLUTE_WINDOWS_PATH");
  assert.equal(scanPublicText('{"x":"~/repo"}')[0].rule, "HOME_ALIAS_PATH");
  assert.equal(scanPublicText('{"x":"person@example.com"}')[0].rule, "EMAIL_ADDRESS");
  assert.equal(scanPublicText('{"x":"sk-abcdefghijkl"}')[0].rule, "MODEL_CREDENTIAL");
  assert.equal(scanPublicText('{"x":"Authorization: Bearer abcdefghijkl"}')[0].rule, "BEARER_TOKEN");
  assert.equal(scanPublicText('{"x":"V7-G1"}')[0].rule, "PRIVATE_TASK_ID");
  assert.equal(scanPublicText('{"x":"implementation_tasks"}')[0].rule, "PRIVATE_EXECUTION_FIELD");
  for (const finding of scanPublicText(`{"x":"${HOST_HOME_SAMPLE}/V7-G1"}`)) {
    assert.equal(typeof finding.rule, "string");
    assert.equal(typeof finding.index, "number");
  }
});

test("a projection carrying a private identifier or a local path refuses to serialize", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });

  graph.modules.get("M01").title = "Development graph (V7-G1)";
  assert.match(expectGraphError(() => serializePublicMap(buildPublicMap(graph))), /PRIVATE_TASK_ID/);
  graph.modules.get("M01").title = "Module M01";

  graph.modules.get("M01").path = `${HOST_HOME_SAMPLE}/checkout/src/provider/`;
  assert.match(expectGraphError(() => serializePublicMap(buildPublicMap(graph))), /ABSOLUTE_POSIX_PATH/);
  graph.modules.get("M01").path = "src/provider/";

  assert.doesNotThrow(() => serializePublicMap(buildPublicMap(graph)));
});

test("rendering is byte reproducible and generates an offline browser page", () => {
  const graph = fixtureGraph({ distribution: fixtureDistribution() });
  const first = temporaryDirectory();
  const second = temporaryDirectory();
  const firstResult = renderViews(graph, first);
  renderViews(graph, second);
  const expectedWidth = Math.max(...graph.developmentDag().layers.map((layer) => layer.length));
  assert.equal(firstResult.summary.layer_width, expectedWidth);
  assert.equal(JSON.parse(fs.readFileSync(path.join(first, "summary.json"), "utf8")).layer_width, expectedWidth);

  for (const name of firstResult.files) {
    assert.deepEqual(
      fs.readFileSync(path.join(first, name)),
      fs.readFileSync(path.join(second, name)),
      `${name} must render to identical bytes for identical input`,
    );
  }

  const html = fs.readFileSync(path.join(first, "index.html"), "utf8");
  assert.ok(html.includes("graph_digest"));
  assert.ok(html.includes(graph.graphDigest));
  assert.equal(/<script[^>]+src=/u.test(html), false, "the offline page must not load external scripts");
  assert.equal(/<(?:img|link|iframe|object|embed)[^>]+(?:href|src)=/u.test(html), false, "no external assets");
  assert.equal(/https?:\/\/(?!www\.w3\.org)/u.test(html), false, "no external URLs in the offline page");
  assert.equal(/cdn|unpkg|jsdelivr/u.test(html), false);
  assert.ok(html.includes("precedes"), "the offline page must explain the only scheduling relation");

  const execution = fs.readFileSync(path.join(first, "execution.mmd"), "utf8");
  assert.ok(execution.startsWith("flowchart LR"));
  assert.ok(execution.includes("precedes"));
  const architecture = fs.readFileSync(path.join(first, "architecture.mmd"), "utf8");
  assert.ok(architecture.includes("depends_on"));
  assert.ok(architecture.includes("runtime_calls"));
  const architectureSvg = fs.readFileSync(path.join(first, "architecture.svg"), "utf8");
  assert.ok(architectureSvg.includes(">M01<") || architectureSvg.includes("M01 "), "node identity must be real text, not an image");
  assert.equal(/<image\b/u.test(architectureSvg), false);
  const distributionSvg = fs.readFileSync(path.join(first, "distribution.svg"), "utf8");
  assert.ok(distributionSvg.includes("org.test.core"), "the install closure must be a real rendered view");

  fs.rmSync(first, { recursive: true, force: true });
  fs.rmSync(second, { recursive: true, force: true });
});

test("the default output locations stay inside the declared scopes", () => {
  assert.equal(DEFAULT_PUBLIC_MAP, "docs/architecture/architecture-map.json");
  assert.equal(DEFAULT_RENDER_DIRECTORY, "docs/plans/v7/graph/generated");
  assert.equal(path.isAbsolute(DEFAULT_PUBLIC_MAP), false);
  assert.equal(path.isAbsolute(DEFAULT_RENDER_DIRECTORY), false);
});
