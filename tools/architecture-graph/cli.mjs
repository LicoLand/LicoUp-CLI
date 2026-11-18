#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";

import {
  DEFAULT_PROJECT_DOCUMENT,
  DEFAULT_PUBLIC_MAP,
  DEFAULT_RENDER_DIRECTORY,
} from "./config.mjs";
import { GraphError, requireFact } from "./lib/canonical.mjs";
import { loadGraph, RELATION_CLASSES, RELATION_CLASS_NOTES } from "./lib/graph-model.mjs";
import { analyzeImpact } from "./lib/impact.mjs";
import { buildRegressionSelection, changedPaths } from "./lib/regression-selection.mjs";
import { checkPublicMap, publishPublicMap } from "./lib/publish.mjs";
import { renderViews } from "./lib/render.mjs";
import { resolveSourceIdentities } from "./lib/source-identity.mjs";
import { buildWorkItems, legacyMapping, relationChecks, requireRelationChecks, verifyEvidenceBinding } from "./lib/work-items.mjs";

/**
 * Architecture and development graph CLI (module M24).
 *
 * Reads the graph, checks structure with the repository's existing JSON Schema
 * library, checks semantics, and projects views. It does not run product code,
 * launch agents, execute the regression commands it selects, mutate the
 * development ledger, or accept work.
 */

const REPOSITORY_ROOT = path.resolve(fileURLToPath(new URL("../..", import.meta.url)));

const USAGE = `usage: node tools/architecture-graph/cli.mjs <command> [options]

commands
  validate     structural + semantic validation of the configured graph
  resolve      resolved typed graph with source-file identity and revisions
  relations    relation classes, edges, and which class may schedule work
  trace        legacy task mapping and directed relation checks
  impact       declared impact for paths, modules, contracts, packages or tasks
  select       existing regression selection for changed paths
  export       machine work items with claim binding
  render       generate Mermaid, deterministic SVG and one offline HTML page
  publish      write or check docs/architecture/architecture-map.json
  bind         check a receipt against the resolved graph (no state, no ledger)

options
  --project <file>            graph project document (default ${DEFAULT_PROJECT_DOCUMENT})
  --repo-root <dir>           repository root to resolve sources against
  --out <path|dir>            output path for resolve, export, render, publish
  --revision <sha>            explicit source revision to bind
  --path <path>               repository-relative changed path (repeatable)
  --module <id> --contract <id> --package <id> --task <id>   (repeatable)
  --changed-from <revision>   resolve changed paths from Git
  --class <relation class>    filter relations output
  --check                     verify the published projection instead of writing
  --evidence <file> --claim <file>   inputs for bind
  --help
`;

function listOption(values) {
  if (values === undefined) return [];
  return Array.isArray(values) ? values : [values];
}

