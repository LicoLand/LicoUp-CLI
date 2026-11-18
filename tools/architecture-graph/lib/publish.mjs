import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { GraphError, requireFact } from "./canonical.mjs";
import { RELATION_CLASSES, RELATION_CLASS_NOTES } from "./graph-model.mjs";
import { architectureMermaid, distributionMermaid } from "./render.mjs";

/**
 * The public architecture projection.
 *
 * `docs/architecture/` is a public tracked directory. The projection is built
 * from an explicit allow-list of architecture fields, so private execution facts
 * (task ids, task prose, acceptance scenarios, write scopes, ledger digests)
 * cannot reach it by accident. An allow-list is used instead of a deny-list
 * because a deny-list silently publishes every field a future graph revision
 * adds.
 */

export const PUBLIC_MAP_KIND = "licoup-architecture-map.v1";

export const PUBLIC_MAP_SHAPE = Object.freeze({
  "$": ["schema_version", "kind", "generator", "source_revision", "architecture_revision", "status", "directions", "relation_classes", "modules", "contracts", "edges", "packages", "profiles", "views", "counts", "sanitization"],
  "generator": ["tool", "module", "invocation"],
  "directions": ["depends_on", "runtime_calls", "precedes", "requires_package"],
  "relation_classes": ["class", "note", "enters_development_dag"],
  "modules[]": ["id", "title", "path", "boundary_kind", "owner", "owns", "forbids", "baseline_status", "target_status", "public"],
  "contracts[]": ["id", "title", "owner_module", "consumers", "semantics", "revision"],
  "edges[]": ["type", "source", "target", "contract"],
  "packages[]": ["id", "title", "requires_package", "modules", "optional", "provides", "activation", "artifact_status", "measured_bytes"],
  "profiles[]": ["id", "title", "selected_packages", "forbidden_packages"],
  "views": ["architecture_mermaid", "distribution_mermaid"],
  "counts": ["modules", "contracts", "architecture_edges", "packages", "profiles"],
  "sanitization": ["local_identity_scan", "private_execution_fields_excluded", "generated"],
});

