#!/usr/bin/env node
// Regenerates the replay corpus from recorded vendor frames.
//
// Frames come from the per-protocol tables. Projections are never authored:
// the recorder runs the real adapter parsers over those frames and writes what
// they reported, and only then is the document sealed with its redaction
// hash. Regenerating therefore reproduces the committed corpus byte for byte
// only while the adapters still parse the recorded frames the same way.
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import {
  adapterIds,
  privacyFindings,
  redactionSecrets,
  scenarioClasses,
  transcriptHash,
} from "./shared.mjs";
import { recordProjections } from "./record-projections.mjs";

const repositoryRoot = resolve(import.meta.dirname, "../../..");
const fixturesRoot = resolve(process.argv[2] || join(repositoryRoot, "apps/desktop/test/fixtures/adapter-replay"));
const buildRoot = join(repositoryRoot, "build", "adapter-replay");
if (fixturesRoot === repositoryRoot || !fixturesRoot.startsWith(`${repositoryRoot}/`)) {
  throw new Error(`fixture_root_must_be_inside_repository:${fixturesRoot}`);
}

mkdirSync(buildRoot, { recursive: true });
const staging = mkdtempSync(join(buildRoot, "staging-"));
let count = 0;
try {
  recordProjections(staging, adapterIds);

  for (const adapterId of adapterIds) {
    for (const scenario of scenarioClasses) {
      const path = join(staging, adapterId, `${scenario}.json`);
      const recorded = JSON.parse(readFileSync(path, "utf8"));
      for (const frame of recorded.frames) {
        if (!Array.isArray(frame.projection)) {
          throw new Error(`projection_not_recorded:${adapterId}:${scenario}:${frame.index}`);
        }
      }
      recorded.review.checklist.projectionsMatchParserOutput = true;
      recorded.redaction = {
        algorithm: "lico-transcript-redaction-v1",
        contentSha256: transcriptHash(recorded),
      };
      const findings = privacyFindings(recorded, redactionSecrets());
      if (findings.length > 0) {
        throw new Error(`privacy_finding_in_recorded_fixture:${adapterId}:${scenario}`);
      }
      writeFileSync(path, `${JSON.stringify(recorded, null, 2)}\n`);
      count += 1;
    }
  }

  rmSync(fixturesRoot, { recursive: true, force: true });
  cpSync(staging, fixturesRoot, { recursive: true });
} finally {
  rmSync(staging, { recursive: true, force: true });
}

process.stdout.write(`generated ${count} recorded replay fixtures under ${fixturesRoot}\n`);