function readJsonFile(filePath) {
  requireFact(fs.existsSync(filePath), `file is missing: ${filePath}`);
  try {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch (error) {
    throw new GraphError(`${filePath} is not valid JSON: ${error.message}`);
  }
}

async function run(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      project: { type: "string" },
      "repo-root": { type: "string" },
      out: { type: "string" },
      revision: { type: "string" },
      path: { type: "string", multiple: true },
      module: { type: "string", multiple: true },
      contract: { type: "string", multiple: true },
      package: { type: "string", multiple: true },
      task: { type: "string", multiple: true },
      "changed-from": { type: "string" },
      class: { type: "string" },
      evidence: { type: "string" },
      claim: { type: "string" },
      check: { type: "boolean" },
      help: { type: "boolean" },
    },
  });

  if (values.help || positionals.length === 0) {
    process.stdout.write(USAGE);
    return 0;
  }

  const command = positionals[0];
  const repoRoot = values["repo-root"] ? path.resolve(values["repo-root"]) : REPOSITORY_ROOT;
  const projectPath = path.resolve(repoRoot, values.project ?? DEFAULT_PROJECT_DOCUMENT);
  const graph = loadGraph({ projectPath, repoRoot });

  switch (command) {
    case "validate": {
      requireRelationChecks(graph);
      const structure = graph.documents.structure;
      return {
        valid: true,
        kind: "licoup-architecture-graph-validation.v1",
        graph_digest: graph.graphDigest,
        graph_version: graph.graphVersion,
        counts: {
          modules: graph.modules.size,
          contracts: graph.contracts.size,
          tasks: graph.tasks.size,
          acceptance_cases: graph.cases.size,
          milestones: graph.milestones.size,
          packages: graph.packages.size,
          profiles: graph.profiles.size,
        },
        structure: structure.map((entry) => ({
          document: path.relative(repoRoot, entry.document).split(path.sep).join("/"),
          schema: path.relative(repoRoot, entry.schema).split(path.sep).join("/"),
          valid: entry.valid,
        })),
        relation_checks: relationChecks(graph).checks.map((check) => ({ id: check.id, severity: check.severity, passed: check.passed })),
        meaning: "Structure is checked with the repository's existing JSON Schema library; semantics, IDs and DAG shape are checked by this tool. Neither proves the current code matches the declared target graph.",
      };
    }

    case "resolve": {
      const identities = resolveSourceIdentities({ graph, repoRoot, revision: values.revision });
      const development = graph.developmentDag();
      const resolved = {
        kind: "licoup-resolved-graph.v1",
        graph_digest: graph.graphDigest,
        graph_version: graph.graphVersion,
        nodes: [
          ...[...graph.modules.values()].map((module) => ({ id: module.id, type: "module" })),
          ...[...graph.contracts.values()].map((contract) => ({ id: contract.id, type: "contract", revision: contract.revision })),
          ...[...graph.tasks.values()].map((task) => ({ id: task.id, type: "task", fingerprint: graph.taskFingerprint(task.id) })),
          ...[...graph.cases.values()].map((entry) => ({ id: entry.id, type: "acceptance", required_level: entry.required_level })),
          ...[...graph.milestones.values()].map((entry) => ({ id: entry.id, type: "milestone" })),
          ...[...graph.packages.values()].map((entry) => ({ id: entry.id, type: "package" })),
          ...[...graph.profiles.values()].map((entry) => ({ id: entry.id, type: "profile" })),
        ],
        relations: graph.relations(),
        development_dag: { edges: development.edges, layers: development.layers },
        source_identities: identities,
        non_claims: [
          "Declared and derived graph facts only; the current code tree is observed by the repository's existing architecture checks and regression catalog.",
          "The resolved graph holds no task status and no claim.",
        ],
      };
      const target = values.out ? path.resolve(repoRoot, values.out) : null;
      if (target) {
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.writeFileSync(target, `${JSON.stringify(resolved, null, 2)}\n`, "utf8");
        return { written: values.out, nodes: resolved.nodes.length, relations: resolved.relations.length, identity_digest: identities.identity_digest };
      }
      return resolved;
    }

    case "relations": {
      const relations = graph.relations().filter((edge) => !values.class || edge.class === values.class);
      if (values.class) {
        requireFact(Object.values(RELATION_CLASSES).includes(values.class),
          `unknown relation class: ${values.class}`);
      }
      const classes = Object.fromEntries(Object.values(RELATION_CLASSES).map((name) => [name, {
        note: RELATION_CLASS_NOTES[name],
        enters_development_dag: name === RELATION_CLASSES.DEVELOPMENT_ORDER,
        declared_edges: graph.relations().filter((edge) => edge.class === name).length,
      }]));
      return { kind: "licoup-relations.v1", graph_digest: graph.graphDigest, classes, count: relations.length, relations };
    }

    case "trace": {
      return { ...legacyMapping(graph), relation_checks: relationChecks(graph) };
    }

    case "impact": {
      const changedFrom = values["changed-from"] ?? null;
      const paths = listOption(values.path);
      if (changedFrom) {
        paths.push(...await changedPaths({ revision: changedFrom, repoRoot }));
      }
      return analyzeImpact({
        graph,
        repoRoot,
        paths,
        modules: listOption(values.module),
        contracts: listOption(values.contract),
        packages: listOption(values.package),
        tasks: listOption(values.task),
        changedFrom,
      });
    }

    case "select": {
      const changedFrom = values["changed-from"] ?? null;
      const paths = listOption(values.path);
      if (changedFrom) paths.push(...await changedPaths({ revision: changedFrom, repoRoot }));
      return buildRegressionSelection({ graph, paths, repoRoot, changedFrom });
    }

    case "export": {
      const workItems = buildWorkItems({ graph, repoRoot, revision: values.revision });
      const target = values.out ? path.resolve(repoRoot, values.out) : null;
      if (target) {
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.writeFileSync(target, `${JSON.stringify(workItems, null, 2)}\n`, "utf8");
        return { written: values.out, items: workItems.items.length, graph_digest: workItems.graph_digest };
      }
      return workItems;
    }

    case "render": {
      const outDirectory = path.resolve(repoRoot, values.out ?? DEFAULT_RENDER_DIRECTORY);
      const result = renderViews(graph, outDirectory);
      return {
        out: values.out ?? DEFAULT_RENDER_DIRECTORY,
        files: result.files,
        summary: result.summary,
        note: "Generated views. Editing them by hand is drift; regenerate from the graph documents.",
      };
    }

    case "publish": {
      const outputPath = path.resolve(repoRoot, values.out ?? DEFAULT_PUBLIC_MAP);
      if (values.check) {
        const result = checkPublicMap({ graph, outputPath });
        if (!result.matches) throw new GraphError(`public architecture projection is not current: ${result.reason}`);
        return { check: "current", path: values.out ?? DEFAULT_PUBLIC_MAP, bytes: result.bytes };
      }
      // Report the caller-supplied path, never the resolved absolute one: tool
      // output is shared and must stay machine-neutral.
      const published = publishPublicMap({ graph, outputPath });
      return { path: values.out ?? DEFAULT_PUBLIC_MAP, bytes: published.bytes, generated: published.generated, note: published.note };
    }

    case "bind": {
      requireFact(values.evidence, "bind needs --evidence <file>");
      const receipt = readJsonFile(path.resolve(repoRoot, values.evidence));
      const claim = values.claim ? readJsonFile(path.resolve(repoRoot, values.claim)) : null;
      const result = verifyEvidenceBinding({ graph, receipt, claim });
      if (!result.consistent) {
        const rejection = new GraphError(`evidence binding rejected: ${result.reasons.join("; ")}`);
        // Carry the machine-readable result so a caller can act on the reasons
        // without parsing a message.
        rejection.checkResult = { ok: false, ...result };
        throw rejection;
      }
      return result;
    }

    default:
      throw new GraphError(`unknown command: ${command}`);
  }
}

try {
  const result = await run(process.argv.slice(2));
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
  process.exitCode = 0;
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  if (error && typeof error === "object" && "checkResult" in error) {
    // A failed check still reports a usable payload; the exit status carries
    // the verdict so a shell caller cannot mistake it for success.
    process.stdout.write(`${JSON.stringify(error.checkResult, null, 2)}\n`);
  } else {
    process.stderr.write(`${JSON.stringify({ ok: false, error: message })}\n`);
  }
  process.exitCode = 2;
}