const LOCAL_IDENTITY_PATTERNS = Object.freeze([
  ["ABSOLUTE_POSIX_PATH", /(?:^|["'\s([{:,\]])\/(?:Users|home|root|private|var|tmp|opt|mnt)\//u],
  ["ABSOLUTE_WINDOWS_PATH", /(?:^|["'\s([{:,\]])[A-Za-z]:[\\/]/u],
  ["HOME_ALIAS_PATH", /(?:^|["'\s([{:,\]])~\//u],
  ["FILE_URL", /file:\/\//iu],
  ["EMAIL_ADDRESS", /\b[\w.+-]+@[\w-]+\.[A-Za-z]{2,}\b/u],
  ["MODEL_CREDENTIAL", /\b(?:sk|pk|rk)-[A-Za-z0-9_-]{8,}\b/u],
  ["BEARER_TOKEN", /\bBearer\s+[A-Za-z0-9._-]{8,}/u],
  ["CREDENTIAL_ASSIGNMENT", /\b(?:api[_-]?key|secret|password|passwd|access[_-]?token)\b\s*[:=]/iu],
  ["PRIVATE_TASK_ID", /\bV7-[A-Za-z0-9]/u],
  ["PRIVATE_SCENARIO_ID", /\bA\d{1,3}@|\bA\d{2,3}\b/u],
  ["PRIVATE_EXECUTION_FIELD", /\b(?:legacy_tasks|write_scopes|audit_findings|implementation_tasks|acceptance_levels|independent_review)\b/u],
  ["TASK_PENDING_PROSE", /待执行|待认领|needs_review/u],
]);

function localIdentityTokens() {
  const tokens = [];
  const username = os.userInfo().username;
  if (typeof username === "string" && username.length >= 4 && !["root", "user", "admin"].includes(username)) {
    tokens.push(["LOCAL_ACCOUNT_NAME", username]);
  }
  const hostname = os.hostname().split(".")[0];
  if (typeof hostname === "string" && hostname.length >= 6) tokens.push(["LOCAL_HOSTNAME", hostname]);
  const home = os.homedir();
  if (typeof home === "string" && home.length > 1) tokens.push(["LOCAL_HOME_DIRECTORY", home]);
  return tokens;
}

/**
 * Scan a candidate public payload. It reports rule names and positions, never
 * the matched text, so a failure message cannot itself leak the value.
 */
export function scanPublicText(text) {
  const findings = [];
  for (const [rule, pattern] of LOCAL_IDENTITY_PATTERNS) {
    const match = pattern.exec(text);
    if (match) findings.push({ rule, index: match.index });
  }
  const lowered = text.toLowerCase();
  for (const [rule, token] of localIdentityTokens()) {
    const index = lowered.indexOf(token.toLowerCase());
    if (index >= 0) findings.push({ rule, index });
  }
  return findings;
}

function assertShapeKeys(value, allowedKeys, where) {
  for (const key of Object.keys(value)) {
    requireFact(allowedKeys.includes(key), `public projection would publish an unlisted field at ${where}: ${key}`);
  }
}

function assertPublicShape(map) {
  assertShapeKeys(map, PUBLIC_MAP_SHAPE.$, "$");
  assertShapeKeys(map.generator, PUBLIC_MAP_SHAPE.generator, "generator");
  assertShapeKeys(map.directions, PUBLIC_MAP_SHAPE.directions, "directions");
  assertShapeKeys(map.views, PUBLIC_MAP_SHAPE.views, "views");
  assertShapeKeys(map.counts, PUBLIC_MAP_SHAPE.counts, "counts");
  assertShapeKeys(map.sanitization, PUBLIC_MAP_SHAPE.sanitization, "sanitization");
  map.modules.forEach((entry, index) => assertShapeKeys(entry, PUBLIC_MAP_SHAPE["modules[]"], `modules[${index}]`));
  map.contracts.forEach((entry, index) => assertShapeKeys(entry, PUBLIC_MAP_SHAPE["contracts[]"], `contracts[${index}]`));
  map.edges.forEach((entry, index) => assertShapeKeys(entry, PUBLIC_MAP_SHAPE["edges[]"], `edges[${index}]`));
  map.packages.forEach((entry, index) => assertShapeKeys(entry, PUBLIC_MAP_SHAPE["packages[]"], `packages[${index}]`));
  map.profiles.forEach((entry, index) => assertShapeKeys(entry, PUBLIC_MAP_SHAPE["profiles[]"], `profiles[${index}]`));
  map.relation_classes.forEach((entry, index) => assertShapeKeys(entry, PUBLIC_MAP_SHAPE["relation_classes"], `relation_classes[${index}]`));
}

export function buildPublicMap(graph) {
  const map = {
    schema_version: 1,
    kind: PUBLIC_MAP_KIND,
    generator: {
      tool: "tools/architecture-graph/cli.mjs",
      module: "M24",
      invocation: "publish",
    },
    source_revision: graph.architecture.baseline_commit,
    architecture_revision: graph.architecture.revision,
    status: graph.architecture.status,
    directions: {
      depends_on: graph.architecture.directions.depends_on,
      runtime_calls: graph.architecture.directions.runtime_calls,
      precedes: graph.architecture.directions.precedes,
      requires_package: graph.architecture.directions.requires_package
        ?? "selected package -> required package; deployment closure only",
    },
    relation_classes: Object.values(RELATION_CLASSES).map((name) => ({
      class: name,
      note: RELATION_CLASS_NOTES[name],
      enters_development_dag: name === RELATION_CLASSES.DEVELOPMENT_ORDER,
    })),
    modules: [...graph.modules.values()].map((module) => ({
      id: module.id,
      title: module.title,
      path: module.path,
      boundary_kind: module.boundary_kind,
      owner: module.owner,
      owns: module.owns,
      forbids: module.forbids,
      baseline_status: module.baseline_status,
      target_status: module.target_status,
      public: module.public,
    })),
    contracts: [...graph.contracts.values()].map((contract) => ({
      id: contract.id,
      title: contract.title,
      owner_module: contract.owner_module,
      consumers: [...contract.consumers],
      semantics: contract.semantics,
      revision: contract.revision,
    })),
    edges: graph.architecture.edges.map((edge) => ({
      type: edge.type,
      source: edge.source,
      target: edge.target,
      ...(edge.contract ? { contract: edge.contract } : {}),
    })),
    packages: [...graph.packages.values()].map((pkg) => ({
      id: pkg.id,
      title: pkg.title,
      requires_package: [...pkg.requires_package],
      modules: [...pkg.modules],
      optional: pkg.optional,
      provides: [...pkg.provides],
      activation: pkg.activation,
      artifact_status: pkg.artifact_status,
      measured_bytes: pkg.measured_bytes,
    })),
    profiles: [...graph.profiles.values()].map((profile) => ({
      id: profile.id,
      title: profile.title,
      selected_packages: [...profile.selected_packages],
      forbidden_packages: [...profile.forbidden_packages],
    })),
    views: {
      architecture_mermaid: architectureMermaid(graph),
      distribution_mermaid: distributionMermaid(graph),
    },
    counts: {
      modules: graph.modules.size,
      contracts: graph.contracts.size,
      architecture_edges: graph.architecture.edges.length,
      packages: graph.packages.size,
      profiles: graph.profiles.size,
    },
    sanitization: {
      local_identity_scan: "allow-list projection plus rule scan; reports rule names only",
      private_execution_fields_excluded: true,
      generated: "Generated by tools/architecture-graph/cli.mjs publish. Do not edit by hand.",
    },
  };
  assertPublicShape(map);
  return map;
}

export function serializePublicMap(map) {
  // Re-checked here as well as in the builder: serialization is the last place
  // before a public file is written, so it refuses an unlisted field too.
  assertPublicShape(map);
  const text = `${JSON.stringify(map, null, 2)}\n`;
  const findings = scanPublicText(text);
  if (findings.length > 0) {
    throw new GraphError(
      `public projection refuses to publish ${findings.length} local-identity finding(s): ${findings.map((finding) => finding.rule).join(", ")}`,
    );
  }
  return text;
}

export function publishPublicMap({ graph, outputPath }) {
  const text = serializePublicMap(buildPublicMap(graph));
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, text, "utf8");
  return {
    path: outputPath,
    bytes: Buffer.byteLength(text, "utf8"),
    generated: true,
    note: "Generated projection. Regenerate with the tool; a hand edit is drift and is detected by --check.",
  };
}

export function checkPublicMap({ graph, outputPath }) {
  const expected = serializePublicMap(buildPublicMap(graph));
  if (!fs.existsSync(outputPath)) {
    return { matches: false, reason: "published projection is missing" };
  }
  const actual = fs.readFileSync(outputPath, "utf8");
  if (actual !== expected) {
    const actualLines = actual.split("\n");
    const expectedLines = expected.split("\n");
    let line = 0;
    while (line < actualLines.length && line < expectedLines.length && actualLines[line] === expectedLines[line]) line += 1;
    return {
      matches: false,
      reason: "published projection differs from the generated projection (hand edit or stale source graph)",
      first_differing_line: line + 1,
      expected_lines: expectedLines.length,
      actual_lines: actualLines.length,
    };
  }
  return { matches: true, bytes: Buffer.byteLength(actual, "utf8") };
}
